/// Label Management System
use crate::error::{PdsError, PdsResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{AnyPool, Row};

/// Content label
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    pub id: i64,
    pub uri: String, // AT-URI
    pub cid: Option<String>,
    pub val: String, // Label value (porn, spam, etc.)
    pub neg: bool,   // Negative label (removal)
    pub src: String, // DID of source
    pub created_at: DateTime<Utc>,
    pub created_by: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub sig: Option<Vec<u8>>,
}

/// Label manager
#[derive(Clone)]
pub struct LabelManager {
    db: AnyPool,
    server_did: String,
}

impl LabelManager {
    pub fn new(db: AnyPool, server_did: String) -> Self {
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
        let mut tx = self.db.begin().await?;
        let label = Self::apply_label_in_tx(
            &mut tx,
            &self.server_did,
            uri,
            cid,
            val,
            created_by,
            expires_in,
        )
        .await?;
        tx.commit().await?;
        Ok(label)
    }

    /// Apply label inside an existing transaction. LB-1 /
    /// chainlink #128 atomic-with-chain entry point.
    ///
    /// Takes `server_did` as a parameter rather than `&self` so the
    /// helper is callable without borrowing the full manager — handlers
    /// that already have the server-DID computed (the batch label
    /// handlers) skip the manager-clone step. The pool-API wrapper
    /// `apply_label` passes `&self.server_did` through.
    #[allow(clippy::too_many_arguments)]
    pub async fn apply_label_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        server_did: &str,
        uri: &str,
        cid: Option<&str>,
        val: &str,
        created_by: &str,
        expires_in: Option<chrono::Duration>,
    ) -> PdsResult<Label> {
        let now = Utc::now();
        let expires_at = expires_in.map(|d| now + d);

        let id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO label (uri, cid, val, neg, src, created_at, created_by, expires_at)
            VALUES ($1, $2, $3, FALSE, $4, $5, $6, $7)
            RETURNING id
            "#,
        )
        .bind(uri)
        .bind(cid)
        .bind(val)
        .bind(server_did)
        .bind(now.to_rfc3339())
        .bind(created_by)
        .bind(expires_at.map(|dt| dt.to_rfc3339()))
        .fetch_one(&mut **tx)
        .await?;

        Ok(Label {
            id,
            uri: uri.to_string(),
            cid: cid.map(String::from),
            val: val.to_string(),
            neg: false,
            src: server_did.to_string(),
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
        let mut tx = self.db.begin().await?;
        let label =
            Self::remove_label_in_tx(&mut tx, &self.server_did, uri, cid, val, created_by).await?;
        tx.commit().await?;
        Ok(label)
    }

    /// Remove label (create negative label) inside an existing
    /// transaction. LB-1 / chainlink #128 atomic-with-chain entry point.
    pub async fn remove_label_in_tx<'c>(
        tx: &mut sqlx::Transaction<'c, sqlx::Any>,
        server_did: &str,
        uri: &str,
        cid: Option<&str>,
        val: &str,
        created_by: &str,
    ) -> PdsResult<Label> {
        let now = Utc::now();

        let id: i64 = sqlx::query_scalar(
            r#"
            INSERT INTO label (uri, cid, val, neg, src, created_at, created_by)
            VALUES ($1, $2, $3, TRUE, $4, $5, $6)
            RETURNING id
            "#,
        )
        .bind(uri)
        .bind(cid)
        .bind(val)
        .bind(server_did)
        .bind(now.to_rfc3339())
        .bind(created_by)
        .fetch_one(&mut **tx)
        .await?;

        Ok(Label {
            id,
            uri: uri.to_string(),
            cid: cid.map(String::from),
            val: val.to_string(),
            neg: true,
            src: server_did.to_string(),
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
            WHERE uri = $1
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
                neg: crate::db::read_bool(&row, "neg")?,
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

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::any::AnyPoolOptions;
    use std::sync::Once;

    async fn open_test_pool() -> AnyPool {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE label (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                uri TEXT NOT NULL,
                cid TEXT,
                val TEXT NOT NULL,
                neg INTEGER NOT NULL,
                src TEXT NOT NULL,
                created_at TEXT NOT NULL,
                created_by TEXT NOT NULL,
                expires_at TEXT,
                sig BLOB
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    // LB-1 / chainlink #128: LabelManager `_in_tx` variants must be
    // rollback-safe — the caller decides whether the label INSERT
    // commits.

    #[tokio::test]
    async fn apply_label_in_tx_rolls_back_on_caller_rollback() {
        let db = open_test_pool().await;
        {
            let mut tx = db.begin().await.unwrap();
            LabelManager::apply_label_in_tx(
                &mut tx,
                "did:web:server.test",
                "at://did:plc:victim/app.bsky.feed.post/abc",
                None,
                "spam",
                "did:plc:moderator",
                None,
            )
            .await
            .unwrap();
            tx.rollback().await.unwrap();
        }

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM label")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "rolled-back tx must not leave a label row"
        );
    }

    #[tokio::test]
    async fn apply_label_in_tx_commits_on_caller_commit() {
        let db = open_test_pool().await;
        let mut tx = db.begin().await.unwrap();
        let label = LabelManager::apply_label_in_tx(
            &mut tx,
            "did:web:server.test",
            "at://did:plc:victim/app.bsky.feed.post/abc",
            Some("bafkreitest"),
            "spam",
            "did:plc:moderator",
            None,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        assert_eq!(label.val, "spam");
        assert!(!label.neg);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM label WHERE val = 'spam'")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn remove_label_in_tx_rolls_back_on_caller_rollback() {
        let db = open_test_pool().await;
        {
            let mut tx = db.begin().await.unwrap();
            LabelManager::remove_label_in_tx(
                &mut tx,
                "did:web:server.test",
                "at://did:plc:victim/app.bsky.feed.post/abc",
                None,
                "spam",
                "did:plc:moderator",
            )
            .await
            .unwrap();
            tx.rollback().await.unwrap();
        }

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM label")
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(count, 0, "rolled-back tx must not leave a label row");
    }

    #[tokio::test]
    async fn remove_label_in_tx_writes_negative_label() {
        let db = open_test_pool().await;
        let mut tx = db.begin().await.unwrap();
        let label = LabelManager::remove_label_in_tx(
            &mut tx,
            "did:web:server.test",
            "at://did:plc:victim/app.bsky.feed.post/abc",
            None,
            "spam",
            "did:plc:moderator",
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        assert!(label.neg, "remove_label produces a negative label");
    }
}
