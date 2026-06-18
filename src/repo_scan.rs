//! Bulk repository repair — across-accounts inconsistency scan (Arc H §7.4.3 /
//! #291). The scan half of the repair mini-arc; the bulk repair that acts on
//! the findings is #292.
//!
//! Where §7.4.1 repository rebuild is per-account (operator names a DID), this
//! is the across-accounts "scrub": walk every account, structurally reconstruct
//! its repo from the sequencer (the #289 fast path — [`reconstruct_and_verify`]
//! with `signing_did_key=None`, no expensive full-signature check), and compare
//! the reconstructed head against the live repo head. Each inconsistency is
//! persisted as a finding the operator can review across admin-UI sessions and
//! later repair (per-account rebuild, #290/#292).
//!
//! ## Severity (category-based, locked)
//!
//! Derived from the single structural reconstruction alone — no extra
//! per-account walk, so the scan stays the fast path:
//!
//! - **high**: reconstruction fails, or the live repo is unrebuildable (a live
//!   root with no backing sequencer history, or history that reconstructs but
//!   no live root) — needs attention beyond a routine rebuild.
//! - **medium**: the reconstructed head CID differs from the live head CID — a
//!   real inconsistency a rebuild fixes.
//! - **low**: heads match but the recorded rev differs — minor drift.
//!
//! A fully-consistent account (reconstructed head == live head, same rev, or a
//! genuinely empty account) produces no finding.
//!
//! ## Job shape
//!
//! Deployment-level single-flight (one scan at a time), [`RewriteJob`]-shaped:
//! in-memory live progress + a cancel flag. Unlike the rotation job, the
//! findings are persisted to the `repo_scan_finding` table (the durable
//! artifact operators review); the live run state is in-memory and resets on
//! restart, which is fine — a restart mid-scan just means re-run the scan.
//!
//! [`RewriteJob`]: crate::kryphocron_rewrite::RewriteJob
//! [`reconstruct_and_verify`]: crate::rebuild::reconstruct_and_verify

use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use sqlx::{AnyPool, Row};
use uuid::Uuid;

use crate::admin::events::{LogEventParams, ModerationEventLogger, ModerationEventType};
use crate::context::AppContext;
use crate::error::{PdsError, PdsResult};

/// Accounts fetched per `list_accounts` page during a scan.
const SCAN_PAGE: i64 = 200;

// ---------------------------------------------------------------------------
// Severity + findings
// ---------------------------------------------------------------------------

/// Category-based finding severity (§7.4.3, locked).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Reconstruction failed or the live repo is unrebuildable.
    High,
    /// Reconstructed head CID differs from the live head CID.
    Medium,
    /// Heads match but the recorded rev differs (minor drift).
    Low,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::High => "high",
            Severity::Medium => "medium",
            Severity::Low => "low",
        }
    }
}

impl FromStr for Severity {
    type Err = PdsError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "high" => Ok(Severity::High),
            "medium" => Ok(Severity::Medium),
            "low" => Ok(Severity::Low),
            other => Err(PdsError::Validation(format!("invalid scan severity: {other}"))),
        }
    }
}

/// One persisted scan finding for an account.
#[derive(Debug, Clone)]
pub struct ScanFinding {
    pub scan_id: String,
    pub did: String,
    pub severity: Severity,
    /// The live repo head CID, if the live repo had a root.
    pub live_head: Option<String>,
    /// The reconstructed head CID, if reconstruction produced one.
    pub recon_head: Option<String>,
    pub detail: String,
    pub created_at: String,
}

/// Severity tallies for a finding set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SeverityCounts {
    pub high: u64,
    pub medium: u64,
    pub low: u64,
}

impl SeverityCounts {
    pub fn total(&self) -> u64 {
        self.high + self.medium + self.low
    }
    fn bump(&mut self, sev: Severity) {
        match sev {
            Severity::High => self.high += 1,
            Severity::Medium => self.medium += 1,
            Severity::Low => self.low += 1,
        }
    }
}

// ---------------------------------------------------------------------------
// Findings store (repo_scan_finding table)
// ---------------------------------------------------------------------------

/// Persistent store over the `repo_scan_finding` table. Holds the findings of
/// the most recent scan (a new scan clears the prior set); rows carry their
/// `scan_id` for correlation with the `ScanCompleted` audit event. Held in
/// [`AppContext`].
pub struct ScanFindingsStore {
    db: AnyPool,
}

impl ScanFindingsStore {
    pub fn new(db: AnyPool) -> Self {
        Self { db }
    }

    /// Clear all findings (a fresh scan replaces the prior set — only the
    /// latest scan's findings are retained).
    pub async fn clear(&self) -> PdsResult<()> {
        sqlx::query("DELETE FROM repo_scan_finding")
            .execute(&self.db)
            .await
            .map_err(PdsError::Database)?;
        Ok(())
    }

    /// Upsert a finding (one row per (scan_id, did)).
    pub async fn insert(&self, f: &ScanFinding) -> PdsResult<()> {
        sqlx::query(
            "INSERT INTO repo_scan_finding \
             (scan_id, did, severity, live_head, recon_head, detail, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT(scan_id, did) DO UPDATE SET \
                severity = excluded.severity, live_head = excluded.live_head, \
                recon_head = excluded.recon_head, detail = excluded.detail, \
                created_at = excluded.created_at",
        )
        .bind(&f.scan_id)
        .bind(&f.did)
        .bind(f.severity.as_str())
        .bind(&f.live_head)
        .bind(&f.recon_head)
        .bind(&f.detail)
        .bind(&f.created_at)
        .execute(&self.db)
        .await
        .map_err(PdsError::Database)?;
        Ok(())
    }

    /// Severity tallies across all current findings.
    pub async fn counts(&self) -> PdsResult<SeverityCounts> {
        let rows = sqlx::query("SELECT severity, COUNT(*) AS n FROM repo_scan_finding GROUP BY severity")
            .fetch_all(&self.db)
            .await
            .map_err(PdsError::Database)?;
        let mut c = SeverityCounts::default();
        for row in rows {
            let sev: String = row.get("severity");
            let n: i64 = row.get("n");
            match Severity::from_str(&sev) {
                Ok(Severity::High) => c.high = n as u64,
                Ok(Severity::Medium) => c.medium = n as u64,
                Ok(Severity::Low) => c.low = n as u64,
                Err(_) => {}
            }
        }
        Ok(c)
    }

    /// List findings, optionally filtered by severity, keyset-paginated by did
    /// (`cursor` = the last did seen). Returns up to `limit` rows ordered by
    /// did.
    pub async fn list(
        &self,
        severity: Option<Severity>,
        limit: i64,
        cursor: Option<&str>,
    ) -> PdsResult<Vec<ScanFinding>> {
        // Build the query with the optional severity + cursor predicates. Bind
        // order tracks the predicates added.
        let mut sql = String::from(
            "SELECT scan_id, did, severity, live_head, recon_head, detail, created_at \
             FROM repo_scan_finding WHERE 1=1",
        );
        if severity.is_some() {
            sql.push_str(" AND severity = ?");
        }
        if cursor.is_some() {
            sql.push_str(" AND did > ?");
        }
        sql.push_str(" ORDER BY did LIMIT ?");

        let mut q = sqlx::query(&sql);
        if let Some(sev) = severity {
            q = q.bind(sev.as_str());
        }
        if let Some(c) = cursor {
            q = q.bind(c);
        }
        q = q.bind(limit);

        let rows = q.fetch_all(&self.db).await.map_err(PdsError::Database)?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let sev: String = row.get("severity");
            out.push(ScanFinding {
                scan_id: row.get("scan_id"),
                did: row.get("did"),
                severity: Severity::from_str(&sev).unwrap_or(Severity::Medium),
                live_head: row.get("live_head"),
                recon_head: row.get("recon_head"),
                detail: row.get("detail"),
                created_at: row.get("created_at"),
            });
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// Classification — the per-account scan decision
// ---------------------------------------------------------------------------

/// The classification of one account: `None` is consistent (no finding);
/// `Some` carries the finding's severity, heads, and a human detail.
struct Classification {
    severity: Severity,
    live_head: Option<String>,
    recon_head: Option<String>,
    detail: String,
}

/// The structural-reconstruction outcome for one account, reduced to what the
/// classifier needs (decoupled from `VerifiedRepo` / I/O so the decision matrix
/// is purely testable).
enum ReconState {
    /// Reconstruction produced a coherent repo at `head` / `rev`.
    Resolved { head: String, rev: String },
    /// No sequencer commit history for the account.
    NoHistory,
    /// Reconstruction failed (broken history or a read error).
    Failed(String),
}

/// The pure scan decision: compare the structural-reconstruction outcome
/// against the live repo head/rev (`None` = no live root). Returns a finding
/// or `None` (consistent). Locked category-based severity (§7.4.3).
fn classify(recon: ReconState, live: Option<(String, String)>) -> Option<Classification> {
    match (recon, live) {
        // Reconstruction failed — the sequencer history doesn't reconstruct a
        // coherent repo (or a transient read error). Unrebuildable → high.
        (ReconState::Failed(e), live) => Some(Classification {
            severity: Severity::High,
            live_head: live.map(|(c, _)| c),
            recon_head: None,
            detail: format!("reconstruction failed: {e}"),
        }),
        // No sequencer history but a live repo exists — the live repo can't be
        // explained or rebuilt from the sequencer. High.
        (ReconState::NoHistory, Some((live_cid, _))) => Some(Classification {
            severity: Severity::High,
            live_head: Some(live_cid),
            recon_head: None,
            detail: "live repository present but sequencer has no commit history".to_string(),
        }),
        // No history and no live repo — a genuinely empty account. Consistent.
        (ReconState::NoHistory, None) => None,
        // History reconstructs but the live repo root is absent — the repo
        // isn't materialised. High.
        (ReconState::Resolved { head, .. }, None) => Some(Classification {
            severity: Severity::High,
            live_head: None,
            recon_head: Some(head),
            detail: "sequencer history reconstructs but the live repository root is absent"
                .to_string(),
        }),
        // Both present — compare heads, then revs.
        (ReconState::Resolved { head, rev }, Some((live_cid, live_rev))) => {
            if head == live_cid {
                if rev == live_rev {
                    None // fully consistent
                } else {
                    Some(Classification {
                        severity: Severity::Low,
                        live_head: Some(live_cid),
                        recon_head: Some(head),
                        detail: format!(
                            "head matches but rev differs: live {live_rev} vs reconstructed {rev}"
                        ),
                    })
                }
            } else {
                Some(Classification {
                    severity: Severity::Medium,
                    live_head: Some(live_cid),
                    recon_head: Some(head),
                    detail: "reconstructed head differs from the live head".to_string(),
                })
            }
        }
    }
}

/// Classify one account by structurally reconstructing it (the #289 fast path,
/// no full-signature verification) and comparing against the live repo head.
/// Read-only; gathers the two inputs and defers the decision to [`classify`].
async fn classify_account(ctx: &AppContext, did: &str) -> Option<Classification> {
    let live = ctx.actor_store.get_repo_root(did).await.ok().map(|r| (r.cid, r.rev));
    let recon = match crate::rebuild::reconstruct_and_verify(&ctx.sequencer, did, None).await {
        Ok(Some(v)) => ReconState::Resolved {
            head: v.commit_cid.to_string(),
            rev: v.rev().to_string(),
        },
        Ok(None) => ReconState::NoHistory,
        Err(e) => ReconState::Failed(e.to_string()),
    };
    classify(recon, live)
}

// ---------------------------------------------------------------------------
// Scan job (deployment single-flight)
// ---------------------------------------------------------------------------

/// Terminal outcome of a scan run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanOutcome {
    Completed,
    Cancelled,
    Failed,
}

impl ScanOutcome {
    fn as_str(self) -> &'static str {
        match self {
            ScanOutcome::Completed => "completed",
            ScanOutcome::Cancelled => "cancelled",
            ScanOutcome::Failed => "failed",
        }
    }
}

struct ScanState {
    running: bool,
    scan_id: Option<String>,
    triggered_by: Option<String>,
    accounts_scanned: u64,
    counts: SeverityCounts,
    started_at: Option<SystemTime>,
    finished_at: Option<SystemTime>,
    last_outcome: Option<ScanOutcome>,
}

/// A live snapshot of the scan job for `getScanProgress`.
#[derive(Debug, Clone)]
pub struct ScanProgress {
    pub running: bool,
    pub scan_id: Option<String>,
    pub accounts_scanned: u64,
    pub counts: SeverityCounts,
    pub started_at: Option<SystemTime>,
    pub finished_at: Option<SystemTime>,
    pub cancel_requested: bool,
    /// `"completed" | "cancelled" | "failed"` of the most recent run.
    pub last_outcome: Option<&'static str>,
}

/// The deployment's single repository-scan job. Held in [`AppContext`].
pub struct ScanJob {
    state: RwLock<ScanState>,
    cancel: AtomicBool,
}

impl Default for ScanJob {
    fn default() -> Self {
        Self::new()
    }
}

impl ScanJob {
    pub fn new() -> Self {
        ScanJob {
            state: RwLock::new(ScanState {
                running: false,
                scan_id: None,
                triggered_by: None,
                accounts_scanned: 0,
                counts: SeverityCounts::default(),
                started_at: None,
                finished_at: None,
                last_outcome: None,
            }),
            cancel: AtomicBool::new(false),
        }
    }

    /// A live snapshot of the scan's progress.
    pub fn progress(&self) -> ScanProgress {
        let st = self.state.read().expect("scan state lock not poisoned");
        ScanProgress {
            running: st.running,
            scan_id: st.scan_id.clone(),
            accounts_scanned: st.accounts_scanned,
            counts: st.counts,
            started_at: st.started_at,
            finished_at: st.finished_at,
            cancel_requested: self.cancel.load(Ordering::Relaxed),
            last_outcome: st.last_outcome.map(ScanOutcome::as_str),
        }
    }

    /// Request cancellation of the in-flight scan. Returns `true` if a scan was
    /// running (the flag is set; the walk stops at the next account boundary),
    /// `false` if none was in flight ("nothing to cancel").
    pub fn request_cancel(&self) -> bool {
        let running = self.state.read().expect("scan state lock not poisoned").running;
        if running {
            self.cancel.store(true, Ordering::Relaxed);
        }
        running
    }

    /// The single-flight guard: mark running + stamp a fresh scan_id. Returns
    /// the scan_id, or `None` if a scan is already in flight. Split from
    /// [`Self::try_start`] so it is testable without spawning the walk.
    fn begin(&self, triggered_by: String) -> Option<String> {
        let mut st = self.state.write().expect("scan state lock not poisoned");
        if st.running {
            return None;
        }
        let scan_id = Uuid::new_v4().to_string();
        st.running = true;
        st.scan_id = Some(scan_id.clone());
        st.triggered_by = Some(triggered_by);
        st.accounts_scanned = 0;
        st.counts = SeverityCounts::default();
        st.started_at = Some(SystemTime::now());
        st.finished_at = None;
        st.last_outcome = None;
        drop(st);
        self.cancel.store(false, Ordering::Relaxed);
        Some(scan_id)
    }

    /// Start a scan: single-flight. Returns the new scan_id, or `None` if a
    /// scan is already running (the XRPC maps that to a 409). Spawns the walk
    /// on a background task.
    pub fn try_start(self: &Arc<Self>, ctx: AppContext, triggered_by: String) -> Option<String> {
        let scan_id = self.begin(triggered_by)?;
        let job = Arc::clone(self);
        let sid = scan_id.clone();
        tokio::spawn(async move {
            let outcome = job.run(&ctx, &sid).await;
            job.finish(&ctx, outcome).await;
        });
        Some(scan_id)
    }

    /// The scan walk: clear prior findings, then iterate every account,
    /// classify it, and persist any finding. Cancellable at each account
    /// boundary.
    async fn run(&self, ctx: &AppContext, scan_id: &str) -> ScanOutcome {
        if let Err(e) = ctx.scan_findings_store.clear().await {
            tracing::error!(target: "aurora_locus::repo_scan", error = %e, "scan: failed to clear prior findings");
            return ScanOutcome::Failed;
        }

        let mut cursor: Option<String> = None;
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                return ScanOutcome::Cancelled;
            }
            let accounts = match ctx.account_manager.list_accounts(cursor.as_deref(), SCAN_PAGE).await {
                Ok(a) => a,
                Err(e) => {
                    tracing::error!(target: "aurora_locus::repo_scan", error = %e, "scan: account enumeration failed");
                    return ScanOutcome::Failed;
                }
            };
            if accounts.is_empty() {
                break;
            }
            let page_len = accounts.len();
            let next_cursor = accounts.last().map(|a| a.did.clone());

            for account in &accounts {
                if self.cancel.load(Ordering::Relaxed) {
                    return ScanOutcome::Cancelled;
                }
                if let Some(c) = classify_account(ctx, &account.did).await {
                    let finding = ScanFinding {
                        scan_id: scan_id.to_string(),
                        did: account.did.clone(),
                        severity: c.severity,
                        live_head: c.live_head,
                        recon_head: c.recon_head,
                        detail: c.detail,
                        created_at: chrono::Utc::now().to_rfc3339(),
                    };
                    if let Err(e) = ctx.scan_findings_store.insert(&finding).await {
                        // A persistence error on one finding is logged and
                        // skipped; the scan continues (best-effort, retryable).
                        tracing::warn!(target: "aurora_locus::repo_scan", did = %account.did, error = %e, "scan: failed to persist finding");
                    } else {
                        self.state.write().expect("scan state lock not poisoned").counts.bump(c.severity);
                    }
                }
                self.state.write().expect("scan state lock not poisoned").accounts_scanned += 1;
            }

            if page_len < SCAN_PAGE as usize {
                break;
            }
            cursor = next_cursor;
        }
        ScanOutcome::Completed
    }

    /// Record terminal state + emit the `ScanCompleted` audit event.
    async fn finish(&self, ctx: &AppContext, outcome: ScanOutcome) {
        let (scan_id, triggered_by, scanned, counts) = {
            let mut st = self.state.write().expect("scan state lock not poisoned");
            st.running = false;
            st.finished_at = Some(SystemTime::now());
            st.last_outcome = Some(outcome);
            (
                st.scan_id.clone().unwrap_or_default(),
                st.triggered_by.clone().unwrap_or_default(),
                st.accounts_scanned,
                st.counts,
            )
        };
        // Audit the run (best-effort; a logging failure never affects the scan).
        let logger = ModerationEventLogger::new(ctx.account_db.clone());
        let details = serde_json::json!({
            "scanId": scan_id,
            "outcome": outcome.as_str(),
            "accountsScanned": scanned,
            "findingsHigh": counts.high,
            "findingsMedium": counts.medium,
            "findingsLow": counts.low,
            "findingsTotal": counts.total(),
        });
        if let Err(e) = logger
            .log_event(LogEventParams {
                event_type: ModerationEventType::ScanCompleted,
                actor_did: &triggered_by,
                subject_did: None,
                subject_uri: None,
                subject_cid: None,
                details,
                meta: None,
            })
            .await
        {
            tracing::error!(target: "aurora_locus::repo_scan", error = %e, "scan finished but ScanCompleted audit emit failed");
        }
        tracing::info!(
            target: "aurora_locus::repo_scan",
            event = "repo_scan_completed",
            outcome = outcome.as_str(),
            accounts_scanned = scanned,
            findings = counts.total(),
            "repository scan finished",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mem_pool() -> AnyPool {
        use std::sync::Once;
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
        let db = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE repo_scan_finding (scan_id TEXT NOT NULL, did TEXT NOT NULL, \
             severity TEXT NOT NULL, live_head TEXT, recon_head TEXT, detail TEXT NOT NULL, \
             created_at TEXT NOT NULL, PRIMARY KEY (scan_id, did))",
        )
        .execute(&db)
        .await
        .unwrap();
        db
    }

    fn finding(scan: &str, did: &str, sev: Severity) -> ScanFinding {
        ScanFinding {
            scan_id: scan.to_string(),
            did: did.to_string(),
            severity: sev,
            live_head: Some("bafylive".to_string()),
            recon_head: Some("bafyrecon".to_string()),
            detail: "test".to_string(),
            created_at: "2026-06-17T00:00:00Z".to_string(),
        }
    }

    #[tokio::test]
    async fn store_insert_count_list_clear_roundtrip() {
        let store = ScanFindingsStore::new(mem_pool().await);
        store.insert(&finding("s1", "did:plc:a", Severity::High)).await.unwrap();
        store.insert(&finding("s1", "did:plc:b", Severity::Medium)).await.unwrap();
        store.insert(&finding("s1", "did:plc:c", Severity::Medium)).await.unwrap();

        let counts = store.counts().await.unwrap();
        assert_eq!(counts.high, 1);
        assert_eq!(counts.medium, 2);
        assert_eq!(counts.total(), 3);

        // Severity filter.
        let med = store.list(Some(Severity::Medium), 10, None).await.unwrap();
        assert_eq!(med.len(), 2);
        assert!(med.iter().all(|f| f.severity == Severity::Medium));

        // Keyset pagination by did.
        let page1 = store.list(None, 2, None).await.unwrap();
        assert_eq!(page1.len(), 2);
        assert_eq!(page1[0].did, "did:plc:a");
        let page2 = store.list(None, 2, Some(&page1.last().unwrap().did)).await.unwrap();
        assert_eq!(page2.len(), 1);
        assert_eq!(page2[0].did, "did:plc:c");

        // Upsert (same scan_id+did) does not duplicate.
        store.insert(&finding("s1", "did:plc:a", Severity::Low)).await.unwrap();
        assert_eq!(store.counts().await.unwrap().total(), 3);
        assert_eq!(store.counts().await.unwrap().high, 0);
        assert_eq!(store.counts().await.unwrap().low, 1);

        // Clear empties the table.
        store.clear().await.unwrap();
        assert_eq!(store.counts().await.unwrap().total(), 0);
    }

    fn rs_resolved(head: &str, rev: &str) -> ReconState {
        ReconState::Resolved { head: head.to_string(), rev: rev.to_string() }
    }

    #[test]
    fn classify_decision_matrix() {
        // Consistent: heads + revs match → no finding.
        assert!(classify(rs_resolved("bafyA", "rev1"), Some(("bafyA".into(), "rev1".into()))).is_none());
        // Empty account (no history, no live root) → no finding.
        assert!(classify(ReconState::NoHistory, None).is_none());

        // Medium: reconstructed head differs from live head.
        let m = classify(rs_resolved("bafyRECON", "rev2"), Some(("bafyLIVE".into(), "rev2".into())))
            .expect("head mismatch is a finding");
        assert_eq!(m.severity, Severity::Medium);
        assert_eq!(m.recon_head.as_deref(), Some("bafyRECON"));
        assert_eq!(m.live_head.as_deref(), Some("bafyLIVE"));

        // Low: heads match, rev differs.
        let l = classify(rs_resolved("bafyA", "rev2"), Some(("bafyA".into(), "rev1".into())))
            .expect("rev drift is a finding");
        assert_eq!(l.severity, Severity::Low);

        // High: reconstruction failed (regardless of live).
        let h1 = classify(ReconState::Failed("missing block".into()), Some(("bafyA".into(), "r".into())))
            .expect("unreconstructable is a finding");
        assert_eq!(h1.severity, Severity::High);
        assert!(h1.detail.contains("missing block"));

        // High: live repo exists but sequencer has no history.
        let h2 = classify(ReconState::NoHistory, Some(("bafyA".into(), "r".into())))
            .expect("live-without-history is a finding");
        assert_eq!(h2.severity, Severity::High);

        // High: history reconstructs but live root absent.
        let h3 = classify(rs_resolved("bafyA", "r"), None).expect("missing-live is a finding");
        assert_eq!(h3.severity, Severity::High);
        assert_eq!(h3.recon_head.as_deref(), Some("bafyA"));
        assert!(h3.live_head.is_none());
    }

    #[test]
    fn severity_str_roundtrip() {
        for s in [Severity::High, Severity::Medium, Severity::Low] {
            assert_eq!(Severity::from_str(s.as_str()).unwrap(), s);
        }
        assert!(Severity::from_str("bogus").is_err());
    }

    #[test]
    fn scan_job_single_flight_and_cancel() {
        let job = ScanJob::new();
        assert!(!job.progress().running);
        assert!(!job.request_cancel(), "cancel with no scan in flight is a no-op");

        let sid = job.begin("did:plc:op".to_string()).expect("first begin starts");
        assert!(job.progress().running);
        assert_eq!(job.progress().scan_id.as_deref(), Some(sid.as_str()));
        assert!(job.begin("did:plc:op".to_string()).is_none(), "second begin rejected");

        assert!(job.request_cancel(), "cancel while running returns true");
        assert!(job.progress().cancel_requested);
    }
}
