//! Side-script verification of the canonical hash-input shape
//! documented in `docs/operator/audit-chain-verification.md` Section
//! C (Arc 3 Step 2). For each Subject variant + cascade shape, this
//! test:
//!
//! 1. Appends a fresh chain entry via the production
//!    `insert_chain_entry_pool` API — Aurora-Locus's writer computes
//!    `current_hash` and stores it.
//! 2. Reads the row back via direct SQL.
//! 3. Reconstructs the canonical hash input from the row's columns
//!    using the rules Section C documents (see `canonical_form`
//!    below).
//! 4. SHA-256-hashes the reconstructed canonical input.
//! 5. Asserts equality with the row's stored `current_hash`.
//!
//! If any test fails, Section C's transformation rules disagree
//! with production behavior — the doc must be fixed before commit.
//! The side-script is the executable form of Section C; doc and
//! script must agree.
//!
//! Worked-example hashes pasted into Section D of the operator
//! doc come from this file's `cargo test ... -- --nocapture`
//! output. To capture: temporarily un-comment the `eprintln!`
//! lines below, run, copy the hashes verbatim, re-comment.

use aurora_locus::admin::audit_chain::{insert_chain_entry_pool, AppendEntryParams};
use aurora_locus::admin::defs::Subject;
use sha2::{Digest, Sha256};
use sqlx::any::AnyPoolOptions;
use sqlx::AnyPool;
use std::sync::Once;

/// Reconstruct the canonical hash input from a row's columns and
/// compute SHA-256. Mirrors `audit_chain.rs:381-447` (the production
/// canonical-form construction). Field order in the JSON object
/// literal here is alphabetical to match `serde_json::Map`'s
/// `BTreeMap` backing — `serde_json` is built without
/// `preserve_order`, so serialized keys come out alphabetically
/// regardless of source-order in the `json!` macro.
fn compute_canonical_hash(row: &Row) -> String {
    let canon = serde_json::json!({
        "action": row.action,
        "actor_did": row.actor_did,
        "cascade_snapshot_ids": row.cascade_snapshot_ids_json,
        "cascade_subjects": row.cascade_subjects_json,
        "event_id": row.event_id,
        "payload": row.payload,
        "previous_hash": row.previous_hash,
        "rationale": row.rationale,
        "sequence": row.sequence,
        "snapshot_id": row.snapshot_id,
        "source": row.source,
        "subject_cid": row.subject_cid,
        "subject_did": row.subject_did,
        "subject_uri": row.subject_uri,
        "timestamp": row.timestamp,
    });
    let canon_str = serde_json::to_string(&canon).expect("canonicalizable");
    let mut hasher = Sha256::new();
    hasher.update(canon_str.as_bytes());
    hex::encode(hasher.finalize())
}

/// Convenience holder mirroring the `audit_chain_entry` row columns
/// the canonical form needs. Names are exactly the SQL column names
/// to keep Section C's documentation faithful.
struct Row {
    sequence: i64,
    timestamp: String,
    actor_did: String,
    action: String,
    subject_did: Option<String>,
    subject_uri: Option<String>,
    subject_cid: Option<String>,
    rationale: String,
    snapshot_id: Option<i64>,
    event_id: Option<i64>,
    previous_hash: Option<String>,
    cascade_subjects_json: Option<String>,
    cascade_snapshot_ids_json: Option<String>,
    // v0.9 format bump (#345): source discriminator (NOT NULL) +
    // action-scalar payload (nullable JSON string), both in the canon.
    source: String,
    payload: Option<String>,
    current_hash: String,
}

async fn fetch_row(db: &AnyPool, sequence: i64) -> Row {
    use sqlx::Row as _;
    let r = sqlx::query(
        "SELECT sequence, created_at, actor_did, action, subject_did, subject_uri, \
                subject_cid, rationale, snapshot_id, event_id, previous_hash, \
                cascade_subjects, cascade_snapshot_ids, source, payload, current_hash \
         FROM audit_chain_entry WHERE sequence = $1",
    )
    .bind(sequence)
    .fetch_one(db)
    .await
    .unwrap();
    Row {
        sequence: r.try_get("sequence").unwrap(),
        timestamp: r.try_get("created_at").unwrap(),
        actor_did: r.try_get("actor_did").unwrap(),
        action: r.try_get("action").unwrap(),
        subject_did: r.try_get::<Option<String>, _>("subject_did").unwrap_or(None),
        subject_uri: r.try_get::<Option<String>, _>("subject_uri").unwrap_or(None),
        subject_cid: r.try_get::<Option<String>, _>("subject_cid").unwrap_or(None),
        rationale: r.try_get("rationale").unwrap(),
        snapshot_id: r.try_get::<Option<i64>, _>("snapshot_id").unwrap_or(None),
        event_id: r.try_get::<Option<i64>, _>("event_id").unwrap_or(None),
        previous_hash: r.try_get::<Option<String>, _>("previous_hash").unwrap_or(None),
        cascade_subjects_json: r.try_get::<Option<String>, _>("cascade_subjects").unwrap_or(None),
        cascade_snapshot_ids_json: r
            .try_get::<Option<String>, _>("cascade_snapshot_ids")
            .unwrap_or(None),
        source: r.try_get("source").unwrap(),
        payload: r.try_get::<Option<String>, _>("payload").unwrap_or(None),
        current_hash: r.try_get("current_hash").unwrap(),
    }
}

/// Mirror of `audit_chain.rs::tests::open_test_pool` — in-memory
/// SQLite with the schema the chain code expects.
async fn open_pool() -> AnyPool {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(sqlx::any::install_default_drivers);
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
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
            source TEXT NOT NULL DEFAULT 'manual',
            payload TEXT,
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
    pool
}

async fn assert_canonical_hash_matches(db: &AnyPool, sequence: i64, label: &str) {
    let row = fetch_row(db, sequence).await;
    let computed = compute_canonical_hash(&row);
    // Uncomment to capture worked-example values for Section D:
    // eprintln!(
    //     "[{}] sequence={} previous_hash={:?} current_hash={}",
    //     label, row.sequence, row.previous_hash, row.current_hash,
    // );
    let _ = label;
    assert_eq!(
        computed, row.current_hash,
        "[{}] reconstructed canonical hash diverged from stored \
         current_hash. Section C of audit-chain-verification.md is \
         wrong (or the side-script's reconstruction is). Stored: {}, \
         Computed: {}",
        label, row.current_hash, computed,
    );
}

// ====================================================================
// Per-variant tests. Each appends one or more entries via the
// production writer, then verifies the side-script's canonical-form
// reconstruction reproduces the stored current_hash.
// ====================================================================

#[tokio::test]
async fn canonical_form_matches_for_repo_ref_subject() {
    let db = open_pool().await;
    let subject = Subject::Repo {
        did: "did:plc:test1234567890abcdef".to_string(),
    };
    insert_chain_entry_pool(
        &db,
        aurora_locus::config::DatabaseBackend::Sqlite,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: "did:plc:moderator",
            action: "TakedownAccount",
            subject: Some(&subject),
            rationale: "test rationale for repoRef",
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .unwrap();
    assert_canonical_hash_matches(&db, 1, "repo_ref").await;
}

#[tokio::test]
async fn canonical_form_matches_for_strong_ref_subject() {
    let db = open_pool().await;
    let subject = Subject::Record {
        uri: "at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc".to_string(),
        cid: "bafyreidemorecord".to_string(),
    };
    insert_chain_entry_pool(
        &db,
        aurora_locus::config::DatabaseBackend::Sqlite,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: "did:plc:moderator",
            action: "TakedownRecord",
            subject: Some(&subject),
            rationale: "test rationale for strongRef",
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .unwrap();
    assert_canonical_hash_matches(&db, 1, "strong_ref").await;
}

#[tokio::test]
async fn canonical_form_matches_for_repo_blob_ref_subject_with_record_uri() {
    let db = open_pool().await;
    let subject = Subject::Blob {
        did: "did:plc:test1234567890abcdef".to_string(),
        cid: "bafyreidemoblob".to_string(),
        record_uri: Some("at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc".to_string()),
    };
    insert_chain_entry_pool(
        &db,
        aurora_locus::config::DatabaseBackend::Sqlite,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: "did:plc:moderator",
            action: "TakedownBlob",
            subject: Some(&subject),
            rationale: "test rationale for repoBlobRef with record_uri",
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .unwrap();
    assert_canonical_hash_matches(&db, 1, "blob_with_record_uri").await;
}

#[tokio::test]
async fn canonical_form_matches_for_repo_blob_ref_subject_without_record_uri() {
    let db = open_pool().await;
    let subject = Subject::Blob {
        did: "did:plc:test1234567890abcdef".to_string(),
        cid: "bafyreidemoblob".to_string(),
        record_uri: None,
    };
    insert_chain_entry_pool(
        &db,
        aurora_locus::config::DatabaseBackend::Sqlite,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: "did:plc:moderator",
            action: "TakedownBlob",
            subject: Some(&subject),
            rationale: "test rationale for repoBlobRef without record_uri",
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .unwrap();
    assert_canonical_hash_matches(&db, 1, "blob_without_record_uri").await;
}

#[tokio::test]
async fn canonical_form_matches_for_batch_with_cascades() {
    let db = open_pool().await;
    let cascade_subjects = vec![
        Subject::Repo {
            did: "did:plc:victim1".to_string(),
        },
        Subject::Repo {
            did: "did:plc:victim2".to_string(),
        },
        Subject::Repo {
            did: "did:plc:victim3".to_string(),
        },
    ];
    let cascade_snapshot_ids = vec![Some(7_i64), None, Some(12_i64)];
    insert_chain_entry_pool(
        &db,
        aurora_locus::config::DatabaseBackend::Sqlite,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: "did:plc:moderator",
            action: "BatchTakedownAccounts",
            subject: None,
            rationale: "test rationale for batch with cascades",
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &cascade_subjects,
            cascade_snapshot_ids: &cascade_snapshot_ids,
        },
    )
    .await
    .unwrap();
    assert_canonical_hash_matches(&db, 1, "batch_with_cascades").await;
}

#[tokio::test]
async fn canonical_form_matches_for_genesis_entry() {
    // Genesis entry: previous_hash is NULL because it's the first row
    // in a fresh chain. Every other test above also exercises the
    // genesis case (each open_pool starts with sequence=1) but this
    // test makes the genesis-specific assertion explicit.
    let db = open_pool().await;
    let subject = Subject::Repo {
        did: "did:plc:genesis".to_string(),
    };
    insert_chain_entry_pool(
        &db,
        aurora_locus::config::DatabaseBackend::Sqlite,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: "did:plc:bootstrap",
            action: "BootstrapGrant",
            subject: Some(&subject),
            rationale: "first chain entry on a fresh deployment",
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &[],
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .unwrap();
    let row = fetch_row(&db, 1).await;
    // Genesis-specific assertion: previous_hash is NULL.
    assert!(
        row.previous_hash.is_none(),
        "genesis row's previous_hash must be NULL; got {:?}",
        row.previous_hash,
    );
    let computed = compute_canonical_hash(&row);
    assert_eq!(
        computed, row.current_hash,
        "genesis row's reconstructed hash must match stored hash"
    );
}

// ====================================================================
// Bonus: chain continuity. Pin that the second entry's hash
// reconstruction succeeds when previous_hash is non-null. Catches
// any off-by-one in how previous_hash is included in the canonical
// form.
// ====================================================================

// ====================================================================
// Section D worked examples — fixed-input deterministic hashes.
//
// The tests above verify Section C's transformation against the
// production writer. The tests below produce the specific hashes
// pasted into Section D so external consumers can verify their
// implementation against known-good values.
//
// Fixed inputs (no Utc::now()), no production writer involvement —
// just the canonical-form construction + SHA-256. If a consumer
// reads Section D, builds the canonical form per Section B/C, and
// hashes it, they should get the same value asserted here.
// ====================================================================

#[allow(clippy::too_many_arguments)]
fn fixed_row(
    sequence: i64,
    actor_did: &str,
    action: &str,
    subject_did: Option<&str>,
    subject_uri: Option<&str>,
    subject_cid: Option<&str>,
    rationale: &str,
    snapshot_id: Option<i64>,
    event_id: Option<i64>,
    previous_hash: Option<&str>,
    cascade_subjects_json: Option<&str>,
    cascade_snapshot_ids_json: Option<&str>,
    source: &str,
    payload: Option<&str>,
) -> Row {
    Row {
        sequence,
        timestamp: "2026-05-09T00:00:00Z".to_string(),
        actor_did: actor_did.to_string(),
        action: action.to_string(),
        subject_did: subject_did.map(String::from),
        subject_uri: subject_uri.map(String::from),
        subject_cid: subject_cid.map(String::from),
        rationale: rationale.to_string(),
        snapshot_id,
        event_id,
        previous_hash: previous_hash.map(String::from),
        cascade_subjects_json: cascade_subjects_json.map(String::from),
        cascade_snapshot_ids_json: cascade_snapshot_ids_json.map(String::from),
        source: source.to_string(),
        payload: payload.map(String::from),
        current_hash: String::new(), // computed, not asserted against
    }
}

#[test]
fn worked_example_1_repo_ref_genesis() {
    // Section D Example 1: Repo Subject, genesis row.
    // Per Arc 4 §8.3.3, single-subject events populate BOTH the flat
    // subject_* columns AND `cascade_subjects: [s]` — `cascade_subjects_json`
    // here is the production writer's `serde_json::to_string` output for the
    // single-element slice, with $type emitted first by serde's internal-tag
    // implementation followed by the struct fields in source-declared order.
    let row = fixed_row(
        1,
        "did:plc:moderator",
        "TakedownAccount",
        Some("did:plc:test1234567890abcdef"),
        None,
        None,
        "spam",
        None,
        None,
        None,
        Some(
            r#"[{"$type":"com.atproto.admin.defs#repoRef","did":"did:plc:test1234567890abcdef"}]"#,
        ),
        None,
        "manual",
        None,
    );
    let hash = compute_canonical_hash(&row);
    assert_eq!(
        hash,
        "f51dd8d375762a1e22954eec59af4972efeea5847ff427eaeaee1aaee5ce24ca",
        "Section D Example 1 hash mismatch — update the doc OR fix the side-script"
    );
}

#[test]
fn worked_example_2_strong_ref() {
    // Section D Example 2: Record Subject (strongRef).
    // Single-subject event: BOTH flat columns AND cascade_subjects: [s] populated.
    let row = fixed_row(
        1,
        "did:plc:moderator",
        "TakedownRecord",
        None,
        Some("at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc"),
        Some("bafyreidemorecord"),
        "off-topic",
        None,
        None,
        None,
        Some(
            r#"[{"$type":"com.atproto.repo.strongRef","uri":"at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc","cid":"bafyreidemorecord"}]"#,
        ),
        None,
        "manual",
        None,
    );
    let hash = compute_canonical_hash(&row);
    assert_eq!(
        hash,
        "16555784f242d5951a46de0ab23d47f0cf061c8651b221900b8995f039e2f9ba",
        "Section D Example 2 hash mismatch"
    );
}

#[test]
fn worked_example_3_repo_blob_ref_with_record_uri() {
    // Section D Example 3: Blob Subject with record_uri populated.
    // All three subject_* columns are populated; cascade_subjects: [s] mirrors.
    let row = fixed_row(
        1,
        "did:plc:moderator",
        "TakedownBlob",
        Some("did:plc:test1234567890abcdef"),
        Some("at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc"),
        Some("bafyreidemoblob"),
        "csam",
        None,
        None,
        None,
        Some(
            r#"[{"$type":"com.atproto.admin.defs#repoBlobRef","did":"did:plc:test1234567890abcdef","cid":"bafyreidemoblob","record_uri":"at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc"}]"#,
        ),
        None,
        "manual",
        None,
    );
    let hash = compute_canonical_hash(&row);
    assert_eq!(
        hash,
        "95a66f39bec9238cca0dd3554de615e222ec6470cf7753cd5068cc5c0591c54e",
        "Section D Example 3 hash mismatch"
    );
}

#[test]
fn worked_example_4_repo_blob_ref_without_record_uri() {
    // Section D Example 4: Blob Subject WITHOUT record_uri.
    // subject_uri is null; subject_did + subject_cid populated. cascade_subjects: [s]
    // omits `record_uri` per skip_serializing_if = "Option::is_none".
    let row = fixed_row(
        1,
        "did:plc:moderator",
        "TakedownBlob",
        Some("did:plc:test1234567890abcdef"),
        None,
        Some("bafyreidemoblob"),
        "csam-orphan-blob",
        None,
        None,
        None,
        Some(
            r#"[{"$type":"com.atproto.admin.defs#repoBlobRef","did":"did:plc:test1234567890abcdef","cid":"bafyreidemoblob"}]"#,
        ),
        None,
        "manual",
        None,
    );
    let hash = compute_canonical_hash(&row);
    assert_eq!(
        hash,
        "3b9f4b0f5b0c93ba166217f19bfb46ddd1354cf4a74e85bd0810d6e88c39159a",
        "Section D Example 4 hash mismatch"
    );
}

#[test]
fn worked_example_5_batch_with_cascades() {
    // Section D Example 5: batch event with 3 cascade subjects, one
    // of which had no snapshot at decision time (the null in the
    // middle position).
    let row = fixed_row(
        1,
        "did:plc:moderator",
        "BatchTakedownAccounts",
        None,
        None,
        None,
        "coordinated spam network",
        None,
        None,
        None,
        Some(
            r#"[{"$type":"com.atproto.admin.defs#repoRef","did":"did:plc:victim1"},{"$type":"com.atproto.admin.defs#repoRef","did":"did:plc:victim2"},{"$type":"com.atproto.admin.defs#repoRef","did":"did:plc:victim3"}]"#,
        ),
        Some("[7,null,12]"),
        "manual",
        None,
    );
    let hash = compute_canonical_hash(&row);
    assert_eq!(
        hash,
        "2f8145772ef1a1972482d1416634921edd358bb1580ca400e7da08c6ea539a3c",
        "Section D Example 5 hash mismatch"
    );
}

#[test]
fn worked_example_6_second_entry_with_previous_hash() {
    // Section D Example 6: chain continuity — second entry, with
    // previous_hash referencing the genesis row's current_hash.
    // Uses Example 1's NEW hash (post-Arc-4 §8.3.3 cascade-populated
    // shape) as the previous_hash to give consumers an end-to-end
    // chain-continuity example.
    let row = fixed_row(
        2,
        "did:plc:moderator",
        "RestoreAccount",
        Some("did:plc:test1234567890abcdef"),
        None,
        None,
        "appeal granted",
        None,
        None,
        Some("f51dd8d375762a1e22954eec59af4972efeea5847ff427eaeaee1aaee5ce24ca"),
        Some(
            r#"[{"$type":"com.atproto.admin.defs#repoRef","did":"did:plc:test1234567890abcdef"}]"#,
        ),
        None,
        "manual",
        None,
    );
    let hash = compute_canonical_hash(&row);
    assert_eq!(
        hash,
        "95d85237bd7c8e5469d648fa854628bf3ef414c2cd651e614972332754c6b1b3",
        "Section D Example 6 hash mismatch"
    );
}

#[test]
fn worked_example_7_substrate_source_with_payload() {
    // Section D Example 7 (v0.9 format bump, #345): a substrate-emitted
    // entry — `source = "auto_label_rule"` (not the operator default
    // "manual") carrying an action-scalar `payload` of {"applied":true}.
    // The other six examples all use the "manual" source and a null
    // payload (the pre-v0.9 shape, now with the two new canonical keys
    // taking their default values). This example is the only one that
    // varies both new fields, so external verifiers can confirm they
    // fold `source`/`payload` into the canon at the right positions
    // (between snapshot_id/subject_cid and event_id/previous_hash
    // respectively). The payload string is hashed verbatim — keys must
    // already be in their stored serialized form.
    let row = fixed_row(
        1,
        "did:system",
        "moderation_auto_label_applied",
        Some("did:plc:test1234567890abcdef"),
        None,
        None,
        "auto-label rule matched report category",
        None,
        None,
        None,
        Some(r#"[{"$type":"com.atproto.admin.defs#repoRef","did":"did:plc:test1234567890abcdef"}]"#),
        None,
        "auto_label_rule",
        Some(r#"{"applied":true}"#),
    );
    let hash = compute_canonical_hash(&row);
    assert_eq!(
        hash,
        "168054b81407fe774f080bdc2dfece49183d249f90c20c237c12006e47fb6d6b",
        "Section D Example 7 hash mismatch"
    );
}

/// Arc 4 §8.3.3: single-subject events populate BOTH the flat
/// subject_* columns AND `cascade_subjects: [s]`. This roundtrip
/// test exercises every Subject variant in cascade form to pin
/// the production writer's `serde_json::to_string` output: $type
/// emitted first by the internal-tag, then struct fields in
/// source-declared order. Section D's worked-example
/// `cascade_subjects_json` strings depend on this exact ordering.
/// If a future serde or struct-field reordering shifts the
/// emitted JSON, this test fails before Section D's hashes
/// diverge from production behavior.
#[tokio::test]
async fn canonical_form_matches_for_single_subject_with_cascade_per_arc4() {
    let db = open_pool().await;
    let cascade = vec![Subject::Record {
        uri: "at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc".to_string(),
        cid: "bafyreidemorecord".to_string(),
    }];
    let subject = cascade[0].clone();
    insert_chain_entry_pool(
        &db,
        aurora_locus::config::DatabaseBackend::Sqlite,
        AppendEntryParams {
            source: "manual",
            payload: None,
            actor_did: "did:plc:moderator",
            action: "TakedownRecord",
            subject: Some(&subject),
            rationale: "Arc 4 single-subject + cascade roundtrip",
            snapshot_id: None,
            event_id: None,
            cascade_subjects: &cascade,
            cascade_snapshot_ids: &[],
        },
    )
    .await
    .unwrap();
    let row = fetch_row(&db, 1).await;
    // Pin the JSON shape Section D's worked examples assume — $type
    // first, then struct fields in source order.
    assert_eq!(
        row.cascade_subjects_json.as_deref(),
        Some(
            r#"[{"$type":"com.atproto.repo.strongRef","uri":"at://did:plc:test1234567890abcdef/app.bsky.feed.post/1abc","cid":"bafyreidemorecord"}]"#
        ),
        "production cascade_subjects shape changed; Section D worked examples + the worked_example_*_test fixtures need to be updated together"
    );
    assert_canonical_hash_matches(&db, 1, "single_subject_with_cascade").await;
}

#[tokio::test]
async fn canonical_form_matches_for_second_entry_with_previous_hash() {
    let db = open_pool().await;
    let subject = Subject::Repo {
        did: "did:plc:victim".to_string(),
    };
    for i in 0..2 {
        insert_chain_entry_pool(
            &db,
            aurora_locus::config::DatabaseBackend::Sqlite,
            AppendEntryParams {
                source: "manual",
                payload: None,
                actor_did: "did:plc:moderator",
                action: "TakedownAccount",
                subject: Some(&subject),
                rationale: &format!("entry-{}", i),
                snapshot_id: None,
                event_id: None,
                cascade_subjects: &[],
                cascade_snapshot_ids: &[],
            },
        )
        .await
        .unwrap();
    }
    // Verify both: genesis (no prev) and second (with prev).
    assert_canonical_hash_matches(&db, 1, "two_entry_chain_first").await;
    assert_canonical_hash_matches(&db, 2, "two_entry_chain_second").await;
    // Cross-check: second entry's previous_hash == first entry's current_hash.
    let r1 = fetch_row(&db, 1).await;
    let r2 = fetch_row(&db, 2).await;
    assert_eq!(
        r2.previous_hash.as_deref(),
        Some(r1.current_hash.as_str()),
        "second entry's previous_hash must equal first entry's current_hash"
    );
}
