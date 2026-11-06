//! `migrate blobs` cutover safety and pause semantics, the orphan sweep
//! and branding uploads, and backups under load (disk backend).

use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use locus_blob::BlobStore;
use locus_core::CoreError;
use locus_core::lease::LeaseToken;
use locus_store::migrate_blobs::MIGRATE_LEASE;

use super::*;
use crate::blob_migration::{Step, step};

fn pool(s: &AppState) -> &sqlx::SqlitePool {
    let Store::Sqlite(p) = &s.store else {
        unreachable!("the harness is SQLite")
    };
    p
}

async fn int(h: &Harness, q: &str) -> i64 {
    sqlx::query_scalar(q).fetch_one(pool(&h.state)).await.unwrap()
}

/// The delete queue: (key, backend epoch).
async fn queued(h: &Harness) -> Vec<(String, i64)> {
    sqlx::query_as("SELECT storage_key, backend_epoch FROM blob_delete ORDER BY id")
        .fetch_all(pool(&h.state))
        .await
        .unwrap()
}

async fn job_of(h: &Harness) -> Option<String> {
    sqlx::query_scalar("SELECT migration_job FROM blob_backend WHERE id = 1")
        .fetch_one(pool(&h.state))
        .await
        .unwrap()
}

async fn put(store: &BlobStore, key: &str, bytes: &[u8]) {
    let body = futures::stream::iter([Ok::<_, std::io::Error>(
        axum::body::Bytes::copy_from_slice(bytes),
    )]);
    store.write_stream(key, body, u64::MAX).await.unwrap();
}

fn cid_of(blob: &Value) -> String {
    blob["ref"]["$link"].as_str().unwrap().to_owned()
}

fn cid_for(bytes: &[u8]) -> String {
    proto_blue_lex_data::Cid::for_raw(bytes).to_string_base32()
}

fn post_with(blob: &Value) -> Value {
    let mut p = post_record("with media");
    p["embed"] = json!({"$type": "app.bsky.embed.images", "images": [{"image": blob, "alt": ""}]});
    p
}

impl Harness {
    /// Posts `record`; its rkey.
    async fn create_post(&self, did: &str, access: &str, record: Value) -> String {
        let (st, v) = self
            .post(
                "com.atproto.repo.createRecord",
                Some(access),
                json!({"repo": did, "collection": "app.bsky.feed.post", "record": record}),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
        v["uri"].as_str().unwrap().rsplit('/').next().unwrap().to_owned()
    }

    async fn delete_post(&self, did: &str, access: &str, rkey: &str) {
        let (st, v) = self
            .post(
                "com.atproto.repo.deleteRecord",
                Some(access),
                json!({"repo": did, "collection": "app.bsky.feed.post", "rkey": rkey}),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
    }

    async fn row(&self, did: &str, cid: &str) -> locus_store::blob::BlobRow {
        self.state.store.blob(did, cid).await.unwrap().unwrap()
    }

    async fn upload_status(&self, access: &str, bytes: &[u8]) -> StatusCode {
        self.raw(
            "POST",
            "/xrpc/com.atproto.repo.uploadBlob",
            Some(access),
            bytes.to_vec(),
            &[("content-type", "image/png")],
        )
        .await
        .0
    }
}

/// A `migrate blobs` job to a disk directory, with its lease.
struct Migration {
    id: String,
    lease: LeaseToken,
    target: tempfile::TempDir,
}

impl Migration {
    fn has(&self, key: &str) -> bool {
        self.target.path().join(key).exists()
    }
}

async fn migrate(h: &Harness) -> Migration {
    let target = tempfile::tempdir().unwrap();
    let id = format!("m-{}", hex::encode(rand::random::<[u8; 4]>()));
    let root = target.path().display().to_string();
    h.state
        .store
        .start_blob_migration(&id, "disk", Some(&root), "test")
        .await
        .unwrap();
    let lease = h
        .state
        .store
        .try_acquire_lease(MIGRATE_LEASE, "test-node", Duration::from_secs(60))
        .await
        .unwrap()
        .unwrap();
    Migration { id, lease, target }
}

/// Clears a fault hook when dropped.
struct Hook(Arc<AppState>, &'static str);

impl Drop for Hook {
    fn drop(&mut self) {
        crate::faults::set(&self.0, self.1, None);
    }
}

/// A hook that runs `f` every time `point` is reached.
fn hook<F, Fut>(h: &Harness, point: &'static str, f: F) -> Hook
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = locus_core::CoreResult<()>> + Send + 'static,
{
    crate::faults::set(&h.state, point, Some(Arc::new(move || Box::pin(f()))));
    Hook(h.state.clone(), point)
}

/// A hook that runs `f` the first time `point` is reached.
fn once<F, Fut>(h: &Harness, point: &'static str, f: F) -> Hook
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = locus_core::CoreResult<()>> + Send + 'static,
{
    let cell = Arc::new(Mutex::new(Some(f)));
    hook(h, point, move || {
        let f = cell.lock().unwrap().take();
        async move {
            match f {
                Some(f) => f().await,
                None => Ok(()),
            }
        }
    })
}

/// A new temp row for bytes written to the active store, as an upload
/// makes one (the backend read first, a fresh key sequence value).
async fn insert_upload(s: &AppState, did: &str, bytes: &[u8]) -> String {
    let key = locus_store::blob::temp_key();
    put(&s.blobs, &key, bytes).await;
    let mut tx = s.store.begin().await.unwrap();
    tx.blob_backend().await.unwrap();
    let size = i64::try_from(bytes.len()).unwrap();
    tx.insert_temp_blob(did, &cid_for(bytes), "image/png", size, &key)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    key
}

/// Forgets a key's copy: no copy record, no object on the target.
async fn drop_copy(s: &AppState, m: &Migration, key: &str) {
    sqlx::query("DELETE FROM blob_copy WHERE storage_key = ?1")
        .bind(key)
        .execute(pool(s))
        .await
        .unwrap();
    std::fs::remove_file(m.target.path().join(key)).unwrap();
}

#[tokio::test]
async fn a_raw_key_change_during_the_final_pass_is_caught_and_the_cutover_retried() {
    let h = harness(false).await;
    if h.state.blobs.name() != "disk" {
        return;
    }
    let (did, access) = h.user("rae.pds.example.com").await;
    let mut cids = Vec::new();
    for n in 0..3u8 {
        let b = h
            .upload(&access, format!("raw bytes {n}").as_bytes(), "image/png")
            .await;
        cids.push(cid_of(&b));
    }
    let m = migrate(&h).await;
    let epoch = h.state.store.blob_backend().await.unwrap().epoch;
    // The fault: during the final pass a row gets a new key without the
    // pause's check (its bytes only on the old backend, a fresh key_seq).
    let raw_key = format!("blobs/{did}/{}.raw", cids[0]);
    let _hook = {
        let (s, did, cid, key) = (h.state.clone(), did.clone(), cids[0].clone(), raw_key.clone());
        once(&h, "migrate.final_pass", move || async move {
            put(&s.blobs, &key, b"raw bytes 0").await;
            let p = pool(&s);
            sqlx::query("UPDATE key_seq_counter SET value = value + 1 WHERE id = 1")
                .execute(p)
                .await
                .unwrap();
            sqlx::query(
                "UPDATE blob SET storage_key = ?1, \
                 key_seq = (SELECT value FROM key_seq_counter WHERE id = 1) \
                 WHERE did = ?2 AND cid = ?3",
            )
            .bind(&key)
            .bind(&did)
            .bind(&cid)
            .execute(p)
            .await
            .unwrap();
            Ok(())
        })
    };
    let r = step(&h.state, &m.lease).await.unwrap();
    assert!(matches!(r, Step::CutoverRetried(_)), "{r:?}");
    let b = h.state.store.blob_backend().await.unwrap();
    assert_eq!(
        (b.disk_root.as_deref(), b.epoch, b.paused),
        (None, epoch, false),
        "the pause is cleared without flipping"
    );
    assert_eq!(
        job_of(&h).await,
        Some(m.id.clone()),
        "the job keeps the migration"
    );
    assert!(!m.has(&raw_key));
    // Back to copy passes: the next one copies it, and the cutover goes.
    let r = step(&h.state, &m.lease).await.unwrap();
    assert!(matches!(r, Step::Flipped(_)), "{r:?}");
    assert_eq!(h.row(&did, &cids[0]).await.storage_key, raw_key);
    for cid in &cids {
        let key = h.row(&did, cid).await.storage_key;
        assert!(m.has(&key), "{key} is on the new backend");
    }
}

/// After a cutover that didn't flip: no pause, the same epoch, and the
/// job still owns a migration in copy passes.
async fn unpaused_and_kept(h: &Harness, m: &Migration, epoch: i64) {
    let b = h.state.store.blob_backend().await.unwrap();
    assert_eq!((b.epoch, b.paused), (epoch, false), "no flip, no pause");
    assert_eq!(job_of(h).await, Some(m.id.clone()), "the job keeps it");
    let mig = h.state.store.latest_blob_migration().await.unwrap().unwrap();
    assert_eq!(mig.phase, "copying");
}

#[tokio::test]
async fn failed_and_overrunning_final_passes_clear_the_pause_and_later_verifies_catch_gaps() {
    let h = harness(false).await;
    if h.state.blobs.name() != "disk" {
        return;
    }
    let (did, access) = h.user("fay.pds.example.com").await;
    let first = h.upload(&access, b"there from the start", "image/png").await;
    let m = migrate(&h).await;
    let epoch = h.state.store.blob_backend().await.unwrap().epoch;

    // A final pass that fails.
    let fail = once(&h, "migrate.final_pass", || async {
        Err(CoreError::Storage("injected".into()))
    });
    let r = step(&h.state, &m.lease).await.unwrap();
    assert!(matches!(r, Step::CutoverRetried(ref n) if n.contains("injected")), "{r:?}");
    unpaused_and_kept(&h, &m, epoch).await;
    drop(fail);

    // A row arrives after a pass's mark, so the final pass copies it; that
    // pass then runs past blobs.cutover_max_pause.
    h.state
        .settings
        .set("blobs.cutover_max_pause_ms", json!(5_000), "test")
        .await
        .unwrap();
    let late = Arc::new(Mutex::new(String::new()));
    let arrive = {
        let (s, did, late) = (h.state.clone(), did.clone(), Arc::clone(&late));
        once(&h, "migrate.copy_pass", move || async move {
            *late.lock().unwrap() = insert_upload(&s, &did, b"arrived after the mark").await;
            Ok(())
        })
    };
    let overrun = once(&h, "migrate.final_pass", || async {
        tokio::time::sleep(Duration::from_millis(5_500)).await;
        Ok(())
    });
    let r = step(&h.state, &m.lease).await.unwrap();
    assert!(matches!(r, Step::CutoverRetried(ref n) if n.contains("longer")), "{r:?}");
    unpaused_and_kept(&h, &m, epoch).await;
    let late = late.lock().unwrap().clone();
    assert!(m.has(&late), "the final pass copied it");
    drop((arrive, overrun));

    // A gap injected into the next copy pass (after it ran, before its
    // verify) among the rows that final pass copied: the pass's verify of
    // (verified_through, its mark] catches it, and the cutover waits.
    let gap = {
        let (s, late) = (h.state.clone(), late.clone());
        let target = m.target.path().to_path_buf();
        once(&h, "migrate.copy_pass", move || async move {
            sqlx::query("DELETE FROM blob_copy WHERE storage_key = ?1")
                .bind(&late)
                .execute(pool(&s))
                .await
                .unwrap();
            std::fs::remove_file(target.join(&late)).unwrap();
            Ok(())
        })
    };
    let r = step(&h.state, &m.lease).await.unwrap();
    assert_eq!(r, Step::Copied(1), "the verify found the gap");
    assert!(m.has(&late));
    unpaused_and_kept(&h, &m, epoch).await;
    drop(gap);
    h.state
        .settings
        .set("blobs.cutover_max_pause_ms", json!(60_000), "test")
        .await
        .unwrap();
    let r = step(&h.state, &m.lease).await.unwrap();
    assert!(matches!(r, Step::Flipped(_)), "{r:?}");
    assert!(m.has(&late));
    assert!(m.has(&h.row(&did, &cid_of(&first)).await.storage_key));
}

#[tokio::test]
async fn the_full_verify_catches_a_gap_below_copied_through() {
    let h = harness(false).await;
    if h.state.blobs.name() != "disk" {
        return;
    }
    let (did, _) = h.user("gus.pds.example.com").await;
    // More rows than a pass that tries the cutover may change.
    let mut keys = Vec::new();
    for i in 0..105 {
        keys.push(insert_upload(&h.state, &did, format!("bulk {i}").as_bytes()).await);
    }
    let m = migrate(&h).await;
    let r = step(&h.state, &m.lease).await.unwrap();
    assert_eq!(r, Step::Copied(105));
    let mig = h.state.store.latest_blob_migration().await.unwrap().unwrap();
    assert!(mig.copied_through > 0);
    assert_eq!(mig.verified_through, 0, "nothing verified yet");
    // A gap below copied_through.
    drop_copy(&h.state, &m, &keys[5]).await;
    // The next pass changes nothing, so the full verify runs: it finds the
    // gap, copies it, and the cutover waits for another pass.
    let r = step(&h.state, &m.lease).await.unwrap();
    assert_eq!(r, Step::Copied(1));
    assert!(m.has(&keys[5]));
    let mig = h.state.store.latest_blob_migration().await.unwrap().unwrap();
    assert_eq!(mig.verified_through, mig.copied_through);
    let r = step(&h.state, &m.lease).await.unwrap();
    assert!(matches!(r, Step::Flipped(_)), "{r:?}");
    for k in &keys {
        assert!(m.has(k), "{k}");
    }
}

#[tokio::test]
async fn the_pause_refuses_only_key_setting_writes_and_moves_restart_after_the_flip() {
    let h = harness(false).await;
    if h.state.blobs.name() != "disk" {
        return;
    }
    let (did, access) = h.user("pia.pds.example.com").await;
    let a = h.upload(&access, b"promoted during the pause", "image/png").await;
    let b = h.upload(&access, b"taken down during the pause", "image/png").await;
    let (cid_a, cid_b) = (cid_of(&a), cid_of(&b));
    let post_b = h.create_post(&did, &access, post_with(&b)).await;
    let m = migrate(&h).await;
    let epoch = h.state.store.blob_backend().await.unwrap().epoch;
    assert!(
        h.state
            .store
            .pause_for_cutover(&m.lease, &m.id)
            .await
            .unwrap()
    );

    // An upload is refused (retryable), writes no row, and its temp
    // object is queued for deletion.
    let before = queued(&h).await;
    assert_eq!(
        h.upload_status(&access, b"refused during the pause").await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(
        h.state
            .store
            .blob(&did, &cid_for(b"refused during the pause"))
            .await
            .unwrap()
            .is_none()
    );
    let new: Vec<_> = queued(&h)
        .await
        .into_iter()
        .filter(|q| !before.contains(q))
        .collect();
    assert!(
        new.len() == 1 && new[0].0.starts_with("tmp/") && new[0].1 == epoch,
        "{new:?}"
    );

    // A post with a temp blob commits: promoted, its move pending (the
    // move's step 3 waits).
    h.create_post(&did, &access, post_with(&a)).await;
    let ra = h.row(&did, &cid_a).await;
    assert_eq!(ra.state, "permanent");
    assert!(ra.storage_key.starts_with("tmp/"), "no key changed");
    let a_pending = ra.pending_move.clone().unwrap();
    // Deleting a post (a reference change) commits.
    h.delete_post(&did, &access, &post_b).await;
    assert!(h.row(&did, &cid_b).await.unreferenced_since.is_some());
    // A move's step 1 commits (a takedown's move to quarantine).
    assert!(
        h.state
            .store
            .set_blob_takedown(&did, &cid_b, Some("t-1"))
            .await
            .unwrap()
    );
    assert_eq!(
        locus_blob::run_placement(&h.state.store, &h.state.blobs, false, 100)
            .await
            .unwrap(),
        1
    );
    let rb = h.row(&did, &cid_b).await;
    let b_pending = rb.pending_move.clone().unwrap();
    assert!(b_pending.starts_with("quarantine/"), "{b_pending}");
    assert!(rb.storage_key.starts_with("blobs/"), "no key changed");
    // Step 3 during the pause: retried later, no key changed.
    assert_eq!(
        locus_blob::run_pending_moves(&h.state.store, &h.state.blobs, 100)
            .await
            .unwrap(),
        0
    );
    assert_eq!(h.row(&did, &cid_a).await.storage_key, ra.storage_key);
    assert_eq!(h.row(&did, &cid_b).await.storage_key, rb.storage_key);

    // The cutover, from the pause.
    let r = step(&h.state, &m.lease).await.unwrap();
    assert_eq!(r, Step::Flipped(epoch + 1));
    // The node dies between the flip and the post-flip batches; another
    // takes the job over and runs them.
    h.state.store.release_lease(&m.lease).await.unwrap();
    let lease = h
        .state
        .store
        .try_acquire_lease(MIGRATE_LEASE, "other-node", Duration::from_secs(60))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(step(&h.state, &lease).await.unwrap(), Step::Done);
    assert_eq!(int(&h, "SELECT COUNT(*) FROM blob_copy").await, 0);
    let q = queued(&h).await;
    for (cid, old) in [(&cid_a, &a_pending), (&cid_b, &b_pending)] {
        let r = h.row(&did, cid).await;
        let fresh = r.pending_move.clone().unwrap();
        assert_ne!(&fresh, old, "restarted with a fresh key");
        let move_epoch = int(
            &h,
            &format!("SELECT move_epoch FROM blob WHERE did = '{did}' AND cid = '{cid}'"),
        )
        .await;
        assert_eq!(move_epoch, epoch + 1);
        assert!(q.contains(&(old.clone(), epoch)), "{old} queued on the old backend");
    }
    // On the new backend the moves complete.
    let new_store = crate::blob_migration::active_store(&h.state).await.unwrap();
    assert_eq!(new_store.epoch(), epoch + 1);
    assert_eq!(
        locus_blob::run_pending_moves(&h.state.store, &new_store, 100)
            .await
            .unwrap(),
        2
    );
    for cid in [&cid_a, &cid_b] {
        let r = h.row(&did, cid).await;
        assert!(r.pending_move.is_none());
        assert!(m.has(&r.storage_key), "{} on the new backend", r.storage_key);
    }
    assert!(h.row(&did, &cid_b).await.storage_key.starts_with("quarantine/"));

    // This node hasn't reloaded: an upload's bytes go to the old backend,
    // so it's refused, writes no row, and its key is queued there.
    let before = queued(&h).await;
    assert_eq!(
        h.upload_status(&access, b"after the flip").await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(
        h.state
            .store
            .blob(&did, &cid_for(b"after the flip"))
            .await
            .unwrap()
            .is_none()
    );
    let new: Vec<_> = queued(&h)
        .await
        .into_iter()
        .filter(|q| !before.contains(q))
        .collect();
    assert!(
        new.len() == 1 && new[0].0.starts_with("tmp/") && new[0].1 == epoch,
        "{new:?}"
    );
    // A read missing on the old backend (a's new key is only on the new
    // one) reloads the backend and finds it.
    let (st, _, body) = h
        .raw(
            "GET",
            &format!("/xrpc/com.atproto.sync.getBlob?did={did}&cid={cid_a}"),
            None,
            Vec::new(),
            &[],
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body, b"promoted during the pause");
}

#[tokio::test]
async fn cancel_during_the_pause_clears_it_without_flipping() {
    let h = harness(false).await;
    if h.state.blobs.name() != "disk" {
        return;
    }
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let (did, access) = h.user("cal.pds.example.com").await;
    let blob = h.upload(&access, b"stays put", "image/png").await;
    let key = h.row(&did, &cid_of(&blob)).await.storage_key;
    let m = migrate(&h).await;
    let before = h.state.store.blob_backend().await.unwrap();
    h.state
        .store
        .record_copy(&m.lease, &key, "copied")
        .await
        .unwrap();
    assert!(
        h.state
            .store
            .pause_for_cutover(&m.lease, &m.id)
            .await
            .unwrap()
    );
    let admin = format!("Bearer {}", h.operator("cy", "admin").await);
    let (st, v) = h
        .admin_call(
            "POST",
            &ns("admin.migrateBlobs.cancel"),
            &admin,
            Some(json!({})),
        )
        .await;
    assert_eq!((st, v["id"].as_str()), (StatusCode::OK, Some(m.id.as_str())));
    let after = h.state.store.blob_backend().await.unwrap();
    assert_eq!(
        (after.active.as_str(), after.disk_root.as_deref(), after.epoch, after.paused),
        (before.active.as_str(), before.disk_root.as_deref(), before.epoch, false),
        "unpaused, not flipped"
    );
    assert_eq!(job_of(&h).await, None);
    let mig = h.state.store.latest_blob_migration().await.unwrap().unwrap();
    assert_eq!(mig.phase, "cancelled");
    assert_eq!(
        int(&h, "SELECT COUNT(*) FROM blob_copy").await,
        0,
        "its copy records go"
    );
    // The job's own transactions fail now, and a step finds nothing to do.
    assert!(
        h.state
            .store
            .record_copy(&m.lease, &key, "copied")
            .await
            .is_err()
    );
    assert!(
        h.state
            .store
            .pause_for_cutover(&m.lease, &m.id)
            .await
            .is_err()
    );
    assert_eq!(step(&h.state, &m.lease).await.unwrap(), Step::Idle);
    assert!(!h.state.store.blob_backend().await.unwrap().paused);
    // Writes go on, on the old backend.
    let again = h.upload(&access, b"after the cancel", "image/png").await;
    let k = h.row(&did, &cid_of(&again)).await.storage_key;
    assert!(h.state.blobs.read(&k, None).await.unwrap().is_some());
    assert!(!m.has(&k));
}

#[tokio::test]
async fn missing_sources_are_retried_and_dropped_with_their_row() {
    let h = harness(false).await;
    if h.state.blobs.name() != "disk" {
        return;
    }
    let (did, access) = h.user("moe.pds.example.com").await;
    let back = h.upload(&access, b"missing, then back", "image/png").await;
    h.create_post(&did, &access, post_with(&back)).await;
    let gone = h.upload(&access, b"missing, then collected", "image/png").await;
    let (k_back, k_gone) = (
        h.row(&did, &cid_of(&back)).await.storage_key,
        h.row(&did, &cid_of(&gone)).await.storage_key,
    );
    h.state.blobs.delete(&k_back).await.unwrap();
    h.state.blobs.delete(&k_gone).await.unwrap();
    let m = migrate(&h).await;
    let r = step(&h.state, &m.lease).await.unwrap();
    assert!(matches!(r, Step::Waiting(_)), "{r:?}");
    for k in [&k_back, &k_gone] {
        assert_eq!(
            h.state.store.copy_record(k).await.unwrap(),
            Some(("missing_source".to_owned(), false)),
            "{k}"
        );
    }
    // Its source is back: the next attempt's retry copies it.
    put(&h.state.blobs, &k_back, b"missing, then back").await;
    let r = step(&h.state, &m.lease).await.unwrap();
    assert!(matches!(r, Step::Waiting(_)), "{r:?}");
    assert_eq!(
        h.state.store.copy_record(&k_back).await.unwrap(),
        Some(("copied".to_owned(), false))
    );
    assert!(m.has(&k_back));
    // GC deletes the other's row: its entry is dropped, with nothing to
    // acknowledge, and the cutover goes ahead.
    let mut tx = h.state.store.begin().await.unwrap();
    let epoch = tx.blob_backend().await.unwrap().epoch;
    assert_eq!(
        tx.gc_blob_batch(-1, 3_600_000, 100, epoch).await.unwrap(),
        1
    );
    tx.commit().await.unwrap();
    let r = step(&h.state, &m.lease).await.unwrap();
    assert!(matches!(r, Step::Flipped(_)), "{r:?}");
    assert!(h.state.store.missing_sources().await.unwrap().is_empty());
}

#[tokio::test]
async fn branding_uploads_move_with_migrate_blobs_and_are_served_after_the_flip() {
    let h = harness(false).await;
    if h.state.blobs.name() != "disk" {
        return;
    }
    let epoch = h.state.blobs.epoch();
    put(&h.state.blobs, "branding/logo7.0001", b"logo bytes").await;
    h.state
        .store
        .put_branding_asset(&brand("logo7", "logo", "branding/logo7.0001"), epoch)
        .await
        .unwrap();
    h.state
        .store
        .save_branding_draft("{}", &["logo7".into()], "op")
        .await
        .unwrap();
    h.state.store.publish_branding_draft("op").await.unwrap();
    let m = migrate(&h).await;
    let r = step(&h.state, &m.lease).await.unwrap();
    assert!(matches!(r, Step::Flipped(_)), "{r:?}");
    assert!(m.has("branding/logo7.0001"), "copied");
    // The old backend's copy is gone (the operator cleaned up); this node,
    // not yet reloaded, serves it from the new one.
    h.state.blobs.delete("branding/logo7.0001").await.unwrap();
    let (st, _, body) = h.raw("GET", "/branding/logo7", None, Vec::new(), &[]).await;
    assert_eq!((st, body.as_slice()), (StatusCode::OK, &b"logo bytes"[..]));
}

fn brand(id: &str, kind: &str, key: &str) -> locus_store::branding::AssetRow {
    locus_store::branding::AssetRow {
        id: id.into(),
        kind: kind.into(),
        name: "x".into(),
        mime: "application/octet-stream".into(),
        size: 1,
        storage_key: key.into(),
        created_at: 0,
        unused_since: None,
    }
}

#[tokio::test]
async fn the_sweep_of_a_data_dir_root_touches_only_blobs_and_keeps_used_branding() {
    let h = harness(false).await;
    if h.state.blobs.name() != "disk" {
        return;
    }
    // A disk backend whose root is a data dir, with the files that live
    // there besides blobs.
    let data = tempfile::tempdir().unwrap();
    let root = data.path();
    let others = [
        "locus.sqlite",
        "locus.sqlite-wal",
        "locus.sqlite-shm",
        "keys/master.key",
        "keys/node.key",
        "blobs.old",
        "backups/1-ab/db.sqlite",
    ];
    for f in others {
        let p = root.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"not a blob").unwrap();
    }
    let _socket = std::os::unix::net::UnixListener::bind(root.join("locus.sock")).unwrap();
    let store = BlobStore::disk(locus_blob::DiskStore::new(root)).with_epoch(h.state.blobs.epoch());
    let epoch = store.epoch();
    put(&store, "blobs/did:plc:gone/bafyorphan.0001", b"x").await;
    put(&store, "tmp/0003", b"x").await;
    // A logo and a font a published theme uses, and an upload none ever did.
    for (id, kind) in [("logo1", "logo"), ("font1", "font"), ("unused1", "font")] {
        let key = format!("branding/{id}.0001");
        put(&store, &key, b"brand").await;
        h.state
            .store
            .put_branding_asset(&brand(id, kind, &key), epoch)
            .await
            .unwrap();
    }
    h.state
        .store
        .save_branding_draft("{}", &["logo1".into(), "font1".into()], "op")
        .await
        .unwrap();
    h.state.store.publish_branding_draft("op").await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    // Everything is older than the sweep's minimum age here.
    assert_eq!(
        locus_blob::run_orphan_sweep(&h.state.store, &store, 0)
            .await
            .unwrap(),
        2
    );
    let keys: Vec<String> = queued(&h).await.into_iter().map(|q| q.0).collect();
    assert_eq!(keys, ["blobs/did:plc:gone/bafyorphan.0001", "tmp/0003"]);
    // After the grace, the unused upload's row goes and its key is queued.
    assert_eq!(h.state.store.collect_branding_assets(-1).await.unwrap(), 1);
    assert!(
        queued(&h)
            .await
            .contains(&("branding/unused1.0001".to_owned(), epoch))
    );
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(
        locus_blob::run_deletes(&h.state.store, &store, 0, 100)
            .await
            .unwrap(),
        3
    );
    for f in others {
        assert!(root.join(f).exists(), "{f} untouched");
    }
    assert!(root.join("locus.sock").exists());
    assert!(root.join("branding/logo1.0001").exists());
    assert!(root.join("branding/font1.0001").exists());
    assert!(!root.join("branding/unused1.0001").exists());
    assert!(!root.join("tmp/0003").exists());
}

/// Waits for a backup to end; its run.
async fn backup_ended(h: &Harness, id: &str) -> locus_store::backups::BackupRun {
    for _ in 0..400 {
        let runs = h.state.store.backup_runs(10).await.unwrap();
        let run = runs.into_iter().find(|r| r.id == id).unwrap();
        if run.status != "running" {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the backup didn't end");
}

#[tokio::test]
async fn backups_under_moves_gc_and_the_delete_job_hold_every_key_the_snapshot_names() {
    let h = harness(false).await;
    if h.state.blobs.name() != "disk" {
        return;
    }
    let (did, access) = h.user("bea.pds.example.com").await;
    let (did2, access2) = h.user("bob.pds.example.com").await;
    let epoch = h.state.blobs.epoch();
    // kept: referenced; collected: unreferenced, GC takes it mid-copy;
    // moved: taken down, its move finished mid-copy; unwritten: taken
    // down, its move's bytes never written; theirs: the account deletion
    // takes it; gone: its object missing already.
    let mut cids = HashMap::new();
    for name in ["kept", "collected", "moved", "unwritten"] {
        let b = h
            .upload(&access, format!("{name} bytes").as_bytes(), "image/png")
            .await;
        let rkey = h.create_post(&did, &access, post_with(&b)).await;
        if name == "collected" {
            h.delete_post(&did, &access, &rkey).await;
        }
        cids.insert(name, cid_of(&b));
    }
    let theirs = h.upload(&access2, b"theirs bytes", "image/png").await;
    h.create_post(&did2, &access2, post_with(&theirs)).await;
    let gone = h.upload(&access, b"gone bytes", "image/png").await;
    let gone_key = h.row(&did, &cid_of(&gone)).await.storage_key;
    h.state.blobs.delete(&gone_key).await.unwrap();
    // A key imported in place, in the reference's layout.
    let in_place_cid = cid_for(b"imported in place");
    let in_place = format!("blocks/{did}/{in_place_cid}");
    put(&h.state.blobs, &in_place, b"imported in place").await;
    let mut tx = h.state.store.begin().await.unwrap();
    tx.insert_imported_blob(
        &did,
        &locus_store::reference::ImportedBlob {
            cid: in_place_cid,
            mime_type: "image/png".into(),
            size: 17,
            storage_key: in_place.clone(),
            state: "permanent".into(),
            created_at: 0,
            takedown_ref: None,
            unreferenced_since: None,
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    // A branding upload.
    put(&h.state.blobs, "branding/logo9.0001", b"logo").await;
    h.state
        .store
        .put_branding_asset(&brand("logo9", "logo", "branding/logo9.0001"), epoch)
        .await
        .unwrap();
    // Two moves' step 1 (takedowns to quarantine), bytes not written.
    let mut moves = HashMap::new();
    for name in ["moved", "unwritten"] {
        h.state
            .store
            .set_blob_takedown(&did, &cids[name], Some("t"))
            .await
            .unwrap();
        let mut tx = h.state.store.begin().await.unwrap();
        tx.blob_backend().await.unwrap();
        let m = tx
            .place_blob(&did, &cids[name], false, epoch)
            .await
            .unwrap()
            .unwrap();
        tx.commit().await.unwrap();
        moves.insert(name, m);
    }
    // The delete queue drains to empty first: the backup's mark is 0.
    let mut tx = h.state.store.begin().await.unwrap();
    tx.queue_blob_delete("tmp/drained", epoch).await.unwrap();
    tx.commit().await.unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    locus_blob::run_deletes(&h.state.store, &h.state.blobs, 0, 100)
        .await
        .unwrap();
    assert!(queued(&h).await.is_empty());
    let counter = int(&h, "SELECT seq FROM sqlite_sequence WHERE name = 'blob_delete'").await;
    let names: HashMap<&str, String> = {
        let mut m = HashMap::new();
        for (n, c) in &cids {
            m.insert(*n, h.row(&did, c).await.storage_key);
        }
        m.insert("theirs", h.row(&did2, &cid_of(&theirs)).await.storage_key);
        m
    };

    // While it copies (slowly): GC, a move's step 3 and an account
    // deletion queue deletes, and the delete job runs.
    let deleted_during = Arc::new(AtomicUsize::new(usize::MAX));
    let loaded = Arc::new(AtomicBool::new(false));
    let _slow = {
        let (s, did2, deleted) = (h.state.clone(), did2.clone(), Arc::clone(&deleted_during));
        let moved = moves["moved"].clone();
        hook(&h, "backup.copy", move || {
            let (s, did2, deleted, moved, loaded) = (
                s.clone(),
                did2.clone(),
                Arc::clone(&deleted),
                moved.clone(),
                Arc::clone(&loaded),
            );
            async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                if loaded.swap(true, Ordering::SeqCst) {
                    return Ok(());
                }
                let mut tx = s.store.begin().await?;
                let e = tx.blob_backend().await?.epoch;
                assert_eq!(tx.gc_blob_batch(3_600_000, -1, 100, e).await?, 1);
                tx.commit().await?;
                assert!(locus_blob::finish_move(&s.store, &s.blobs, &moved).await?);
                let mut tx = s.store.begin().await?;
                assert_eq!(tx.delete_account_blobs(&did2, 100, e).await?, 1);
                tx.commit().await?;
                tokio::time::sleep(Duration::from_millis(5)).await;
                let n = locus_blob::run_deletes(&s.store, &s.blobs, 0, 100).await?;
                deleted.store(n, Ordering::SeqCst);
                Ok(())
            }
        })
    };
    let id = crate::backups::start(h.state.clone(), None, "test")
        .await
        .unwrap();
    // One at a time, even from the same node (it may not take the
    // running backup's lease over).
    assert!(
        crate::backups::start(h.state.clone(), None, "test")
            .await
            .is_err()
    );
    let run = backup_ended(&h, &id).await;
    assert_eq!(run.status, "done", "{:?}", run.error);
    assert_eq!(run.delete_mark, Some(0));
    assert_eq!(deleted_during.load(Ordering::SeqCst), 0, "nothing deleted during the backup");
    let during = queued(&h).await;
    assert!(during.len() >= 3, "{during:?}");
    assert!(
        int(&h, "SELECT MIN(id) FROM blob_delete").await > counter,
        "new ids above the mark and the AUTOINCREMENT counter"
    );
    // Every key the snapshot names is in the backup but the missing one,
    // which the report lists without failing.
    let dir = std::path::PathBuf::from(&run.dir);
    let manifest: crate::backups::Manifest =
        serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!((manifest.status.as_str(), manifest.missing), ("done", vec![gone_key.clone()]));
    let snap = Store::open_sqlite_read_only(&dir.join("db.sqlite"))
        .await
        .unwrap();
    let mut named = Vec::new();
    let mut after = String::new();
    loop {
        let page = snap.tracked_keys_after(&after, 500).await.unwrap();
        let Some(last) = page.last().cloned() else {
            break;
        };
        named.extend(page);
        after = last;
    }
    snap.close().await;
    for k in names.values().chain([&in_place, &"branding/logo9.0001".to_owned()]) {
        assert!(named.contains(k), "the snapshot names {k}");
    }
    for k in &named {
        if k != &gone_key {
            assert!(dir.join("objects").join(k).exists(), "{k} copied");
        }
    }
    // After the backup, the skipped deletes run.
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert!(
        locus_blob::run_deletes(&h.state.store, &h.state.blobs, 0, 100)
            .await
            .unwrap()
            >= 3
    );
    for n in ["collected", "theirs", "moved"] {
        assert!(
            h.state.blobs.read(&names[n], None).await.unwrap().is_none(),
            "{n}'s old key deleted after the backup"
        );
    }
}

#[tokio::test]
async fn a_backup_that_loses_its_lease_mid_copy_is_marked_failed() {
    let h = harness(false).await;
    if h.state.blobs.name() != "disk" {
        return;
    }
    let (_did, access) = h.user("lee.pds.example.com").await;
    for n in 0..2u8 {
        h.upload(&access, format!("lease bytes {n}").as_bytes(), "image/png")
            .await;
    }
    // Mid-copy the lease lapses (a long stall), and meanwhile the delete
    // job runs again.
    let ran = Arc::new(AtomicUsize::new(0));
    let _lapse = {
        let (s, ran) = (h.state.clone(), Arc::clone(&ran));
        once(&h, "backup.copy", move || async move {
            sqlx::query("UPDATE job_lease SET expires_at = 0 WHERE name = 'backup'")
                .execute(pool(&s))
                .await
                .unwrap();
            let mut tx = s.store.begin().await?;
            tx.queue_blob_delete("tmp/during-the-lapse", s.blobs.epoch())
                .await?;
            tx.commit().await?;
            tokio::time::sleep(Duration::from_millis(5)).await;
            ran.store(
                locus_blob::run_deletes(&s.store, &s.blobs, 0, 100).await? + 1,
                Ordering::SeqCst,
            );
            Ok(())
        })
    };
    let id = crate::backups::start(h.state.clone(), None, "test")
        .await
        .unwrap();
    let run = backup_ended(&h, &id).await;
    assert_eq!(run.status, "failed");
    assert!(
        run.error.as_deref().unwrap_or("").contains("lease"),
        "{:?}",
        run.error
    );
    assert!(ran.load(Ordering::SeqCst) > 0, "the delete job ran in the lapse");
    assert!(queued(&h).await.is_empty(), "its row was taken");
    let manifest: crate::backups::Manifest = serde_json::from_slice(
        &std::fs::read(std::path::Path::new(&run.dir).join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest.status, "failed");
}
