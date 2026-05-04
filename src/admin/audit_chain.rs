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

/// What kind of failure `verify_chain_range` hit. Carried alongside
/// the failing sequence so callers can distinguish a tampered row
/// (PerRowMismatch) from a relinked chain (LinkageMismatch) from a
/// missing row (Gap).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainFailureKind {
    /// The entry's content rehashes to a value other than its stored
    /// `current_hash`. The row's fields were modified after seal.
    PerRowMismatch,
    /// The entry's `previous_hash` does not match the prior entry's
    /// `current_hash`. Either the prior entry's content was modified
    /// AND its `current_hash` was rewritten consistently (so per-row
    /// passes) but the linkage was not, or the prior entry was
    /// substituted with a forged row.
    LinkageMismatch,
    /// A sequence number in the requested range has no row. Indicates
    /// a deletion or that the range bounds are wrong; either way the
    /// chain in the requested window is not contiguous.
    Gap,
}

/// Result of a chain-level verification failure. The first violation
/// in the scanned window short-circuits — callers wanting a full audit
/// can rerun starting after `failing_sequence` to find subsequent
/// failures.
#[derive(Debug, Clone, Copy)]
pub struct ChainVerificationError {
    pub failing_sequence: i64,
    pub kind: ChainFailureKind,
}

impl std::fmt::Display for ChainVerificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.kind {
            ChainFailureKind::PerRowMismatch => "per-row hash mismatch",
            ChainFailureKind::LinkageMismatch => "linkage hash mismatch",
            ChainFailureKind::Gap => "missing sequence",
        };
        write!(f, "chain verification failed at sequence {}: {}", self.failing_sequence, kind)
    }
}

impl std::error::Error for ChainVerificationError {}

/// Verify the chain is internally consistent across `[start_seq, end_seq]`.
/// Walks the rows in ascending sequence order and checks two things per
/// row:
///
/// 1. The row's content rehashes to its stored `current_hash` (per-row).
/// 2. The row's `previous_hash` equals the prior row's `current_hash`
///    (linkage).
///
/// Returns `Ok(())` if every checked entry passes both. Returns the
/// first failure on mismatch — caller can re-scan after the failing
/// sequence if it wants a complete picture.
///
/// Pre-Phase-3.8 sentinel rows (`current_hash="pre-chain"`) are skipped
/// entirely; their linkage is undefined by design (§8.4) and verifying
/// them as if they were real chain entries would produce false
/// negatives.
///
/// `start_seq` and `end_seq` are inclusive. If `start_seq > end_seq` the
/// function returns `Ok(())` (empty window is trivially consistent).
pub async fn verify_chain_range(
    db: &AnyPool,
    start_seq: i64,
    end_seq: i64,
) -> Result<(), ChainVerificationError> {
    if start_seq > end_seq {
        return Ok(());
    }
    // Fetch the latest row strictly before start_seq so the linkage of
    // the first row in the window can be checked against its
    // predecessor. We do NOT filter sentinels here — when the
    // immediate predecessor is a sentinel, the first real row's
    // stored `previous_hash` is the literal string "pre-chain" and
    // that's what we want to match against.
    let mut prev_hash: Option<String> = sqlx::query_scalar::<_, String>(
        "SELECT current_hash FROM audit_chain_entry \
         WHERE sequence < $1 \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(start_seq)
    .fetch_optional(db)
    .await
    .map_err(|_| ChainVerificationError {
        failing_sequence: start_seq,
        kind: ChainFailureKind::Gap,
    })?;

    // Fetch the window in ascending order. We treat a query error as a
    // gap because the verification semantics require all rows in the
    // requested window to be readable.
    let rows = sqlx::query(
        "SELECT sequence, created_at, actor_did, action, subject_did, subject_uri, \
                subject_cid, rationale, snapshot_id, event_id, current_hash, previous_hash, \
                cascade_subjects \
         FROM audit_chain_entry \
         WHERE sequence >= $1 AND sequence <= $2 \
         ORDER BY sequence ASC",
    )
    .bind(start_seq)
    .bind(end_seq)
    .fetch_all(db)
    .await
    .map_err(|_| ChainVerificationError {
        failing_sequence: start_seq,
        kind: ChainFailureKind::Gap,
    })?;

    let mut expected_seq = start_seq;
    for row in rows {
        let seq: i64 = row.try_get("sequence").map_err(|_| ChainVerificationError {
            failing_sequence: expected_seq,
            kind: ChainFailureKind::Gap,
        })?;
        let current_hash: String = row.try_get("current_hash").map_err(|_| {
            ChainVerificationError {
                failing_sequence: seq,
                kind: ChainFailureKind::PerRowMismatch,
            }
        })?;

        // Sentinel rows: skip per-row + linkage checks but DO update
        // prev_hash so the next real row's stored_prev correctly
        // matches the literal "pre-chain" string. Sentinels still
        // count toward gap detection — the chain must be contiguous
        // even if some rows are sentinel.
        if current_hash == "pre-chain" {
            if seq != expected_seq {
                return Err(ChainVerificationError {
                    failing_sequence: expected_seq,
                    kind: ChainFailureKind::Gap,
                });
            }
            prev_hash = Some(current_hash);
            expected_seq = seq + 1;
            continue;
        }

        // Gap detection: every requested sequence must be present.
        if seq != expected_seq {
            return Err(ChainVerificationError {
                failing_sequence: expected_seq,
                kind: ChainFailureKind::Gap,
            });
        }

        let created_at_str: String = row.try_get("created_at").map_err(|_| {
            ChainVerificationError {
                failing_sequence: seq,
                kind: ChainFailureKind::PerRowMismatch,
            }
        })?;
        let actor_did: String = row.try_get("actor_did").map_err(|_| {
            ChainVerificationError {
                failing_sequence: seq,
                kind: ChainFailureKind::PerRowMismatch,
            }
        })?;
        let action: String = row.try_get("action").map_err(|_| {
            ChainVerificationError {
                failing_sequence: seq,
                kind: ChainFailureKind::PerRowMismatch,
            }
        })?;
        let subject_did: Option<String> = row.try_get("subject_did").ok().flatten();
        let subject_uri: Option<String> = row.try_get("subject_uri").ok().flatten();
        let subject_cid: Option<String> = row.try_get("subject_cid").ok().flatten();
        let rationale: String = row.try_get("rationale").map_err(|_| {
            ChainVerificationError {
                failing_sequence: seq,
                kind: ChainFailureKind::PerRowMismatch,
            }
        })?;
        let snapshot_id: Option<i64> = row.try_get("snapshot_id").ok().flatten();
        let event_id: Option<i64> = row.try_get("event_id").ok().flatten();
        let stored_prev: Option<String> = row.try_get("previous_hash").ok().flatten();
        let cascade_str: Option<String> = row.try_get("cascade_subjects").ok().flatten();

        // Linkage check: the row's stored previous_hash must equal the
        // prior non-sentinel row's current_hash. The very first non-
        // sentinel row of the chain has stored_prev == None and
        // prev_hash == None; both branches of the cmp below handle that
        // case correctly.
        if stored_prev.as_deref() != prev_hash.as_deref() {
            return Err(ChainVerificationError {
                failing_sequence: seq,
                kind: ChainFailureKind::LinkageMismatch,
            });
        }

        // Per-row check: rehash content + stored previous_hash, compare
        // to stored current_hash.
        let row_ok = verify_entry(
            seq,
            &created_at_str,
            &actor_did,
            &action,
            subject_did.as_deref(),
            subject_uri.as_deref(),
            subject_cid.as_deref(),
            &rationale,
            snapshot_id,
            event_id,
            stored_prev.as_deref(),
            cascade_str.as_deref(),
            &current_hash,
        );
        if !row_ok {
            return Err(ChainVerificationError {
                failing_sequence: seq,
                kind: ChainFailureKind::PerRowMismatch,
            });
        }

        prev_hash = Some(current_hash);
        expected_seq = seq + 1;
    }

    // If the loop ran out of rows before reaching end_seq, the tail of
    // the requested window is missing. Caller asked us to verify
    // through end_seq; failing to find rows up to that point is a gap.
    if expected_seq <= end_seq {
        // Confirm whether end_seq actually exists: if the chain head is
        // below end_seq, `Ok` is correct (caller asked for an
        // open-ended window). If the head is at or above end_seq but
        // we didn't see the row, it's a real gap.
        let head_seq: Option<i64> = sqlx::query_scalar(
            "SELECT MAX(sequence) FROM audit_chain_entry",
        )
        .fetch_optional(db)
        .await
        .ok()
        .flatten();
        if let Some(head) = head_seq {
            if head >= expected_seq {
                return Err(ChainVerificationError {
                    failing_sequence: expected_seq,
                    kind: ChainFailureKind::Gap,
                });
            }
        }
    }

    Ok(())
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
    async fn verify_chain_range_passes_for_clean_chain() {
        let db = open_test_pool().await;
        let subject = Subject::Repo {
            did: "did:plc:s".to_string(),
        };
        for i in 0..3 {
            append_entry(&db, AppendEntryParams {
                actor_did: "did:plc:m1",
                action: "TakedownAccount",
                subject: Some(&subject),
                rationale: &format!("rationale-{}", i),
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[],
            }).await.unwrap();
        }
        verify_chain_range(&db, 1, 3).await.expect("clean chain verifies");
    }

    #[tokio::test]
    async fn verify_chain_range_detects_per_row_tamper() {
        let db = open_test_pool().await;
        let subject = Subject::Repo {
            did: "did:plc:s".to_string(),
        };
        for i in 0..3 {
            append_entry(&db, AppendEntryParams {
                actor_did: "did:plc:m1", action: "TakedownAccount",
                subject: Some(&subject), rationale: &format!("orig-{}", i),
                snapshot_id: None, event_id: None, cascade_subjects: &[],
            }).await.unwrap();
        }
        // Tamper with row 2's rationale only — current_hash NOT updated,
        // so per-row recompute mismatches.
        sqlx::query("UPDATE audit_chain_entry SET rationale = 'tampered' WHERE sequence = 2")
            .execute(&db).await.unwrap();
        let err = verify_chain_range(&db, 1, 3).await.expect_err("tampered chain must fail");
        assert_eq!(err.failing_sequence, 2);
        assert_eq!(err.kind, ChainFailureKind::PerRowMismatch);
    }

    #[tokio::test]
    async fn verify_chain_range_detects_consistent_rewrite_via_linkage() {
        // The test the per-row verifier alone misses: an attacker
        // rewrites both content AND current_hash on a prior entry so
        // per-row passes, but the next entry's previous_hash still
        // points to the OLD current_hash → linkage breaks.
        let db = open_test_pool().await;
        let subject = Subject::Repo {
            did: "did:plc:s".to_string(),
        };
        for i in 0..3 {
            append_entry(&db, AppendEntryParams {
                actor_did: "did:plc:m1", action: "TakedownAccount",
                subject: Some(&subject), rationale: &format!("orig-{}", i),
                snapshot_id: None, event_id: None, cascade_subjects: &[],
            }).await.unwrap();
        }
        // Attacker recomputes a per-row-consistent hash for row 2 over
        // the new content. We simulate that by reading row 2's row and
        // recomputing the hash exactly the same way append_entry does,
        // but with a new rationale.
        let row = sqlx::query(
            "SELECT created_at, actor_did, action, subject_did, subject_uri, subject_cid, \
                    rationale, snapshot_id, event_id, previous_hash, cascade_subjects \
             FROM audit_chain_entry WHERE sequence = 2",
        )
        .fetch_one(&db).await.unwrap();
        let new_rationale = "attacker-rewrite";
        let canon = serde_json::json!({
            "sequence": 2,
            "timestamp": row.try_get::<String, _>("created_at").unwrap(),
            "actor_did": row.try_get::<String, _>("actor_did").unwrap(),
            "action": row.try_get::<String, _>("action").unwrap(),
            "subject_did": row.try_get::<Option<String>, _>("subject_did").unwrap(),
            "subject_uri": row.try_get::<Option<String>, _>("subject_uri").unwrap(),
            "subject_cid": row.try_get::<Option<String>, _>("subject_cid").unwrap(),
            "rationale": new_rationale,
            "snapshot_id": row.try_get::<Option<i64>, _>("snapshot_id").unwrap(),
            "event_id": row.try_get::<Option<i64>, _>("event_id").unwrap(),
            "previous_hash": row.try_get::<Option<String>, _>("previous_hash").unwrap(),
            "cascade_subjects": row.try_get::<Option<String>, _>("cascade_subjects").unwrap(),
        });
        let canon_str = serde_json::to_string(&canon).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(canon_str.as_bytes());
        let new_current = hex::encode(hasher.finalize());
        sqlx::query(
            "UPDATE audit_chain_entry SET rationale = $1, current_hash = $2 WHERE sequence = 2",
        )
        .bind(new_rationale)
        .bind(&new_current)
        .execute(&db).await.unwrap();
        // Per-row verification of row 2 alone now passes (the attacker
        // was careful), so the previous primitive would miss this.
        // verify_chain_range catches it because row 3's previous_hash
        // still points to the OLD row-2 current_hash.
        let err = verify_chain_range(&db, 1, 3).await.expect_err(
            "linkage tamper must fail even when per-row hashes look clean",
        );
        assert_eq!(err.failing_sequence, 3);
        assert_eq!(err.kind, ChainFailureKind::LinkageMismatch);
    }

    #[tokio::test]
    async fn verify_chain_range_skips_pre_chain_sentinel_rows() {
        let db = open_test_pool().await;
        // Insert a sentinel row at sequence 1 with current_hash="pre-chain".
        // Real chain entries follow at sequence 2..4.
        sqlx::query(
            "INSERT INTO audit_chain_entry \
             (sequence, created_at, actor_did, action, rationale, current_hash, previous_hash) \
             VALUES (1, '2026-01-01T00:00:00Z', 'did:plc:legacy', 'PreChain', \
                     'pre-Phase-3.8', 'pre-chain', NULL)",
        )
        .execute(&db).await.unwrap();
        let subject = Subject::Repo { did: "did:plc:s".to_string() };
        // Two real chain entries follow. They should chain together
        // independently of the sentinel row above.
        for i in 0..2 {
            append_entry(&db, AppendEntryParams {
                actor_did: "did:plc:m1", action: "TakedownAccount",
                subject: Some(&subject), rationale: &format!("post-{}", i),
                snapshot_id: None, event_id: None, cascade_subjects: &[],
            }).await.unwrap();
        }
        // Sentinel skipped; real rows verify cleanly.
        verify_chain_range(&db, 1, 3).await.expect("sentinel + real chain verifies");
    }

    #[tokio::test]
    async fn verify_chain_range_empty_window_is_ok() {
        let db = open_test_pool().await;
        // No entries written.
        verify_chain_range(&db, 1, 5).await.expect("empty chain → empty window passes");
    }

    #[tokio::test]
    async fn verify_chain_range_inverted_window_is_ok() {
        let db = open_test_pool().await;
        verify_chain_range(&db, 5, 1).await.expect("inverted window is trivially consistent");
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
