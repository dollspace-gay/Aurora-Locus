//! atproto-OAuth device registry (Arc 2 Phase ε, chainlink #422 / LOCKED §3.2
//! step 8 / β Inv-9 finding).
//!
//! A device is a durable holder-side identity — it persists across many issued
//! tokens, carries a holder-supplied name, and holds the DPoP public key the
//! device signs its bearer-bound requests with. Phase ε.3 gates general-XRPC
//! OAuth-bearer access on the DPoP proof key matching a registered (non-revoked)
//! device row for the bearer's DID.
//!
//! Backs the `atproto_device` table (sqlite migration `0031` / pg `0032`) — a
//! did-keyed table dedicated to the atproto provider, parallel to the legacy
//! `device` / `account_device` tables (strangler-fig: SD-A2 = (c)). All SQL
//! touching `atproto_device` lives here.

use chrono::Utc;
use sqlx::{AnyPool, FromRow};
use uuid::Uuid;

use crate::error::{PdsError, PdsResult};

/// One `atproto_device` row. Mirrors the migration column-for-column; nullable
/// columns are `Option`.
#[derive(Debug, Clone, FromRow, PartialEq, Eq)]
pub struct AtprotoDeviceRow {
    pub device_id: String,
    pub did: String,
    pub dpop_public_key: String,
    pub dpop_jkt: String,
    pub device_name: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: String,
    pub last_seen_at: String,
    pub revoked_at: Option<String>,
}

const COLUMNS: &str = "device_id, did, dpop_public_key, dpop_jkt, device_name, \
     user_agent, created_at, last_seen_at, revoked_at";

/// Registry of atproto-OAuth holder devices.
#[derive(Clone)]
pub struct AtprotoDeviceManager {
    db: AnyPool,
}

impl AtprotoDeviceManager {
    pub fn new(db: AnyPool) -> Self {
        Self { db }
    }

    /// Register a new device for `did`. `dpop_public_key` is the device's DPoP
    /// public key as a JWK JSON string; its RFC 7638 thumbprint (the same one
    /// `verify_dpop_proof` computes for incoming proofs) becomes the row's
    /// `dpop_jkt` lookup key. Rejects a JWK already active on another device
    /// (one key = one active device — `Conflict`).
    pub async fn register_device(
        &self,
        did: &str,
        dpop_public_key: &str,
        device_name: Option<&str>,
        user_agent: Option<&str>,
    ) -> PdsResult<AtprotoDeviceRow> {
        let jwk: serde_json::Value = serde_json::from_str(dpop_public_key)
            .map_err(|e| PdsError::Validation(format!("dpop_public_key is not valid JWK JSON: {e}")))?;
        let jkt = crate::federation::dpop::compute_jwk_thumbprint(&jwk)?;

        // One active device per key (global). Pre-check for a clear error rather
        // than surfacing the unique-index violation.
        if self.jkt_is_active(&jkt).await? {
            return Err(PdsError::Conflict(
                "this DPoP key is already registered to an active device".to_string(),
            ));
        }

        let now = Utc::now().to_rfc3339();
        let row = AtprotoDeviceRow {
            device_id: Uuid::new_v4().to_string(),
            did: did.to_string(),
            dpop_public_key: dpop_public_key.to_string(),
            dpop_jkt: jkt,
            device_name: device_name.map(|s| s.to_string()),
            user_agent: user_agent.map(|s| s.to_string()),
            created_at: now.clone(),
            last_seen_at: now,
            revoked_at: None,
        };
        sqlx::query(
            "INSERT INTO atproto_device (device_id, did, dpop_public_key, dpop_jkt, \
             device_name, user_agent, created_at, last_seen_at, revoked_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(&row.device_id)
        .bind(&row.did)
        .bind(&row.dpop_public_key)
        .bind(&row.dpop_jkt)
        .bind(&row.device_name)
        .bind(&row.user_agent)
        .bind(&row.created_at)
        .bind(&row.last_seen_at)
        .bind(&row.revoked_at)
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;
        Ok(row)
    }

    /// List a holder's active (non-revoked) devices, most-recently-seen first.
    pub async fn list_devices(&self, did: &str) -> PdsResult<Vec<AtprotoDeviceRow>> {
        let sql = format!(
            "SELECT {COLUMNS} FROM atproto_device \
             WHERE did = $1 AND revoked_at IS NULL ORDER BY last_seen_at DESC"
        );
        sqlx::query_as::<_, AtprotoDeviceRow>(&sql)
            .bind(did)
            .fetch_all(&self.db)
            .await
            .map_err(PdsError::Database)
    }

    /// Look up an ACTIVE device by DPoP thumbprint, scoped to `did` — the ε.3
    /// registry gate. Returns `None` for an unknown, revoked, or wrong-holder
    /// thumbprint.
    pub async fn get_device_by_jkt(
        &self,
        did: &str,
        jkt: &str,
    ) -> PdsResult<Option<AtprotoDeviceRow>> {
        let sql = format!(
            "SELECT {COLUMNS} FROM atproto_device \
             WHERE did = $1 AND dpop_jkt = $2 AND revoked_at IS NULL"
        );
        sqlx::query_as::<_, AtprotoDeviceRow>(&sql)
            .bind(did)
            .bind(jkt)
            .fetch_optional(&self.db)
            .await
            .map_err(PdsError::Database)
    }

    /// Refresh `last_seen_at` (called by the ε.3 gate on each successful bearer
    /// request). Best-effort activity tracking; not did-scoped because the
    /// caller already resolved the device by (did, jkt).
    pub async fn touch(&self, device_id: &str) -> PdsResult<()> {
        sqlx::query("UPDATE atproto_device SET last_seen_at = $1 WHERE device_id = $2")
            .bind(Utc::now().to_rfc3339())
            .bind(device_id)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;
        Ok(())
    }

    /// Revoke a holder's device (soft-delete) AND cascade-revoke every token
    /// bound to that device's DPoP key. After this, no bearer whose DPoP proof
    /// uses this key can pass the ε.3 gate, and its tokens are `revoked`.
    /// `NotFound` if the holder has no active device with that id.
    pub async fn revoke_device(&self, did: &str, device_id: &str) -> PdsResult<()> {
        // Fetch the active row for its jkt (also enforces did-scoping + liveness).
        let sql = format!(
            "SELECT {COLUMNS} FROM atproto_device \
             WHERE did = $1 AND device_id = $2 AND revoked_at IS NULL"
        );
        let device = sqlx::query_as::<_, AtprotoDeviceRow>(&sql)
            .bind(did)
            .bind(device_id)
            .fetch_optional(&self.db)
            .await
            .map_err(PdsError::Database)?
            .ok_or_else(|| {
                PdsError::NotFound(format!("no active device {device_id} for this account"))
            })?;

        let now = Utc::now().to_rfc3339();
        sqlx::query("UPDATE atproto_device SET revoked_at = $1 WHERE device_id = $2")
            .bind(&now)
            .bind(device_id)
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;

        // Cascade: revoke tokens bound to this device's key (jkt ==
        // token.dpop_thumbprint). `revoked = TRUE` is the dual-dialect literal.
        sqlx::query(
            "UPDATE token SET revoked = TRUE, revoked_at = $1 \
             WHERE did = $2 AND dpop_thumbprint = $3 AND NOT revoked",
        )
        .bind(&now)
        .bind(did)
        .bind(&device.dpop_jkt)
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;
        Ok(())
    }

    /// True iff some ACTIVE device already holds this thumbprint (any holder).
    async fn jkt_is_active(&self, jkt: &str) -> PdsResult<bool> {
        let row = sqlx::query_scalar::<_, String>(
            "SELECT device_id FROM atproto_device WHERE dpop_jkt = $1 AND revoked_at IS NULL",
        )
        .bind(jkt)
        .fetch_optional(&self.db)
        .await
        .map_err(PdsError::Database)?;
        Ok(row.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn ctx() -> crate::context::AppContext {
        crate::api::federation_peers::test_support::create_test_context_with(|_| {}).await
    }

    /// atproto_device.did has an FK to actor(did); seed the actor first.
    async fn seed_actor(db: &AnyPool, did: &str) {
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind(did)
            .bind(format!("{}.example.com", did.replace(':', "-")))
            .bind("2026-01-01T00:00:00Z")
            .execute(db)
            .await
            .unwrap();
    }

    fn jwk(x: &str) -> String {
        // A synthetic P-256 JWK — the manager only thumbprints/stores it.
        format!(r#"{{"kty":"EC","crv":"P-256","x":"{x}","y":"stubY"}}"#)
    }

    #[tokio::test]
    async fn register_list_revoke_round_trip() {
        let ctx = ctx().await;
        let did = "did:web:alice.example.com";
        seed_actor(&ctx.account_db, did).await;
        let mgr = AtprotoDeviceManager::new(ctx.account_db.clone());

        let dev = mgr
            .register_device(did, &jwk("keyAAA"), Some("laptop"), Some("curl/8"))
            .await
            .expect("register");
        assert_eq!(dev.did, did);
        assert!(!dev.dpop_jkt.is_empty());
        assert_eq!(dev.device_name.as_deref(), Some("laptop"));

        // Listed for the holder.
        let list = mgr.list_devices(did).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].device_id, dev.device_id);

        // Found by (did, jkt); not for a wrong did; gone after revoke.
        assert!(mgr.get_device_by_jkt(did, &dev.dpop_jkt).await.unwrap().is_some());
        assert!(mgr
            .get_device_by_jkt("did:web:bob.example.com", &dev.dpop_jkt)
            .await
            .unwrap()
            .is_none());

        // touch advances last_seen_at.
        sqlx::query("UPDATE atproto_device SET last_seen_at = '2000-01-01T00:00:00Z' WHERE device_id = $1")
            .bind(&dev.device_id)
            .execute(&ctx.account_db)
            .await
            .unwrap();
        mgr.touch(&dev.device_id).await.unwrap();
        let seen = mgr
            .get_device_by_jkt(did, &dev.dpop_jkt)
            .await
            .unwrap()
            .unwrap()
            .last_seen_at;
        assert!(seen.as_str() > "2000-01-01T00:00:00Z");

        // Revoke → gone from the active list + not found by jkt.
        mgr.revoke_device(did, &dev.device_id).await.unwrap();
        assert!(mgr.list_devices(did).await.unwrap().is_empty());
        assert!(mgr.get_device_by_jkt(did, &dev.dpop_jkt).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn duplicate_active_jkt_conflicts_but_revoked_can_reregister() {
        let ctx = ctx().await;
        let did = "did:web:carol.example.com";
        seed_actor(&ctx.account_db, did).await;
        let mgr = AtprotoDeviceManager::new(ctx.account_db.clone());

        let d1 = mgr.register_device(did, &jwk("dup"), None, None).await.unwrap();
        // Same key, still active → Conflict.
        let err = mgr
            .register_device(did, &jwk("dup"), None, None)
            .await
            .unwrap_err();
        assert!(matches!(err, PdsError::Conflict(_)));
        // After revoking the first, the key can be registered again.
        mgr.revoke_device(did, &d1.device_id).await.unwrap();
        assert!(mgr.register_device(did, &jwk("dup"), None, None).await.is_ok());
    }

    #[tokio::test]
    async fn revoke_unknown_device_is_not_found() {
        let ctx = ctx().await;
        let did = "did:web:dave.example.com";
        seed_actor(&ctx.account_db, did).await;
        let mgr = AtprotoDeviceManager::new(ctx.account_db.clone());
        let err = mgr.revoke_device(did, "no-such-device").await.unwrap_err();
        assert!(matches!(err, PdsError::NotFound(_)));
    }

    #[tokio::test]
    async fn revoke_device_cascades_token_revocation() {
        let ctx = ctx().await;
        let did = "did:web:erin.example.com";
        seed_actor(&ctx.account_db, did).await;
        let mgr = AtprotoDeviceManager::new(ctx.account_db.clone());
        let dev = mgr.register_device(did, &jwk("erinkey"), None, None).await.unwrap();

        // A token bound to the device's key (dpop_thumbprint == device jkt).
        sqlx::query(
            "INSERT INTO token (token_id, did, client_id, scope, created_at, updated_at, \
             expires_at, dpop_thumbprint, access_token_hash) \
             VALUES ($1,$2,$3,$4,$5,$5,$6,$7,$8)",
        )
        .bind("tok-erin")
        .bind(did)
        .bind("https://app/cm.json")
        .bind("atproto")
        .bind("2026-01-01T00:00:00Z")
        .bind("2099-01-01T00:00:00Z")
        .bind(&dev.dpop_jkt)
        .bind("hash-erin")
        .execute(&ctx.account_db)
        .await
        .unwrap();

        mgr.revoke_device(did, &dev.device_id).await.unwrap();

        // The cascade revoked the bound token.
        let revoked: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM token WHERE token_id = 'tok-erin' AND revoked",
        )
        .fetch_one(&ctx.account_db)
        .await
        .unwrap();
        assert_eq!(revoked, 1, "token bound to the revoked device must be revoked");
    }
}
