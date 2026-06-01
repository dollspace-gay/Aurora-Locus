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
//!
//! # Canonical commitment: action-ID surfacing
//!
//! This module is the canonical commitment location for the
//! action-ID contract for Aurora-namespace handlers. Per
//! `docs/V03_DESIGN.md` §6.3.4: every `tools.aurora.*` admin
//! handler that writes an `audit_chain_entry` row MUST surface
//! `auditEntryId` in its response, on a typed `*Output` struct
//! whose Rust-side field is named `audit_entry_id` and whose wire
//! form is camelCase via `#[serde(rename_all = "camelCase")]`.
//! Handlers that also write `moderation_event` rows additionally
//! surface `eventId` (typed `event_id` Rust-side).
//!
//! Upstream-lexicon handlers (`com.atproto.*`) are carved out —
//! their wire shapes preserve lexicon conformance; the contract
//! applies only to the Aurora namespace.
//!
//! Drift is caught by `tests/admin_handler_contract.rs`, a
//! structural lint that scans Aurora-namespace handler files for
//! `pub async fn` declarations whose body invokes
//! `insert_chain_entry` and asserts each one returns a typed
//! `*Output` struct with the required `audit_entry_id` field. A
//! short allowlist carves out handlers that surface the ID outside
//! the typed-JSON convention (currently only `export_account_forensic`,
//! which returns a binary tar response and surfaces the ID via the
//! `X-Aurora-Audit-Entry-Id` HTTP header).
//!
//! # External verification
//!
//! Consumers verifying chain integrity independently (e.g., a
//! third-party tool that reads `tools.aurora.admin.getAuditTrail`
//! responses and recomputes SHA-256 hashes to confirm the chain
//! hasn't been tampered with) should consult
//! `docs/operator/audit-chain-verification.md` for the
//! wire-to-canonical bridge specification, including per-variant
//! Subject decomposition rules, the canonical hash-input shape
//! (alphabetical key order, JSON-encoded cascade fields, numeric
//! i64 ids in canonical form vs stringified ids on the wire), and
//! six worked examples with byte-equal canonical forms and SHA-256
//! hashes. The side-script at
//! `tests/audit_chain_canonical_verification.rs` is the executable
//! form of that document — both must agree, and the doc's worked
//! examples are sourced from the side-script's deterministic
//! hash captures.
//!
//! # Centralized chain-write commitment (v0.7 arc 1)
//!
//! This module is the ONLY path that issues
//! `INSERT INTO audit_chain_entry`. The canonical helpers are
//! [`insert_chain_entry`] (caller-managed transaction with explicit
//! backend) and its pool-API sibling [`insert_chain_entry_pool`].
//! A build-script grep linter at the repo root (`build.rs`) fails
//! the build if any source file under `src/` other than this module
//! contains the literal `INSERT INTO audit_chain_entry`. New
//! audit-inserting code MUST go through these helpers; raw SQL
//! bypasses are a build failure with an actionable diagnostic.
//!
//! The structural enforcement is the grep linter + this
//! module-level commitment + the in-process [`AppendChainGuard`]
//! that callers acquire ahead of the transaction. On Postgres,
//! [`insert_chain_entry`] additionally acquires
//! `pg_advisory_xact_lock(AUDIT_CHAIN_LOCK_KEY)` for cross-process
//! serialization in multi-instance deployments.
//!
//! ## BEGIN IMMEDIATE on SQLite — known limitation
//!
//! v07_audit_coherence.md §6.1 specifies `BEGIN IMMEDIATE` for
//! SQLite chain-writing transactions. sqlx 0.8's Any-backed
//! `Pool::begin()` does not expose transaction-mode options for
//! SQLite, so `BEGIN IMMEDIATE` cannot be set without abandoning
//! the typed `Transaction` wrapper. AppendChainGuard (in-process
//! mutex held across the caller's `tx.commit()`) plus SQLite's
//! DB-level write lock provides equivalent single-writer-at-a-time
//! serialization for single-instance SQLite deployments — the
//! v0.6 deployment posture. Multi-instance SQLite is not a
//! supported posture; multi-instance deployments use Postgres,
//! where the advisory lock provides true cross-process
//! serialization. A future sqlx upgrade exposing per-transaction
//! BEGIN modes (or a refactor to backend-specific drivers for
//! chain writes) would let this defense-in-depth land cleanly.
//!

use crate::{admin::defs::Subject, error::PdsError};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{AnyPool, Row};
use std::sync::OnceLock;
use tokio::sync::Mutex as AsyncMutex;

/// Process-local serialization guard for chain appends. Combined
/// with the per-call transaction and (on Postgres) the advisory
/// lock inside that transaction, this gives correct behavior under
/// concurrent multi-writer load on both backends:
///
/// - SQLite: the engine serializes WRITES at the database level,
///   but a `BEGIN DEFERRED` transaction running SELECT-head + INSERT
///   can't atomically upgrade SHARED → RESERVED if another writer
///   already holds RESERVED, so it returns SQLITE_BUSY. Per-process
///   mutex ahead of the transaction makes the SELECT-head + INSERT
///   sequence single-flighted within the process; SQLite's own write
///   lock takes care of cross-process serialization (in v0.2 SQLite
///   deployments are single-instance anyway).
///
/// - Postgres: this mutex is mostly cheap-no-op overhead because
///   pg_advisory_xact_lock already serializes appenders inside the
///   transaction. It serves as a small in-process queue ahead of
///   the network round-trip — useful for keeping appenders fair
///   under bursty load — but the cross-process correctness comes
///   from the advisory lock, not this mutex.
fn append_serialize_guard() -> &'static AsyncMutex<()> {
    static GUARD: OnceLock<AsyncMutex<()>> = OnceLock::new();
    GUARD.get_or_init(|| AsyncMutex::new(()))
}

/// Stable advisory-lock key for the audit_chain_entry table. Held
/// inside a Postgres transaction during `append_entry` to serialize
/// the read-head + insert-next-row sequence; without this, two
/// concurrent appends both observe head = (N, hash X) and both
/// compute next = (N+1, prev_hash=X), producing a UNIQUE-constraint
/// race that loses one entry per collision.
///
/// Derivation matches `SEQUENCER_LEADER_LOCK_KEY` (same const-time
/// SHA-256 → first 8 bytes BE → i64 pattern) but over a different
/// input string so the keyspaces don't collide.
///
/// SQLite gets serialization for free via its database-level write
/// lock; the advisory-lock query is skipped on SQLite. The
/// surrounding transaction provides equivalent ordering on both
/// backends.
pub const AUDIT_CHAIN_LOCK_KEY: i64 = audit_chain_lock_key();

const fn audit_chain_lock_key() -> i64 {
    // SHA-256("aurora.audit_chain") first 8 bytes (big-endian) as i64.
    // Pre-computed for const-time evaluation. Verified by
    // `audit_chain_lock_key_matches_runtime_hash` against a fresh
    // runtime computation, AND by `audit_chain_lock_key_distinct_from_leader`
    // against the sequencer leader lock so the two keyspaces don't
    // collide.
    i64::from_be_bytes([0x2a, 0x28, 0x50, 0x8f, 0x76, 0xb6, 0x8d, 0x08])
}

/// Sentinel advisory-lock key used by `insert_chain_entry`'s debug-mode
/// runtime assertion (v0.7 arc 1 step 2e). The probe attempts
/// `pg_try_advisory_xact_lock(AUDIT_CHAIN_LOCK_SENTINEL_KEY)` to confirm
/// the advisory-lock primitive is reachable on the active transaction;
/// it intentionally uses a distinct key from `AUDIT_CHAIN_LOCK_KEY` so
/// the probe never collides with the actual chain-serialization lock.
/// Postgres-only; not relevant on SQLite.
pub const AUDIT_CHAIN_LOCK_SENTINEL_KEY: i64 = AUDIT_CHAIN_LOCK_KEY.wrapping_add(1);

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

    /// Snapshot IDs from the cascade subjects, paired by index with
    /// `cascade_subjects`. Populated when this entry was produced by a
    /// batch event; empty Vec for non-cascade entries. Each element
    /// is `None` when the subject at that index wasn't snapshottable
    /// at decision time, otherwise `Some(<id>)`.
    ///
    /// Wire form: stringified i64 values for JS-precision parity with
    /// `snapshot_id` and `event_id`. Consumers decoding in JavaScript
    /// should use the string values directly or convert via
    /// `BigInt(value)` rather than `Number(value)` to avoid precision
    /// loss above `Number.MAX_SAFE_INTEGER` (2^53 - 1).
    ///
    /// On-disk shape (in the `audit_chain_entry.cascade_snapshot_ids`
    /// TEXT column) is a JSON array of i64 numbers, e.g.
    /// `[7, null, 12]`, with SQL NULL when empty. The row decoder
    /// converts to stringified form for the wire. The canonical hash
    /// form (used for chain verification) sees the on-disk
    /// JSON-encoded string, NOT the wire form — this asymmetry is
    /// documented in `docs/operator/audit-chain-verification.md`
    /// (Arc 3 Step 2).
    pub cascade_snapshot_ids: Vec<Option<String>>,
}

/// Build an [`AuditEntry`] from a fetched `audit_chain_entry` row.
///
/// Centralises the column-extraction + cascade-parsing +
/// `verify_entry` + `Subject::from_columns` wiring used to surface
/// the canonical `AuditEntry` wire shape. `exportAccountForensic`
/// calls this helper to keep its `audit-entries.json` payload
/// lock-step with what `getAuditTrail` emits per item (Arc 9 Step 4
/// / chainlink #55 Item 2).
///
/// The row MUST include the columns the production SELECT statements
/// fetch: `id, sequence, created_at, actor_did, action, subject_did,
/// subject_uri, subject_cid, rationale, snapshot_id, event_id,
/// current_hash, previous_hash, cascade_subjects, cascade_snapshot_ids`.
/// Missing columns surface as `PdsError::Internal`.
///
/// Tolerates malformed `cascade_subjects` / `cascade_snapshot_ids`
/// JSON by falling back to empty vectors, mirroring `getAuditTrail`'s
/// behaviour. SQL column errors and `created_at` parse failures
/// propagate as `PdsError::Internal`.
///
/// Consumed by both `exportAccountForensic`'s audit-chain section
/// and `getAuditTrail` (v0.6 batch tail A.1 / G2 closure — the
/// latter's manual row-parse block was DRY'd onto this helper).
/// The unit test `forensic_audit_entries_match_get_audit_trail_shape`
/// pins the byte-identical-shape invariant between the two callers
/// — touching this helper must keep both paths' wire output stable.
pub fn audit_entry_from_row(row: &sqlx::any::AnyRow) -> Result<AuditEntry, PdsError> {
    use sqlx::Row as _;
    let to_internal = |e: sqlx::Error| PdsError::Internal(e.to_string());

    let id: i64 = row.try_get("id").map_err(to_internal)?;
    let sequence: i64 = row.try_get("sequence").map_err(to_internal)?;
    let created_at_str: String = row.try_get("created_at").map_err(to_internal)?;
    let timestamp = chrono::DateTime::parse_from_rfc3339(&created_at_str)
        .map_err(|e| PdsError::Internal(e.to_string()))?
        .with_timezone(&chrono::Utc);
    let actor_did: String = row.try_get("actor_did").map_err(to_internal)?;
    let action: String = row.try_get("action").map_err(to_internal)?;
    let subject_did: Option<String> = row.try_get("subject_did").ok().flatten();
    let subject_uri: Option<String> = row.try_get("subject_uri").ok().flatten();
    let subject_cid: Option<String> = row.try_get("subject_cid").ok().flatten();
    let rationale: String = row.try_get("rationale").map_err(to_internal)?;
    let snapshot_id: Option<i64> = row.try_get("snapshot_id").ok().flatten();
    let event_id: Option<i64> = row.try_get("event_id").ok().flatten();
    let current_hash: String = row.try_get("current_hash").map_err(to_internal)?;
    let previous_hash: Option<String> = row.try_get("previous_hash").ok().flatten();

    let cascade_str: Option<String> = row.try_get("cascade_subjects").ok().flatten();
    let cascade_subjects: Vec<Subject> = cascade_str
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    let cascade_snapshot_ids_str: Option<String> =
        row.try_get("cascade_snapshot_ids").ok().flatten();
    // Parse the on-disk numeric JSON for the wire field. The
    // verify_entry call below still receives the raw string form
    // because the canonical hash sees the JSON-encoded string
    // nested inside the canonical object (Arc 3 Step 2 documents
    // the wire-vs-canonical asymmetry).
    let cascade_snapshot_ids_i64: Vec<Option<i64>> = cascade_snapshot_ids_str
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let cascade_snapshot_ids: Vec<Option<String>> = cascade_snapshot_ids_i64
        .iter()
        .map(|opt| opt.map(|v| v.to_string()))
        .collect();

    let verified = verify_entry(
        sequence,
        &created_at_str,
        &actor_did,
        &action,
        subject_did.as_deref(),
        subject_uri.as_deref(),
        subject_cid.as_deref(),
        &rationale,
        snapshot_id,
        event_id,
        previous_hash.as_deref(),
        cascade_str.as_deref(),
        cascade_snapshot_ids_str.as_deref(),
        &current_hash,
    );
    let subject_ref = Subject::from_columns(
        subject_did.as_deref(),
        subject_uri.as_deref(),
        subject_cid.as_deref(),
    );

    Ok(AuditEntry {
        id: id.to_string(),
        sequence,
        timestamp,
        actor_did,
        action,
        subject_ref,
        rationale,
        snapshot_id: snapshot_id.map(|i| i.to_string()),
        event_id: event_id.map(|i| i.to_string()),
        current_hash,
        previous_hash,
        verified,
        cascade_subjects,
        cascade_snapshot_ids,
    })
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
/// rationale, snapshot_id, event_id, previous_hash, cascade_subjects,
/// cascade_snapshot_ids) — re-hashing later in the same way
/// reproduces `current_hash`, which is the verification primitive.
pub struct AppendEntryParams<'a> {
    pub actor_did: &'a str,
    pub action: &'a str,
    pub subject: Option<&'a Subject>,
    pub rationale: &'a str,
    pub snapshot_id: Option<i64>,
    pub event_id: Option<i64>,
    pub cascade_subjects: &'a [Subject],
    /// Per-subject snapshot ids paired by index with `cascade_subjects`.
    /// Empty for single-subject entries (where the scalar `snapshot_id`
    /// applies). For batch entries, one element per cascade subject;
    /// element is `None` if the subject was not snapshottable. Must be
    /// either empty or the same length as `cascade_subjects`. Recorded
    /// in `cascade_snapshot_ids` and included in the canonical hash so
    /// chain verification covers the snapshot linkage.
    pub cascade_snapshot_ids: &'a [Option<i64>],
}

/// In-process serialization guard for audit-chain appends.
///
/// Per LB-1 / chainlink #122: when callers do their own
/// transaction management (so the chain entry lands atomically
/// with the underlying mutation via `insert_chain_entry`), the
/// in-process AsyncMutex that single-flights chain appends must
/// be held _across_ the caller's `tx.commit()`. Otherwise, a
/// second appender could observe the same chain head between
/// guard-release-on-helper-return and commit-on-caller, racing
/// for the same `seq` value.
///
/// Acquire before opening the caller's transaction; drop after
/// `tx.commit()` (an explicit `drop(guard)` works, but normal
/// scope-end is fine — the only thing that matters is that the
/// guard outlives the commit).
///
/// `insert_chain_entry_pool` (the pool-API wrapper that opens its
/// own tx) continues to acquire the guard internally, so existing call
/// sites are unaffected.
pub struct AppendChainGuard {
    _inner: tokio::sync::MutexGuard<'static, ()>,
}

impl AppendChainGuard {
    /// Acquire the guard. Returns once the lock is held.
    pub async fn acquire() -> Self {
        Self {
            _inner: append_serialize_guard().lock().await,
        }
    }
}

/// Append an entry to the chain inside an existing transaction (the
/// canonical v0.7 helper).
///
/// Backend-gated: on [`crate::config::DatabaseBackend::Postgres`] the
/// helper issues `pg_advisory_xact_lock(AUDIT_CHAIN_LOCK_KEY)` to
/// serialize concurrent appenders across processes. On
/// [`crate::config::DatabaseBackend::Sqlite`] the advisory-lock query
/// is skipped (the function does not exist on SQLite); the
/// caller-acquired [`AppendChainGuard`] plus SQLite's DB-level write
/// lock serialize chain writers in single-instance deployments. See the
/// module-level "BEGIN IMMEDIATE on SQLite — known limitation" note
/// for the multi-instance-SQLite caveat.
///
/// Caller contract:
/// - Hold an [`AppendChainGuard`] across the caller's `tx.commit()`.
/// - Pass the active [`crate::config::DatabaseBackend`] (typically
///   `ctx.config.database.backend`).
/// - The caller-managed transaction wraps both the underlying
///   mutation and this chain append for atomicity (LB-1 / chainlink
///   #122).
///
/// Runtime assertion: in debug builds on Postgres, a sentinel-key
/// `pg_try_advisory_xact_lock(AUDIT_CHAIN_LOCK_SENTINEL_KEY)` probe
/// confirms the advisory-lock primitive is reachable on the current
/// transaction. The probe runs after the real chain-lock acquisition
/// and is compiled out in release builds.
pub async fn insert_chain_entry(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    backend: crate::config::DatabaseBackend,
    params: AppendEntryParams<'_>,
) -> Result<i64, PdsError> {
    match backend {
        crate::config::DatabaseBackend::Postgres => {
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(AUDIT_CHAIN_LOCK_KEY)
                .execute(&mut **tx)
                .await?;
            #[cfg(debug_assertions)]
            {
                // Debug-only defense-in-depth: probe the advisory-lock
                // infrastructure via a sentinel key. The real chain lock
                // is held above; the sentinel acquisition confirms the
                // primitive is reachable on the active transaction.
                let _: Option<bool> =
                    sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
                        .bind(AUDIT_CHAIN_LOCK_SENTINEL_KEY)
                        .fetch_optional(&mut **tx)
                        .await?;
            }
        }
        crate::config::DatabaseBackend::Sqlite => {
            // No advisory-lock equivalent. Serialization via
            // AppendChainGuard (caller-acquired) + SQLite's
            // DB-write lock. See module-level note for the
            // BEGIN IMMEDIATE limitation.
        }
    }
    write_chain_entry_inner(tx, params).await
}

/// Inner chain-write body. Computes sequence, canonical hash, and
/// issues the `INSERT INTO audit_chain_entry` statement. Called by
/// [`insert_chain_entry`] after the backend-conditional chain-
/// serialization primitive has been acquired.
///
/// Pre-condition: the caller has acquired the appropriate
/// chain-serialization primitive for the active backend (Postgres
/// advisory lock or SQLite AppendChainGuard).
async fn write_chain_entry_inner(
    tx: &mut sqlx::Transaction<'_, sqlx::Any>,
    params: AppendEntryParams<'_>,
) -> Result<i64, PdsError> {
    // Determine sequence + previous hash from the chain head.
    // Inside the transaction so this read sees the same snapshot
    // the subsequent INSERT will write into.
    let head: Option<(i64, String)> = sqlx::query_as::<_, (i64, String)>(
        "SELECT sequence, current_hash FROM audit_chain_entry ORDER BY sequence DESC LIMIT 1",
    )
    .fetch_optional(&mut **tx)
    .await?;
    let (seq, prev_hash) = match head {
        Some((s, h)) => (s + 1, Some(h)),
        None => (1, None),
    };
    let now = Utc::now();
    let now_str = now.to_rfc3339();
    // Per CR-1 / chainlink #121: `record_uri` on Blob subjects must
    // round-trip through the chain row's flat columns. Pre-fix we
    // dropped it on the floor (`..` pattern), and read-back through
    // `Subject::from_columns` saw `(Some(did), None, Some(cid))` and
    // — without a Blob arm for that shape — returned `None`. Now we
    // preserve `record_uri` in `subject_uri` when present, and
    // `from_columns` has a dedicated arm for the no-record_uri case.
    let (subject_did, subject_uri, subject_cid) = match params.subject {
        Some(Subject::Repo { did }) => (Some(did.clone()), None, None),
        Some(Subject::Record { uri, cid }) => (None, Some(uri.clone()), Some(cid.clone())),
        Some(Subject::Blob { did, cid, record_uri }) => {
            (Some(did.clone()), record_uri.clone(), Some(cid.clone()))
        }
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
    // cascade_snapshot_ids must be either empty (no per-subject
    // snapshots — pre-CR-2 chain rows or single-subject entries) or
    // exactly the same length as cascade_subjects. Index `i` of one
    // pairs with index `i` of the other.
    if !params.cascade_snapshot_ids.is_empty()
        && params.cascade_snapshot_ids.len() != params.cascade_subjects.len()
    {
        return Err(PdsError::Internal(format!(
            "cascade_snapshot_ids length {} does not match cascade_subjects length {}",
            params.cascade_snapshot_ids.len(),
            params.cascade_subjects.len(),
        )));
    }
    let cascade_snapshot_ids_json = if params.cascade_snapshot_ids.is_empty() {
        None
    } else {
        Some(
            serde_json::to_string(params.cascade_snapshot_ids)
                .map_err(|e| PdsError::Internal(e.to_string()))?,
        )
    };
    // Canonical content for hashing — order matters; future schema
    // additions append new fields rather than reorder existing ones.
    // cascade_snapshot_ids is appended at the tail per the additive
    // policy. Pre-CR-2 entries hash with cascade_snapshot_ids = None,
    // which is what the JSON form serializes to for an empty input;
    // their stored hashes still verify because the legacy code-path
    // produced JSON without that field at all. We accept that:
    // re-running verification on legacy rows uses the new canonical
    // form, but legacy rows would never be in the cascade-snapshot-ids
    // population since the field defaults to empty.
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
        "cascade_snapshot_ids": cascade_snapshot_ids_json,
    });
    let canon_str = serde_json::to_string(&canon)
        .map_err(|e| PdsError::Internal(e.to_string()))?;
    let mut hasher = Sha256::new();
    hasher.update(canon_str.as_bytes());
    let current_hash = hex::encode(hasher.finalize());
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO audit_chain_entry \
         (sequence, created_at, actor_did, action, subject_did, subject_uri, subject_cid, \
          rationale, snapshot_id, event_id, current_hash, previous_hash, cascade_subjects, \
          cascade_snapshot_ids) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) RETURNING id",
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
    .bind(&cascade_snapshot_ids_json)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

/// Append an entry to the chain via a self-managed transaction (the
/// canonical pool-API helper, v0.7).
///
/// Acquires an [`AppendChainGuard`] across the open/INSERT/commit
/// sequence, opens a sqlx transaction, calls [`insert_chain_entry`]
/// with the supplied backend, and commits. For callers without an
/// enclosing mutation transaction; per LB-1 / chainlink #122,
/// callers mutating other tables should open their own transaction
/// and call [`insert_chain_entry`] directly so the chain entry lands
/// atomically with the mutation.
pub async fn insert_chain_entry_pool(
    db: &AnyPool,
    backend: crate::config::DatabaseBackend,
    params: AppendEntryParams<'_>,
) -> Result<i64, PdsError> {
    let _guard = AppendChainGuard::acquire().await;
    let mut tx = db.begin().await?;
    let id = insert_chain_entry(&mut tx, backend, params).await?;
    tx.commit().await?;
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
                cascade_subjects, cascade_snapshot_ids \
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
        let cascade_snapshot_ids_str: Option<String> =
            row.try_get("cascade_snapshot_ids").ok().flatten();

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
            cascade_snapshot_ids_str.as_deref(),
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
#[allow(clippy::too_many_arguments)]
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
    cascade_snapshot_ids: Option<&str>,
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
        "cascade_snapshot_ids": cascade_snapshot_ids,
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

/// Test-only addressing mode for [`corrupt_entry_rationale`]. Site 1 of
/// the open-coded tamper pattern (`verify_entry_fails_when_rationale_tampered`)
/// targets a row by its primary-key id (the `i64` returned from
/// `append_entry`); sites 2, 3, and 4 target by `sequence` (the chain
/// position, hardcoded to `2` because their tests append three entries
/// and tamper the middle one). Both addressing modes are kept rather
/// than rekeying everyone to one because the original choice is
/// load-bearing for test readability.
#[cfg(test)]
#[derive(Debug, Clone, Copy)]
pub(crate) enum EntryRef {
    Id(i64),
    Sequence(i64),
}

/// Test-only helper: rewrite a row's `rationale` directly via SQL,
/// bypassing the normal append-and-hash pathway. Verification primitives
/// (`verify_entry`, `verify_chain_range`) will subsequently fail on the
/// tampered row because the recomputed hash diverges from the stored
/// `current_hash`. Field-agnostic at the column level — operates against
/// raw SQL so it survives any future addition to `AuditEntry` or to the
/// canonical hash input.
///
/// Use cases (the pattern this consolidates):
/// - **Per-row tamper**: simplest case, used by 3 of the 4 open-coded
///   sites this helper replaced. The recomputed hash diverges from the
///   stored `current_hash` and `verify_entry` returns `false`.
/// - **Linkage tamper** (NOT this helper): when the test wants the
///   tampered row to PASS per-row verification but break chain linkage,
///   the test must additionally write a per-row-consistent
///   `current_hash` computed via SHA-256 over the canonical input. That
///   case is intentionally left inline at
///   `verify_chain_range_detects_consistent_rewrite_via_linkage` because
///   the SHA-256 recompute is the test's payload, not boilerplate.
#[cfg(test)]
pub(crate) async fn corrupt_entry_rationale(
    db: &AnyPool,
    target: EntryRef,
    new_rationale: &str,
) -> sqlx::Result<()> {
    let (sql, key) = match target {
        EntryRef::Id(id) => (
            "UPDATE audit_chain_entry SET rationale = $1 WHERE id = $2",
            id,
        ),
        EntryRef::Sequence(seq) => (
            "UPDATE audit_chain_entry SET rationale = $1 WHERE sequence = $2",
            seq,
        ),
    };
    sqlx::query(sql)
        .bind(new_rationale)
        .bind(key)
        .execute(db)
        .await
        .map(|_| ())
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
                cascade_snapshot_ids TEXT,
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

    // LB-1 / chainlink #122: atomicity regression. The per-tx
    // entry point `insert_chain_entry` must roll back together
    // with the caller's underlying mutation when the caller's tx
    // is rolled back. Pre-LB-1, `insert_chain_entry_pool` opened its own tx
    // and committed before returning — so a caller's later error
    // couldn't unwind the chain entry, leaving the chain row
    // out of sync with whatever the caller's mutation should
    // have done.
    #[tokio::test]
    async fn insert_chain_entry_rolls_back_with_caller_mutation() {
        let db = open_test_pool().await;
        sqlx::query(
            "INSERT INTO actor (did, handle, created_at) \
             VALUES ('did:plc:victim', 'v.test', '2026-05-04T00:00:00Z')",
        )
        .execute(&db)
        .await
        .unwrap();

        let subject = Subject::Repo {
            did: "did:plc:victim".to_string(),
        };

        // Run the underlying mutation + chain append inside one
        // tx; deliberately roll back without commit. Both writes
        // must be undone together.
        {
            let _guard = AppendChainGuard::acquire().await;
            let mut tx = db.begin().await.unwrap();
            sqlx::query(
                "UPDATE actor SET handle = 'tampered' WHERE did = 'did:plc:victim'",
            )
            .execute(&mut *tx)
            .await
            .unwrap();
            let chain_id = insert_chain_entry(
                &mut tx,
                crate::config::DatabaseBackend::Sqlite,
                AppendEntryParams {
                    actor_did: "did:plc:m1",
                    action: "TakedownAccount",
                    subject: Some(&subject),
                    rationale: "would be rolled back",
                    snapshot_id: None,
                    event_id: None,
                    cascade_subjects: &[],
                    cascade_snapshot_ids: &[],
                },
            )
            .await
            .unwrap();
            assert!(chain_id > 0, "chain insert succeeded inside tx");
            tx.rollback().await.unwrap();
        }

        let handle: String =
            sqlx::query_scalar("SELECT handle FROM actor WHERE did = 'did:plc:victim'")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(
            handle, "v.test",
            "underlying mutation must roll back when tx is rolled back"
        );
        let chain_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_chain_entry")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(
            chain_count, 0,
            "chain entry must roll back when tx is rolled back"
        );
    }

    // The flip side: when the caller's tx commits, both the
    // underlying mutation and the chain entry land. This is the
    // happy path the atomicity contract guarantees.
    #[tokio::test]
    async fn insert_chain_entry_commits_with_caller_mutation() {
        let db = open_test_pool().await;
        sqlx::query(
            "INSERT INTO actor (did, handle, created_at) \
             VALUES ('did:plc:victim', 'v.test', '2026-05-04T00:00:00Z')",
        )
        .execute(&db)
        .await
        .unwrap();

        let subject = Subject::Repo {
            did: "did:plc:victim".to_string(),
        };

        let _guard = AppendChainGuard::acquire().await;
        let mut tx = db.begin().await.unwrap();
        sqlx::query(
            "UPDATE actor SET handle = 'updated' WHERE did = 'did:plc:victim'",
        )
        .execute(&mut *tx)
        .await
        .unwrap();
        let chain_id = insert_chain_entry(
            &mut tx,
            crate::config::DatabaseBackend::Sqlite,
            AppendEntryParams {
                actor_did: "did:plc:m1",
                action: "TakedownAccount",
                subject: Some(&subject),
                rationale: "atomic commit",
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[],
                cascade_snapshot_ids: &[],
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();

        let handle: String =
            sqlx::query_scalar("SELECT handle FROM actor WHERE did = 'did:plc:victim'")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(handle, "updated", "mutation committed");
        let chain_seq: i64 =
            sqlx::query_scalar("SELECT sequence FROM audit_chain_entry WHERE id = $1")
                .bind(chain_id)
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(chain_seq, 1, "chain entry committed");
    }

    #[tokio::test]
    async fn first_entry_has_no_previous_hash() {
        let db = open_test_pool().await;
        let subject = Subject::Repo {
            did: "did:plc:s".to_string(),
        };
        let id = insert_chain_entry_pool(
            &db,
            crate::config::DatabaseBackend::Sqlite,
            AppendEntryParams {
                actor_did: "did:plc:m1",
                action: "TakedownAccount",
                subject: Some(&subject),
                rationale: "spam",
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[], cascade_snapshot_ids: &[],
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
        insert_chain_entry_pool(&db, crate::config::DatabaseBackend::Sqlite, AppendEntryParams {
            actor_did: "did:plc:m1", action: "TakedownAccount",
            subject: Some(&subject), rationale: "first",
            snapshot_id: None, event_id: None, cascade_subjects: &[], cascade_snapshot_ids: &[],
        }).await.unwrap();
        insert_chain_entry_pool(&db, crate::config::DatabaseBackend::Sqlite, AppendEntryParams {
            actor_did: "did:plc:m1", action: "RestoreAccount",
            subject: Some(&subject), rationale: "second",
            snapshot_id: None, event_id: None, cascade_subjects: &[], cascade_snapshot_ids: &[],
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

    // CR-1 / chainlink #121: Blob subjects must round-trip
    // `record_uri` through the chain row's flat columns. Pre-fix
    // the producer destructured `Subject::Blob { did, cid, .. }`
    // and dropped record_uri on the floor; reconstruction via
    // `Subject::from_columns` then saw `(Some, None, Some)` which
    // had no matching arm and fell through to `None`, losing the
    // subject identity entirely.
    #[tokio::test]
    async fn audit_chain_blob_subject_roundtrips_with_record_uri() {
        let db = open_test_pool().await;
        let subject = Subject::Blob {
            did: "did:plc:victim".to_string(),
            cid: "bafkreiabc".to_string(),
            record_uri: Some("at://did:plc:victim/app.bsky.feed.post/3kxyz".to_string()),
        };
        let id = insert_chain_entry_pool(
            &db,
            crate::config::DatabaseBackend::Sqlite,
            AppendEntryParams {
                actor_did: "did:plc:m1",
                action: "TakedownRecord",
                subject: Some(&subject),
                rationale: "blob with record_uri",
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[],
                cascade_snapshot_ids: &[],
            },
        )
        .await
        .unwrap();
        let (subject_did, subject_uri, subject_cid): (Option<String>, Option<String>, Option<String>) =
            sqlx::query_as(
                "SELECT subject_did, subject_uri, subject_cid FROM audit_chain_entry WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(subject_did.as_deref(), Some("did:plc:victim"));
        assert_eq!(
            subject_uri.as_deref(),
            Some("at://did:plc:victim/app.bsky.feed.post/3kxyz"),
            "record_uri must persist into subject_uri column"
        );
        assert_eq!(subject_cid.as_deref(), Some("bafkreiabc"));
        let reconstructed = Subject::from_columns(
            subject_did.as_deref(),
            subject_uri.as_deref(),
            subject_cid.as_deref(),
        );
        assert_eq!(reconstructed, Some(subject));
    }

    #[tokio::test]
    async fn audit_chain_blob_subject_roundtrips_without_record_uri() {
        let db = open_test_pool().await;
        let subject = Subject::Blob {
            did: "did:plc:victim".to_string(),
            cid: "bafkreidef".to_string(),
            record_uri: None,
        };
        let id = insert_chain_entry_pool(
            &db,
            crate::config::DatabaseBackend::Sqlite,
            AppendEntryParams {
                actor_did: "did:plc:m1",
                action: "TakedownRecord",
                subject: Some(&subject),
                rationale: "blob without record_uri",
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[],
                cascade_snapshot_ids: &[],
            },
        )
        .await
        .unwrap();
        let (subject_did, subject_uri, subject_cid): (Option<String>, Option<String>, Option<String>) =
            sqlx::query_as(
                "SELECT subject_did, subject_uri, subject_cid FROM audit_chain_entry WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&db)
            .await
            .unwrap();
        assert_eq!(subject_did.as_deref(), Some("did:plc:victim"));
        assert_eq!(subject_uri, None, "no record_uri → no subject_uri");
        assert_eq!(subject_cid.as_deref(), Some("bafkreidef"));
        let reconstructed = Subject::from_columns(
            subject_did.as_deref(),
            subject_uri.as_deref(),
            subject_cid.as_deref(),
        );
        assert_eq!(reconstructed, Some(subject));
    }

    // Backwards compatibility: chain rows written before CR-1 stored
    // Blob subjects as (subject_did, NULL, subject_cid). The
    // `Subject::from_columns` reconstructor must still recognize
    // that shape as a Blob (with record_uri = None) rather than
    // returning None and losing the subject identity. This test
    // simulates a legacy row by inserting directly via SQL,
    // bypassing the post-fix `insert_chain_entry_pool` path.
    #[tokio::test]
    async fn audit_chain_legacy_blob_row_reads_back_as_blob() {
        let db = open_test_pool().await;
        // Anchor the chain head with one regular entry so the
        // legacy row can chain off it. We bypass append_entry's
        // hash logic by inserting directly with previous_hash =
        // current_hash of seed entry, but for the from_columns
        // assertion we actually only care about the three subject
        // columns — read back via SELECT.
        let id = insert_chain_entry_pool(
            &db,
            crate::config::DatabaseBackend::Sqlite,
            AppendEntryParams {
                actor_did: "did:plc:m1",
                action: "TakedownAccount",
                subject: Some(&Subject::Repo {
                    did: "did:plc:seed".to_string(),
                }),
                rationale: "seed",
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[],
                cascade_snapshot_ids: &[],
            },
        )
        .await
        .unwrap();
        // Mutate the seed row to look like a legacy Blob entry.
        // (subject_did, subject_uri, subject_cid) =
        //   (Some, None, Some) is the pre-CR-1 Blob shape.
        sqlx::query(
            "UPDATE audit_chain_entry \
             SET subject_did = 'did:plc:legacy', \
                 subject_uri = NULL, \
                 subject_cid = 'bafkreilegacyblob', \
                 action = 'TakedownRecord' \
             WHERE id = $1",
        )
        .bind(id)
        .execute(&db)
        .await
        .unwrap();
        let (subject_did, subject_uri, subject_cid): (Option<String>, Option<String>, Option<String>) =
            sqlx::query_as(
                "SELECT subject_did, subject_uri, subject_cid FROM audit_chain_entry WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&db)
            .await
            .unwrap();
        let reconstructed = Subject::from_columns(
            subject_did.as_deref(),
            subject_uri.as_deref(),
            subject_cid.as_deref(),
        );
        assert_eq!(
            reconstructed,
            Some(Subject::Blob {
                did: "did:plc:legacy".to_string(),
                cid: "bafkreilegacyblob".to_string(),
                record_uri: None,
            }),
            "legacy (Some, None, Some) row must read back as Blob with record_uri=None"
        );
    }

    #[tokio::test]
    async fn verify_entry_round_trips_for_freshly_written_entry() {
        let db = open_test_pool().await;
        let subject = Subject::Repo {
            did: "did:plc:victim".to_string(),
        };
        let id = insert_chain_entry_pool(&db, crate::config::DatabaseBackend::Sqlite, AppendEntryParams {
            actor_did: "did:plc:m1", action: "SuspendAccount",
            subject: Some(&subject), rationale: "rationale text",
            snapshot_id: Some(42), event_id: Some(7), cascade_subjects: &[], cascade_snapshot_ids: &[],
        }).await.unwrap();
        let row = sqlx::query(
            "SELECT sequence, created_at, actor_did, action, subject_did, subject_uri, \
                    subject_cid, rationale, snapshot_id, event_id, current_hash, previous_hash, \
                    cascade_subjects, cascade_snapshot_ids \
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
            row.try_get::<Option<String>, _>("cascade_snapshot_ids").unwrap().as_deref(),
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
        let id = insert_chain_entry_pool(&db, crate::config::DatabaseBackend::Sqlite, AppendEntryParams {
            actor_did: "did:plc:m1", action: "TakedownAccount",
            subject: Some(&subject), rationale: "original rationale",
            snapshot_id: None, event_id: None, cascade_subjects: &[], cascade_snapshot_ids: &[],
        }).await.unwrap();
        // Simulate tamper: rewrite rationale in place after the chain
        // entry was sealed. Verification should fail because the
        // recomputed hash diverges from the stored current_hash.
        corrupt_entry_rationale(&db, EntryRef::Id(id), "tampered rationale")
            .await
            .unwrap();
        let row = sqlx::query(
            "SELECT sequence, created_at, actor_did, action, subject_did, subject_uri, \
                    subject_cid, rationale, snapshot_id, event_id, current_hash, previous_hash, \
                    cascade_subjects, cascade_snapshot_ids \
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
            row.try_get::<Option<String>, _>("cascade_snapshot_ids").unwrap().as_deref(),
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
            insert_chain_entry_pool(&db, crate::config::DatabaseBackend::Sqlite, AppendEntryParams {
                actor_did: "did:plc:m1",
                action: "TakedownAccount",
                subject: Some(&subject),
                rationale: &format!("rationale-{}", i),
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[], cascade_snapshot_ids: &[],
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
            insert_chain_entry_pool(&db, crate::config::DatabaseBackend::Sqlite, AppendEntryParams {
                actor_did: "did:plc:m1", action: "TakedownAccount",
                subject: Some(&subject), rationale: &format!("orig-{}", i),
                snapshot_id: None, event_id: None, cascade_subjects: &[], cascade_snapshot_ids: &[],
            }).await.unwrap();
        }
        // Tamper with row 2's rationale only — current_hash NOT updated,
        // so per-row recompute mismatches.
        corrupt_entry_rationale(&db, EntryRef::Sequence(2), "tampered")
            .await
            .unwrap();
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
            insert_chain_entry_pool(&db, crate::config::DatabaseBackend::Sqlite, AppendEntryParams {
                actor_did: "did:plc:m1", action: "TakedownAccount",
                subject: Some(&subject), rationale: &format!("orig-{}", i),
                snapshot_id: None, event_id: None, cascade_subjects: &[], cascade_snapshot_ids: &[],
            }).await.unwrap();
        }
        // Attacker recomputes a per-row-consistent hash for row 2 over
        // the new content. We simulate that by reading row 2's row and
        // recomputing the hash exactly the same way append_entry does,
        // but with a new rationale.
        let row = sqlx::query(
            "SELECT created_at, actor_did, action, subject_did, subject_uri, subject_cid, \
                    rationale, snapshot_id, event_id, previous_hash, cascade_subjects, \
                    cascade_snapshot_ids \
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
            "cascade_snapshot_ids": row.try_get::<Option<String>, _>("cascade_snapshot_ids").unwrap(),
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
            insert_chain_entry_pool(&db, crate::config::DatabaseBackend::Sqlite, AppendEntryParams {
                actor_did: "did:plc:m1", action: "TakedownAccount",
                subject: Some(&subject), rationale: &format!("post-{}", i),
                snapshot_id: None, event_id: None, cascade_subjects: &[], cascade_snapshot_ids: &[],
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

    // ---- Lock-key derivation + collision verification ----

    #[test]
    fn audit_chain_lock_key_matches_runtime_hash() {
        // Verify the const lock key matches the first 8 bytes of
        // SHA-256("aurora.audit_chain"). Mirrors the same pattern
        // `test_lock_key_derivation_matches_runtime_hash` uses for
        // the leader-election key. If this fires, the const bytes
        // need to be updated to match the actual hash.
        let mut h = Sha256::new();
        h.update(b"aurora.audit_chain");
        let digest = h.finalize();
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&digest[..8]);
        let expected = i64::from_be_bytes(bytes);
        assert_eq!(AUDIT_CHAIN_LOCK_KEY, expected);
    }

    #[test]
    fn audit_chain_lock_key_distinct_from_leader() {
        // Both keys live in the same Postgres advisory-lock keyspace.
        // If they collide, the leader election and the audit-chain
        // append would interfere with each other (a held leader lock
        // would block all chain appends, etc.). The two derivation
        // strings differ; this test pins the assumption.
        use crate::sequencer::leader_election::SEQUENCER_LEADER_LOCK_KEY;
        assert_ne!(
            AUDIT_CHAIN_LOCK_KEY, SEQUENCER_LEADER_LOCK_KEY,
            "audit-chain lock key must not collide with sequencer leader key"
        );
    }

    // ---- Concurrency stress test ----
    //
    // Spawns N concurrent tasks that all call `insert_chain_entry_pool` and
    // verifies each task succeeded, exactly N rows landed,
    // sequences are contiguous from 1..=N, and verify_chain_range
    // passes across the whole window.
    //
    // Without the transaction wrapping + advisory-lock contract in
    // append_entry, two tasks racing through SELECT-head would both
    // compute next-sequence = N+1, the second INSERT would fail with
    // a UNIQUE-violation error, and the test would see < N rows
    // (with the failing tasks returning Err). With the contract,
    // both backends serialize at the writer level (Postgres via
    // pg_advisory_xact_lock; SQLite via its database-level write
    // lock + the surrounding transaction).
    //
    // The pool helper here uses shared-cache in-memory SQLite with a
    // unique per-test database name so multiple connections within
    // this pool share the same database without colluding with other
    // tests' pools (those use anonymous `sqlite::memory:` and stay
    // private per connection). PRAGMA busy_timeout via after_connect
    // bounds the SQLITE_BUSY wait under contention.

    async fn open_concurrent_test_pool(max_connections: u32) -> AnyPool {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(sqlx::any::install_default_drivers);
        // Unique tempfile-backed SQLite database so two concurrent
        // tests don't share state. File-backed (rather than
        // shared-cache in-memory) because sqlx::Any only recognizes
        // `sqlite:` and `postgres:` URL schemes — the SQLite-native
        // `file:foo?mode=memory&cache=shared` URI rides through but
        // requires a different prefix style. tempfile gives us
        // per-test isolation for free.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let url = format!("sqlite://{}?mode=rwc", db_path.display());
        let pool = AnyPoolOptions::new()
            .max_connections(max_connections)
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    // 5s busy_timeout per connection. SQLite's
                    // database-level write lock is what serializes
                    // concurrent appenders on this backend; the
                    // timeout gives concurrent transactions enough
                    // headroom to queue up cleanly under load.
                    let _ = sqlx::query("PRAGMA busy_timeout = 5000")
                        .execute(conn)
                        .await;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        // Leak the tempdir so it lives as long as the pool — sqlx
        // keeps the file open via the connection, and dropping the
        // dir would yank the file out from under it.
        std::mem::forget(dir);
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
                cascade_snapshot_ids TEXT,
                UNIQUE(sequence)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn append_entry_serializes_concurrent_writers() {
        let db = open_concurrent_test_pool(8).await;
        let n: usize = 20;

        // Spawn N concurrent appenders, each with a distinct
        // rationale string so the resulting rows are easy to
        // distinguish if anything diverges.
        let mut handles = Vec::with_capacity(n);
        for i in 0..n {
            let pool = db.clone();
            let handle = tokio::spawn(async move {
                let subject = Subject::Repo {
                    did: format!("did:plc:s{}", i),
                };
                insert_chain_entry_pool(
                    &pool,
                    crate::config::DatabaseBackend::Sqlite,
                    AppendEntryParams {
                        actor_did: "did:plc:m1",
                        action: "TakedownAccount",
                        subject: Some(&subject),
                        rationale: &format!("concurrent-{}", i),
                        snapshot_id: None,
                        event_id: None,
                        cascade_subjects: &[], cascade_snapshot_ids: &[],
                    },
                )
                .await
            });
            handles.push(handle);
        }

        // All N must complete successfully — no UNIQUE-constraint
        // surprises, no silent drops.
        let mut ok_count = 0;
        for h in handles {
            match h.await.expect("task joins") {
                Ok(_) => ok_count += 1,
                Err(e) => panic!("append_entry returned Err under concurrent load: {}", e),
            }
        }
        assert_eq!(ok_count, n, "all {} concurrent appends must succeed", n);

        // Exactly N rows landed.
        let row_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_chain_entry")
                .fetch_one(&db)
                .await
                .unwrap();
        assert_eq!(row_count as usize, n);

        // Sequences are contiguous 1..=N. The interleaving order is
        // arbitrary (any of the N tasks could be first); we only
        // require that each integer in the range appears exactly once.
        let sequences: Vec<i64> = sqlx::query_scalar(
            "SELECT sequence FROM audit_chain_entry ORDER BY sequence ASC",
        )
        .fetch_all(&db)
        .await
        .unwrap();
        let expected: Vec<i64> = (1..=n as i64).collect();
        assert_eq!(sequences, expected, "sequences must be 1..=N with no gaps");

        // Linkage holds end-to-end: every row's previous_hash
        // matches the prior row's current_hash. This is the
        // chain-level invariant the lock+transaction is supposed to
        // preserve. Without the lock, a UNIQUE-violation would have
        // already torpedoed the test above; this is the further
        // assurance that nothing weird happened to the linkage.
        verify_chain_range(&db, 1, n as i64)
            .await
            .expect("clean chain after concurrent appends");
    }

    // ====================================================================
    // Arc 3 Step 0.6 (§7.4.0.6) — invariants on `corrupt_entry_rationale`.
    //
    // The helper consolidates the open-coded tamper pattern that 3 of
    // the 4 prior call sites used. These two tests pin its semantics:
    // (1) the targeted row fails verify_entry; (2) untouched rows
    // continue to verify cleanly. Future maintenance changes that
    // accidentally broaden the helper's blast radius (e.g., a typo
    // dropping the WHERE clause) get caught here.
    // ====================================================================

    /// Fixture: append `n` chain entries with deterministic rationales
    /// and return the row ids in order. Used by both invariant tests.
    async fn append_n_entries(db: &AnyPool, n: usize) -> Vec<i64> {
        let subject = Subject::Repo {
            did: "did:plc:victim".to_string(),
        };
        let mut ids = Vec::with_capacity(n);
        for i in 0..n {
            let id = insert_chain_entry_pool(
                db,
                crate::config::DatabaseBackend::Sqlite,
                AppendEntryParams {
                    actor_did: "did:plc:m1",
                    action: "TakedownAccount",
                    subject: Some(&subject),
                    rationale: &format!("orig-{}", i),
                    snapshot_id: None,
                    event_id: None,
                    cascade_subjects: &[],
                    cascade_snapshot_ids: &[],
                },
            )
            .await
            .unwrap();
            ids.push(id);
        }
        ids
    }

    /// Read a row by id and call `verify_entry` against its current
    /// stored hash. Returns the boolean verdict.
    async fn verify_row_by_id(db: &AnyPool, id: i64) -> bool {
        let row = sqlx::query(
            "SELECT sequence, created_at, actor_did, action, subject_did, subject_uri, \
                    subject_cid, rationale, snapshot_id, event_id, current_hash, previous_hash, \
                    cascade_subjects, cascade_snapshot_ids \
             FROM audit_chain_entry WHERE id = $1",
        )
        .bind(id)
        .fetch_one(db)
        .await
        .unwrap();
        verify_entry(
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
            row.try_get::<Option<String>, _>("cascade_snapshot_ids").unwrap().as_deref(),
            &row.try_get::<String, _>("current_hash").unwrap(),
        )
    }

    #[tokio::test]
    async fn corrupt_entry_rationale_breaks_verify_entry_for_target_row() {
        let db = open_test_pool().await;
        let ids = append_n_entries(&db, 3).await;
        // Pre-tamper: target row verifies cleanly.
        assert!(
            verify_row_by_id(&db, ids[1]).await,
            "row 2 must verify before tamper"
        );
        // Tamper via the helper (sequence-based, since the helper's
        // most common use is sequence-keyed).
        corrupt_entry_rationale(&db, EntryRef::Sequence(2), "tampered-by-helper")
            .await
            .unwrap();
        // Post-tamper: target row no longer verifies.
        assert!(
            !verify_row_by_id(&db, ids[1]).await,
            "row 2 must fail verification after corrupt_entry_rationale"
        );
    }

    #[tokio::test]
    async fn corrupt_entry_rationale_preserves_neighbor_verification() {
        let db = open_test_pool().await;
        let ids = append_n_entries(&db, 3).await;
        // Tamper the middle row only.
        corrupt_entry_rationale(&db, EntryRef::Sequence(2), "tampered-by-helper")
            .await
            .unwrap();
        // Neighbors (rows 1 and 3) are untouched at the row level —
        // their per-row hashes still match their stored content.
        // (Note: verify_chain_range would still flag the chain as
        // broken because row 3's previous_hash chains through row 2's
        // current_hash; that's verify_chain_range's contract, not
        // verify_entry's. This test pins per-row blast radius only.)
        assert!(
            verify_row_by_id(&db, ids[0]).await,
            "row 1 must still verify after tampering row 2"
        );
        assert!(
            verify_row_by_id(&db, ids[2]).await,
            "row 3 must still verify after tampering row 2 \
             (per-row check ignores prior-row linkage)"
        );
    }
}
