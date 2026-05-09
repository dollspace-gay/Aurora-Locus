//! Structural-lint contract test for the action-ID surfacing
//! invariant on Aurora-namespace admin handlers (Arc 2 §6.4.2).
//!
//! Scans Aurora-namespace handler files; for every `pub async fn`
//! whose body invokes `append_entry_in_tx`, asserts the function
//! returns a typed `*Output` struct that is defined in the same
//! file with a Rust-side `audit_entry_id` field. Drift is loud:
//! drop the field, drop the typed-struct conversion, or return
//! ad-hoc `serde_json::json!` and the test fails.
//!
//! ## Allowlist
//!
//! Handlers that legitimately surface the audit entry id outside
//! the typed-JSON convention (e.g., binary responses surfacing
//! via HTTP header) are listed in `ALLOWLIST` with a documented
//! justification per entry.
//!
//! ## Limitations (accepted v0.3 trade-offs)
//!
//! - **Type aliases not resolved.** A return type of `MyAlias`
//!   that aliases `GrantRoleOutput` would not be followed; the
//!   lint expects the literal struct name in the return type.
//! - **Field population not verified.** The lint catches
//!   "the struct lacks the field"; it does NOT catch handlers
//!   that declare the field but populate it with an empty/dummy
//!   value at runtime. Step 0 recon Q3 confirmed every Aurora
//!   handler genuinely populates the field; this lint guards
//!   future drift, not current behaviour.
//! - **Macro-generated handlers not handled.** All current
//!   Aurora handlers are hand-written `async fn`s.
//! - **Allowlist requires manual update** when new
//!   Aurora-namespace handlers are added to shared files
//!   (see the second tuple in `SCAN_TARGETS`).
//!
//! Sophisticated drift requires sophisticated detection that's
//! out of v0.3 scope. The lint catches the common failure mode
//! at low implementation cost.

use std::fs;

/// Files to scan, paired with the set of handler names to inspect
/// inside that file.
///
/// - `&[]` (empty slice) means "scan every `pub async fn` in the
///   file" — used for files that are entirely Aurora-namespace.
/// - A non-empty slice scopes the scan to the named handlers only,
///   for files that mix Aurora and upstream-lexicon handlers.
const SCAN_TARGETS: &[(&str, &[&str])] = &[
    // Whole-file scans — every `pub async fn` is fair game.
    ("src/api/aurora_admin.rs", &[]),
    ("src/api/aurora_moderator.rs", &[]),
    ("src/api/aurora_subscribe.rs", &[]),
    // Mixed-namespace file — only the listed Aurora handlers.
    // Adding a new Aurora-namespace handler to this file requires
    // updating this tuple.
    ("src/api/admin.rs", &["grant_role", "revoke_role"]),
];

/// Handlers that surface the audit entry ID outside the typed-JSON
/// response convention. Each entry must include a documented
/// justification.
const ALLOWLIST: &[(&str, &str)] = &[(
    "export_account_forensic",
    "Returns binary tar response; surfaces audit entry ID via the \
     X-Aurora-Audit-Entry-Id HTTP header per Arc 2 Step 0 recon Q3 \
     finding.",
)];

#[test]
fn aurora_namespace_handlers_surface_audit_entry_id() {
    let mut violations: Vec<String> = Vec::new();

    for (path, scope) in SCAN_TARGETS {
        let source = fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read {}: {}", path, e));
        for handler in extract_handlers(&source) {
            // Scope filter: empty scope means scan all; non-empty
            // means restrict to the named handlers.
            if !scope.is_empty() && !scope.contains(&handler.name.as_str()) {
                continue;
            }
            if !handler.body.contains("append_entry_in_tx") {
                continue;
            }
            if let Some((_, justification)) =
                ALLOWLIST.iter().find(|(name, _)| *name == handler.name)
            {
                eprintln!(
                    "lint: skipping allowlisted handler `{}` in {} ({})",
                    handler.name, path, justification
                );
                continue;
            }
            // The body invokes `append_entry_in_tx` and the handler
            // is not allowlisted — the contract requires a typed
            // `*Output` return with an `audit_entry_id` field.
            let Some(output_type) = extract_output_type(&handler.return_type) else {
                violations.push(format!(
                    "{}::{} writes an audit chain entry but its return type \
                     `{}` does not match the expected `Result<Json<*Output>, ...>` \
                     pattern. Convert the handler to return a typed `*Output` \
                     struct, OR add it to ALLOWLIST in this test with a documented \
                     justification.",
                    path, handler.name, handler.return_type.trim(),
                ));
                continue;
            };
            if !struct_has_audit_entry_id_field(&source, &output_type) {
                violations.push(format!(
                    "{}::{} returns `Json<{}>` but `{}` does not declare an \
                     `audit_entry_id` field (Rust-side snake_case). The action-ID \
                     contract committed in `crate::admin::audit_chain` requires \
                     every Aurora-namespace handler that writes an audit chain \
                     entry to surface `audit_entry_id` on the typed Output struct \
                     (which serialises as `auditEntryId` via `rename_all = \
                     \"camelCase\"`). Add the field, OR add this handler to \
                     ALLOWLIST in this test with a documented justification.",
                    path, handler.name, output_type, output_type,
                ));
            }
        }
    }

    if !violations.is_empty() {
        let mut msg = String::from(
            "Aurora-namespace action-ID contract violations:\n\n",
        );
        for v in &violations {
            msg.push_str("  - ");
            msg.push_str(v);
            msg.push_str("\n\n");
        }
        msg.push_str(
            "Contract details: see `crate::admin::audit_chain` module doc \
             and `tests/admin_handler_contract.rs` for the lint shape.\n",
        );
        panic!("{}", msg);
    }
}

struct Handler {
    name: String,
    return_type: String,
    body: String,
}

/// Find every `async fn <name>(...) -> <return> { <body> }`
/// declaration in `source`. Accepts both `pub async fn` and
/// private `async fn` — Aurora-namespace handlers in
/// `src/api/admin.rs` are private (registered via the router but
/// not pub-exported), while handlers in `aurora_admin.rs` /
/// `aurora_moderator.rs` are typically `pub` so other modules can
/// reference them. Both shapes are valid handler declarations.
///
/// The return type and body are extracted via brace-counting
/// heuristics — sufficient for the hand-written async-fn shape
/// every current Aurora handler uses.
fn extract_handlers(source: &str) -> Vec<Handler> {
    let mut handlers = Vec::new();
    let needle = "async fn ";
    let mut cursor = 0;
    while let Some(rel) = source[cursor..].find(needle) {
        let start = cursor + rel;
        // Name: from after the needle to the next `(`.
        let after_needle = start + needle.len();
        let Some(paren) = source[after_needle..].find('(') else {
            break;
        };
        let name = source[after_needle..after_needle + paren].trim().to_string();
        // Skip generics — names like `foo<T>` aren't current shape.
        if name.contains('<') {
            cursor = after_needle + paren;
            continue;
        }
        // Find the `->` for the return type, then the next `{` that
        // opens the body. Both must precede the next `pub async fn`
        // declaration to be valid.
        let next_decl = source[after_needle + paren..]
            .find("async fn ")
            .map(|i| after_needle + paren + i)
            .unwrap_or(source.len());
        let arrow_search = &source[after_needle + paren..next_decl];
        let Some(arrow_off) = arrow_search.find("->") else {
            cursor = next_decl;
            continue;
        };
        let arrow = after_needle + paren + arrow_off + 2;
        let body_open_search = &source[arrow..next_decl];
        let Some(brace_off) = body_open_search.find('{') else {
            cursor = next_decl;
            continue;
        };
        let body_open = arrow + brace_off;
        let return_type = source[arrow..body_open].trim().to_string();
        // Body via brace-counting from body_open.
        let Some(body_close) = matching_brace(source, body_open) else {
            cursor = next_decl;
            continue;
        };
        let body = source[body_open + 1..body_close].to_string();
        handlers.push(Handler {
            name,
            return_type,
            body,
        });
        cursor = body_close + 1;
    }
    handlers
}

/// Return the index of the `}` that matches the `{` at `open_idx`,
/// counting braces while skipping string literals and line comments.
/// Sufficient for the hand-written async-fn bodies in the scan
/// targets; doesn't attempt to skip block comments or nested
/// multiline strings (none of the current Aurora handlers contain
/// pathological cases).
fn matching_brace(source: &str, open_idx: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes.get(open_idx) != Some(&b'{') {
        return None;
    }
    let mut depth: i32 = 0;
    let mut i = open_idx;
    let mut in_string = false;
    let mut in_char = false;
    let mut in_line_comment = false;
    while i < bytes.len() {
        let b = bytes[i];
        if in_line_comment {
            if b == b'\n' {
                in_line_comment = false;
            }
            i += 1;
            continue;
        }
        if in_string {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if in_char {
            if b == b'\\' {
                i += 2;
                continue;
            }
            if b == b'\'' {
                in_char = false;
            }
            i += 1;
            continue;
        }
        if b == b'/' && bytes.get(i + 1) == Some(&b'/') {
            in_line_comment = true;
            i += 2;
            continue;
        }
        if b == b'"' {
            in_string = true;
            i += 1;
            continue;
        }
        if b == b'\'' {
            // Char literal vs lifetime: a lifetime is `'ident` with
            // no closing quote. Look ahead to detect.
            if bytes.get(i + 2) == Some(&b'\'')
                || (bytes.get(i + 1) == Some(&b'\\') && bytes.get(i + 3) == Some(&b'\''))
            {
                in_char = true;
            }
            i += 1;
            continue;
        }
        if b == b'{' {
            depth += 1;
        } else if b == b'}' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Extract `<name>` from a return type that matches one of:
/// - `Result<Json<<name>>, ...>`
/// - `Result<(StatusCode, Json<<name>>), ...>`
/// Returns `None` if the return type doesn't carry a `Json<*Output>`.
fn extract_output_type(return_type: &str) -> Option<String> {
    let json_marker = "Json<";
    let json_off = return_type.find(json_marker)?;
    let after = &return_type[json_off + json_marker.len()..];
    // Take everything up to the first `>` that closes the Json<...>
    // — the lint expects a non-generic struct name, no nested `<>`.
    let close = after.find('>')?;
    let name = after[..close].trim();
    if name.is_empty() {
        return None;
    }
    Some(name.to_string())
}

/// Look in `source` for a struct definition named `struct_name`
/// (with `pub struct` or `struct` prefix) and return whether the
/// struct body literally declares an `audit_entry_id` field.
fn struct_has_audit_entry_id_field(source: &str, struct_name: &str) -> bool {
    // Find `pub struct <name>` or `struct <name>` with a `{` body.
    for prefix in &["pub struct ", "struct "] {
        let needle = format!("{}{}", prefix, struct_name);
        let mut cursor = 0;
        while let Some(rel) = source[cursor..].find(&needle) {
            let after = cursor + rel + needle.len();
            // Next non-whitespace must be either `<` (generics) or
            // `{` (body) — anything else means a different identifier
            // (e.g., `MyOutputThing` matched `MyOutput`).
            let trimmed = source[after..].trim_start();
            let trim_off = source[after..].len() - trimmed.len();
            if !trimmed.starts_with('{') && !trimmed.starts_with('<') {
                cursor = after;
                continue;
            }
            // Walk to the opening `{`; for generics like
            // `struct Foo<T> { ... }` skip past the `<...>` first.
            let body_open = if trimmed.starts_with('<') {
                let Some(gt) = trimmed.find('>').and_then(|i| i.checked_add(1)) else {
                    cursor = after;
                    continue;
                };
                let after_generics = &trimmed[gt..];
                let Some(brace_off) = after_generics.find('{') else {
                    cursor = after;
                    continue;
                };
                after + trim_off + gt + brace_off
            } else {
                after + trim_off
            };
            let Some(body_close) = matching_brace(source, body_open) else {
                cursor = after;
                continue;
            };
            let body = &source[body_open + 1..body_close];
            // Field declarations look like `pub audit_entry_id:`
            // or `audit_entry_id:`. Accept either. Use word
            // boundaries so we don't match e.g. `xaudit_entry_id`.
            if has_field_named(body, "audit_entry_id") {
                return true;
            }
            cursor = body_close + 1;
        }
    }
    false
}

fn has_field_named(struct_body: &str, field: &str) -> bool {
    for line in struct_body.lines() {
        // Strip leading whitespace and any `pub`/`pub(crate)`
        // qualifiers, then check for `<field>:`.
        let trimmed = line.trim_start();
        let stripped = trimmed
            .strip_prefix("pub(crate) ")
            .or_else(|| trimmed.strip_prefix("pub "))
            .unwrap_or(trimmed);
        if stripped.starts_with(field) {
            // Must be followed by `:` (with optional whitespace).
            let rest = &stripped[field.len()..];
            if rest.trim_start().starts_with(':') {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod self_tests {
    //! Unit tests for the lint helpers. Each helper is small and
    //! pure; these tests document the intended parsing semantics
    //! and catch regressions if a future refactor changes the
    //! brace-counting or field-extraction shape.
    use super::*;

    #[test]
    fn extract_output_type_picks_up_simple_json_output() {
        assert_eq!(
            extract_output_type("Result<Json<GrantRoleOutput>, (StatusCode, String)>")
                .as_deref(),
            Some("GrantRoleOutput"),
        );
    }

    #[test]
    fn extract_output_type_handles_status_tuple() {
        assert_eq!(
            extract_output_type(
                "Result<Json<EmitEventOutput>, (StatusCode, Json<serde_json::Value>)>",
            )
            .as_deref(),
            Some("EmitEventOutput"),
        );
    }

    #[test]
    fn extract_output_type_returns_none_when_no_json() {
        assert!(extract_output_type("Result<(), PdsError>").is_none());
    }

    #[test]
    fn has_field_named_detects_pub_field() {
        assert!(has_field_named("pub audit_entry_id: String,", "audit_entry_id"));
    }

    #[test]
    fn has_field_named_rejects_substring_match() {
        assert!(!has_field_named("pub xaudit_entry_id: String,", "audit_entry_id"));
        assert!(!has_field_named("pub audit_entry_id_other: String,", "audit_entry_id"));
    }
}
