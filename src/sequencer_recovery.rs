//! Sequencer recovery (Arc H §7.4.2 / #294) — the escalation surface for
//! sequencer-level intervention beyond the routine §5.4.4 pause/resume/reset
//! controls. "The sequencer is in a weird state; what can we do."
//!
//! ## Scope (recon-grounded)
//!
//! v0.9 ships ONE operation: a read-only **deep integrity validation**
//! ([`Sequencer::validate_integrity`](crate::sequencer::Sequencer::validate_integrity)).
//! It surfaces two anomaly classes the substrate otherwise hides — undecodable
//! event blobs (the firehose silently drops them) and per-DID rev
//! non-monotonicity (concurrent-write ordering bugs) — plus the live state
//! counts.
//!
//! The other conceivable recovery operations were deferred during recon, with
//! carryforward reasons:
//! - **Prune invalidated rows**: no production path sets `invalidated = 1`
//!   (account deletion hard-deletes via `delete_all_for_user`), so a prune would
//!   target an always-empty set — a no-op masquerading as a recovery op.
//!   Precondition for revival: the substrate adopting soft-delete semantics.
//! - **Re-sequencing / gap-closing**: reassigning `seq` would break every
//!   firehose subscriber's cursor and violate the monotonic-cursor contract;
//!   needs a coordinated cursor-migration protocol that does not exist. Gaps are
//!   non-data-loss (deletion leaves expected gaps).
//! - **Malformed-blob repair**: reconstructing event bytes is the rebuild
//!   domain (§7.4.1 re-derives canonical bytes from the actor repo). Validation
//!   detects; the operator-facing fix routes to a per-account rebuild.
//!
//! The XRPC surface is nonetheless the generic
//! `sequencerRecoveryOptions` / `runSequencerRecovery` shape the design specs,
//! so adding a future operation is additive.
//!
//! Deployment single-flight (one recovery op at a time), [`ScanJob`]-shaped:
//! in-memory live progress + a cancel flag. The validation result lives on the
//! job (read once by the operator); it is not persisted.
//!
//! [`ScanJob`]: crate::repo_scan::ScanJob

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use uuid::Uuid;

use crate::admin::events::{LogEventParams, ModerationEventLogger, ModerationEventType};
use crate::context::AppContext;
use crate::sequencer::IntegrityReport;

/// The wire id of the one shipped recovery operation.
pub const OP_VALIDATE: &str = "validate";

/// Terminal outcome of a recovery run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryOutcome {
    Completed,
    Cancelled,
    Failed,
}

impl RecoveryOutcome {
    fn as_str(self) -> &'static str {
        match self {
            RecoveryOutcome::Completed => "completed",
            RecoveryOutcome::Cancelled => "cancelled",
            RecoveryOutcome::Failed => "failed",
        }
    }
}

struct RecoveryState {
    running: bool,
    operation: Option<&'static str>,
    job_id: Option<String>,
    triggered_by: Option<String>,
    started_at: Option<SystemTime>,
    finished_at: Option<SystemTime>,
    last_outcome: Option<RecoveryOutcome>,
    report: Option<IntegrityReport>,
    error: Option<String>,
}

/// A live snapshot of the recovery job for `getSequencerRecoveryProgress`.
#[derive(Debug, Clone)]
pub struct RecoveryProgress {
    pub running: bool,
    pub operation: Option<&'static str>,
    pub job_id: Option<String>,
    pub rows_scanned: u64,
    pub started_at: Option<SystemTime>,
    pub finished_at: Option<SystemTime>,
    pub cancel_requested: bool,
    pub last_outcome: Option<&'static str>,
    pub report: Option<IntegrityReport>,
    pub error: Option<String>,
}

/// The deployment's single sequencer-recovery job. Held in [`AppContext`].
pub struct SequencerRecoveryJob {
    state: RwLock<RecoveryState>,
    cancel: AtomicBool,
    scanned: AtomicU64,
}

impl Default for SequencerRecoveryJob {
    fn default() -> Self {
        Self::new()
    }
}

impl SequencerRecoveryJob {
    pub fn new() -> Self {
        SequencerRecoveryJob {
            state: RwLock::new(RecoveryState {
                running: false,
                operation: None,
                job_id: None,
                triggered_by: None,
                started_at: None,
                finished_at: None,
                last_outcome: None,
                report: None,
                error: None,
            }),
            cancel: AtomicBool::new(false),
            scanned: AtomicU64::new(0),
        }
    }

    /// A live snapshot of the job's progress.
    pub fn progress(&self) -> RecoveryProgress {
        let st = self.state.read().expect("recovery state lock not poisoned");
        RecoveryProgress {
            running: st.running,
            operation: st.operation,
            job_id: st.job_id.clone(),
            rows_scanned: self.scanned.load(Ordering::Relaxed),
            started_at: st.started_at,
            finished_at: st.finished_at,
            cancel_requested: self.cancel.load(Ordering::Relaxed),
            last_outcome: st.last_outcome.map(RecoveryOutcome::as_str),
            report: st.report.clone(),
            error: st.error.clone(),
        }
    }

    /// Request cancellation of the in-flight operation. Returns `true` if one
    /// was running, `false` if none was ("nothing to cancel").
    pub fn request_cancel(&self) -> bool {
        let running = self.state.read().expect("recovery state lock not poisoned").running;
        if running {
            self.cancel.store(true, Ordering::Relaxed);
        }
        running
    }

    /// Single-flight guard: mark running + stamp a fresh job id. Returns the
    /// job id, or `None` if an operation is already in flight.
    fn begin(&self, operation: &'static str, triggered_by: String) -> Option<String> {
        let mut st = self.state.write().expect("recovery state lock not poisoned");
        if st.running {
            return None;
        }
        let job_id = Uuid::new_v4().to_string();
        st.running = true;
        st.operation = Some(operation);
        st.job_id = Some(job_id.clone());
        st.triggered_by = Some(triggered_by);
        st.started_at = Some(SystemTime::now());
        st.finished_at = None;
        st.last_outcome = None;
        st.report = None;
        st.error = None;
        drop(st);
        self.scanned.store(0, Ordering::Relaxed);
        self.cancel.store(false, Ordering::Relaxed);
        Some(job_id)
    }

    /// Start the deep integrity validation. Deployment single-flight — returns
    /// the job id, or `None` if a recovery operation is already running (the
    /// XRPC maps that to a 409). Spawns the walk on a background task.
    pub fn try_start_validate(self: &Arc<Self>, ctx: AppContext, triggered_by: String) -> Option<String> {
        let job_id = self.begin(OP_VALIDATE, triggered_by)?;
        let job = Arc::clone(self);
        tokio::spawn(async move {
            let outcome = job.run_validate(&ctx).await;
            job.finish(&ctx, outcome).await;
        });
        Some(job_id)
    }

    async fn run_validate(&self, ctx: &AppContext) -> RecoveryOutcome {
        match ctx.sequencer.validate_integrity(&self.cancel, &self.scanned).await {
            Ok(report) => {
                let cancelled = self.cancel.load(Ordering::Relaxed);
                self.state.write().expect("recovery state lock not poisoned").report = Some(report);
                if cancelled {
                    RecoveryOutcome::Cancelled
                } else {
                    RecoveryOutcome::Completed
                }
            }
            Err(e) => {
                self.state.write().expect("recovery state lock not poisoned").error =
                    Some(e.to_string());
                RecoveryOutcome::Failed
            }
        }
    }

    /// Record terminal state + emit the `SequencerValidated` audit event.
    async fn finish(&self, ctx: &AppContext, outcome: RecoveryOutcome) {
        let (job_id, triggered_by, report) = {
            let mut st = self.state.write().expect("recovery state lock not poisoned");
            st.running = false;
            st.finished_at = Some(SystemTime::now());
            st.last_outcome = Some(outcome);
            (
                st.job_id.clone().unwrap_or_default(),
                st.triggered_by.clone().unwrap_or_default(),
                st.report.clone(),
            )
        };

        // Audit the run (best-effort). Only the validate op exists today, so the
        // event is SequencerValidated regardless.
        let logger = ModerationEventLogger::new(ctx.account_db.clone());
        let details = match &report {
            Some(r) => serde_json::json!({
                "jobId": job_id,
                "outcome": outcome.as_str(),
                "rowsScanned": r.rows_scanned,
                "totalRows": r.total_rows,
                "invalidatedRows": r.invalidated_rows,
                "headSeq": r.head_seq,
                "malformedCount": r.malformed_count,
                "nonMonotonicCount": r.non_monotonic_count,
            }),
            None => serde_json::json!({
                "jobId": job_id,
                "outcome": outcome.as_str(),
            }),
        };
        if let Err(e) = logger
            .log_event(LogEventParams {
                event_type: ModerationEventType::SequencerValidated,
                actor_did: &triggered_by,
                subject_did: None,
                subject_uri: None,
                subject_cid: None,
                details,
                meta: None,
            })
            .await
        {
            tracing::error!(target: "aurora_locus::sequencer_recovery", error = %e, "sequencer validate finished but SequencerValidated audit emit failed");
        }
        tracing::info!(
            target: "aurora_locus::sequencer_recovery",
            event = "sequencer_validate_finished",
            outcome = outcome.as_str(),
            malformed = report.as_ref().map(|r| r.malformed_count).unwrap_or(0),
            non_monotonic = report.as_ref().map(|r| r.non_monotonic_count).unwrap_or(0),
            "sequencer deep validation finished",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_single_flight_and_cancel() {
        let job = SequencerRecoveryJob::new();
        assert!(!job.progress().running);
        assert!(!job.request_cancel(), "cancel with nothing in flight is a no-op");

        let jid = job.begin(OP_VALIDATE, "did:plc:op".to_string()).expect("first begin starts");
        let p = job.progress();
        assert!(p.running);
        assert_eq!(p.operation, Some("validate"));
        assert_eq!(p.job_id.as_deref(), Some(jid.as_str()));
        assert!(job.begin(OP_VALIDATE, "did:plc:op".to_string()).is_none(), "second begin rejected");

        assert!(job.request_cancel());
        assert!(job.progress().cancel_requested);
    }
}
