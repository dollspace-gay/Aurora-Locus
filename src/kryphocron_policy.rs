//! Deployment-wide Kryphocron policy consumers (#334 / design §6.6.2).
//!
//! The Kryphocron Policy page sets deployment policy as runtime-settings rows
//! (registered in [`crate::api::aurora_admin`]); this module reads those rows at
//! the substrate decision points that enforce them. Distinct from
//! [`crate::kryphocron_override`], which is the *per-account* override table —
//! this is the deployment default each account is measured against.
//!
//! Reads are **fail-soft**: a settings-store read error resolves to the
//! permissive default (the policy's absence never hard-blocks a write the
//! operator would otherwise allow). Values are stored JSON-encoded in
//! `runtime_settings.value` (e.g. `"\"delayed\""`, `"7"`), mirroring the
//! rotation-oracle's read in `context.rs`.

use crate::api::aurora_admin::{
    KRYPHOCRON_ACCESS_DELAY_DAYS_KEY, KRYPHOCRON_NEW_ACCOUNT_ACCESS_KEY,
};
use chrono::{DateTime, Duration, Utc};
use sqlx::{AnyPool, Row};

/// Read and JSON-decode a runtime-setting value, or `None` if absent/unreadable.
async fn read_runtime_value(pool: &AnyPool, key: &str) -> Option<serde_json::Value> {
    let raw: Option<String> = sqlx::query("SELECT value FROM runtime_settings WHERE key = $1")
        .bind(key)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .and_then(|r| r.try_get::<String, _>("value").ok());
    raw.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
}

/// New-account private-write access gate (§6.6.2 item 1). Returns
/// `Some(days_remaining)` when the deployment policy is `delayed` and `did` is
/// still inside its delay window, else `None` (allowed). The caller turns
/// `Some` into a clear authorization error.
///
/// Fail-soft and default-open: `immediate` (the default), an unreadable policy,
/// or an account whose `created_at` can't be resolved all return `None` — the
/// guard only ever *adds* friction the operator explicitly configured, and a
/// store hiccup can't wedge private writes deployment-wide.
pub async fn new_account_access_delay_remaining(
    pool: &AnyPool,
    did: &str,
    now: DateTime<Utc>,
) -> Option<i64> {
    // Only `delayed` gates; immediate/unset/unreadable are open.
    let policy = read_runtime_value(pool, KRYPHOCRON_NEW_ACCOUNT_ACCESS_KEY).await;
    if policy.as_ref().and_then(|v| v.as_str()) != Some("delayed") {
        return None;
    }
    let days = read_runtime_value(pool, KRYPHOCRON_ACCESS_DELAY_DAYS_KEY)
        .await
        .and_then(|v| v.as_i64())
        .filter(|n| *n > 0)
        .unwrap_or(7);

    let created_raw: Option<String> = sqlx::query("SELECT created_at FROM actor WHERE did = $1")
        .bind(did)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .and_then(|r| r.try_get::<String, _>("created_at").ok());
    let Some(created_at) = created_raw
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
    else {
        return None; // unknown/unparseable account age → don't block here
    };

    let unlock = created_at + Duration::days(days);
    if now < unlock {
        // Whole days remaining, rounded up (so a partial final day still reads
        // ≥ 1 while locked, and an exact N-day remainder reads N).
        let secs = (unlock - now).num_seconds();
        Some((secs + 86_399) / 86_400)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;

    async fn pool() -> AnyPool {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
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
        sqlx::query(
            "CREATE TABLE actor (did TEXT PRIMARY KEY, handle TEXT NOT NULL, created_at TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn set(pool: &AnyPool, key: &str, json_value: &str) {
        sqlx::query(
            "INSERT INTO runtime_settings (key, value, last_modified, last_modified_by) \
             VALUES ($1, $2, '2026-06-21T00:00:00Z', 'did:plc:op')",
        )
        .bind(key)
        .bind(json_value)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn add_actor(pool: &AnyPool, did: &str, created_at: &str) {
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $1, $2)")
            .bind(did)
            .bind(created_at)
            .execute(pool)
            .await
            .unwrap();
    }

    fn now() -> DateTime<Utc> {
        "2026-06-21T00:00:00Z".parse().unwrap()
    }

    #[tokio::test]
    async fn immediate_or_unset_policy_never_delays() {
        let p = pool().await;
        add_actor(&p, "did:plc:new", "2026-06-20T00:00:00Z").await; // 1 day old
        // Unset → open.
        assert_eq!(new_account_access_delay_remaining(&p, "did:plc:new", now()).await, None);
        // Explicit immediate → open.
        set(&p, KRYPHOCRON_NEW_ACCOUNT_ACCESS_KEY, "\"immediate\"").await;
        assert_eq!(new_account_access_delay_remaining(&p, "did:plc:new", now()).await, None);
    }

    #[tokio::test]
    async fn delayed_policy_blocks_young_and_allows_aged() {
        let p = pool().await;
        set(&p, KRYPHOCRON_NEW_ACCOUNT_ACCESS_KEY, "\"delayed\"").await;
        set(&p, KRYPHOCRON_ACCESS_DELAY_DAYS_KEY, "7").await;
        // 2 days old under a 7-day window → blocked, ~5 days remaining.
        add_actor(&p, "did:plc:young", "2026-06-19T00:00:00Z").await;
        let rem = new_account_access_delay_remaining(&p, "did:plc:young", now()).await;
        assert_eq!(rem, Some(5), "exactly 5 whole days remain in the 7-day window");
        // 10 days old → past the window → allowed.
        add_actor(&p, "did:plc:aged", "2026-06-11T00:00:00Z").await;
        assert_eq!(new_account_access_delay_remaining(&p, "did:plc:aged", now()).await, None);
        // 1.5 days old → 5 days 12h remain → rounds UP to 6 whole days.
        add_actor(&p, "did:plc:partial", "2026-06-19T12:00:00Z").await;
        assert_eq!(
            new_account_access_delay_remaining(&p, "did:plc:partial", now()).await,
            Some(6),
            "a partial final day rounds up"
        );
    }

    #[tokio::test]
    async fn delayed_defaults_to_seven_days_when_unset() {
        let p = pool().await;
        set(&p, KRYPHOCRON_NEW_ACCOUNT_ACCESS_KEY, "\"delayed\"").await;
        // No access-delay-days row → default 7. A 3-day-old account is blocked.
        add_actor(&p, "did:plc:x", "2026-06-18T00:00:00Z").await;
        assert!(new_account_access_delay_remaining(&p, "did:plc:x", now()).await.is_some());
    }

    #[tokio::test]
    async fn unknown_account_is_not_blocked() {
        let p = pool().await;
        set(&p, KRYPHOCRON_NEW_ACCOUNT_ACCESS_KEY, "\"delayed\"").await;
        set(&p, KRYPHOCRON_ACCESS_DELAY_DAYS_KEY, "7").await;
        // No actor row for this DID → fail-soft to open (the write-authz layer
        // confirms repo ownership separately).
        assert_eq!(new_account_access_delay_remaining(&p, "did:plc:ghost", now()).await, None);
    }
}
