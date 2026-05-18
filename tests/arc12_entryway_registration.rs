//! Arc 12 §5.3.4 — paired registration tests for the forwarded
//! handlers (static-text grep half).
//!
//! For each of the four §5.3.8 forwarded NSIDs, this test reads the
//! handler source file and asserts that the handler function is wired
//! to the forwarded auth surface in the correct shape:
//!
//! - **Mint-pattern handlers** (`signPlcOperation`, `updateHandle`,
//!   `getSession`): the handler body MUST use either
//!   `AuthContextForwarded` (extractor) or call
//!   `middleware::require_auth_forwarded` (free function). Crucially
//!   it must NOT use the bare `require_auth` or `AuthContext` —
//!   those would reject entryway-issued tokens whose aud is the
//!   entryway DID.
//! - **Passthru-pattern handler** (`requestPasswordReset`): the
//!   handler MUST guard a `ctx.entryway_client` branch that uses
//!   `entryway_passthru_headers` (the §5.3.6 header filter).
//!
//! This is the static-text half of the §5.3.4 paired-test discipline.
//! The behavioral integration half (Phase B integration via the live
//! entryway stub) lands in Step 5 / Phase B regression scripts.
//!
//! The test is intentionally fragile to formatting changes: a
//! refactor that removes the forwarded surface or accidentally
//! reverts a handler to the non-forwarded variant trips immediately,
//! with a clear per-handler diagnostic.

use std::fs;
use std::path::PathBuf;

/// Crate root resolved relative to the integration-test binary's
/// own source location.
fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel_path: &str) -> String {
    let path = crate_root().join(rel_path);
    fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("could not read {}: {}", path.display(), e);
    })
}

/// Extract the body of `pub async fn $name(` … `}` (matching balanced
/// braces from the function signature to the closing brace). Returns
/// the entire function source including the signature.
fn extract_fn(source: &str, name: &str) -> String {
    let needle = format!("fn {}(", name);
    let start = source
        .find(&needle)
        .unwrap_or_else(|| panic!("function `{}` not found in source", name));
    // Find the opening `{` after the signature.
    let body_start = source[start..]
        .find('{')
        .unwrap_or_else(|| panic!("no `{{` after `fn {}(`", name))
        + start;
    // Walk forward counting braces.
    let bytes = source.as_bytes();
    let mut depth: i32 = 0;
    let mut i = body_start;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return source[start..=i].to_string();
                }
            }
            _ => {}
        }
        i += 1;
    }
    panic!("unbalanced braces in `fn {}`", name);
}

// ---------- Mint-pattern handlers (3 of 4) ----------

#[test]
fn sign_plc_operation_uses_forwarded_auth_and_entryway_branch() {
    let src = read("src/api/identity.rs");
    let body = extract_fn(&src, "sign_plc_operation");
    assert!(
        body.contains("AuthContextForwarded"),
        "sign_plc_operation must use AuthContextForwarded extractor \
         (§5.3.4 — non-forwarded variant rejects entryway-issued aud)"
    );
    assert!(
        body.contains("ctx.entryway_client"),
        "sign_plc_operation must gate forwarding on ctx.entryway_client"
    );
    assert!(
        body.contains("entryway_auth_headers")
            && body.contains("com.atproto.identity.signPlcOperation"),
        "sign_plc_operation must mint auth headers for its own NSID"
    );
    assert!(
        body.contains("xrpc_post_json"),
        "sign_plc_operation must forward via EntrywayClient::xrpc_post_json"
    );
}

#[test]
fn update_handle_uses_forwarded_auth_and_entryway_branch() {
    let src = read("src/api/identity.rs");
    let body = extract_fn(&src, "update_handle");
    assert!(
        body.contains("AuthContextForwarded"),
        "update_handle must use AuthContextForwarded extractor"
    );
    assert!(
        body.contains("ctx.entryway_client"),
        "update_handle must gate forwarding on ctx.entryway_client"
    );
    assert!(
        body.contains("entryway_auth_headers")
            && body.contains("com.atproto.identity.updateHandle"),
        "update_handle must mint auth headers for its own NSID"
    );
    assert!(
        body.contains("xrpc_post_json"),
        "update_handle must forward via EntrywayClient::xrpc_post_json"
    );
}

#[test]
fn get_session_uses_require_auth_forwarded_and_entryway_branch() {
    let src = read("src/api/server.rs");
    let body = extract_fn(&src, "get_session");
    assert!(
        body.contains("require_auth_forwarded"),
        "get_session must call middleware::require_auth_forwarded \
         (mint-pattern + GET endpoint, no extractor in current shape)"
    );
    assert!(
        body.contains("ctx.entryway_client"),
        "get_session must gate forwarding on ctx.entryway_client"
    );
    assert!(
        body.contains("entryway_auth_headers")
            && body.contains("com.atproto.server.getSession"),
        "get_session must mint auth headers for its own NSID"
    );
    assert!(
        body.contains("xrpc_get_json"),
        "get_session must forward via EntrywayClient::xrpc_get_json (GET)"
    );
}

// ---------- Passthru-pattern handler (1 of 4) ----------

#[test]
fn request_password_reset_uses_entryway_passthru_branch() {
    let src = read("src/api/server.rs");
    let body = extract_fn(&src, "request_password_reset");
    assert!(
        body.contains("ctx.entryway_client"),
        "request_password_reset must gate forwarding on ctx.entryway_client"
    );
    assert!(
        body.contains("entryway_passthru_headers"),
        "request_password_reset must use the §5.3.6 passthru-header filter"
    );
    assert!(
        body.contains("xrpc_post_json")
            && body.contains("com.atproto.server.requestPasswordReset"),
        "request_password_reset must forward its own NSID via xrpc_post_json"
    );
    // Negative: the auth-mint helper must NOT appear in a passthru handler.
    assert!(
        !body.contains("entryway_auth_headers"),
        "request_password_reset is the passthru handler — entryway_auth_headers \
         is mint-pattern only and must not appear here"
    );
}

// ---------- Cross-handler invariants ----------

#[test]
fn forwarded_handlers_do_not_use_bare_auth_context() {
    // Bare AuthContext / require_auth on a forwarded route would
    // reject entryway-issued tokens — exactly the regression
    // §5.3.4's paired tests are designed to catch.
    let identity_src = read("src/api/identity.rs");
    let server_src = read("src/api/server.rs");

    for (label, src, fn_name) in [
        ("sign_plc_operation", &identity_src, "sign_plc_operation"),
        ("update_handle", &identity_src, "update_handle"),
        ("get_session", &server_src, "get_session"),
    ] {
        let body = extract_fn(src, fn_name);
        // Allow AuthContextForwarded (substring "AuthContext" matches);
        // the negative we care about is bare `AuthContext` as an
        // extractor parameter type or `require_auth(` (without the
        // _forwarded suffix).
        assert!(
            !body.contains(": AuthContext\n")
                && !body.contains(": AuthContext,")
                && !body.contains(": AuthContext)"),
            "{}: must not use the bare AuthContext extractor — \
             use AuthContextForwarded per §5.3.4",
            label
        );
        assert!(
            !body.contains("require_auth(") || body.contains("require_auth_forwarded("),
            "{}: must not call middleware::require_auth — \
             use require_auth_forwarded per §5.3.4",
            label
        );
    }
}
