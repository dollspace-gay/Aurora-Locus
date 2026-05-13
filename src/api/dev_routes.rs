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
