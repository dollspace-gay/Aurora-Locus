//! Localhost-only dev routes for development workflow.
//!
//! Compiled into debug builds only via `#[cfg(debug_assertions)]`.
//! Release builds do not include this module; the routes do not
//! exist on production binaries.
//!
//! Purpose: collapse the stop-PDS / cargo-run-CLI / restart-PDS
//! cycle for admin operations during local development and
//! Phase B sweeps. The CLI counterpart (`cargo run -- grant-admin`)
//! acquires a PDS-liveness lock that contends with any running
//! PDS — by design, so the offline grant path doesn't race the
//! live writer. The dev endpoints route through `AppContext`'s
//! existing pool and managers, so writes serialize naturally
//! against the live PDS instead of contending against it.
//!
//! Threat model: the `#[cfg(debug_assertions)]` gate IS the auth.
//! Localhost development is the trusted environment; release
//! builds never include these endpoints, so production deployment
//! risk is zero. Path namespace `dev.aurora.*` is List C by design
//! — NEVER registered in `RouteRegistry`, never advertised by
//! `tools.aurora.describeCapabilities`.
//!
//! See `docs/internal/dev-routes.md` for curl examples and a
//! typical workflow.

#![cfg(debug_assertions)]

use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use std::str::FromStr;

use crate::admin::roles::Role;
use crate::context::AppContext;
use crate::error::PdsError;

/// Mount the dev-only endpoint set.
///
/// Caller (`crate::api::mod`) merges the result into the top-level
/// router under its own `#[cfg(debug_assertions)]` gate, so this
/// module compiles away entirely in release builds.
pub fn routes() -> Router<AppContext> {
    Router::new()
        .route("/xrpc/dev.aurora.grantAdmin", post(grant_admin))
        .route("/xrpc/dev.aurora.revokeAdmin", post(revoke_admin))
        .route("/xrpc/dev.aurora.listAdmins", get(list_admins))
        .route("/xrpc/dev.aurora.createAccount", post(create_account))
        .route("/xrpc/dev.aurora.mintToken", post(mint_token))
        // Arc 12 §5.8.1 dev.aurora.federation.* — Phase B affordances.
        .route(
            "/xrpc/dev.aurora.federation.inspectAccount",
            get(fed_inspect_account),
        )
        .route(
            "/xrpc/dev.aurora.federation.listKnownPeers",
            get(fed_list_known_peers),
        )
        .route(
            "/xrpc/dev.aurora.federation.mintServiceToken",
            post(fed_mint_service_token),
        )
        .route(
            "/xrpc/dev.aurora.federation.simulateForward",
            post(fed_simulate_forward),
        )
}

/// Convert PdsError into an HTTP response. The dev surface is
/// developer-facing; surface the error message verbatim rather
/// than gating it behind operator-friendly translation.
fn http_error(e: PdsError) -> (StatusCode, String) {
    let status = match &e {
        PdsError::Validation(_) => StatusCode::BAD_REQUEST,
        PdsError::Conflict(_) => StatusCode::CONFLICT,
        PdsError::NotFound(_) => StatusCode::NOT_FOUND,
        PdsError::Authentication(_) => StatusCode::UNAUTHORIZED,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, e.to_string())
}

// ============================================================
// dev.aurora.grantAdmin
// ============================================================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrantAdminBody {
    did: String,
    /// Role string: `moderator` | `admin` | `superadmin`
    /// (case-insensitive — `Role::from_str` lowercases input).
    role: String,
    #[serde(default)]
    notes: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GrantAdminResponse {
    did: String,
    role: String,
    granted_at: String,
}

/// Grant admin role to a DID without stopping the PDS.
///
/// Routes through `AdminRoleManager::grant_role` — same code path
/// as `cargo run -- grant-admin`, minus the PDS-liveness lock that
/// the CLI uses to fail fast against a running PDS. Sharing
/// `ctx.account_db` means the dev endpoint's write serializes
/// against the live PDS's writes via the pool, which is the
/// correct concurrency story for a dev tool meant to be used
/// while the PDS is up.
async fn grant_admin(
    State(ctx): State<AppContext>,
    Json(body): Json<GrantAdminBody>,
) -> Result<Json<GrantAdminResponse>, (StatusCode, String)> {
    let role = Role::from_str(&body.role).map_err(http_error)?;
    let granted = ctx
        .admin_role_manager
        .grant_role(&body.did, role, "dev:grant-admin", body.notes)
        .await
        .map_err(http_error)?;
    Ok(Json(GrantAdminResponse {
        did: granted.did,
        role: granted.role.as_str().to_string(),
        granted_at: granted.granted_at.to_rfc3339(),
    }))
}

// ============================================================
// dev.aurora.revokeAdmin
// ============================================================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevokeAdminBody {
    did: String,
    /// Role argument preserved for symmetry with grantAdmin's body
    /// shape. The underlying revoke is keyed by DID (the schema
    /// permits one active role per DID), but a future variant
    /// might accept role to disambiguate revoked history. Pinned
    /// in the body shape now so the endpoint surface doesn't
    /// shift later.
    #[serde(default)]
    #[allow(dead_code)]
    role: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RevokeAdminResponse {
    did: String,
    revoked_at: String,
}

/// Revoke the active admin role for a DID.
async fn revoke_admin(
    State(ctx): State<AppContext>,
    Json(body): Json<RevokeAdminBody>,
) -> Result<Json<RevokeAdminResponse>, (StatusCode, String)> {
    ctx.admin_role_manager
        .revoke_role(&body.did, "dev:revoke-admin", body.reason)
        .await
        .map_err(http_error)?;
    Ok(Json(RevokeAdminResponse {
        did: body.did,
        revoked_at: chrono::Utc::now().to_rfc3339(),
    }))
}

// ============================================================
// dev.aurora.listAdmins
// ============================================================

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AdminRow {
    did: String,
    role: String,
    granted_by: Option<String>,
    granted_at: String,
    revoked: bool,
    revoked_at: Option<String>,
    revoked_by: Option<String>,
    notes: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListAdminsResponse {
    admins: Vec<AdminRow>,
}

/// Enumerate every row in `admin_roles` — active and revoked.
/// `AdminRoleManager::list_active_roles` only returns active rows;
/// the dev surface intentionally surfaces revoked history too so
/// developers can sanity-check what grants exist without sqlite3
/// in a second terminal.
async fn list_admins(
    State(ctx): State<AppContext>,
) -> Result<Json<ListAdminsResponse>, (StatusCode, String)> {
    let rows = sqlx::query(
        "SELECT did, role, granted_by, granted_at, revoked, revoked_at, revoked_by, notes \
         FROM admin_roles ORDER BY granted_at DESC",
    )
    .fetch_all(&ctx.account_db)
    .await
    .map_err(|e| http_error(PdsError::Database(e)))?;

    let mut admins = Vec::with_capacity(rows.len());
    for row in rows {
        let revoked = crate::db::read_bool(&row, "revoked")
            .map_err(|e| http_error(PdsError::Database(e)))?;
        admins.push(AdminRow {
            did: row.try_get("did").map_err(|e| http_error(PdsError::Database(e)))?,
            role: row.try_get("role").map_err(|e| http_error(PdsError::Database(e)))?,
            granted_by: row.try_get("granted_by").ok(),
            granted_at: row.try_get("granted_at").map_err(|e| http_error(PdsError::Database(e)))?,
            revoked,
            revoked_at: row.try_get("revoked_at").ok(),
            revoked_by: row.try_get("revoked_by").ok(),
            notes: row.try_get("notes").ok(),
        });
    }

    Ok(Json(ListAdminsResponse { admins }))
}

// ============================================================
// dev.aurora.createAccount
// ============================================================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateAccountBody {
    handle: String,
    email: String,
    password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateAccountResponse {
    did: String,
    handle: String,
    access_jwt: String,
}

/// Create a throwaway test account.
///
/// Bypasses the handler-layer invite-code check + the email-
/// verification token generation that `com.atproto.server.createAccount`
/// performs. Preserves the DB-invariant checks inside
/// `AccountManager::create_account` (handle uniqueness, email
/// uniqueness, password hashing, DID generation + actor row).
/// Initialises the repository so the account is usable
/// end-to-end. Returns the access JWT directly so the caller
/// doesn't need a follow-up createSession.
///
/// Note: if `config.invites.required = true`, the underlying
/// manager will still reject — its check runs unconditionally.
/// For typical localhost dev that flag is false (the default),
/// so the bypass is observable.
async fn create_account(
    State(ctx): State<AppContext>,
    Json(body): Json<CreateAccountBody>,
) -> Result<Json<CreateAccountResponse>, (StatusCode, String)> {
    let account = ctx
        .account_manager
        .create_account(body.handle, Some(body.email), body.password, None)
        .await
        .map_err(http_error)?;

    // Initialise the repository so this account is usable for
    // record writes immediately. Mirrors the production handler's
    // post-create step in `src/api/server.rs::create_account`.
    use crate::actor_store::RepositoryManager;
    let repo_mgr = RepositoryManager::with_validation_mode(
        account.did.clone(),
        (*ctx.actor_store).clone(),
        ctx.config.validation_mode,
    );
    repo_mgr.initialize().await.map_err(http_error)?;

    let session = ctx
        .account_manager
        .create_session(&account.did, None)
        .await
        .map_err(http_error)?;

    Ok(Json(CreateAccountResponse {
        did: account.did,
        handle: account.handle.unwrap_or_default(),
        access_jwt: session.access_token,
    }))
}

// ============================================================
// dev.aurora.mintToken
// ============================================================

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MintTokenBody {
    did: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MintTokenResponse {
    did: String,
    access_jwt: String,
}

/// Mint a fresh local-session JWT for the given DID.
///
/// `AdminAuthContext`'s Layer 1 (local-session) path validates
/// the JWT against the `session` table, looks up the DID, then
/// asks `admin_role_manager` whether the DID has an admin grant
/// (`src/auth.rs:230-332`). Admin authority is NOT baked into
/// the JWT itself — it's queried at request time from
/// `admin_roles`. So minting a fresh session for a DID that
/// has an admin grant is exactly what the
/// "grant-then-immediately-use" workflow needs: the existing
/// `create_session` produces the access_token, and the grant
/// lookup picks up the new role as soon as the row lands.
///
/// 404 if no actor row exists for the DID — sessions reference
/// the actor table and a session for a non-existent account
/// would be useless even if it minted.
async fn mint_token(
    State(ctx): State<AppContext>,
    Json(body): Json<MintTokenBody>,
) -> Result<Json<MintTokenResponse>, (StatusCode, String)> {
    // Verify the DID has a backing account row so the minted
    // token isn't useless.
    let _account = ctx
        .account_manager
        .get_account(&body.did)
        .await
        .map_err(http_error)?;

    let session = ctx
        .account_manager
        .create_session(&body.did, None)
        .await
        .map_err(http_error)?;

    Ok(Json(MintTokenResponse {
        did: body.did,
        access_jwt: session.access_token,
    }))
}

// ============================================================
// Arc 12 §5.8.1 — dev.aurora.federation.*
// Phase B affordances. All four behind the file-level
// `#[cfg(debug_assertions)]` gate; release builds return 404.
// ============================================================

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct FedInspectAccountQuery {
    did: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FedInspectAccountResponse {
    did: String,
    actor_present: bool,
    handle: Option<String>,
    /// Arc 13 Step 0.7.1 removed the per-account rotation_key
    /// column; the PDS-wide rotation key lives in config and is
    /// not surfaced per-account here.
    has_atproto_signing_key: bool,
    is_service_did: bool,
    is_peer_pds: bool,
    is_entryway_did: bool,
}

/// Arc 12 §5.8.1 — inspect an account's identity surface:
/// DID, handle, key-column presence, and trusted-iss-set membership.
/// Read-only; safe to call repeatedly during Phase B.
async fn fed_inspect_account(
    State(ctx): State<AppContext>,
    axum::extract::Query(q): axum::extract::Query<FedInspectAccountQuery>,
) -> Result<Json<FedInspectAccountResponse>, (StatusCode, String)> {
    let actor_row: Option<(String,)> =
        sqlx::query_as("SELECT handle FROM actor WHERE did = $1")
            .bind(&q.did)
            .fetch_optional(&ctx.account_db)
            .await
            .map_err(|e| http_error(PdsError::Database(e)))?;
    let (actor_present, handle) = match actor_row {
        Some((h,)) => (true, Some(h)),
        None => (false, None),
    };

    let plc_row: Option<(String,)> = sqlx::query_as(
        "SELECT atproto_signing_key FROM plc_keys WHERE did = $1",
    )
    .bind(&q.did)
    .fetch_optional(&ctx.account_db)
    .await
    .map_err(|e| http_error(PdsError::Database(e)))?;
    let has_atproto_signing_key = match plc_row {
        Some((atp,)) => !atp.is_empty(),
        None => false,
    };

    let is_service_did = q.did == ctx.service_did();
    let is_peer_pds = ctx
        .config
        .federation
        .peer_pds
        .iter()
        .any(|p| p.did == q.did);
    let is_entryway_did = ctx.entryway_did() == Some(q.did.as_str());

    Ok(Json(FedInspectAccountResponse {
        did: q.did,
        actor_present,
        handle,
        has_atproto_signing_key,
        is_service_did,
        is_peer_pds,
        is_entryway_did,
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FedListKnownPeersResponse {
    peers: Vec<FedPeerEntry>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FedPeerEntry {
    did: String,
    url: String,
    /// Source of the registration: `config` (Step 0.5 Gap 3
    /// bootstrap from `PDS_FEDERATION_PEER_PDS`) vs `discovery`
    /// (runtime via `PdsDiscovery::add_instance`).
    source: &'static str,
}

/// Arc 12 §5.8.1 — enumerate this PDS's registered peer-PDS map.
/// In standalone mode with no `peer_pds` configured returns an
/// empty list (200 OK). When `PdsDiscovery` is wired (federation
/// enabled), iterates its known instances; otherwise falls back
/// to the static `config.federation.peer_pds` list.
async fn fed_list_known_peers(
    State(ctx): State<AppContext>,
) -> Result<Json<FedListKnownPeersResponse>, (StatusCode, String)> {
    let mut peers: Vec<FedPeerEntry> = ctx
        .config
        .federation
        .peer_pds
        .iter()
        .map(|p| FedPeerEntry {
            did: p.did.clone(),
            url: p.url.clone(),
            source: "config",
        })
        .collect();

    if let Some(discovery) = ctx.pds_discovery.as_ref() {
        let instances = discovery.get_known_instances().await;
        for inst in instances {
            // De-dupe by DID against the config-bootstrap list.
            if !peers.iter().any(|p| p.did == inst.did) {
                peers.push(FedPeerEntry {
                    did: inst.did,
                    url: inst.url,
                    source: "discovery",
                });
            }
        }
    }

    Ok(Json(FedListKnownPeersResponse { peers }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FedMintServiceTokenBody {
    user_did: String,
    aud: String,
    lxm: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FedMintServiceTokenResponse {
    access_jwt: String,
}

/// Arc 12 §5.8.1 — mint a service-auth JWT against a specified
/// `user_did + aud + lxm` for Phase B exercise. Reads
/// `plc_keys.atproto_signing_key` (Step 1.5 column) and signs an
/// ES256K JWT with `kid="aurora-local-v1"`. Same crypto path as
/// `entryway_auth_headers`, but `aud` is caller-supplied so Phase
/// B scripts can drive arbitrary audiences (e.g., a stub
/// entryway DID, the test PDS's own DID, etc.) without standing
/// up an entryway configuration first.
async fn fed_mint_service_token(
    State(ctx): State<AppContext>,
    Json(body): Json<FedMintServiceTokenBody>,
) -> Result<Json<FedMintServiceTokenResponse>, (StatusCode, String)> {
    let headers = crate::federation::entryway_auth_headers(
        &ctx.account_db,
        &body.user_did,
        &body.aud,
        &body.lxm,
    )
    .await
    .map_err(http_error)?;
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .ok_or_else(|| {
            http_error(PdsError::Internal(
                "entryway_auth_headers returned without Authorization header".to_string(),
            ))
        })?
        .to_str()
        .map_err(|e| http_error(PdsError::Internal(format!("non-ASCII header: {}", e))))?
        .to_string();
    let jwt = auth
        .strip_prefix("Bearer ")
        .ok_or_else(|| {
            http_error(PdsError::Internal(
                "Authorization header missing Bearer prefix".to_string(),
            ))
        })?
        .to_string();
    Ok(Json(FedMintServiceTokenResponse { access_jwt: jwt }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FedSimulateForwardBody {
    /// One of the four §5.3.8 forwarded NSIDs.
    nsid: String,
    /// Caller-supplied user_did. Required for mint-pattern NSIDs;
    /// ignored for `requestPasswordReset`.
    user_did: Option<String>,
    /// Stub URL — typically a localhost echo server during Phase B
    /// so the test can read back what Aurora-Locus actually sent.
    stub_url: String,
    /// Opaque body to forward. Defaults to `{}` if omitted.
    #[serde(default)]
    body: serde_json::Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FedSimulateForwardResponse {
    /// Final outbound URL Aurora-Locus would POST/GET against.
    outbound_url: String,
    /// Headers Aurora-Locus would attach. Captured pre-send.
    headers: std::collections::BTreeMap<String, String>,
    /// Upstream response status code from the stub.
    stub_status: u16,
    /// Upstream response body (text). May be empty.
    stub_body: String,
}

/// Arc 12 §5.8.1 — exercise the forwarded-handler dispatch against
/// a caller-supplied stub URL. Constructs the auth/passthru headers
/// per the NSID's pattern, sends to the stub, and returns
/// (outbound_url, headers, stub_status, stub_body) for inspection.
///
/// Useful Phase B scenarios: point at a local echo server, mint
/// the headers a real forwarded call would carry, and confirm
/// shape without standing up a full entryway.
async fn fed_simulate_forward(
    State(ctx): State<AppContext>,
    Json(body): Json<FedSimulateForwardBody>,
) -> Result<Json<FedSimulateForwardResponse>, (StatusCode, String)> {
    let mint_nsids = [
        "com.atproto.identity.signPlcOperation",
        "com.atproto.identity.updateHandle",
        "com.atproto.server.getSession",
    ];
    let passthru_nsids = ["com.atproto.server.requestPasswordReset"];
    let is_mint = mint_nsids.contains(&body.nsid.as_str());
    let is_passthru = passthru_nsids.contains(&body.nsid.as_str());
    if !is_mint && !is_passthru {
        return Err(http_error(PdsError::Validation(format!(
            "nsid {:?} is not a §5.3.8 forwarded handler",
            body.nsid
        ))));
    }

    let headers_axum = if is_mint {
        let user_did = body.user_did.as_deref().ok_or_else(|| {
            http_error(PdsError::Validation(
                "mint-pattern nsid requires userDid".to_string(),
            ))
        })?;
        // Use the requested stub URL's host as a synthetic aud so
        // the JWT is exercise-shaped without requiring a configured
        // entryway. The aud is just a string here; the stub doesn't
        // verify it.
        let synthetic_aud = format!("did:web:{}", host_from_url(&body.stub_url));
        crate::federation::entryway_auth_headers(
            &ctx.account_db,
            user_did,
            &synthetic_aud,
            &body.nsid,
        )
        .await
        .map_err(http_error)?
    } else {
        // Passthru handler: simulate an empty incoming request set
        // (no auth header, no proxy headers). Real Phase B drivers
        // can prepend headers to the stub side.
        crate::federation::entryway_passthru_headers(&axum::http::HeaderMap::new(), None)
            .map_err(http_error)?
    };

    let outbound_url = format!("{}/xrpc/{}", body.stub_url.trim_end_matches('/'), body.nsid);
    let mut headers_out: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for (k, v) in headers_axum.iter() {
        if let Ok(v_str) = v.to_str() {
            headers_out.insert(k.as_str().to_string(), v_str.to_string());
        }
    }

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| http_error(PdsError::Internal(format!("reqwest: {}", e))))?;
    let mut req = http.post(&outbound_url).json(&body.body);
    for (k, v) in headers_axum.iter() {
        if let (Ok(name), Ok(value)) = (
            reqwest::header::HeaderName::from_bytes(k.as_str().as_bytes()),
            reqwest::header::HeaderValue::from_bytes(v.as_bytes()),
        ) {
            req = req.header(name, value);
        }
    }
    let resp = req
        .send()
        .await
        .map_err(|e| http_error(PdsError::Internal(format!("stub call failed: {}", e))))?;
    let status = resp.status().as_u16();
    let body_text = resp.text().await.unwrap_or_default();

    Ok(Json(FedSimulateForwardResponse {
        outbound_url,
        headers: headers_out,
        stub_status: status,
        stub_body: body_text,
    }))
}

fn host_from_url(url: &str) -> String {
    let no_scheme = url
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    no_scheme
        .split('/')
        .next()
        .unwrap_or(no_scheme)
        .to_string()
}
