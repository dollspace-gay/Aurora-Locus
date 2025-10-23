/// Label Management System
use crate::error::{PdsError, PdsResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

/// Content label
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    pub id: i64,
    pub uri: String,  // AT-URI
    pub cid: Option<String>,
    pub val: String,  // Label value (porn, spam, etc.)
    pub neg: bool,    // Negative label (removal)
    pub src: String,  // DID of source
    pub created_at: DateTime<Utc>,
    pub created_by: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub sig: Option<Vec<u8>>,
}

/// Label manager
#[derive(Clone)]
pub struct LabelManager {
    db: SqlitePool,
    server_did: String,
}

impl LabelManager {
    pub fn new(db: SqlitePool, server_did: String) -> Self {
        Self { db, server_did }
    }

    /// Apply label to content or account
    pub async fn apply_label(
        &self,
        uri: &str,
        cid: Option<&str>,
        val: &str,
        created_by: &str,
        expires_in: Option<chrono::Duration>,
    ) -> PdsResult<Label> {
        let now = Utc::now();
        let expires_at = expires_in.map(|d| now + d);

        let result = sqlx::query(
            r#"
            INSERT INTO label (uri, cid, val, neg, src, created_at, created_by, expires_at)
            VALUES (?, ?, ?, 0, ?, ?, ?, ?)
            "#,
        )
        .bind(uri)
        .bind(cid)
        .bind(val)
        .bind(&self.server_did)
        .bind(now.to_rfc3339())
        .bind(created_by)
        .bind(expires_at.map(|dt| dt.to_rfc3339()))
        .execute(&self.db)
        .await?;

        Ok(Label {
            id: result.last_insert_rowid(),
            uri: uri.to_string(),
            cid: cid.map(String::from),
            val: val.to_string(),
            neg: false,
            src: self.server_did.clone(),
            created_at: now,
            created_by: created_by.to_string(),
            expires_at,
            sig: None,
        })
    }

    /// Remove label (create negative label)
    pub async fn remove_label(
        &self,
        uri: &str,
        cid: Option<&str>,
        val: &str,
        created_by: &str,
    ) -> PdsResult<Label> {
        let now = Utc::now();

        let result = sqlx::query(
            r#"
            INSERT INTO label (uri, cid, val, neg, src, created_at, created_by)
            VALUES (?, ?, ?, 1, ?, ?, ?)
            "#,
        )
        .bind(uri)
        .bind(cid)
        .bind(val)
        .bind(&self.server_did)
        .bind(now.to_rfc3339())
        .bind(created_by)
        .execute(&self.db)
        .await?;

        Ok(Label {
            id: result.last_insert_rowid(),
            uri: uri.to_string(),
            cid: cid.map(String::from),
            val: val.to_string(),
            neg: true,
            src: self.server_did.clone(),
            created_at: now,
            created_by: created_by.to_string(),
            expires_at: None,
            sig: None,
        })
    }

    /// Get all labels for a URI
    pub async fn get_labels(&self, uri: &str) -> PdsResult<Vec<Label>> {
        let rows = sqlx::query(
            r#"
            SELECT id, uri, cid, val, neg, src, created_at, created_by, expires_at, sig
            FROM label
            WHERE uri = ?
            ORDER BY created_at DESC
            "#,
        )
        .bind(uri)
        .fetch_all(&self.db)
        .await?;

        let mut labels = Vec::new();
        for row in rows {
            let created_at_str: String = row.get("created_at");
            let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                .map_err(|e| PdsError::Internal(format!("Invalid timestamp: {}", e)))?
                .with_timezone(&Utc);

            let expires_at = row
                .try_get::<String, _>("expires_at")
                .ok()
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));

            labels.push(Label {
                id: row.get("id"),
                uri: row.get("uri"),
                cid: row.get("cid"),
                val: row.get("val"),
                neg: row.get("neg"),
                src: row.get("src"),
                created_at,
                created_by: row.get("created_by"),
                expires_at,
                sig: row.get("sig"),
            });
        }

        Ok(labels)
    }
}
