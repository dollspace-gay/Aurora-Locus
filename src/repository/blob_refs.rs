//! Walker for blob references embedded in record bodies + reader for
//! existing refs attached to a record in the `record_blob` table.
//!
//! Relocated from `src/api/repo.rs` per Arc 16e §9.5.4 Step 1 (#105).
//! `extract_blob_cids` is byte-for-byte unchanged from the prior
//! location; the Step 1.1 signature change to
//! `Result<Vec<Cid>, PdsError::InvalidCid>` per #107 wire-vocabulary
//! is deferred to Step 2 along with the validate-phase walker wiring
//! per V05_DESIGN.md §9.5.3.2.0.

use crate::error::PdsResult;
use sqlx::{Any, Row, Transaction};
use std::collections::BTreeSet;

/// Extract blob CIDs from a record value.
///
/// Recursively scans a JSON value for blob references in ATProto format.
/// Blobs are represented as `{ "$type": "blob", "ref": { "$link": "CID" } }`.
pub fn extract_blob_cids(value: &serde_json::Value) -> Vec<String> {
    let mut cids = Vec::new();
    extract_blob_cids_recursive(value, &mut cids);
    cids
}

fn extract_blob_cids_recursive(value: &serde_json::Value, cids: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(obj) => {
            // Check if this is a blob reference
            if let Some(type_val) = obj.get("$type") {
                if type_val.as_str() == Some("blob") {
                    // Extract the CID from ref.$link
                    if let Some(ref_obj) = obj.get("ref") {
                        if let Some(link) = ref_obj.get("$link") {
                            if let Some(cid) = link.as_str() {
                                cids.push(cid.to_string());
                            }
                        }
                    }
                }
            }
            // Recurse into object values
            for (_, v) in obj {
                extract_blob_cids_recursive(v, cids);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                extract_blob_cids_recursive(v, cids);
            }
        }
        _ => {}
    }
}

/// Read the set of blob CIDs currently attached to `record_uri` in
/// `record_blob`.
///
/// Reads inside the caller's transaction so the result reflects the
/// same snapshot Phase B operates under (per V05_DESIGN.md §9.5.3.2.2 —
/// Phase B shared-DB transaction). `BTreeSet` ordering yields the
/// deterministic iteration Phase B's STRICT-before-unref planning
/// depends on.
///
/// The `String` element type is a Step 1 interim; Step 2 adopts
/// `BTreeSet<Cid>` (proto-blue `lex_data::Cid`) alongside the
/// validate-phase walker upgrade per V05_DESIGN.md §1.1/§1.2.
pub async fn read_existing_refs<'tx>(
    tx: &mut Transaction<'tx, Any>,
    record_uri: &str,
) -> PdsResult<BTreeSet<String>> {
    let rows = sqlx::query("SELECT blob_cid FROM record_blob WHERE record_uri = $1")
        .bind(record_uri)
        .fetch_all(&mut **tx)
        .await?;
    let mut cids = BTreeSet::new();
    for row in rows {
        let cid: String = row.try_get("blob_cid")?;
        cids.insert(cid);
    }
    Ok(cids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Once;

    // ---- extract_blob_cids ----

    #[test]
    fn extract_returns_empty_for_non_blob_record() {
        let v = json!({
            "$type": "app.bsky.feed.post",
            "text": "hello",
            "createdAt": "2026-05-20T22:00:00Z",
        });
        assert!(extract_blob_cids(&v).is_empty());
    }

    #[test]
    fn extract_returns_single_blob_at_top_level() {
        let v = json!({
            "$type": "app.bsky.actor.profile",
            "avatar": {
                "$type": "blob",
                "ref": {"$link": "bafyrei-avatar"},
                "mimeType": "image/png",
                "size": 1024,
            }
        });
        let cids = extract_blob_cids(&v);
        assert_eq!(cids, vec!["bafyrei-avatar".to_string()]);
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
                            "ref": {"$link": "bafyrei-img1"},
                            "mimeType": "image/jpeg",
                            "size": 5000,
                        }
                    },
                    {
                        "alt": "second",
                        "image": {
                            "$type": "blob",
                            "ref": {"$link": "bafyrei-img2"},
                            "mimeType": "image/jpeg",
                            "size": 6000,
                        }
                    }
                ]
            }
        });
        let cids = extract_blob_cids(&v);
        assert_eq!(cids.len(), 2);
        assert!(cids.contains(&"bafyrei-img1".to_string()));
        assert!(cids.contains(&"bafyrei-img2".to_string()));
    }

    #[test]
    fn extract_ignores_non_blob_typed_objects_with_ref_link() {
        // The walker only fires on $type == "blob"; arbitrary
        // {ref: {$link}} shapes (e.g. record refs in repost subjects)
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
        assert!(extract_blob_cids(&v).is_empty());
    }

    #[test]
    fn extract_skips_blob_without_ref_link() {
        // Defensive: a malformed blob with missing $link is silently
        // skipped. Step 2's validate-phase walker upgrades this to a
        // typed `PdsError::InvalidCid` rejection per §9.5.3.2.0 + #107.
        let v = json!({
            "$type": "app.bsky.actor.profile",
            "avatar": {
                "$type": "blob",
                "mimeType": "image/png",
            }
        });
        assert!(extract_blob_cids(&v).is_empty());
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
        insert_ref(&pool, "bafyrei-x", record).await;
        insert_ref(&pool, "bafyrei-y", record).await;
        insert_ref(
            &pool,
            "bafyrei-z",
            "at://did:plc:alice/app.bsky.feed.post/different",
        )
        .await;
        let mut tx = pool.begin().await.unwrap();
        let refs = read_existing_refs(&mut tx, record).await.unwrap();
        assert_eq!(refs.len(), 2);
        assert!(refs.contains("bafyrei-x"));
        assert!(refs.contains("bafyrei-y"));
        assert!(!refs.contains("bafyrei-z"));
    }

    #[tokio::test]
    async fn read_existing_refs_iterates_sorted() {
        // Phase B planning relies on deterministic iteration for the
        // sorted-CID STRICT-before-unref ordering (V05_DESIGN.md
        // §9.5.3.2.2). BTreeSet guarantees lexicographic order.
        let pool = setup_pool().await;
        let record = "at://did:plc:alice/app.bsky.feed.post/abc";
        insert_ref(&pool, "bafyrei-c", record).await;
        insert_ref(&pool, "bafyrei-a", record).await;
        insert_ref(&pool, "bafyrei-b", record).await;
        let mut tx = pool.begin().await.unwrap();
        let refs = read_existing_refs(&mut tx, record).await.unwrap();
        let collected: Vec<_> = refs.iter().cloned().collect();
        assert_eq!(
            collected,
            vec![
                "bafyrei-a".to_string(),
                "bafyrei-b".to_string(),
                "bafyrei-c".to_string(),
            ]
        );
    }
}
