// Allow dead_code - appeals system is defined for future moderation features
#![allow(dead_code)]

//! Moderation Appeal System
//!
//! Allows users to appeal moderation decisions.
//! Provides due process and oversight for moderation actions.

use crate::admin::defs::Subject;
use crate::error::{PdsError, PdsResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{AnyPool, Row};
use std::str::FromStr;

/// Appeal status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppealStatus {
    /// Pending review
    Pending,
    /// Under review by moderator
    UnderReview,
    /// Appeal approved, action reversed
    Approved,
    /// Appeal denied, action upheld
    Denied,
    /// Appeal escalated to senior moderator
    Escalated,
}

impl AppealStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AppealStatus::Pending => "pending",
            AppealStatus::UnderReview => "under_review",
            AppealStatus::Approved => "approved",
            AppealStatus::Denied => "denied",
            AppealStatus::Escalated => "escalated",
        }
    }
}

impl FromStr for AppealStatus {
    type Err = PdsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pending" => Ok(AppealStatus::Pending),
            "under_review" => Ok(AppealStatus::UnderReview),
            "approved" => Ok(AppealStatus::Approved),
            "denied" => Ok(AppealStatus::Denied),
            "escalated" => Ok(AppealStatus::Escalated),
            _ => Err(PdsError::Validation(format!(
                "Invalid appeal status: {}",
                s
            ))),
        }
    }
}

/// Appeal record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Appeal {
    pub id: i64,
    pub moderation_id: Option<i64>,
    pub report_id: Option<i64>,
    pub quarantine_id: Option<i64>,
    pub appellant_did: String,
    pub reason: String,
    pub details: Option<String>,
    pub submitted_at: DateTime<Utc>,
    pub status: AppealStatus,
    pub reviewed_by: Option<String>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub decision: Option<String>,
    pub notes: Option<String>,
}

/// Appeal manager
#[derive(Clone)]
pub struct AppealManager {
    db: AnyPool,
}

impl AppealManager {
    pub fn new(db: AnyPool) -> Self {
        Self { db }
    }

    /// Submit an appeal
    pub async fn submit_appeal(
        &self,
        moderation_id: Option<i64>,
        report_id: Option<i64>,
        quarantine_id: Option<i64>,
        appellant_did: &str,
        reason: &str,
        details: Option<&str>,
    ) -> PdsResult<Appeal> {
        let now = Utc::now();

        // Validate that at least one reference is provided
        if moderation_id.is_none() && report_id.is_none() && quarantine_id.is_none() {
            return Err(PdsError::Validation(
                "Must provide moderation_id, report_id, or quarantine_id".to_string(),
            ));
        }

        // Check for duplicate appeals
        if let Some(mod_id) = moderation_id {
            let existing: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM appeal WHERE moderation_id = $1 AND status IN ('pending', 'under_review', 'escalated')"
            )
            .bind(mod_id)
            .fetch_one(&self.db)
            .await?;

            if existing > 0 {
                return Err(PdsError::Conflict(
                    "An active appeal already exists for this moderation action".to_string(),
                ));
            }
        }

        // RETURNING id is portable (SQLite 3.35+, Postgres). AnyPool's
        // last_insert_id() is unreliable on SQLite, so we round-trip the
        // generated id explicitly.
        let id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO appeal (moderation_id, report_id, quarantine_id, appellant_did, reason, details, submitted_at, status)
            VALUES ($1, $2, $3, $4, $5, $6, $7, 'pending')
            RETURNING id
            "#,
        )
        .bind(moderation_id)
        .bind(report_id)
        .bind(quarantine_id)
        .bind(appellant_did)
        .bind(reason)
        .bind(details)
        .bind(now.to_rfc3339())
        .fetch_one(&self.db)
        .await?;

        tracing::info!(
            "Appeal submitted by {} for moderation_id: {:?}, report_id: {:?}",
            appellant_did,
            moderation_id,
            report_id
        );

        Ok(Appeal {
            id,
            moderation_id,
            report_id,
            quarantine_id,
            appellant_did: appellant_did.to_string(),
            reason: reason.to_string(),
            details: details.map(String::from),
            submitted_at: now,
            status: AppealStatus::Pending,
            reviewed_by: None,
            reviewed_at: None,
            decision: None,
            notes: None,
        })
    }

    /// Update appeal status. Pool-API wrapper that opens its own
    /// transaction; for atomic-with-chain entry AND subject-target
    /// validation, callers should use [`Self::update_status_in_tx`]
    /// (Arc 4 §8.4.0.5 / Step 0.6 §2 — chainlink #130).
    ///
    /// This wrapper does **not** validate the appeal's subject target
    /// — it preserves the v0.2 behaviour for legacy callers
    /// (`approve_appeal`, `deny_appeal`, `escalate_appeal` internally).
    /// New callers reaching `update_status` from `dispatch_action`'s
    /// `ResolveAppeal` / `EscalateAppeal` arms migrate to
    /// `update_status_in_tx` so they get JOIN-and-validate against
    /// `subjects[0]`.
    pub async fn update_status(
        &self,
        appeal_id: i64,
        status: AppealStatus,
        reviewed_by: &str,
        decision: Option<&str>,
        notes: Option<&str>,
    ) -> PdsResult<()> {
        let mut tx = self.db.begin().await?;
        Self::update_status_unchecked_in_tx(
            &mut tx,
            appeal_id,
            status,
            reviewed_by,
            decision,
            notes,
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Update appeal status inside an existing transaction, with
    /// subject-target validation. Arc 4 §8.4.0.5 / Step 0.6 §2 (JOIN-and-
    /// validate decision / chainlink #130) atomic-with-chain entry
    /// point.
    ///
    /// Resolves the appeal's target Subject via the appropriate FK
    /// path (`moderation_id` → `account_moderation`, `report_id` →
    /// `report`, `quarantine_id` → `blob_quarantine`), then compares
    /// the resolved Subject to `expected_subject` per §8.3.4's rules:
    ///
    /// - Variant equality first → [`PdsError::SubjectVariantMismatch`]
    ///   on mismatch.
    /// - Identifier equality per variant → [`PdsError::SubjectTargetMismatch`]
    ///   on mismatch (DID for Repo; URI for Record (CID informational
    ///   per URI-level semantics); CID for Blob (DID informational
    ///   per quarantine_id path)).
    /// - All three FK columns NULL → [`PdsError::OrphanedAppeal`]
    ///   (defensive — `submit_appeal` enforces at-least-one but no
    ///   schema CHECK exists; v0.4 follow-up #24).
    ///
    /// All three error variants map to HTTP 400 at the handler.
    ///
    /// FK precedence for the resolution: `moderation_id` first, then
    /// `report_id`, then `quarantine_id`. If multiple are set
    /// (today's invariant: at most one), the first present FK wins.
    pub async fn update_status_in_tx<'tx>(
        tx: &mut sqlx::Transaction<'tx, sqlx::Any>,
        appeal_id: i64,
        status: AppealStatus,
        reviewed_by: &str,
        decision: Option<&str>,
        notes: Option<&str>,
        expected_subject: &Subject,
    ) -> PdsResult<()> {
        // 1. Read the appeal's FK columns inside the wrapping tx so
        //    pending writes from the same caller are visible.
        let row = sqlx::query(
            "SELECT moderation_id, report_id, quarantine_id FROM appeal WHERE id = $1",
        )
        .bind(appeal_id)
        .fetch_optional(&mut **tx)
        .await?;
        let row = row.ok_or_else(|| {
            PdsError::NotFound(format!("Appeal {} not found", appeal_id))
        })?;
        let moderation_id: Option<i64> = row.try_get("moderation_id").ok().flatten();
        let report_id: Option<i64> = row.try_get("report_id").ok().flatten();
        let quarantine_id: Option<i64> = row.try_get("quarantine_id").ok().flatten();

        // 2. Resolve to materialized Subject per FK precedence.
        let resolved = if let Some(mid) = moderation_id {
            // account_moderation has only `did`; resolves to Repo.
            let did: Option<String> = sqlx::query_scalar(
                "SELECT did FROM account_moderation WHERE id = $1",
            )
            .bind(mid)
            .fetch_optional(&mut **tx)
            .await?;
            let did = did.ok_or_else(|| {
                PdsError::NotFound(format!(
                    "Appeal {} references moderation {} which no longer exists",
                    appeal_id, mid
                ))
            })?;
            Subject::Repo { did }
        } else if let Some(rid) = report_id {
            // report carries flat subject_did/uri/cid columns; could
            // be any of Repo/Record/Blob.
            let r = sqlx::query(
                "SELECT subject_did, subject_uri, subject_cid FROM report WHERE id = $1",
            )
            .bind(rid)
            .fetch_optional(&mut **tx)
            .await?;
            let r = r.ok_or_else(|| {
                PdsError::NotFound(format!(
                    "Appeal {} references report {} which no longer exists",
                    appeal_id, rid
                ))
            })?;
            let s_did: Option<String> = r.try_get("subject_did").ok().flatten();
            let s_uri: Option<String> = r.try_get("subject_uri").ok().flatten();
            let s_cid: Option<String> = r.try_get("subject_cid").ok().flatten();
            Subject::from_columns(s_did.as_deref(), s_uri.as_deref(), s_cid.as_deref())
                .ok_or_else(|| {
                    PdsError::Internal(format!(
                        "Appeal {}'s report {} has no decodable subject columns",
                        appeal_id, rid
                    ))
                })?
        } else if let Some(qid) = quarantine_id {
            // blob_quarantine has only `cid`; resolves to Blob with
            // the caller's expected DID (since the table has no DID
            // column). Comparison only matches on CID per Step 0.6 §2.
            let cid: Option<String> = sqlx::query_scalar(
                "SELECT cid FROM blob_quarantine WHERE id = $1",
            )
            .bind(qid)
            .fetch_optional(&mut **tx)
            .await?;
            let cid = cid.ok_or_else(|| {
                PdsError::NotFound(format!(
                    "Appeal {} references quarantine {} which no longer exists",
                    appeal_id, qid
                ))
            })?;
            // Borrow the expected DID for the constructed Subject.
            // The DID is informational on this path; the equality
            // check below short-circuits to CID-only.
            let did = match expected_subject {
                Subject::Blob { did, .. } => did.clone(),
                _ => String::new(),
            };
            Subject::Blob {
                did,
                cid,
                record_uri: None,
            }
        } else {
            return Err(PdsError::OrphanedAppeal { appeal_id });
        };

        // 3. Compare resolved against expected per §8.3.4.
        compare_subjects(expected_subject, &resolved)?;

        // 4. Validation passed — apply the status update.
        Self::update_status_unchecked_in_tx(
            tx,
            appeal_id,
            status,
            reviewed_by,
            decision,
            notes,
        )
        .await
    }

    /// The status-update SQL with no validation, factored out so the
    /// wrapper and the validating `_in_tx` variant share it.
    async fn update_status_unchecked_in_tx<'tx>(
        tx: &mut sqlx::Transaction<'tx, sqlx::Any>,
        appeal_id: i64,
        status: AppealStatus,
        reviewed_by: &str,
        decision: Option<&str>,
        notes: Option<&str>,
    ) -> PdsResult<()> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            UPDATE appeal
            SET status = $1,
                reviewed_by = $2,
                reviewed_at = $3,
                decision = $4,
                notes = $5
            WHERE id = $6
            "#,
        )
        .bind(status.as_str())
        .bind(reviewed_by)
        .bind(now.to_rfc3339())
        .bind(decision)
        .bind(notes)
        .bind(appeal_id)
        .execute(&mut **tx)
        .await?;

        if result.rows_affected() == 0 {
            return Err(PdsError::NotFound(format!(
                "Appeal {} not found",
                appeal_id
            )));
        }

        tracing::info!(
            "Appeal {} updated to status: {:?} by {}",
            appeal_id,
            status,
            reviewed_by
        );

        Ok(())
    }

    /// Approve an appeal
    pub async fn approve_appeal(
        &self,
        appeal_id: i64,
        reviewed_by: &str,
        decision: &str,
    ) -> PdsResult<()> {
        self.update_status(
            appeal_id,
            AppealStatus::Approved,
            reviewed_by,
            Some(decision),
            None,
        )
        .await
    }

    /// Deny an appeal
    pub async fn deny_appeal(
        &self,
        appeal_id: i64,
        reviewed_by: &str,
        decision: &str,
    ) -> PdsResult<()> {
        self.update_status(
            appeal_id,
            AppealStatus::Denied,
            reviewed_by,
            Some(decision),
            None,
        )
        .await
    }

    /// Escalate an appeal
    pub async fn escalate_appeal(
        &self,
        appeal_id: i64,
        reviewed_by: &str,
        notes: &str,
    ) -> PdsResult<()> {
        self.update_status(
            appeal_id,
            AppealStatus::Escalated,
            reviewed_by,
            None,
            Some(notes),
        )
        .await
    }

    /// Get appeal by ID
    pub async fn get_appeal(&self, appeal_id: i64) -> PdsResult<Option<Appeal>> {
        let row = sqlx::query(
            r#"
            SELECT id, moderation_id, report_id, quarantine_id, appellant_did, reason, details,
                   submitted_at, status, reviewed_by, reviewed_at, decision, notes
            FROM appeal
            WHERE id = $1
            "#,
        )
        .bind(appeal_id)
        .fetch_optional(&self.db)
        .await?;

        let Some(row) = row else {
            return Ok(None);
        };

        Ok(Some(self.parse_appeal(row)?))
    }

    /// Get pending appeals
    pub async fn get_pending_appeals(&self, limit: i64) -> PdsResult<Vec<Appeal>> {
        let rows = sqlx::query(
            r#"
            SELECT id, moderation_id, report_id, quarantine_id, appellant_did, reason, details,
                   submitted_at, status, reviewed_by, reviewed_at, decision, notes
            FROM appeal
            WHERE status = 'pending'
            ORDER BY submitted_at ASC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.db)
        .await?;

        self.parse_appeals(rows).await
    }

    /// Get appeals by appellant
    pub async fn get_appeals_by_appellant(&self, did: &str) -> PdsResult<Vec<Appeal>> {
        let rows = sqlx::query(
            r#"
            SELECT id, moderation_id, report_id, quarantine_id, appellant_did, reason, details,
                   submitted_at, status, reviewed_by, reviewed_at, decision, notes
            FROM appeal
            WHERE appellant_did = $1
            ORDER BY submitted_at DESC
            "#,
        )
        .bind(did)
        .fetch_all(&self.db)
        .await?;

        self.parse_appeals(rows).await
    }

    /// Get appeals for a moderation action
    pub async fn get_appeals_for_moderation(&self, moderation_id: i64) -> PdsResult<Vec<Appeal>> {
        let rows = sqlx::query(
            r#"
            SELECT id, moderation_id, report_id, quarantine_id, appellant_did, reason, details,
                   submitted_at, status, reviewed_by, reviewed_at, decision, notes
            FROM appeal
            WHERE moderation_id = $1
            ORDER BY submitted_at DESC
            "#,
        )
        .bind(moderation_id)
        .fetch_all(&self.db)
        .await?;

        self.parse_appeals(rows).await
    }

    /// Parse database rows into Appeal objects
    async fn parse_appeals(&self, rows: Vec<sqlx::any::AnyRow>) -> PdsResult<Vec<Appeal>> {
        let mut appeals = Vec::new();
        for row in rows {
            appeals.push(self.parse_appeal(row)?);
        }
        Ok(appeals)
    }

    /// Parse single database row into Appeal
    fn parse_appeal(&self, row: sqlx::any::AnyRow) -> PdsResult<Appeal> {
        let status_str: String = row.get("status");
        let status = status_str.parse()?;

        let submitted_at_str: String = row.get("submitted_at");
        let submitted_at = DateTime::parse_from_rfc3339(&submitted_at_str)
            .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))?
            .with_timezone(&Utc);

        let reviewed_at = row
            .try_get::<String, _>("reviewed_at")
            .ok()
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        Ok(Appeal {
            id: row.get("id"),
            moderation_id: row.get("moderation_id"),
            report_id: row.get("report_id"),
            quarantine_id: row.get("quarantine_id"),
            appellant_did: row.get("appellant_did"),
            reason: row.get("reason"),
            details: row.get("details"),
            submitted_at,
            status,
            reviewed_by: row.get("reviewed_by"),
            reviewed_at,
            decision: row.get("decision"),
            notes: row.get("notes"),
        })
    }
}

/// Subject equality per Step 0.6 §2 / §8.3.4. Variant equality first
/// (`SubjectVariantMismatch` if different); then identifier equality
/// per variant (`SubjectTargetMismatch` on mismatch). The identifier
/// comparison rules:
/// - `Repo`: compare DIDs.
/// - `Record`: compare URIs only (CID is informational; record
///   takedowns are URI-level per Arc 4 Step 0 recon Q2).
/// - `Blob`: compare CIDs only (DID is informational on the
///   `quarantine_id` resolution path because `blob_quarantine` has
///   no DID column).
fn compare_subjects(expected: &Subject, resolved: &Subject) -> PdsResult<()> {
    let expected_variant = subject_variant_name(expected);
    let resolved_variant = subject_variant_name(resolved);
    if expected_variant != resolved_variant {
        return Err(PdsError::SubjectVariantMismatch {
            expected: expected_variant.to_string(),
            got: resolved_variant.to_string(),
        });
    }
    let identifier_match = match (expected, resolved) {
        (Subject::Repo { did: e_did }, Subject::Repo { did: r_did }) => e_did == r_did,
        (Subject::Record { uri: e_uri, .. }, Subject::Record { uri: r_uri, .. }) => {
            e_uri == r_uri
        }
        (Subject::Blob { cid: e_cid, .. }, Subject::Blob { cid: r_cid, .. }) => e_cid == r_cid,
        _ => unreachable!("variant equality already checked above"),
    };
    if !identifier_match {
        return Err(PdsError::SubjectTargetMismatch {
            expected: format_subject_identifier(expected),
            got: format_subject_identifier(resolved),
        });
    }
    Ok(())
}

fn subject_variant_name(s: &Subject) -> &'static str {
    match s {
        Subject::Repo { .. } => "Repo",
        Subject::Record { .. } => "Record",
        Subject::Blob { .. } => "Blob",
    }
}

fn format_subject_identifier(s: &Subject) -> String {
    match s {
        Subject::Repo { did } => format!("Repo({})", did),
        Subject::Record { uri, .. } => format!("Record({})", uri),
        Subject::Blob { cid, .. } => format!("Blob({})", cid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn open_test_pool() -> AnyPool {
        use std::sync::Once;
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
        sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_submit_and_process_appeal() {
        let db = open_test_pool().await;

        sqlx::query(
            r#"
            CREATE TABLE appeal (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                moderation_id INTEGER,
                report_id INTEGER,
                quarantine_id INTEGER,
                appellant_did TEXT NOT NULL,
                reason TEXT NOT NULL,
                details TEXT,
                submitted_at TEXT NOT NULL,
                status TEXT NOT NULL,
                reviewed_by TEXT,
                reviewed_at TEXT,
                decision TEXT,
                notes TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        let manager = AppealManager::new(db);

        // Submit appeal
        let appeal = manager
            .submit_appeal(
                Some(123),
                None,
                None,
                "did:plc:user",
                "False positive",
                Some("This was a mistake, I did not violate any rules"),
            )
            .await
            .unwrap();

        assert_eq!(appeal.status, AppealStatus::Pending);
        assert_eq!(appeal.appellant_did, "did:plc:user");

        // Get pending appeals
        let pending = manager.get_pending_appeals(10).await.unwrap();
        assert_eq!(pending.len(), 1);

        // Approve appeal
        manager
            .approve_appeal(
                appeal.id,
                "did:plc:admin",
                "Appeal granted, action reversed",
            )
            .await
            .unwrap();

        // Verify approval
        let updated = manager.get_appeal(appeal.id).await.unwrap().unwrap();
        assert_eq!(updated.status, AppealStatus::Approved);
        assert_eq!(updated.reviewed_by, Some("did:plc:admin".to_string()));

        // No more pending appeals
        let pending = manager.get_pending_appeals(10).await.unwrap();
        assert_eq!(pending.len(), 0);
    }

    #[tokio::test]
    async fn test_duplicate_appeal_prevention() {
        let db = open_test_pool().await;

        sqlx::query(
            r#"
            CREATE TABLE appeal (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                moderation_id INTEGER,
                report_id INTEGER,
                quarantine_id INTEGER,
                appellant_did TEXT NOT NULL,
                reason TEXT NOT NULL,
                details TEXT,
                submitted_at TEXT NOT NULL,
                status TEXT NOT NULL,
                reviewed_by TEXT,
                reviewed_at TEXT,
                decision TEXT,
                notes TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();

        let manager = AppealManager::new(db);

        // Submit first appeal
        manager
            .submit_appeal(Some(123), None, None, "did:plc:user", "First appeal", None)
            .await
            .unwrap();

        // Try to submit duplicate appeal
        let result = manager
            .submit_appeal(Some(123), None, None, "did:plc:user", "Second appeal", None)
            .await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PdsError::Conflict(_)));
    }

    // ====================================================================
    // Arc 4 §8.4.0.5 / Step 0.6 §2 (chainlink #130) — update_status_in_tx
    // tests: commit + rollback semantics, plus the four validation paths
    // (matching subject success, variant mismatch, identifier mismatch,
    // orphaned appeal).
    // ====================================================================

    /// Stand up the appeal/account_moderation/report/blob_quarantine
    /// schemas needed by `update_status_in_tx` resolution. Used by every
    /// validation test below.
    async fn setup_appeal_schema() -> AnyPool {
        let db = open_test_pool().await;
        sqlx::query(
            r#"
            CREATE TABLE appeal (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                moderation_id INTEGER,
                report_id INTEGER,
                quarantine_id INTEGER,
                appellant_did TEXT NOT NULL,
                reason TEXT NOT NULL,
                details TEXT,
                submitted_at TEXT NOT NULL,
                status TEXT NOT NULL,
                reviewed_by TEXT,
                reviewed_at TEXT,
                decision TEXT,
                notes TEXT
            )
            "#,
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE account_moderation (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                did TEXT NOT NULL,
                action TEXT NOT NULL,
                reason TEXT,
                created_by TEXT NOT NULL,
                created_at TEXT NOT NULL
            )",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE report (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                subject_did TEXT,
                subject_uri TEXT,
                subject_cid TEXT,
                reporter_did TEXT NOT NULL,
                reason TEXT NOT NULL,
                created_at TEXT NOT NULL
            )",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE blob_quarantine (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                cid TEXT NOT NULL,
                reason TEXT NOT NULL,
                details TEXT,
                quarantined_by TEXT NOT NULL,
                quarantined_at TEXT NOT NULL,
                restored_at TEXT,
                restored_by TEXT,
                legal_reference TEXT
            )",
        )
        .execute(&db)
        .await
        .unwrap();
        db
    }

    /// Insert an appeal row attached to a single moderation row keyed
    /// to `did`. Returns (appeal_id, moderation_id).
    async fn insert_repo_appeal(db: &AnyPool, did: &str) -> (i64, i64) {
        let mod_id: i64 = sqlx::query_scalar(
            "INSERT INTO account_moderation (did, action, reason, created_by, created_at)
             VALUES ($1, 'takedown', NULL, 'did:plc:admin', $2) RETURNING id",
        )
        .bind(did)
        .bind(Utc::now().to_rfc3339())
        .fetch_one(db)
        .await
        .unwrap();
        let appeal_id: i64 = sqlx::query_scalar(
            "INSERT INTO appeal (moderation_id, appellant_did, reason, submitted_at, status)
             VALUES ($1, $2, 'unfair', $3, 'pending') RETURNING id",
        )
        .bind(mod_id)
        .bind(did)
        .bind(Utc::now().to_rfc3339())
        .fetch_one(db)
        .await
        .unwrap();
        (appeal_id, mod_id)
    }

    async fn appeal_status(db: &AnyPool, appeal_id: i64) -> String {
        sqlx::query_scalar("SELECT status FROM appeal WHERE id = $1")
            .bind(appeal_id)
            .fetch_one(db)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn update_status_in_tx_matching_subject_commit() {
        let db = setup_appeal_schema().await;
        let (appeal_id, _) = insert_repo_appeal(&db, "did:plc:alice").await;

        let mut tx = db.begin().await.unwrap();
        AppealManager::update_status_in_tx(
            &mut tx,
            appeal_id,
            AppealStatus::Approved,
            "did:plc:admin",
            Some("granted"),
            None,
            &Subject::Repo {
                did: "did:plc:alice".to_string(),
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        assert_eq!(appeal_status(&db, appeal_id).await, "approved");
    }

    #[tokio::test]
    async fn update_status_in_tx_rolls_back_on_caller_rollback() {
        let db = setup_appeal_schema().await;
        let (appeal_id, _) = insert_repo_appeal(&db, "did:plc:alice").await;

        {
            let mut tx = db.begin().await.unwrap();
            AppealManager::update_status_in_tx(
                &mut tx,
                appeal_id,
                AppealStatus::Approved,
                "did:plc:admin",
                Some("granted"),
                None,
                &Subject::Repo {
                    did: "did:plc:alice".to_string(),
                },
            )
            .await
            .unwrap();
            tx.rollback().await.unwrap();
        }

        // Status must remain pending — the rolled-back tx must not
        // leak through.
        assert_eq!(appeal_status(&db, appeal_id).await, "pending");
    }

    #[tokio::test]
    async fn update_status_in_tx_rejects_variant_mismatch() {
        let db = setup_appeal_schema().await;
        let (appeal_id, _) = insert_repo_appeal(&db, "did:plc:alice").await;

        // Appeal is attached to a moderation row → resolves to Repo.
        // Caller passes a Record subject → variant mismatch.
        let mut tx = db.begin().await.unwrap();
        let err = AppealManager::update_status_in_tx(
            &mut tx,
            appeal_id,
            AppealStatus::Approved,
            "did:plc:admin",
            None,
            None,
            &Subject::Record {
                uri: "at://did:plc:alice/app.bsky.feed.post/1".to_string(),
                cid: "bafyrec".to_string(),
            },
        )
        .await
        .unwrap_err();
        let _ = tx.rollback().await;

        match err {
            PdsError::SubjectVariantMismatch { expected, got } => {
                assert_eq!(expected, "Record");
                assert_eq!(got, "Repo");
            }
            other => panic!("expected SubjectVariantMismatch, got {:?}", other),
        }
        // Status must remain pending — validation failure must not
        // mutate the row.
        assert_eq!(appeal_status(&db, appeal_id).await, "pending");
    }

    #[tokio::test]
    async fn update_status_in_tx_rejects_identifier_mismatch() {
        let db = setup_appeal_schema().await;
        let (appeal_id, _) = insert_repo_appeal(&db, "did:plc:alice").await;

        // Appeal resolves to Repo(did:plc:alice). Caller passes
        // Repo(did:plc:bob) — same variant, different DID → identifier
        // mismatch.
        let mut tx = db.begin().await.unwrap();
        let err = AppealManager::update_status_in_tx(
            &mut tx,
            appeal_id,
            AppealStatus::Approved,
            "did:plc:admin",
            None,
            None,
            &Subject::Repo {
                did: "did:plc:bob".to_string(),
            },
        )
        .await
        .unwrap_err();
        let _ = tx.rollback().await;

        match err {
            PdsError::SubjectTargetMismatch { expected, got } => {
                assert_eq!(expected, "Repo(did:plc:bob)");
                assert_eq!(got, "Repo(did:plc:alice)");
            }
            other => panic!("expected SubjectTargetMismatch, got {:?}", other),
        }
        assert_eq!(appeal_status(&db, appeal_id).await, "pending");
    }

    #[tokio::test]
    async fn update_status_in_tx_rejects_orphaned_appeal() {
        let db = setup_appeal_schema().await;
        // Insert an appeal row with all three FK columns NULL — this
        // shouldn't exist via `submit_appeal` but the in-tx method
        // defensively rejects it.
        let appeal_id: i64 = sqlx::query_scalar(
            "INSERT INTO appeal (appellant_did, reason, submitted_at, status)
             VALUES ($1, 'orphan', $2, 'pending') RETURNING id",
        )
        .bind("did:plc:alice")
        .bind(Utc::now().to_rfc3339())
        .fetch_one(&db)
        .await
        .unwrap();

        let mut tx = db.begin().await.unwrap();
        let err = AppealManager::update_status_in_tx(
            &mut tx,
            appeal_id,
            AppealStatus::Approved,
            "did:plc:admin",
            None,
            None,
            &Subject::Repo {
                did: "did:plc:alice".to_string(),
            },
        )
        .await
        .unwrap_err();
        let _ = tx.rollback().await;

        match err {
            PdsError::OrphanedAppeal { appeal_id: id } => assert_eq!(id, appeal_id),
            other => panic!("expected OrphanedAppeal, got {:?}", other),
        }
        assert_eq!(appeal_status(&db, appeal_id).await, "pending");
    }
}
