//! v0.9 Federation runtime-mutability arc §3.2/§3.3 (#391) —
//! restart-coordination markers.
//!
//! The save-and-restart save handlers (D-phase) write a marker into
//! `pending_restart_action` in the SAME outer transaction as the
//! runtime-settings write (§3.5), so the value-change and the
//! "what-to-do-after-restart" record land atomically. On the next boot the
//! [`process_pending_restart_actions`] hook reads the markers and dispatches on
//! each action — clearing the restart-only ones (the restart itself was the
//! action) and handing `bulk-diddoc-update` to its background task.
//!
//! The table is deliberately NOT a `runtime_settings` key: not allowlisted, not
//! exposed via the settings XRPC, not audited. It's internal coordination; the
//! operator-visible audit lives on the `runtime_settings` write it composes
//! with. Markers carry a JSON `payload` with an integer `version` for forward
//! compatibility — the boot hook skips any version it doesn't recognise.

use crate::context::AppContext;
use crate::error::PdsResult;
use sqlx::Row as _;

/// Marker action names (§3.2). The D-phase save handlers set these in the same
/// outer tx as the runtime-settings write; the boot hook dispatches on them.
pub const ACTION_RESTART_FEDERATION_ENABLED: &str = "restart-required-for-federation-enabled";
pub const ACTION_RESTART_SERVICE_PUBLIC_URL: &str = "restart-required-for-service-public-url";
pub const ACTION_BULK_DIDDOC_UPDATE: &str = "bulk-diddoc-update";

/// Current marker payload schema version (§3.2 / L-1). The boot hook leaves any
/// marker carrying a different version in place (forward compatibility with
/// markers a future cycle writes but this binary doesn't understand).
pub const MARKER_PAYLOAD_VERSION: i64 = 1;

/// What the boot hook should do with a marker (§3.3).
#[derive(Debug, PartialEq, Eq)]
enum MarkerDisposition {
    /// Known restart-only marker: the restart was the action — clear it.
    Clear,
    /// `bulk-diddoc-update`: hand off to the background task (Phase E2); leave
    /// the marker for that task to clear on completion.
    SpawnBulkUpdate,
    /// Unknown action or unknown payload version: leave in place (forward
    /// compatibility — a future cycle may understand it).
    Leave,
}

/// Pure dispatch decision (§3.3). Extracted from the IO loop so the
/// version-gating + action-matching is unit-testable without a database.
fn classify_marker(action: &str, payload: &serde_json::Value) -> MarkerDisposition {
    let version = payload.get("version").and_then(|v| v.as_i64()).unwrap_or(0);
    if version != MARKER_PAYLOAD_VERSION {
        return MarkerDisposition::Leave;
    }
    match action {
        ACTION_RESTART_FEDERATION_ENABLED | ACTION_RESTART_SERVICE_PUBLIC_URL => {
            MarkerDisposition::Clear
        }
        ACTION_BULK_DIDDOC_UPDATE => MarkerDisposition::SpawnBulkUpdate,
        _ => MarkerDisposition::Leave,
    }
}

/// Boot-time marker detection hook (§3.3). Called from `main` after the DB pool
/// is open (migrations applied) and `AppContext` is ready, before the serve
/// loop starts. Best-effort: the caller logs and continues booting on error.
pub async fn process_pending_restart_actions(ctx: &AppContext) -> PdsResult<()> {
    let rows = sqlx::query("SELECT action, payload FROM pending_restart_action")
        .fetch_all(&ctx.account_db)
        .await?;
    for row in rows {
        let action: String = row.try_get("action")?;
        let payload_str: String = row.try_get("payload")?;
        let payload: serde_json::Value =
            serde_json::from_str(&payload_str).unwrap_or(serde_json::Value::Null);
        match classify_marker(&action, &payload) {
            MarkerDisposition::Clear => {
                clear_marker(&ctx.account_db, &action).await?;
                tracing::info!(
                    action = %action,
                    "pending-restart marker handled (restart complete); cleared"
                );
            }
            MarkerDisposition::SpawnBulkUpdate => {
                // §2.3 (#399 / E3) — spawn the post-restart bulk did:plc update.
                // The task clears the marker on completion; an interruption leaves
                // it set so the next boot re-runs (idempotent via compare-and-skip).
                let run_id = payload
                    .get("run_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let started_at = payload
                    .get("started_at")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let ctx_owned = ctx.clone();
                tokio::spawn(async move {
                    if let Err(e) = crate::api::bulk_diddoc_result::run_bulk_diddoc_update(
                        &ctx_owned,
                        &run_id,
                        &started_at,
                    )
                    .await
                    {
                        tracing::error!(error = %e, "bulk did:plc update task failed");
                    }
                });
                tracing::info!(action = %action, "spawned post-restart bulk did:plc update task");
            }
            MarkerDisposition::Leave => {
                tracing::warn!(
                    action = %action,
                    "pending-restart marker has unknown action/version; leaving in place (forward compatibility)"
                );
            }
        }
    }
    Ok(())
}

/// Upsert a marker into a caller-owned transaction (§3.5 / M-1). Composed into
/// the SAME outer tx as the triggering runtime-settings write so the value
/// change and the restart record land atomically. `INSERT ... ON CONFLICT(action)
/// DO UPDATE` so re-queuing (or reverting) an already-pending field amends the
/// payload rather than colliding on the `action` primary key. Used by the
/// delete/revert path (C4) and the D-phase save handlers.
pub async fn upsert_marker(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    action: &str,
    payload: &str,
    created_at: &str,
) -> PdsResult<()> {
    sqlx::query(
        "INSERT INTO pending_restart_action (action, payload, created_at) \
         VALUES ($1, $2, $3) \
         ON CONFLICT(action) DO UPDATE SET payload = excluded.payload, \
         created_at = excluded.created_at",
    )
    .bind(action)
    .bind(payload)
    .bind(created_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Delete a marker by its `action` key. Used by the boot hook (and, in later
/// phases, by the bulk-update task on completion).
pub async fn clear_marker(pool: &sqlx::AnyPool, action: &str) -> PdsResult<()> {
    sqlx::query("DELETE FROM pending_restart_action WHERE action = $1")
        .bind(action)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v1(action_marker: &str) -> serde_json::Value {
        serde_json::json!({ "version": 1, "note": action_marker })
    }

    #[test]
    fn classify_known_restart_markers_clear() {
        assert_eq!(
            classify_marker(ACTION_RESTART_FEDERATION_ENABLED, &v1("a")),
            MarkerDisposition::Clear
        );
        assert_eq!(
            classify_marker(ACTION_RESTART_SERVICE_PUBLIC_URL, &v1("b")),
            MarkerDisposition::Clear
        );
    }

    #[test]
    fn classify_bulk_update_spawns() {
        assert_eq!(
            classify_marker(ACTION_BULK_DIDDOC_UPDATE, &v1("c")),
            MarkerDisposition::SpawnBulkUpdate
        );
    }

    #[test]
    fn classify_unknown_action_left_in_place() {
        assert_eq!(
            classify_marker("some-future-marker", &v1("d")),
            MarkerDisposition::Leave
        );
    }

    #[test]
    fn classify_unknown_payload_version_left_in_place() {
        // A known action but a future payload version → leave (forward compat).
        let future = serde_json::json!({ "version": 99 });
        assert_eq!(
            classify_marker(ACTION_RESTART_FEDERATION_ENABLED, &future),
            MarkerDisposition::Leave
        );
        // Missing version (parses to 0) → also leave.
        let no_version = serde_json::json!({});
        assert_eq!(
            classify_marker(ACTION_BULK_DIDDOC_UPDATE, &no_version),
            MarkerDisposition::Leave
        );
    }

    async fn pool() -> sqlx::AnyPool {
        sqlx::any::install_default_drivers();
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE pending_restart_action (action TEXT PRIMARY KEY, \
             payload TEXT NOT NULL, created_at TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn upsert(pool: &sqlx::AnyPool, action: &str, payload: &str) {
        // The exact upsert the D-phase save handlers use (§3.5 / M-1).
        sqlx::query(
            "INSERT INTO pending_restart_action (action, payload, created_at) \
             VALUES ($1, $2, $3) \
             ON CONFLICT(action) DO UPDATE SET \
             payload = excluded.payload, created_at = excluded.created_at",
        )
        .bind(action)
        .bind(payload)
        .bind("2026-06-27T00:00:00Z")
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn clear_marker_removes_the_row() {
        let p = pool().await;
        upsert(&p, ACTION_RESTART_FEDERATION_ENABLED, r#"{"version":1}"#).await;
        clear_marker(&p, ACTION_RESTART_FEDERATION_ENABLED).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pending_restart_action")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(n, 0, "marker should be cleared");
    }

    #[tokio::test]
    async fn marker_upsert_on_conflict_overwrites_in_place() {
        let p = pool().await;
        upsert(&p, ACTION_RESTART_SERVICE_PUBLIC_URL, r#"{"version":1,"run_id":"a"}"#).await;
        // Re-queue the same field with a new payload — must overwrite, not error
        // on the `action` PK (the M-1 amend-a-queued-change flow).
        upsert(&p, ACTION_RESTART_SERVICE_PUBLIC_URL, r#"{"version":1,"run_id":"b"}"#).await;
        let (n, payload): (i64, String) = {
            let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pending_restart_action")
                .fetch_one(&p)
                .await
                .unwrap();
            let payload: String =
                sqlx::query_scalar("SELECT payload FROM pending_restart_action WHERE action = $1")
                    .bind(ACTION_RESTART_SERVICE_PUBLIC_URL)
                    .fetch_one(&p)
                    .await
                    .unwrap();
            (n, payload)
        };
        assert_eq!(n, 1, "upsert must not create a second row");
        assert!(payload.contains("\"run_id\":\"b\""), "payload should be overwritten: {payload}");
    }
}
