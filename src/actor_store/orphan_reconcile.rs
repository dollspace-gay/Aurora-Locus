//! v0.8 arc 1 (#180) — bind-audit orphan-marker reconciliation sweep.
//!
//! Walks `bind_audit_orphan_marker` rows in `state = 'unresolved'`,
//! verifies against the actor store whether each orphaned audit row's
//! paired record eventually landed, and flips the marker to its
//! terminal state:
//!
//! * `confirmed_orphan` — the record is absent (or the actor itself is
//!   gone): the audit row is permanent forensic evidence of an
//!   attempted-and-failed write.
//! * `record_present` — the record exists: the atomicity invariant is
//!   restored. **Forward-compat / unreachable in Arc 1 ship state**
//!   (no write-path recovery mechanism this cycle, §8) — Arc 2 lights
//!   it up. Every Arc 1 marker resolves to `confirmed_orphan`.
//!
//! Design: `docs/internal/design/v08_arc1.md` §4. Mirrors the existing
//! `row_sweep` (`src/blob_store/gc.rs`) keyset-pagination shape, cursored
//! on the monotonic `id` PK so the "filter + mutate-out-of-set" walk
//! neither skips nor loops. Per-marker errors are collect-all: logged at
//! `warn` and counted, never aborting the cycle; the next cycle re-acquires
//! the row because the `state = 'unresolved'` filter still matches.

use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::actor_store::store::ActorStore;
use crate::error::PdsError;

/// Aggregate outcome of one reconciliation cycle. Surfaced to operators
/// via the job's cycle-summary log (§4.4). In Arc 1 ship state
/// `marked_record_present` is always 0 (§2.2 M4); `marked_confirmed_orphan`
/// is the meaningful counter.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Markers read off the keyset walk this cycle.
    pub examined: usize,
    /// Markers flipped to `confirmed_orphan`.
    pub marked_confirmed_orphan: usize,
    /// Markers flipped to `record_present` (forward-compat; 0 in Arc 1).
    pub marked_record_present: usize,
    /// Markers left `unresolved` because verification errored
    /// (collect-all; re-tried next cycle).
    pub left_unresolved_for_retry: usize,
    /// Pages fetched by the keyset walk.
    pub pages_scanned: usize,
    /// Wall-clock duration of the cycle.
    pub duration: Duration,
}

/// Terminal state a marker resolves to, plus the human-readable
/// `resolution_detail` written alongside it.
struct Resolution {
    state: &'static str,
    detail: &'static str,
}

const STATE_CONFIRMED_ORPHAN: &str = "confirmed_orphan";
const STATE_RECORD_PRESENT: &str = "record_present";

/// Run one reconciliation pass over `bind_audit_orphan_marker`.
///
/// Pass-level errors (the initial/continuation page query fails) return
/// `Err(_)` and the caller logs at error level. Per-marker errors stay
/// inside and are counted in `left_unresolved_for_retry` (collect-all,
/// §4.4) — a transient actor-store miss must not strand every other
/// orphan.
///
/// `page_size` bounds each keyset page; `now` is injected (RFC3339 at
/// write time) so the caller controls the clock for testability.
pub async fn run_reconcile_pass(
    shared_pool: &sqlx::AnyPool,
    actor_store: &ActorStore,
    page_size: usize,
    now: DateTime<Utc>,
) -> Result<ReconcileReport, PdsError> {
    let start = std::time::Instant::now();
    let mut report = ReconcileReport::default();
    let now_rfc3339 = now.to_rfc3339();

    // Keyset cursor on the monotonic `id` PK. Restarts at 0 each cycle;
    // rows left `unresolved` (per-marker error) are re-acquired next
    // cycle because the WHERE filter still matches. The cursor advances
    // past every observed row regardless of outcome, so a stuck row
    // never loops within a cycle (§4.3).
    let mut cursor: i64 = 0;

    loop {
        let page = sqlx::query(
            "SELECT id, moderation_event_id, actor_did, subject_uri \
             FROM bind_audit_orphan_marker \
             WHERE state = 'unresolved' AND id > $1 \
             ORDER BY id ASC \
             LIMIT $2",
        )
        .bind(cursor)
        .bind(page_size as i64)
        .fetch_all(shared_pool)
        .await
        .map_err(PdsError::Database)?;

        if page.is_empty() {
            break;
        }
        report.pages_scanned += 1;

        for row in &page {
            // `id` is a NOT NULL i64 PK; a decode failure here is a
            // pass-level structural fault, not a per-marker miss, so it
            // propagates (mirrors gc.rs row_sweep's `try_get` posture).
            let id: i64 = row.try_get("id").map_err(PdsError::Database)?;
            let actor_did: String = row.try_get("actor_did").map_err(PdsError::Database)?;
            let subject_uri: String =
                row.try_get("subject_uri").map_err(PdsError::Database)?;

            // Advance the cursor before any fallible per-marker work so
            // an error leaves the row behind the cursor (re-tried next
            // cycle), never re-tried within this one.
            cursor = id;
            report.examined += 1;

            let resolution = match resolve_marker(actor_store, &actor_did, &subject_uri).await {
                Ok(r) => r,
                Err(e) => {
                    // Collect-all: log the actual error variant (not a
                    // catch-all) and leave the row `unresolved`.
                    tracing::warn!(
                        target: "aurora_locus::orphan_reconcile",
                        event = "bind_audit_orphan_reconcile_marker_unverified",
                        marker_id = id,
                        actor_did = %actor_did,
                        subject_uri = %subject_uri,
                        error = %e,
                        "could not verify orphan marker this cycle; left \
                         unresolved for retry"
                    );
                    report.left_unresolved_for_retry += 1;
                    continue;
                }
            };

            // Guarded UPDATE: the `state = 'unresolved'` predicate makes
            // the transition idempotent against a concurrent instance's
            // sweep (§6.7) — the loser matches zero rows, which is
            // correct, not an error.
            let update = sqlx::query(
                "UPDATE bind_audit_orphan_marker \
                 SET state = $1, resolved_at = $2, resolution_detail = $3 \
                 WHERE id = $4 AND state = 'unresolved'",
            )
            .bind(resolution.state)
            .bind(&now_rfc3339)
            .bind(resolution.detail)
            .bind(id)
            .execute(shared_pool)
            .await;

            match update {
                Ok(_) => match resolution.state {
                    STATE_CONFIRMED_ORPHAN => report.marked_confirmed_orphan += 1,
                    STATE_RECORD_PRESENT => report.marked_record_present += 1,
                    _ => unreachable!("resolve_marker only yields the two terminal states"),
                },
                Err(e) => {
                    // UPDATE failure is per-marker: the row stays
                    // `unresolved`, re-tried next cycle.
                    tracing::warn!(
                        target: "aurora_locus::orphan_reconcile",
                        event = "bind_audit_orphan_reconcile_update_failed",
                        marker_id = id,
                        actor_did = %actor_did,
                        error = %e,
                        "marker state UPDATE failed; left unresolved for retry"
                    );
                    report.left_unresolved_for_retry += 1;
                }
            }
        }
    }

    report.duration = start.elapsed();
    Ok(report)
}

/// Verify a single marker against the actor store and decide its
/// terminal state. Returns `Err` only when verification genuinely
/// couldn't run (actor store I/O error) — the caller treats that as
/// collect-all retry, distinct from a clean `Ok(None)` "record absent".
async fn resolve_marker(
    actor_store: &ActorStore,
    actor_did: &str,
    subject_uri: &str,
) -> Result<Resolution, PdsError> {
    // The actor's whole repository may be gone (account deleted) — that
    // is a confirmed orphan, not a verification failure.
    if !actor_store.exists(actor_did).await {
        return Ok(Resolution {
            state: STATE_CONFIRMED_ORPHAN,
            detail: "actor not present in store",
        });
    }

    match actor_store.get_record(actor_did, subject_uri).await {
        // Forward-compat (§2.2 M4): unreachable in Arc 1 ship state
        // because no path lands a record after the actor-tx rollback.
        // The branch ships so Arc 2's recovery mechanism needs no
        // schema/sweep change.
        Ok(Some(_)) => Ok(Resolution {
            state: STATE_RECORD_PRESENT,
            detail: "record found at subject_uri",
        }),
        Ok(None) => Ok(Resolution {
            state: STATE_CONFIRMED_ORPHAN,
            detail: "actor store reports record absent",
        }),
        Err(e) => Err(e),
    }
}
