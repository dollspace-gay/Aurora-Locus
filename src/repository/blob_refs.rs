//! Walker for blob references embedded in record bodies + reader for
//! existing refs attached to a record in the `record_blob` table.
//!
//! Established by Arc 16e §9.5.4 Step 1 (#105); upgraded to typed
//! `Cid` values + DASL validation in Step 2 (#105 + #107).
//!
//! - `extract_blob_cids` is the validate-phase walker: it parses every
//!   `{ "$type": "blob", "ref": { "$link": "..." } }` shape in the
//!   record body, requires each link to be DASL-compliant per
//!   `Cid::is_dasl_compliant`, and surfaces any malformed input as
//!   `PdsError::InvalidCid` (HTTP 400 per V05_DESIGN.md §9.5.3.5).
//!   Invoked before Phase A so client errors produce client-error
//!   responses with no state mutation (V05_DESIGN.md §9.5.3.2.0).
//! - `read_existing_refs` is the Phase B reader: it pulls the current
//!   `record_blob` rows for a `record_uri` so the per-record loop can
//!   compute added/dropped CID sets via `BTreeSet` differences. The
//!   `is_dasl_compliant` gate runs here too as a belt-and-suspenders
//!   defense against pre-Arc-16b stale rows (V05_DESIGN.md §9.5.3.2.3).

use crate::error::{PdsError, PdsResult};
use proto_blue::lex_data::Cid;
use sqlx::{Any, Row, Transaction};
use std::collections::BTreeSet;

/// Extract DASL-compliant blob CIDs from a record body.
///
/// Recursively scans a JSON value for blob references in ATProto
/// format (`{ "$type": "blob", "ref": { "$link": "CID" } }`),
/// rejecting unparseable or non-DASL CIDs as `PdsError::InvalidCid`.
///
/// Skips well-formed objects with no `ref.$link` (these are
/// silently dropped — they encode pre-payload blob descriptors
/// without a resolved CID). A blob shape whose `$link` is a JSON
/// non-string is treated as malformed input (returns `InvalidCid`),
/// matching the spec's wire-error contract for client-side body
/// errors.
pub fn extract_blob_cids(value: &serde_json::Value) -> PdsResult<Vec<Cid>> {
    let mut cids = Vec::new();
    extract_blob_cids_recursive(value, &mut cids)?;
    Ok(cids)
}

fn extract_blob_cids_recursive(
    value: &serde_json::Value,
    cids: &mut Vec<Cid>,
) -> PdsResult<()> {
    match value {
        serde_json::Value::Object(obj) => {
            // Check if this is a blob reference shape.
            let is_blob = obj
                .get("$type")
                .and_then(|t| t.as_str())
                .map(|s| s == "blob")
                .unwrap_or(false);
            if is_blob {
                if let Some(ref_obj) = obj.get("ref") {
                    if let Some(link) = ref_obj.get("$link") {
                        let cid_str = link.as_str().ok_or_else(|| {
                            PdsError::InvalidCid(
                                "blob ref.$link is not a string".to_string(),
                            )
                        })?;
                        let cid = Cid::from_str_multibase(cid_str).map_err(|e| {
                            PdsError::InvalidCid(format!("{}: {}", cid_str, e))
                        })?;
                        if !cid.is_dasl_compliant() {
                            return Err(PdsError::InvalidCid(cid_str.to_string()));
                        }
                        cids.push(cid);
                    }
                    // Blob shape with `ref` object but no `$link`: skip.
                }
                // Blob shape with no `ref` field: skip.
            }
            // Recurse into all object values regardless of whether this
            // node was a blob (nested blobs in non-blob structures are
            // standard, e.g. embed.images[].image).
            for (_, v) in obj {
                extract_blob_cids_recursive(v, cids)?;
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                extract_blob_cids_recursive(v, cids)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Read the set of DASL-compliant blob CIDs currently attached to
/// `record_uri` in `record_blob`.
///
/// Reads inside the caller's transaction so the result reflects the
/// same snapshot Phase B operates under (V05_DESIGN.md §9.5.3.2.2 —
/// Phase B shared-DB transaction). `BTreeSet` ordering yields the
/// deterministic iteration Phase B's STRICT-before-unref planning
/// depends on.
///
/// The DASL gate (`Cid::is_dasl_compliant`) is belt-and-suspenders:
/// post-Arc-16b every stored row already satisfies it (Arc 16b
/// helpers only accept compliant CIDs), so under normal operation
/// no row triggers `InvalidCid`. The check defends against pre-Arc-16b
/// stale rows, FK-disabled replicas, or direct-SQL operator
/// intervention (V05_DESIGN.md §9.5.3.2.3 R0d.C trigger-condition
/// note).
pub async fn read_existing_refs<'tx>(
    tx: &mut Transaction<'tx, Any>,
    record_uri: &str,
) -> PdsResult<BTreeSet<Cid>> {
    let rows = sqlx::query("SELECT blob_cid FROM record_blob WHERE record_uri = $1")
        .bind(record_uri)
        .fetch_all(&mut **tx)
        .await?;
    let mut cids = BTreeSet::new();
    for row in rows {
        let cid_str: String = row.try_get("blob_cid")?;
        let cid = Cid::from_str_multibase(&cid_str)
            .map_err(|e| PdsError::InvalidCid(format!("{}: {}", cid_str, e)))?;
        if !cid.is_dasl_compliant() {
            return Err(PdsError::InvalidCid(cid_str));
        }
        cids.insert(cid);
    }
    Ok(cids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto_blue::lex_data::Cid as PbCid;
    use serde_json::json;
    use std::sync::Once;

    // Cached DASL-compliant test CIDs.
    fn cid_a() -> PbCid {
        PbCid::for_raw(b"aurora-test-blob-a")
    }
    fn cid_b() -> PbCid {
        PbCid::for_raw(b"aurora-test-blob-b")
    }
    fn cid_c() -> PbCid {
        PbCid::for_raw(b"aurora-test-blob-c")
    }

    // ---- extract_blob_cids ----

    #[test]
    fn extract_returns_empty_for_non_blob_record() {
        let v = json!({
            "$type": "app.bsky.feed.post",
            "text": "hello",
            "createdAt": "2026-05-20T22:00:00Z",
        });
        assert!(extract_blob_cids(&v).unwrap().is_empty());
    }

    #[test]
    fn extract_returns_single_blob_at_top_level() {
        let v = json!({
            "$type": "app.bsky.actor.profile",
            "avatar": {
                "$type": "blob",
                "ref": {"$link": cid_a().to_string_base32()},
                "mimeType": "image/png",
                "size": 1024,
            }
        });
        let cids = extract_blob_cids(&v).unwrap();
        assert_eq!(cids, vec![cid_a()]);
    }

    #[test]
    fn extract_returns_multiple_blobs_in_array() {
        let v = json!({
            "$type": "app.bsky.feed.post",
            "embed": {
                "$type": "app.bsky.embed.images",
                "images": [
                    {
                        "alt": "first",
                        "image": {
                            "$type": "blob",
                            "ref": {"$link": cid_a().to_string_base32()},
                            "mimeType": "image/jpeg",
                            "size": 5000,
                        }
                    },
                    {
                        "alt": "second",
                        "image": {
                            "$type": "blob",
                            "ref": {"$link": cid_b().to_string_base32()},
                            "mimeType": "image/jpeg",
                            "size": 6000,
                        }
                    }
                ]
            }
        });
        let cids = extract_blob_cids(&v).unwrap();
        assert_eq!(cids.len(), 2);
        assert!(cids.contains(&cid_a()));
        assert!(cids.contains(&cid_b()));
    }

    #[test]
    fn extract_ignores_non_blob_typed_objects_with_ref_link() {
        // The walker only fires on $type == "blob"; arbitrary
        // {ref: {$link}} shapes (record refs in repost subjects)
        // must not be treated as blob refs.
        let v = json!({
            "$type": "app.bsky.feed.repost",
            "subject": {
                "uri": "at://did:plc:other/app.bsky.feed.post/abc",
                "cid": "bafyrei-not-a-blob",
            },
            "wrapper": {
                "ref": {"$link": "bafyrei-also-not-a-blob"},
            }
        });
        assert!(extract_blob_cids(&v).unwrap().is_empty());
    }

    #[test]
    fn extract_skips_blob_without_ref_link() {
        // A malformed blob shape with missing $link is silently skipped:
        // there's nothing to link to, so no CID to surface. (Step 1's
        // pre-validation walker also silently skipped this case.)
        let v = json!({
            "$type": "app.bsky.actor.profile",
            "avatar": {
                "$type": "blob",
                "mimeType": "image/png",
            }
        });
        assert!(extract_blob_cids(&v).unwrap().is_empty());
    }

    #[test]
    fn extract_rejects_malformed_cid_string_as_invalid_cid() {
        // V05_DESIGN.md §9.5.3.2.0 + #107: client-malformed CID in a
        // blob ref produces PdsError::InvalidCid (→ HTTP 400, no
        // state mutation).
        let v = json!({
            "$type": "app.bsky.actor.profile",
            "avatar": {
                "$type": "blob",
                "ref": {"$link": "not-a-cid-at-all"},
                "mimeType": "image/png",
            }
        });
        let err = extract_blob_cids(&v).unwrap_err();
        assert!(
            matches!(err, PdsError::InvalidCid(_)),
            "expected InvalidCid, got {:?}",
            err
        );
    }

    #[test]
    fn extract_rejects_non_dasl_cid_as_invalid_cid() {
        // V05_DESIGN.md §9.5.3.2.3 R0d.C: non-DASL CIDs (CIDv0,
        // non-raw/non-CBOR codec, non-SHA256 hash) reject as
        // InvalidCid even when structurally parseable. We approximate
        // by constructing a CID with an unsupported codec and
        // verifying is_dasl_compliant returns false.
        let non_dasl_cid_str = {
            // Build a CID with codec 0x70 (dag-pb — not DASL-allowed),
            // SHA-256 digest of arbitrary bytes. Use proto-blue's
            // constructors so the bytes encode a valid varint stream.
            let bad = PbCid::new(0x70, 0x12, [0u8; 32]);
            bad.to_string_base32()
        };
        let v = json!({
            "$type": "app.bsky.actor.profile",
            "avatar": {
                "$type": "blob",
                "ref": {"$link": non_dasl_cid_str},
                "mimeType": "image/png",
            }
        });
        let err = extract_blob_cids(&v).unwrap_err();
        assert!(
            matches!(err, PdsError::InvalidCid(_)),
            "expected InvalidCid, got {:?}",
            err
        );
    }

    #[test]
    fn extract_rejects_non_string_link_as_invalid_cid() {
        let v = json!({
            "$type": "app.bsky.actor.profile",
            "avatar": {
                "$type": "blob",
                "ref": {"$link": 42},
                "mimeType": "image/png",
            }
        });
        let err = extract_blob_cids(&v).unwrap_err();
        assert!(matches!(err, PdsError::InvalidCid(_)));
    }

    // ---- read_existing_refs ----

    static INSTALL: Once = Once::new();

    async fn setup_pool() -> sqlx::AnyPool {
        INSTALL.call_once(sqlx::any::install_default_drivers);
        let pool = sqlx::any::AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            r#"CREATE TABLE record_blob (
                blob_cid TEXT NOT NULL,
                record_uri TEXT NOT NULL,
                indexed_at TEXT NOT NULL,
                PRIMARY KEY (blob_cid, record_uri)
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn insert_ref(pool: &sqlx::AnyPool, blob_cid: &str, record_uri: &str) {
        sqlx::query(
            "INSERT INTO record_blob (blob_cid, record_uri, indexed_at) VALUES ($1, $2, $3)",
        )
        .bind(blob_cid)
        .bind(record_uri)
        .bind("2026-05-20T22:00:00Z")
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn read_existing_refs_returns_empty_for_unknown_record() {
        let pool = setup_pool().await;
        let mut tx = pool.begin().await.unwrap();
        let refs = read_existing_refs(&mut tx, "at://did:plc:nobody/app.bsky.feed.post/xyz")
            .await
            .unwrap();
        assert!(refs.is_empty());
    }

    #[tokio::test]
    async fn read_existing_refs_returns_blobs_attached_to_record_only() {
        let pool = setup_pool().await;
        let record = "at://did:plc:alice/app.bsky.feed.post/abc";
        insert_ref(&pool, &cid_a().to_string_base32(), record).await;
        insert_ref(&pool, &cid_b().to_string_base32(), record).await;
        insert_ref(
            &pool,
            &cid_c().to_string_base32(),
            "at://did:plc:alice/app.bsky.feed.post/different",
        )
        .await;
        let mut tx = pool.begin().await.unwrap();
        let refs = read_existing_refs(&mut tx, record).await.unwrap();
        assert_eq!(refs.len(), 2);
        assert!(refs.contains(&cid_a()));
        assert!(refs.contains(&cid_b()));
        assert!(!refs.contains(&cid_c()));
    }

    #[tokio::test]
    async fn read_existing_refs_iterates_sorted() {
        // Phase B planning relies on deterministic iteration (sorted-
        // CID STRICT-before-unref ordering per V05_DESIGN.md
        // §9.5.3.2.2). BTreeSet<Cid> guarantees byte-lex order of the
        // CID's binary form per proto-blue's Ord derivation.
        let pool = setup_pool().await;
        let record = "at://did:plc:alice/app.bsky.feed.post/abc";
        insert_ref(&pool, &cid_c().to_string_base32(), record).await;
        insert_ref(&pool, &cid_a().to_string_base32(), record).await;
        insert_ref(&pool, &cid_b().to_string_base32(), record).await;
        let mut tx = pool.begin().await.unwrap();
        let refs = read_existing_refs(&mut tx, record).await.unwrap();
        let mut expected = BTreeSet::new();
        expected.insert(cid_a());
        expected.insert(cid_b());
        expected.insert(cid_c());
        let actual: Vec<_> = refs.iter().cloned().collect();
        let exp: Vec<_> = expected.iter().cloned().collect();
        assert_eq!(actual, exp);
    }

    #[tokio::test]
    async fn read_existing_refs_rejects_non_dasl_row_as_invalid_cid() {
        // Belt-and-suspenders defense per V05_DESIGN.md §9.5.3.2.3:
        // direct-SQL INSERT of a non-DASL CID surfaces as InvalidCid
        // during Phase B read. Under normal operation post-Arc-16b
        // this row can't be created via helpers; the test bypasses
        // them deliberately to exercise the defense.
        let pool = setup_pool().await;
        let record = "at://did:plc:alice/app.bsky.feed.post/abc";
        insert_ref(&pool, "not-a-cid-at-all", record).await;
        let mut tx = pool.begin().await.unwrap();
        let err = read_existing_refs(&mut tx, record).await.unwrap_err();
        assert!(
            matches!(err, PdsError::InvalidCid(_)),
            "expected InvalidCid, got {:?}",
            err
        );
    }
}
