//! atproto-OAuth holder device + grant management endpoints (Arc 2 Phase ε,
//! chainlink #422 / LOCKED §3.2 step 8).
//!
//! Browser-session-authenticated (login-α; β.2's [`BrowserSessionContext`])
//! endpoints under `/oauth/atproto/{device,grant}/*` that let a holder manage
//! their own registered devices and active OAuth grants. Every endpoint is
//! DID-scoped to the session holder (FC-2 — no cross-account read/write), and
//! every POST requires the session's CSRF token in the body (β.3 consent
//! discipline).

use axum::extract::State;
use axum::http::{header, HeaderMap};
use axum::Json;
use serde::{Deserialize, Serialize};
use sqlx::Row;

use super::browser_session::BrowserSessionContext;
use crate::context::AppContext;
use crate::error::{PdsError, PdsResult};

// ---------- device/register ----------

#[derive(Debug, Deserialize)]
pub struct RegisterDeviceRequest {
    /// The device's DPoP public key, JWK-serialised.
    pub dpop_public_key: String,
    /// The session's anti-CSRF token (β.2 `browser_session.csrf_token`).
    pub csrf_token: String,
    /// Optional holder-supplied label.
    #[serde(default)]
    pub device_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RegisterDeviceResponse {
    pub device_id: String,
    pub created_at: String,
}

/// `POST /oauth/atproto/device/register`
pub async fn register(
    session: BrowserSessionContext,
    State(ctx): State<AppContext>,
    headers: HeaderMap,
    Json(body): Json<RegisterDeviceRequest>,
) -> PdsResult<Json<RegisterDeviceResponse>> {
    check_csrf(&session, &body.csrf_token)?;
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok());
    let row = ctx
        .atproto_device_manager
        .register_device(
            &session.session.did,
            &body.dpop_public_key,
            body.device_name.as_deref(),
            user_agent,
        )
        .await?;
    Ok(Json(RegisterDeviceResponse {
        device_id: row.device_id,
        created_at: row.created_at,
    }))
}

// ---------- device/list ----------

#[derive(Debug, Serialize)]
pub struct DeviceListItem {
    pub device_id: String,
    pub device_name: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: String,
    pub last_seen_at: String,
}

#[derive(Debug, Serialize)]
pub struct DeviceListResponse {
    pub devices: Vec<DeviceListItem>,
}

/// `GET /oauth/atproto/device/list`
pub async fn list(
    session: BrowserSessionContext,
    State(ctx): State<AppContext>,
) -> PdsResult<Json<DeviceListResponse>> {
    let rows = ctx
        .atproto_device_manager
        .list_devices(&session.session.did)
        .await?;
    let devices = rows
        .into_iter()
        .map(|r| DeviceListItem {
            device_id: r.device_id,
            device_name: r.device_name,
            user_agent: r.user_agent,
            created_at: r.created_at,
            last_seen_at: r.last_seen_at,
        })
        .collect();
    Ok(Json(DeviceListResponse { devices }))
}

// ---------- device/revoke ----------

#[derive(Debug, Deserialize)]
pub struct RevokeDeviceRequest {
    pub device_id: String,
    pub csrf_token: String,
}

/// `POST /oauth/atproto/device/revoke` — soft-deletes the device and
/// cascade-revokes tokens bound to its DPoP key.
pub async fn revoke(
    session: BrowserSessionContext,
    State(ctx): State<AppContext>,
    Json(body): Json<RevokeDeviceRequest>,
) -> PdsResult<Json<serde_json::Value>> {
    check_csrf(&session, &body.csrf_token)?;
    ctx.atproto_device_manager
        .revoke_device(&session.session.did, &body.device_id)
        .await?;
    Ok(Json(serde_json::json!({ "revoked": true })))
}

// ---------- grant/list ----------

#[derive(Debug, Serialize)]
pub struct GrantItem {
    pub token_id: String,
    pub client_id: String,
    pub scope: String,
    pub device_id: Option<String>,
    pub issued_at: String,
    pub expires_at: String,
    pub last_used_at: String,
}

#[derive(Debug, Serialize)]
pub struct GrantListResponse {
    pub grants: Vec<GrantItem>,
}

/// `GET /oauth/atproto/grant/list` — the holder's active (non-revoked) tokens.
pub async fn grant_list(
    session: BrowserSessionContext,
    State(ctx): State<AppContext>,
) -> PdsResult<Json<GrantListResponse>> {
    let rows = sqlx::query(
        "SELECT token_id, client_id, scope, device_id, created_at, updated_at, expires_at \
         FROM token WHERE did = $1 AND NOT revoked ORDER BY created_at DESC",
    )
    .bind(&session.session.did)
    .fetch_all(&ctx.account_db)
    .await
    .map_err(PdsError::Database)?;

    let grants = rows
        .into_iter()
        .map(|r| GrantItem {
            token_id: r.get("token_id"),
            client_id: r.get("client_id"),
            scope: r.get("scope"),
            device_id: r.get("device_id"),
            issued_at: r.get("created_at"),
            expires_at: r.get("expires_at"),
            last_used_at: r.get("updated_at"),
        })
        .collect();
    Ok(Json(GrantListResponse { grants }))
}

// ---------- grant/revoke ----------

#[derive(Debug, Deserialize)]
pub struct RevokeGrantRequest {
    pub csrf_token: String,
    /// Revoke a specific token by id, OR all of a client's tokens by client_id.
    #[serde(default)]
    pub token_id: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
}

/// `POST /oauth/atproto/grant/revoke` — revoke a grant by `token_id` or all
/// grants for a `client_id`. DID-scoped: a holder can only revoke their own.
pub async fn grant_revoke(
    session: BrowserSessionContext,
    State(ctx): State<AppContext>,
    Json(body): Json<RevokeGrantRequest>,
) -> PdsResult<Json<serde_json::Value>> {
    check_csrf(&session, &body.csrf_token)?;
    let now = chrono::Utc::now().to_rfc3339();
    let did = &session.session.did;

    // `revoked = TRUE` is the dual-dialect literal (sqlite 1 / pg boolean).
    let affected = match (&body.token_id, &body.client_id) {
        (Some(token_id), _) => {
            sqlx::query(
                "UPDATE token SET revoked = TRUE, revoked_at = $1 \
                 WHERE did = $2 AND token_id = $3 AND NOT revoked",
            )
            .bind(&now)
            .bind(did)
            .bind(token_id)
            .execute(&ctx.account_db)
            .await
        }
        (None, Some(client_id)) => {
            sqlx::query(
                "UPDATE token SET revoked = TRUE, revoked_at = $1 \
                 WHERE did = $2 AND client_id = $3 AND NOT revoked",
            )
            .bind(&now)
            .bind(did)
            .bind(client_id)
            .execute(&ctx.account_db)
            .await
        }
        (None, None) => {
            return Err(PdsError::Validation(
                "grant/revoke requires either token_id or client_id".to_string(),
            ))
        }
    }
    .map_err(PdsError::Database)?;

    Ok(Json(serde_json::json!({ "revoked": affected.rows_affected() })))
}

/// The consent-endpoint CSRF discipline (β.3): the POSTed token must match the
/// session's per-session token. Mismatch → 403 with no hint about which half
/// was wrong.
fn check_csrf(session: &BrowserSessionContext, presented: &str) -> PdsResult<()> {
    if presented != session.session.csrf_token {
        return Err(PdsError::Authorization("CSRF token mismatch".to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::atproto::browser_session::{self, BrowserSession};

    async fn ctx() -> AppContext {
        crate::api::federation_peers::test_support::create_test_context_with(|_| {}).await
    }

    async fn seed_actor(ctx: &AppContext, did: &str) {
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind(did)
            .bind(format!("{}.example.com", did.replace(':', "-")))
            .bind("2026-01-01T00:00:00Z")
            .execute(&ctx.account_db)
            .await
            .unwrap();
    }

    async fn session_for(ctx: &AppContext, did: &str) -> BrowserSession {
        seed_actor(ctx, did).await;
        browser_session::create_session(&ctx.account_db, did, None, None)
            .await
            .unwrap()
    }

    fn sctx(session: BrowserSession) -> BrowserSessionContext {
        BrowserSessionContext { session }
    }

    fn jwk(x: &str) -> String {
        format!(r#"{{"kty":"EC","crv":"P-256","x":"{x}","y":"stubY"}}"#)
    }

    #[tokio::test]
    async fn register_requires_matching_csrf() {
        let ctx = ctx().await;
        let s = session_for(&ctx, "did:web:a.example.com").await;
        let err = register(
            sctx(s),
            State(ctx.clone()),
            HeaderMap::new(),
            Json(RegisterDeviceRequest {
                dpop_public_key: jwk("k1"),
                csrf_token: "wrong".to_string(),
                device_name: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, PdsError::Authorization(_)));
    }

    #[tokio::test]
    async fn register_list_revoke_flow_is_did_scoped() {
        let ctx = ctx().await;
        let did = "did:web:b.example.com";
        let s = session_for(&ctx, did).await;
        let csrf = s.csrf_token.clone();

        let reg = register(
            sctx(s.clone()),
            State(ctx.clone()),
            HeaderMap::new(),
            Json(RegisterDeviceRequest {
                dpop_public_key: jwk("bkey"),
                csrf_token: csrf.clone(),
                device_name: Some("phone".to_string()),
            }),
        )
        .await
        .expect("register")
        .0;

        let listed = list(sctx(s.clone()), State(ctx.clone())).await.unwrap().0;
        assert_eq!(listed.devices.len(), 1);
        assert_eq!(listed.devices[0].device_id, reg.device_id);
        assert_eq!(listed.devices[0].device_name.as_deref(), Some("phone"));

        // A different holder sees none of b's devices.
        let other = session_for(&ctx, "did:web:c.example.com").await;
        let other_list = list(sctx(other), State(ctx.clone())).await.unwrap().0;
        assert!(other_list.devices.is_empty());

        // Revoke → list empty.
        let _ = revoke(
            sctx(s.clone()),
            State(ctx.clone()),
            Json(RevokeDeviceRequest {
                device_id: reg.device_id.clone(),
                csrf_token: csrf,
            }),
        )
        .await
        .expect("revoke");
        let after = list(sctx(s), State(ctx.clone())).await.unwrap().0;
        assert!(after.devices.is_empty());
    }

    #[tokio::test]
    async fn grant_list_and_revoke_are_did_scoped() {
        let ctx = ctx().await;
        let did = "did:web:d.example.com";
        let s = session_for(&ctx, did).await;
        let csrf = s.csrf_token.clone();

        // Seed a token (grant) for the holder.
        sqlx::query(
            "INSERT INTO token (token_id, did, client_id, scope, created_at, updated_at, \
             expires_at, access_token_hash) VALUES ($1,$2,$3,$4,$5,$5,$6,$7)",
        )
        .bind("tok-d")
        .bind(did)
        .bind("https://app.example.com/cm.json")
        .bind("atproto")
        .bind("2026-01-01T00:00:00Z")
        .bind("2099-01-01T00:00:00Z")
        .bind("hash-d")
        .execute(&ctx.account_db)
        .await
        .unwrap();

        let grants = grant_list(sctx(s.clone()), State(ctx.clone())).await.unwrap().0;
        assert_eq!(grants.grants.len(), 1);
        assert_eq!(grants.grants[0].token_id, "tok-d");
        assert_eq!(grants.grants[0].client_id, "https://app.example.com/cm.json");

        // Revoke by token_id → grant list empties.
        let _ = grant_revoke(
            sctx(s.clone()),
            State(ctx.clone()),
            Json(RevokeGrantRequest {
                csrf_token: csrf,
                token_id: Some("tok-d".to_string()),
                client_id: None,
            }),
        )
        .await
        .expect("grant revoke");
        let after = grant_list(sctx(s), State(ctx.clone())).await.unwrap().0;
        assert!(after.grants.is_empty());
    }

    #[tokio::test]
    async fn grant_revoke_requires_a_criterion() {
        let ctx = ctx().await;
        let s = session_for(&ctx, "did:web:e.example.com").await;
        let csrf = s.csrf_token.clone();
        let err = grant_revoke(
            sctx(s),
            State(ctx.clone()),
            Json(RevokeGrantRequest {
                csrf_token: csrf,
                token_id: None,
                client_id: None,
            }),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, PdsError::Validation(_)));
    }
}
