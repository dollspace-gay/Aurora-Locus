/// Report Management System
use crate::error::{PdsError, PdsResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

/// Report reason types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReportReason {
    Spam,
    Violation,
    Misleading,
    Sexual,
    Rude,
    Other,
}

impl ReportReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReportReason::Spam => "spam",
            ReportReason::Violation => "violation",
            ReportReason::Misleading => "misleading",
            ReportReason::Sexual => "sexual",
            ReportReason::Rude => "rude",
            ReportReason::Other => "other",
        }
    }

    pub fn from_str(s: &str) -> PdsResult<Self> {
        match s.to_lowercase().as_str() {
            "spam" => Ok(ReportReason::Spam),
            "violation" => Ok(ReportReason::Violation),
            "misleading" => Ok(ReportReason::Misleading),
            "sexual" => Ok(ReportReason::Sexual),
            "rude" => Ok(ReportReason::Rude),
            "other" => Ok(ReportReason::Other),
            _ => Err(PdsError::Validation(format!("Invalid report reason: {}", s))),
        }
    }
}

/// Report status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReportStatus {
    Open,
    Acknowledged,
    Resolved,
    Escalated,
}

impl ReportStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReportStatus::Open => "open",
            ReportStatus::Acknowledged => "acknowledged",
            ReportStatus::Resolved => "resolved",
            ReportStatus::Escalated => "escalated",
        }
    }

    pub fn from_str(s: &str) -> PdsResult<Self> {
        match s.to_lowercase().as_str() {
            "open" => Ok(ReportStatus::Open),
            "acknowledged" => Ok(ReportStatus::Acknowledged),
            "resolved" => Ok(ReportStatus::Resolved),
            "escalated" => Ok(ReportStatus::Escalated),
            _ => Err(PdsError::Validation(format!("Invalid report status: {}", s))),
        }
    }
}

/// Report record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub id: i64,
    pub subject_did: Option<String>,
    pub subject_uri: Option<String>,
    pub subject_cid: Option<String>,
    pub reason_type: ReportReason,
    pub reason: Option<String>,
    pub reported_by: String,
    pub reported_at: DateTime<Utc>,
    pub status: ReportStatus,
    pub reviewed_by: Option<String>,
    pub reviewed_at: Option<DateTime<Utc>>,
    pub resolution: Option<String>,
}

/// Report manager
#[derive(Clone)]
pub struct ReportManager {
    db: SqlitePool,
}

impl ReportManager {
    pub fn new(db: SqlitePool) -> Self {
        Self { db }
    }

    /// Submit a report
    pub async fn submit_report(
        &self,
        subject_did: Option<&str>,
        subject_uri: Option<&str>,
        subject_cid: Option<&str>,
        reason_type: ReportReason,
        reason: Option<&str>,
        reported_by: &str,
    ) -> PdsResult<Report> {
        let now = Utc::now();

        // Validate that at least one subject is provided
        if subject_did.is_none() && subject_uri.is_none() {
            return Err(PdsError::Validation(
                "Must provide either subject_did or subject_uri".to_string(),
            ));
        }

        let result = sqlx::query(
            r#"
            INSERT INTO report (subject_did, subject_uri, subject_cid, reason_type, reason, reported_by, reported_at, status)
            VALUES (?, ?, ?, ?, ?, ?, ?, 'open')
            "#,
        )
        .bind(subject_did)
        .bind(subject_uri)
        .bind(subject_cid)
        .bind(reason_type.as_str())
        .bind(reason)
        .bind(reported_by)
        .bind(now.to_rfc3339())
        .execute(&self.db)
        .await?;

        Ok(Report {
            id: result.last_insert_rowid(),
            subject_did: subject_did.map(String::from),
            subject_uri: subject_uri.map(String::from),
            subject_cid: subject_cid.map(String::from),
            reason_type,
            reason: reason.map(String::from),
            reported_by: reported_by.to_string(),
            reported_at: now,
            status: ReportStatus::Open,
            reviewed_by: None,
            reviewed_at: None,
            resolution: None,
        })
    }

    /// Update report status
    pub async fn update_status(
        &self,
        report_id: i64,
        status: ReportStatus,
        reviewed_by: &str,
        resolution: Option<&str>,
    ) -> PdsResult<()> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            UPDATE report
            SET status = ?,
                reviewed_by = ?,
                reviewed_at = ?,
                resolution = ?
            WHERE id = ?
            "#,
        )
        .bind(status.as_str())
        .bind(reviewed_by)
        .bind(now.to_rfc3339())
        .bind(resolution)
        .bind(report_id)
        .execute(&self.db)
        .await?;

        if result.rows_affected() == 0 {
            return Err(PdsError::NotFound(format!("Report {} not found", report_id)));
        }

        Ok(())
    }

    /// Get report by ID
    pub async fn get_report(&self, report_id: i64) -> PdsResult<Option<Report>> {
        let row = sqlx::query(
            r#"
            SELECT id, subject_did, subject_uri, subject_cid, reason_type, reason,
                   reported_by, reported_at, status, reviewed_by, reviewed_at, resolution
            FROM report
            WHERE id = ?
            "#,
        )
        .bind(report_id)
        .fetch_optional(&self.db)
        .await?;

        if let Some(row) = row {
            Ok(Some(self.parse_report(row)?))
        } else {
            Ok(None)
        }
    }

    /// List reports with optional filters
    pub async fn list_reports(
        &self,
        status: Option<ReportStatus>,
        limit: Option<i64>,
    ) -> PdsResult<Vec<Report>> {
        let query = if let Some(status) = status {
            sqlx::query(
                r#"
                SELECT id, subject_did, subject_uri, subject_cid, reason_type, reason,
                       reported_by, reported_at, status, reviewed_by, reviewed_at, resolution
                FROM report
                WHERE status = ?
                ORDER BY reported_at DESC
                LIMIT ?
                "#,
            )
            .bind(status.as_str())
            .bind(limit.unwrap_or(100))
        } else {
            sqlx::query(
                r#"
                SELECT id, subject_did, subject_uri, subject_cid, reason_type, reason,
                       reported_by, reported_at, status, reviewed_by, reviewed_at, resolution
                FROM report
                ORDER BY reported_at DESC
                LIMIT ?
                "#,
            )
            .bind(limit.unwrap_or(100))
        };

        let rows = query.fetch_all(&self.db).await?;

        let mut reports = Vec::new();
        for row in rows {
            reports.push(self.parse_report(row)?);
        }

        Ok(reports)
    }

    fn parse_report(&self, row: sqlx::sqlite::SqliteRow) -> PdsResult<Report> {
        let reason_type_str: String = row.get("reason_type");
        let reason_type = ReportReason::from_str(&reason_type_str)?;

        let status_str: String = row.get("status");
        let status = ReportStatus::from_str(&status_str)?;

        let reported_at_str: String = row.get("reported_at");
        let reported_at = DateTime::parse_from_rfc3339(&reported_at_str)
            .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))?
            .with_timezone(&Utc);

        let reviewed_at = row
            .try_get::<String, _>("reviewed_at")
            .ok()
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc));

        Ok(Report {
            id: row.get("id"),
            subject_did: row.get("subject_did"),
            subject_uri: row.get("subject_uri"),
            subject_cid: row.get("subject_cid"),
            reason_type,
            reason: row.get("reason"),
            reported_by: row.get("reported_by"),
            reported_at,
            status,
            reviewed_by: row.get("reviewed_by"),
            reviewed_at,
            resolution: row.get("resolution"),
        })
    }
}
