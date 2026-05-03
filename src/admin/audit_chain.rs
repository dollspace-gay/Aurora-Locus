//! Hash-chained audit log + snapshot infrastructure (Phase 3.8 /
//! chainlink #105 / docs/AURORA_ADMIN_UI_DESIGN.md §3.4, §8.4).
//!
//! Two co-equal pieces:
//! - **audit_snapshot** — content captured at decision time. The
//!   snapshot is what the subject looked like when an operator made
//!   a call.
//! - **audit_chain_entry** — append-only chain of operator decisions.
//!   Each entry's `current_hash` is SHA-256 over its content plus
//!   the previous entry's hash, giving tamper-evident replay.
//!
//! Verification: re-hash the entry content + previous_hash, compare
//! against current_hash. If any field changes between write and
//! later inspection, the recomputed hash diverges and the entry's
//! `verified` flag flips false.
//!
//! Pre-Phase-3.8 events have neither — the historical
//! `moderation_event` rows pre-date this infrastructure. Per §8.4
//! they surface in `getAuditTrail` with `current_hash="pre-chain"`
//! sentinel and `verified=false` so consumers know the difference.

use crate::{admin::defs::Subject, error::PdsError};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{AnyPool, Row};

/// One audit chain entry. Ships in `getAuditTrail` responses and is
/// what the Audit page renders rows for.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    pub id: String,
    pub sequence: i64,
    pub timestamp: DateTime<Utc>,
    pub actor_did: String,
    pub action: String,
    pub subject_ref: Option<Subject>,
    pub rationale: String,
    pub snapshot_id: Option<String>,
    pub event_id: Option<String>,
    pub current_hash: String,
    pub previous_hash: Option<String>,
    pub verified: bool,
    pub cascade_subjects: Vec<Subject>,
}

/// Compact subject capture for snapshot content. v0.2 ships the
/// account-shape; record/blob shapes added in v0.3 when per-record
/// state is more meaningful (Phase 3.7 aggregations open the door).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotContent {
    pub kind: &'static str,
    /// Non-empty for account snapshots.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub did: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub takedown_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deactivated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_action: Option<String>,
    /// Non-empty for record snapshots; future-friendly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
}

/// Capture a snapshot for the given subject. Returns the snapshot's
/// row id if a meaningful snapshot was captured; `None` for subjects
/// that aren't snapshottable yet (per §3.4: "snapshot capture is
/// opt-out for actions that don't benefit from snapshots").
pub async fn capture_snapshot(
    db: &AnyPool,
    subject: &Subject,
) -> Result<Option<i64>, PdsError> {
    let content = match subject {
        Subject::Repo { did } => fetch_account_snapshot(db, did).await?,
        Subject::Record { uri, cid } => SnapshotContent {
            kind: "record",
            did: None,
            handle: None,
            takedown_ref: None,
            deactivated_at: None,
            active_action: None,
            uri: Some(uri.clone()),
            cid: Some(cid.clone()),
        },
        Subject::Blob { did, cid, .. } => SnapshotContent {
            kind: "blob",
            did: Some(did.clone()),
            handle: None,
            takedown_ref: None,
            deactivated_at: None,
            active_action: None,
            uri: None,
            cid: Some(cid.clone()),
        },
    };
    let content_json =
        serde_json::to_string(&content).map_err(|e| PdsError::Internal(e.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(content_json.as_bytes());
    let content_hash = hex::encode(hasher.finalize());
    let now = Utc::now().to_rfc3339();
    let (subject_did, subject_uri, subject_cid) = match subject {
        Subject::Repo { did } => (Some(did.clone()), None, None),
        Subject::Record { uri, cid } => (None, Some(uri.clone()), Some(cid.clone())),
        Subject::Blob { did, cid, .. } => (Some(did.clone()), None, Some(cid.clone())),
    };
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO audit_snapshot (captured_at, subject_did, subject_uri, subject_cid, \
                                     content, content_hash) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(&now)
    .bind(&subject_did)
    .bind(&subject_uri)
    .bind(&subject_cid)
    .bind(&content_json)
    .bind(&content_hash)
    .fetch_one(db)
    .await?;
    Ok(Some(id))
}

async fn fetch_account_snapshot(db: &AnyPool, did: &str) -> Result<SnapshotContent, PdsError> {
    let row = sqlx::query(
        "SELECT handle, takedown_ref, deactivated_at FROM actor WHERE did = $1",
    )
    .bind(did)
    .fetch_optional(db)
    .await?;
    let (handle, takedown_ref, deactivated_at) = match row {
        Some(r) => (
            r.try_get::<Option<String>, _>("handle").ok().flatten(),
            r.try_get::<Option<String>, _>("takedown_ref").ok().flatten(),
            r.try_get::<Option<String>, _>("deactivated_at").ok().flatten(),
        ),
        None => (None, None, None),
    };
    let active_action: Option<String> = sqlx::query_scalar(
        "SELECT action FROM account_moderation \
         WHERE did = $1 AND NOT reversed \
         ORDER BY moderated_at DESC LIMIT 1",
    )
    .bind(did)
    .fetch_optional(db)
    .await?;
    Ok(SnapshotContent {
        kind: "account",
        did: Some(did.to_string()),
        handle,
        takedown_ref,
        deactivated_at,
        active_action,
        uri: None,
        cid: None,
    })
}

/// Append a new entry to the chain. Computes the SHA-256 over a
/// canonical JSON of (sequence, timestamp, actor, action, subject,
/// rationale, snapshot_id, event_id, previous_hash) — re-hashing
/// later in the same way reproduces `current_hash`, which is the
/// verification primitive.
pub struct AppendEntryParams<'a> {
    pub actor_did: &'a str,
    pub action: &'a str,
    pub subject: Option<&'a Subject>,
    pub rationale: &'a str,
    pub snapshot_id: Option<i64>,
    pub event_id: Option<i64>,
    pub cascade_subjects: &'a [Subject],
}

pub async fn append_entry(
    db: &AnyPool,
    params: AppendEntryParams<'_>,
) -> Result<i64, PdsError> {
    // Determine sequence + previous hash from the chain head.
    let head: Option<(i64, String)> = sqlx::query_as::<_, (i64, String)>(
        "SELECT sequence, current_hash FROM audit_chain_entry ORDER BY sequence DESC LIMIT 1",
    )
    .fetch_optional(db)
    .await?;
    let (seq, prev_hash) = match head {
        Some((s, h)) => (s + 1, Some(h)),
        None => (1, None),
    };
    let now = Utc::now();
    let now_str = now.to_rfc3339();
    let (subject_did, subject_uri, subject_cid) = match params.subject {
        Some(Subject::Repo { did }) => (Some(did.clone()), None, None),
        Some(Subject::Record { uri, cid }) => (None, Some(uri.clone()), Some(cid.clone())),
        Some(Subject::Blob { did, cid, .. }) => (Some(did.clone()), None, Some(cid.clone())),
        None => (None, None, None),
    };
    let cascade_json = if params.cascade_subjects.is_empty() {
        None
    } else {
        Some(
            serde_json::to_string(params.cascade_subjects)
                .map_err(|e| PdsError::Internal(e.to_string()))?,
        )
    };
    // Canonical content for hashing — order matters; future schema
    // additions append new fields rather than reorder existing ones.
    let canon = serde_json::json!({
        "sequence": seq,
        "timestamp": now_str,
        "actor_did": params.actor_did,
        "action": params.action,
        "subject_did": subject_did,
        "subject_uri": subject_uri,
        "subject_cid": subject_cid,
        "rationale": params.rationale,
        "snapshot_id": params.snapshot_id,
        "event_id": params.event_id,
        "previous_hash": prev_hash,
        "cascade_subjects": cascade_json,
    });
    let canon_str = serde_json::to_string(&canon)
        .map_err(|e| PdsError::Internal(e.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(canon_str.as_bytes());
    let current_hash = hex::encode(hasher.finalize());
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO audit_chain_entry \
         (sequence, created_at, actor_did, action, subject_did, subject_uri, subject_cid, \
          rationale, snapshot_id, event_id, current_hash, previous_hash, cascade_subjects) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) RETURNING id",
    )
    .bind(seq)
    .bind(&now_str)
    .bind(params.actor_did)
    .bind(params.action)
    .bind(&subject_did)
    .bind(&subject_uri)
    .bind(&subject_cid)
    .bind(params.rationale)
    .bind(params.snapshot_id)
    .bind(params.event_id)
    .bind(&current_hash)
    .bind(&prev_hash)
    .bind(&cascade_json)
    .fetch_one(db)
    .await?;
    Ok(id)
}

/// Recompute an entry's hash from its stored fields and compare to
/// `current_hash`. Used by getAuditTrail to set the `verified` flag
/// per row at query time.
pub fn verify_entry(
    sequence: i64,
    timestamp: &str,
    actor_did: &str,
    action: &str,
    subject_did: Option<&str>,
    subject_uri: Option<&str>,
    subject_cid: Option<&str>,
    rationale: &str,
    snapshot_id: Option<i64>,
    event_id: Option<i64>,
    previous_hash: Option<&str>,
    cascade_subjects: Option<&str>,
    expected_hash: &str,
) -> bool {
    let canon = serde_json::json!({
        "sequence": sequence,
        "timestamp": timestamp,
        "actor_did": actor_did,
        "action": action,
        "subject_did": subject_did,
        "subject_uri": subject_uri,
        "subject_cid": subject_cid,
        "rationale": rationale,
        "snapshot_id": snapshot_id,
        "event_id": event_id,
        "previous_hash": previous_hash,
        "cascade_subjects": cascade_subjects,
    });
    let canon_str = match serde_json::to_string(&canon) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let mut hasher = Sha256::new();
    hasher.update(canon_str.as_bytes());
    let computed = hex::encode(hasher.finalize());
    computed == expected_hash
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
        // Minimal schema for the tables we exercise.
        sqlx::query(
            "CREATE TABLE audit_chain_entry (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                sequence INTEGER NOT NULL,
                created_at TEXT NOT NULL,
                actor_did TEXT NOT NULL,
                action TEXT NOT NULL,
                subject_did TEXT,
                subject_uri TEXT,
                subject_cid TEXT,
                rationale TEXT NOT NULL,
                snapshot_id INTEGER,
                event_id INTEGER,
                current_hash TEXT NOT NULL,
                previous_hash TEXT,
                cascade_subjects TEXT,
                UNIQUE(sequence)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE audit_snapshot (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                captured_at TEXT NOT NULL,
                subject_did TEXT,
                subject_uri TEXT,
                subject_cid TEXT,
                content TEXT NOT NULL,
                content_hash TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE actor (did TEXT PRIMARY KEY, handle TEXT, takedown_ref TEXT, \
             deactivated_at TEXT, created_at TEXT)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE account_moderation (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                did TEXT, action TEXT, reason TEXT, moderated_by TEXT,
                moderated_at TEXT, expires_at TEXT, reversed INTEGER NOT NULL DEFAULT 0,
                reversed_at TEXT, report_id INTEGER
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn first_entry_has_no_previous_hash() {
        let db = open_test_pool().await;
        let subject = Subject::Repo {
            did: "did:plc:s".to_string(),
        };
        let id = append_entry(
            &db,
            AppendEntryParams {
                actor_did: "did:plc:m1",
                action: "TakedownAccount",
                subject: Some(&subject),
                rationale: "spam",
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[],
            },
        )
        .await
        .unwrap();
        assert!(id > 0);
        let prev: Option<String> =
            sqlx::query_scalar("SELECT previous_hash FROM audit_chain_entry WHERE id = $1")
                .bind(id)
                .fetch_one(&db)
                .await
                .unwrap();
        assert!(prev.is_none(), "first entry should have NULL previous_hash");
    }

    #[tokio::test]
    async fn second_entry_links_to_first() {
        let db = open_test_pool().await;
        let subject = Subject::Repo {
            did: "did:plc:s".to_string(),
        };
        append_entry(&db, AppendEntryParams {
            actor_did: "did:plc:m1", action: "TakedownAccount",
            subject: Some(&subject), rationale: "first",
            snapshot_id: None, event_id: None, cascade_subjects: &[],
        }).await.unwrap();
        append_entry(&db, AppendEntryParams {
            actor_did: "did:plc:m1", action: "RestoreAccount",
            subject: Some(&subject), rationale: "second",
            snapshot_id: None, event_id: None, cascade_subjects: &[],
        }).await.unwrap();
        let entries: Vec<(i64, String, Option<String>)> =
            sqlx::query_as("SELECT sequence, current_hash, previous_hash FROM audit_chain_entry ORDER BY sequence ASC")
                .fetch_all(&db)
                .await
                .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, 1);
        assert_eq!(entries[1].0, 2);
        assert_eq!(entries[1].2.as_deref(), Some(entries[0].1.as_str()));
    }

    #[tokio::test]
    async fn verify_entry_round_trips_for_freshly_written_entry() {
        let db = open_test_pool().await;
        let subject = Subject::Repo {
            did: "did:plc:victim".to_string(),
        };
        let id = append_entry(&db, AppendEntryParams {
            actor_did: "did:plc:m1", action: "SuspendAccount",
            subject: Some(&subject), rationale: "rationale text",
            snapshot_id: Some(42), event_id: Some(7), cascade_subjects: &[],
        }).await.unwrap();
        let row = sqlx::query(
            "SELECT sequence, created_at, actor_did, action, subject_did, subject_uri, \
                    subject_cid, rationale, snapshot_id, event_id, current_hash, previous_hash, \
                    cascade_subjects \
             FROM audit_chain_entry WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&db)
        .await
        .unwrap();
        let verified = verify_entry(
            row.try_get::<i64, _>("sequence").unwrap(),
            &row.try_get::<String, _>("created_at").unwrap(),
            &row.try_get::<String, _>("actor_did").unwrap(),
            &row.try_get::<String, _>("action").unwrap(),
            row.try_get::<Option<String>, _>("subject_did").unwrap().as_deref(),
            row.try_get::<Option<String>, _>("subject_uri").unwrap().as_deref(),
            row.try_get::<Option<String>, _>("subject_cid").unwrap().as_deref(),
            &row.try_get::<String, _>("rationale").unwrap(),
            row.try_get::<Option<i64>, _>("snapshot_id").unwrap(),
            row.try_get::<Option<i64>, _>("event_id").unwrap(),
            row.try_get::<Option<String>, _>("previous_hash").unwrap().as_deref(),
            row.try_get::<Option<String>, _>("cascade_subjects").unwrap().as_deref(),
            &row.try_get::<String, _>("current_hash").unwrap(),
        );
        assert!(verified, "fresh entry should verify against its stored hash");
    }

    #[tokio::test]
    async fn verify_entry_fails_when_rationale_tampered() {
        let db = open_test_pool().await;
        let subject = Subject::Repo {
            did: "did:plc:victim".to_string(),
        };
        let id = append_entry(&db, AppendEntryParams {
            actor_did: "did:plc:m1", action: "TakedownAccount",
            subject: Some(&subject), rationale: "original rationale",
            snapshot_id: None, event_id: None, cascade_subjects: &[],
        }).await.unwrap();
        // Simulate tamper: rewrite rationale in place after the chain
        // entry was sealed. Verification should fail because the
        // recomputed hash diverges from the stored current_hash.
        sqlx::query("UPDATE audit_chain_entry SET rationale = $1 WHERE id = $2")
            .bind("tampered rationale")
            .bind(id)
            .execute(&db)
            .await
            .unwrap();
        let row = sqlx::query(
            "SELECT sequence, created_at, actor_did, action, subject_did, subject_uri, \
                    subject_cid, rationale, snapshot_id, event_id, current_hash, previous_hash, \
                    cascade_subjects \
             FROM audit_chain_entry WHERE id = $1",
        )
        .bind(id)
        .fetch_one(&db)
        .await
        .unwrap();
        let verified = verify_entry(
            row.try_get::<i64, _>("sequence").unwrap(),
            &row.try_get::<String, _>("created_at").unwrap(),
            &row.try_get::<String, _>("actor_did").unwrap(),
            &row.try_get::<String, _>("action").unwrap(),
            row.try_get::<Option<String>, _>("subject_did").unwrap().as_deref(),
            row.try_get::<Option<String>, _>("subject_uri").unwrap().as_deref(),
            row.try_get::<Option<String>, _>("subject_cid").unwrap().as_deref(),
            &row.try_get::<String, _>("rationale").unwrap(),
            row.try_get::<Option<i64>, _>("snapshot_id").unwrap(),
            row.try_get::<Option<i64>, _>("event_id").unwrap(),
            row.try_get::<Option<String>, _>("previous_hash").unwrap().as_deref(),
            row.try_get::<Option<String>, _>("cascade_subjects").unwrap().as_deref(),
            &row.try_get::<String, _>("current_hash").unwrap(),
        );
        assert!(!verified, "tampered entry must fail verification");
    }

    #[tokio::test]
    async fn capture_snapshot_for_account_subject_writes_row() {
        let db = open_test_pool().await;
        sqlx::query("INSERT INTO actor (did, handle, takedown_ref, created_at) VALUES ($1, $2, $3, $4)")
            .bind("did:plc:s")
            .bind("s.test")
            .bind("ticket-1")
            .bind(Utc::now().to_rfc3339())
            .execute(&db)
            .await
            .unwrap();
        let id = capture_snapshot(&db, &Subject::Repo { did: "did:plc:s".to_string() })
            .await
            .unwrap()
            .unwrap();
        assert!(id > 0);
        let content: String = sqlx::query_scalar("SELECT content FROM audit_snapshot WHERE id = $1")
            .bind(id)
            .fetch_one(&db)
            .await
            .unwrap();
        assert!(content.contains("\"handle\":\"s.test\""));
        assert!(content.contains("\"takedownRef\":\"ticket-1\""));
    }
}
