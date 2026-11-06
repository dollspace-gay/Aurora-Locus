//! `migrate storage` against `migrate blobs`, and the sequences it seeds.

use super::*;
use crate::migrate_blobs::MIGRATE_LEASE;

/// The SQLite store and the empty Postgres one, or None without Postgres.
fn pair(all: &[Backend]) -> Option<(&Store, &Store)> {
    let pg = all.iter().find(|b| b.name == "postgres")?;
    let src = &all.iter().find(|b| b.name == "sqlite")?.store;
    Some((src, &pg.store))
}

async fn sqlite_value(src: &Store, q: &str) -> i64 {
    let Store::Sqlite(p) = src else {
        unreachable!()
    };
    sqlx::query_scalar(q).fetch_one(p).await.unwrap()
}

#[tokio::test]
async fn migrate_storage_waits_for_blob_migrations_and_seeds_above_the_counters() {
    let all = backends().await;
    let Some((src, dst)) = pair(&all) else {
        eprintln!("no Postgres to copy into; skipping");
        return;
    };
    // Rows for two accounts; the second's draw the highest key_seq values.
    let mut tx = src.begin().await.unwrap();
    for (did, n) in [("did:plc:low", 3), ("did:plc:top", 4)] {
        for i in 0..n {
            let (cid, key) = (format!("bafy{did}{i}"), format!("tmp/{did}{i}"));
            tx.insert_temp_blob(did, &cid, "image/png", 1, &key)
                .await
                .unwrap();
        }
    }
    for k in ["tmp/q1", "tmp/q2"] {
        tx.queue_blob_delete(k, 0).await.unwrap();
    }
    tx.commit().await.unwrap();

    // A migration paused mid-copy: refused, naming it, nothing copied.
    src.start_blob_migration("mig-1", "s3", None, "op")
        .await
        .unwrap();
    let lease = src
        .try_acquire_lease(MIGRATE_LEASE, "n1", Duration::from_secs(60))
        .await
        .unwrap()
        .unwrap();
    assert!(src.pause_for_cutover(&lease, "mig-1").await.unwrap());
    let err = crate::copy::sqlite_to_postgres(src, dst)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("mig-1"), "names the job: {err}");
    assert!(dst.key_store_empty().await.unwrap(), "nothing copied");
    assert!(src.cancel_blob_migration().await.unwrap().is_some());

    // GC takes the rows that drew the highest values, and the delete
    // queue drains to empty: both counters are above every row.
    let mut tx = src.begin().await.unwrap();
    assert_eq!(tx.delete_account_blobs("did:plc:top", 100, 0).await.unwrap(), 4);
    tx.commit().await.unwrap();
    for (id, _, _) in src.due_blob_deletes(-1_000_000, 100, None).await.unwrap() {
        let mut tx = src.begin().await.unwrap();
        tx.remove_blob_delete(id).await.unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(sqlite_value(src, "SELECT COUNT(*) FROM blob_delete").await, 0);
    let counter = sqlite_value(src, "SELECT value FROM key_seq_counter WHERE id = 1").await;
    let row_max = sqlite_value(src, "SELECT MAX(key_seq) FROM blob").await;
    let delete_counter =
        sqlite_value(src, "SELECT seq FROM sqlite_sequence WHERE name = 'blob_delete'").await;
    assert!(counter > row_max, "{counter} vs {row_max}");
    assert_eq!(delete_counter, 6);

    let report = crate::copy::sqlite_to_postgres(src, dst).await.unwrap();
    assert_eq!(report.key_seq, counter);
    let mut tx = dst.begin().await.unwrap();
    let next = tx.next_key_seq().await.unwrap();
    tx.queue_blob_delete("tmp/after", 0).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(next, counter + 1, "above the SQLite counter, not the rows");
    let after = dst
        .due_blob_deletes(-1_000_000, 10, None)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.1 == "tmp/after")
        .unwrap()
        .0;
    assert_eq!(
        after,
        delete_counter + 1,
        "a new delete id above the AUTOINCREMENT counter of an empty queue"
    );
}

#[tokio::test]
async fn migrate_storage_seeds_key_seq_above_recorded_marks() {
    let all = backends().await;
    let Some((src, dst)) = pair(&all) else {
        eprintln!("no Postgres to copy into; skipping");
        return;
    };
    let mut tx = src.begin().await.unwrap();
    tx.insert_temp_blob("did:plc:a", "bafya", "image/png", 1, "tmp/a")
        .await
        .unwrap();
    tx.commit().await.unwrap();
    // A finished job's mark above the counter (the counter is what a
    // mark is read from, so only a lost counter row gets here): the
    // sequence still starts above it.
    src.start_blob_migration("mig-2", "s3", None, "op")
        .await
        .unwrap();
    let lease = src
        .try_acquire_lease(MIGRATE_LEASE, "n1", Duration::from_secs(60))
        .await
        .unwrap()
        .unwrap();
    src.set_migration_marks(&lease, "mig-2", 40, 30, None)
        .await
        .unwrap();
    assert!(src.cancel_blob_migration().await.unwrap().is_some());
    let report = crate::copy::sqlite_to_postgres(src, dst).await.unwrap();
    assert_eq!(report.key_seq, 40);
    let mut tx = dst.begin().await.unwrap();
    assert_eq!(tx.next_key_seq().await.unwrap(), 41);
    tx.commit().await.unwrap();
}
