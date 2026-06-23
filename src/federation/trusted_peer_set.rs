//! v0.9 Federation Pattern-1 Phase A — `TrustedPeerSet` (#351 / design §2.2).
//!
//! The consumer-facing read-site for the runtime-mutable peer allowlist. The
//! constitutional commitment (§1 "operator owns federation policy") is
//! delivered by routing every trust read through this type rather than the
//! static `FederationConfig.peer_pds` directly, so phases B+ can make the
//! allowlist runtime-mutable without re-touching consumers.
//!
//! **Per-call freshness (design §2.2 / commit 6, LB-3):** `is_trusted` reads
//! the runtime store fresh on every call — an operator removing a peer stops
//! trust immediately. `snapshot()` is the explicit point-in-time view for the
//! describe surface. Any future cache MUST satisfy the §7.4 property (post-
//! commit `is_trusted` returns the post-update truth); Phase A's direct-read
//! shape satisfies it trivially (no cache window exists).
//!
//! **Local-idiom translations (memory #18, recorded for the close report):**
//! - The design's sync `is_trusted(&self, did) -> bool` is **async** here —
//!   Aurora-Locus's runtime-settings read is a DB query (`account_db`), so
//!   "reads fresh every call" is inherently async.
//! - The design's `from_runtime_settings(&RuntimeSettings)` maps to
//!   `new(account_db, fallback)` — Aurora-Locus has no `RuntimeSettings`
//!   handle type; the runtime store is the `account_db` pool + the
//!   `FederationConfig.peer_pds` boot fallback.
//! - Phase A reads only the runtime *row* tier (+ config fallback); the row is
//!   always absent in Phase A (no seed, no CRUD), so behavior is unchanged
//!   from the static `peer_pds` reads it replaces.

use crate::config::PeerPdsConfig;
use crate::error::PdsResult;
use sqlx::{AnyPool, Row as _};
use std::sync::Arc;

use crate::api::aurora_admin::FEDERATION_POLICY_PEER_ALLOWLIST_KEY;

/// One trusted-peer entry (the resolved allowlist shape).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PeerEntry {
    pub did: String,
    pub url: String,
}

struct Inner {
    account_db: AnyPool,
    /// Boot fallback: `FederationConfig.peer_pds`, used when the runtime key is
    /// unset (always, in Phase A).
    fallback: Vec<PeerEntry>,
}

/// Live, runtime-backed trusted-peer set. Cheap to clone (`Arc` inner).
#[derive(Clone)]
pub struct TrustedPeerSet {
    inner: Arc<Inner>,
}

/// Point-in-time view of the trusted set (design §2.2 `snapshot()`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct TrustedPeerSnapshot {
    pub peers: Vec<PeerEntry>,
}

impl TrustedPeerSet {
    /// Construct from the runtime store handle (`account_db`) + the boot
    /// fallback (`FederationConfig.peer_pds`). The design's
    /// `from_runtime_settings` translated to the local substrate idiom.
    pub fn new(account_db: AnyPool, fallback: &[PeerPdsConfig]) -> Self {
        let fallback = fallback
            .iter()
            .map(|p| PeerEntry { did: p.did.clone(), url: p.url.clone() })
            .collect();
        Self {
            inner: Arc::new(Inner { account_db, fallback }),
        }
    }

    /// Resolve the current allowlist fresh: the runtime row if present (even
    /// an explicit empty array = "no peers trusted", per §2.4 "if set, use
    /// runtime store"), otherwise the config fallback ("if unset").
    async fn resolve(&self) -> Vec<PeerEntry> {
        let row: Option<String> =
            sqlx::query("SELECT value FROM runtime_settings WHERE key = $1")
                .bind(FEDERATION_POLICY_PEER_ALLOWLIST_KEY)
                .fetch_optional(&self.inner.account_db)
                .await
                .ok()
                .flatten()
                .and_then(|r| r.try_get::<String, _>("value").ok());
        match row {
            Some(value_str) => serde_json::from_str::<Vec<PeerEntry>>(&value_str)
                .unwrap_or_else(|_| self.inner.fallback.clone()),
            None => self.inner.fallback.clone(),
        }
    }

    /// Whether `did` is currently trusted. Per-call freshness (§2.2): reads the
    /// runtime store every call. Async because the read is a DB query.
    pub async fn is_trusted(&self, did: &str) -> bool {
        self.resolve().await.iter().any(|p| p.did == did)
    }

    /// Point-in-time snapshot for the describe surface / multi-check
    /// transactional consistency (§2.2).
    pub async fn snapshot(&self) -> TrustedPeerSnapshot {
        TrustedPeerSnapshot { peers: self.resolve().await }
    }
}

/// Multi-key boot-seed primitive (design §2.4 / commit 14, recon-verify §10.2
/// #3 — **multi-key transaction feasible**, so this is the primary path, not
/// the per-key-sequential fallback). Writes all `seeds` in ONE transaction:
/// all commit or none (atomic). Phase A invokes it with an empty slice (no-op
/// smoke test) — actual seeding from `FederationConfig` lands in phase B+ when
/// each key gets its first write.
pub async fn seed_federation_policy(
    account_db: &AnyPool,
    seeds: &[(String, serde_json::Value)],
    actor: &str,
) -> PdsResult<()> {
    if seeds.is_empty() {
        return Ok(()); // Phase A no-op: nothing to seed yet.
    }
    let now = chrono::Utc::now().to_rfc3339();
    let mut tx = account_db.begin().await?;
    for (key, value) in seeds {
        let encoded = serde_json::to_string(value).map_err(|e| {
            crate::error::PdsError::Internal(e.to_string())
        })?;
        // Seed only if absent (§2.4: seed when unset). Idempotent re-boot.
        sqlx::query(
            "INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) \
             SELECT $1, $2, $3, $4 \
             WHERE NOT EXISTS (SELECT 1 FROM runtime_settings WHERE key = $1)",
        )
        .bind(key)
        .bind(&encoded)
        .bind(&now)
        .bind(actor)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::aurora_admin::FEDERATION_POLICY_RELAY_URLS_KEY;

    async fn pool() -> AnyPool {
        sqlx::any::install_default_drivers();
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE runtime_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, \
             last_modified TEXT NOT NULL, last_modified_by TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn cfg(dids: &[&str]) -> Vec<PeerPdsConfig> {
        dids.iter()
            .map(|d| PeerPdsConfig { did: d.to_string(), url: format!("https://{}.example", d) })
            .collect()
    }

    #[tokio::test]
    async fn falls_back_to_config_when_runtime_unset() {
        let p = pool().await;
        let tps = TrustedPeerSet::new(p, &cfg(&["did:plc:a", "did:plc:b"]));
        assert!(tps.is_trusted("did:plc:a").await);
        assert!(tps.is_trusted("did:plc:b").await);
        assert!(!tps.is_trusted("did:plc:z").await);
        assert_eq!(tps.snapshot().await.peers.len(), 2);
    }

    #[tokio::test]
    async fn per_call_freshness_reflects_runtime_change() {
        let p = pool().await;
        let tps = TrustedPeerSet::new(p.clone(), &cfg(&["did:plc:a"]));
        assert!(tps.is_trusted("did:plc:a").await);
        // Simulate a phase-B operator mutation: set the runtime row to a new
        // set that drops :a and adds :c. Next call MUST reflect it (§7.4).
        sqlx::query("INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) VALUES ($1, $2, 'now', 'op')")
            .bind(FEDERATION_POLICY_PEER_ALLOWLIST_KEY)
            .bind(r#"[{"did":"did:plc:c","url":"https://c.example"}]"#)
            .execute(&p)
            .await
            .unwrap();
        assert!(!tps.is_trusted("did:plc:a").await, "removal takes effect immediately");
        assert!(tps.is_trusted("did:plc:c").await, "addition takes effect immediately");
    }

    #[tokio::test]
    async fn snapshot_is_point_in_time() {
        let p = pool().await;
        let tps = TrustedPeerSet::new(p.clone(), &cfg(&["did:plc:a"]));
        let snap = tps.snapshot().await;
        // Mutate the runtime store after taking the snapshot.
        sqlx::query("INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) VALUES ($1, '[]', 'now', 'op')")
            .bind(FEDERATION_POLICY_PEER_ALLOWLIST_KEY)
            .execute(&p)
            .await
            .unwrap();
        // The snapshot is unaffected; a fresh is_trusted reflects the change.
        assert!(snap.peers.iter().any(|p| p.did == "did:plc:a"));
        assert!(!tps.is_trusted("did:plc:a").await);
    }

    #[tokio::test]
    async fn explicit_empty_runtime_means_no_peers() {
        let p = pool().await;
        let tps = TrustedPeerSet::new(p.clone(), &cfg(&["did:plc:a"]));
        sqlx::query("INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) VALUES ($1, '[]', 'now', 'op')")
            .bind(FEDERATION_POLICY_PEER_ALLOWLIST_KEY)
            .execute(&p)
            .await
            .unwrap();
        // Present-but-empty = "operator trusts no peers", not "fall back".
        assert!(!tps.is_trusted("did:plc:a").await);
    }

    #[tokio::test]
    async fn seed_empty_is_noop_and_multi_key_seeds_when_absent() {
        let p = pool().await;
        // Phase A: empty seed is a clean no-op.
        seed_federation_policy(&p, &[], "did:system").await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runtime_settings").fetch_one(&p).await.unwrap();
        assert_eq!(n, 0);
        // Multi-key seed writes all keys atomically (the phase-B+ path).
        seed_federation_policy(
            &p,
            &[
                (FEDERATION_POLICY_PEER_ALLOWLIST_KEY.to_string(), serde_json::json!([{"did":"did:plc:a","url":"https://a.example"}])),
                (FEDERATION_POLICY_RELAY_URLS_KEY.to_string(), serde_json::json!(["https://relay.example"])),
            ],
            "did:system",
        )
        .await
        .unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runtime_settings").fetch_one(&p).await.unwrap();
        assert_eq!(n, 2);
        // Idempotent: a re-seed does not overwrite (seed-if-absent).
        seed_federation_policy(
            &p,
            &[(FEDERATION_POLICY_PEER_ALLOWLIST_KEY.to_string(), serde_json::json!([]))],
            "did:system",
        )
        .await
        .unwrap();
        let tps = TrustedPeerSet::new(p, &[]);
        assert!(tps.is_trusted("did:plc:a").await, "re-seed did not clobber the existing allowlist");
    }
}
