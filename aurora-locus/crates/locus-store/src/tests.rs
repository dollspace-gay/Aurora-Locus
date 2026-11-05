//! Store tests, run against SQLite always and against Postgres when
//! `LOCUS_TEST_POSTGRES_URL` points at a server (each test gets its own
//! throwaway database).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use locus_core::CoreError;
use locus_core::bootstrap::DatabaseUrl;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::env::{CLOCK_SLOT_FAMILY, collect_env};
use crate::runtime::{Settings, run_leased};
use crate::{Store, Tx};

struct Backend {
    name: &'static str,
    store: Store,
    _dir: Option<tempfile::TempDir>,
}

async fn backends() -> Vec<Backend> {
    let mut out = Vec::new();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&DatabaseUrl::Sqlite(dir.path().join("t.sqlite")))
        .await
        .unwrap();
    out.push(Backend {
        name: "sqlite",
        store,
        _dir: Some(dir),
    });

    if let Ok(admin_url) = std::env::var("LOCUS_TEST_POSTGRES_URL") {
        let admin = sqlx::PgPool::connect(&admin_url)
            .await
            .expect("connect to test postgres");
        let db = format!("locus_test_{}", hex::encode(rand::random::<[u8; 8]>()));
        sqlx::query(&format!("CREATE DATABASE {db}"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
        let url = match admin_url.rsplit_once('/') {
            Some((base, _)) => format!("{base}/{db}"),
            None => panic!("LOCUS_TEST_POSTGRES_URL must end in /<database>"),
        };
        let store = Store::open(&DatabaseUrl::Postgres(url)).await.unwrap();
        out.push(Backend {
            name: "postgres",
            store,
            _dir: None,
        });
    }
    out
}

#[tokio::test]
async fn migrations_are_idempotent_and_newer_schema_is_refused() {
    for b in backends().await {
        let s = &b.store;
        s.migrate()
            .await
            .unwrap_or_else(|e| panic!("{}: re-migrate: {e}", b.name));
        assert_eq!(
            s.applied_schema_version().await.unwrap(),
            s.known_schema_version(),
            "{}",
            b.name
        );

        // Pretend a newer build migrated this database.
        let sql = "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
                   VALUES (99999999999999, 'future', true, x'00', 0)";
        let pg_sql = "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
                      VALUES (99999999999999, 'future', true, '\\x00'::bytea, 0)";
        match s {
            Store::Sqlite(p) => {
                sqlx::query(sql).execute(p).await.unwrap();
            }
            Store::Postgres(p) => {
                sqlx::query(pg_sql).execute(p).await.unwrap();
            }
        }
        match s.migrate().await {
            Err(CoreError::Config(msg)) => assert!(msg.contains("newer"), "{}: {msg}", b.name),
            other => panic!("{}: expected refusal, got {other:?}", b.name),
        }
    }
}

#[tokio::test]
async fn settings_store_validate_version_and_audit() {
    for b in backends().await {
        let s = &b.store;
        assert_eq!(s.settings_version().await.unwrap(), 0);
        let v1 = s
            .set_setting("log.level", json!("debug"), "op:alice")
            .await
            .unwrap();
        let v2 = s
            .set_setting(
                "sync.deleted_history_preservation_days",
                json!(30),
                "op:alice",
            )
            .await
            .unwrap();
        assert_eq!((v1, v2), (1, 2), "{}", b.name);

        let snap = s.load_snapshot().await.unwrap();
        assert_eq!(snap.version(), 2);
        assert_eq!(snap.text("log.level"), "debug");
        assert_eq!(snap.int("sync.deleted_history_preservation_days"), 30);

        // Invalid values and unknown keys are refused and change nothing.
        assert!(matches!(
            s.set_setting("sync.deleted_history_preservation_days", json!(91), "op")
                .await,
            Err(CoreError::Invalid(_))
        ));
        assert!(matches!(
            s.set_setting("no.such.key", json!(1), "op").await,
            Err(CoreError::NotFound(_))
        ));
        assert_eq!(s.settings_version().await.unwrap(), 2, "{}", b.name);

        let audits: i64 = match s {
            Store::Sqlite(p) => {
                sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE action = 'setting.set'")
                    .fetch_one(p)
                    .await
                    .unwrap()
            }
            Store::Postgres(p) => {
                sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE action = 'setting.set'")
                    .fetch_one(p)
                    .await
                    .unwrap()
            }
        };
        assert_eq!(audits, 2, "{}", b.name);
    }
}

#[tokio::test]
async fn settings_propagate_to_other_handles() {
    for b in backends().await {
        let a = Settings::load(b.store.clone()).await.unwrap();
        let other = Settings::load(b.store.clone()).await.unwrap();
        let cancel = CancellationToken::new();
        let watcher = other.spawn_watcher(cancel.clone());
        let mut rx = other.subscribe();

        a.set("server.landing_page", json!(true), "op")
            .await
            .unwrap();
        assert!(
            a.current().bool("server.landing_page"),
            "{}: own node applies at once",
            b.name
        );

        // Postgres: NOTIFY delivers well under the poll interval. SQLite: the poll.
        let limit = if b.store.is_postgres() {
            Duration::from_secs(2)
        } else {
            Duration::from_secs(7)
        };
        tokio::time::timeout(limit, async {
            while !rx.borrow_and_update().bool("server.landing_page") {
                rx.changed().await.unwrap();
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{}: other node never saw the change", b.name));

        cancel.cancel();
        watcher.await.unwrap();
    }
}

#[tokio::test]
async fn leases_are_exclusive_fenced_and_taken_over_after_expiry() {
    for b in backends().await {
        let s = &b.store;
        let ttl = Duration::from_millis(600);
        let a = s
            .try_acquire_lease("sequencer", "node-a", ttl)
            .await
            .unwrap()
            .expect("a acquires");
        assert!(
            s.try_acquire_lease("sequencer", "node-b", ttl)
                .await
                .unwrap()
                .is_none(),
            "{}",
            b.name
        );
        assert!(s.renew_lease(&a).await.unwrap());

        let mut tx = s.begin().await.unwrap();
        tx.fence(&a).await.expect("holder passes the fence");
        tx.rollback().await.unwrap();

        tokio::time::sleep(Duration::from_millis(900)).await;
        let bt = s
            .try_acquire_lease("sequencer", "node-b", ttl)
            .await
            .unwrap()
            .expect("b takes over");
        assert_eq!(bt.epoch, a.epoch + 1, "{}", b.name);

        // The old holder can neither renew nor commit.
        assert!(!s.renew_lease(&a).await.unwrap(), "{}", b.name);
        let mut tx = s.begin().await.unwrap();
        assert!(
            matches!(tx.fence(&a).await, Err(CoreError::LeaseLost(_))),
            "{}",
            b.name
        );
        tx.rollback().await.unwrap();

        // Release frees it at once.
        s.release_lease(&bt).await.unwrap();
        assert!(
            s.try_acquire_lease("sequencer", "node-a", ttl)
                .await
                .unwrap()
                .is_some(),
            "{}",
            b.name
        );
    }
}

#[tokio::test]
async fn fencing_rolls_back_a_stale_holders_write() {
    for b in backends().await {
        let s = &b.store;
        let ttl = Duration::from_millis(400);
        let a = s
            .try_acquire_lease("gc", "node-a", ttl)
            .await
            .unwrap()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
        let _b = s
            .try_acquire_lease("gc", "node-b", ttl)
            .await
            .unwrap()
            .unwrap();

        // The stale holder writes, then fences before committing (as every
        // leased job's transaction does); the fence fails, so it rolls back.
        let mut tx: Tx = s.begin().await.unwrap();
        tx.set_setting("log.level", json!("trace"), "job:gc")
            .await
            .unwrap();
        assert!(
            matches!(tx.fence(&a).await, Err(CoreError::LeaseLost(_))),
            "{}",
            b.name
        );
        tx.rollback().await.unwrap();
        assert_eq!(s.settings_version().await.unwrap(), 0, "{}", b.name);
        assert_eq!(
            s.load_snapshot().await.unwrap().text("log.level"),
            "info",
            "{}",
            b.name
        );
    }
}

#[tokio::test]
async fn slots_go_to_different_nodes() {
    for b in backends().await {
        let s = &b.store;
        let ttl = Duration::from_secs(30);
        let (na, _) = s
            .claim_slot(CLOCK_SLOT_FAMILY, 4, "node-a", ttl)
            .await
            .unwrap()
            .unwrap();
        let (nb, _) = s
            .claim_slot(CLOCK_SLOT_FAMILY, 4, "node-b", ttl)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(na, nb, "{}", b.name);
        let mut live = s.live_slot_holders(CLOCK_SLOT_FAMILY).await.unwrap();
        live.sort();
        assert_eq!(live, vec!["node-a", "node-b"], "{}", b.name);
    }
}

fn env_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[tokio::test]
async fn env_values_apply_once_every_live_node_agrees() {
    for b in backends().await {
        let s = &b.store;
        let ttl = Duration::from_secs(30);
        s.claim_slot(CLOCK_SLOT_FAMILY, 8, "node-a", ttl)
            .await
            .unwrap()
            .unwrap();
        s.claim_slot(CLOCK_SLOT_FAMILY, 8, "node-b", ttl)
            .await
            .unwrap()
            .unwrap();

        let (v1, errs) = collect_env(&env_of(&[("PDS_SERVICE_NAME", "Example PDS")]));
        assert!(errs.is_empty());

        // Only node A reports it so far: nothing applies.
        assert!(
            s.report_and_apply_env("node-a", &v1)
                .await
                .unwrap()
                .is_empty(),
            "{}",
            b.name
        );
        // Node B agrees: it applies, once.
        assert_eq!(
            s.report_and_apply_env("node-b", &v1).await.unwrap(),
            vec!["server.service_name"]
        );
        assert!(
            s.report_and_apply_env("node-a", &v1)
                .await
                .unwrap()
                .is_empty(),
            "{}: applied once",
            b.name
        );
        assert_eq!(
            s.load_snapshot().await.unwrap().text("server.service_name"),
            "Example PDS"
        );

        // A console change isn't overridden by the unchanged env value.
        s.set_setting("server.service_name", json!("Console Name"), "op")
            .await
            .unwrap();
        assert!(
            s.report_and_apply_env("node-a", &v1)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            s.load_snapshot().await.unwrap().text("server.service_name"),
            "Console Name"
        );

        // A changed env value applies again once both nodes run with it.
        let (v2, _) = collect_env(&env_of(&[("PDS_SERVICE_NAME", "Renamed PDS")]));
        assert!(
            s.report_and_apply_env("node-a", &v2)
                .await
                .unwrap()
                .is_empty(),
            "{}: rolling restart",
            b.name
        );
        assert_eq!(
            s.report_and_apply_env("node-b", &v2).await.unwrap(),
            vec!["server.service_name"]
        );
        assert_eq!(
            s.load_snapshot().await.unwrap().text("server.service_name"),
            "Renamed PDS"
        );
    }
}

#[tokio::test]
async fn bad_env_values_are_reported_not_applied() {
    let (vals, errs) = collect_env(&env_of(&[("LOG_LEVEL", "loud")]));
    assert!(vals.is_empty());
    assert_eq!(errs.len(), 1);
}

#[tokio::test]
async fn run_leased_runs_the_job_on_one_node_at_a_time() {
    for b in backends().await {
        let running = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));
        let ran = Arc::new(AtomicUsize::new(0));
        let cancel = CancellationToken::new();
        let mut handles = Vec::new();
        for node in ["node-a", "node-b", "node-c"] {
            let (running, max_seen, ran) = (running.clone(), max_seen.clone(), ran.clone());
            let store = b.store.clone();
            handles.push(tokio::spawn(run_leased(
                store,
                "singleton".into(),
                node.into(),
                Duration::from_millis(600),
                cancel.clone(),
                move |_token, job_cancel| {
                    let (running, max_seen, ran) = (running.clone(), max_seen.clone(), ran.clone());
                    async move {
                        let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                        max_seen.fetch_max(now, Ordering::SeqCst);
                        ran.fetch_add(1, Ordering::SeqCst);
                        tokio::select! {
                            () = job_cancel.cancelled() => {}
                            () = tokio::time::sleep(Duration::from_millis(300)) => {}
                        }
                        running.fetch_sub(1, Ordering::SeqCst);
                        Ok(())
                    }
                },
            )));
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
        cancel.cancel();
        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(
            max_seen.load(Ordering::SeqCst),
            1,
            "{}: two nodes ran it at once",
            b.name
        );
        assert!(
            ran.load(Ordering::SeqCst) >= 2,
            "{}: job should run repeatedly",
            b.name
        );
    }
}

#[tokio::test]
async fn secrets_are_sealed_and_a_different_master_key_is_refused() {
    use crate::secrets::KeyCheck;
    use locus_core::keys::{MasterKey, SourceKind};
    for b in backends().await {
        let s = &b.store;
        let key = MasterKey::generate(SourceKind::File);
        let other = MasterKey::generate(SourceKind::Env);

        assert!(s.key_store_empty().await.unwrap(), "{}", b.name);
        assert_eq!(s.check_master_key(&key).await.unwrap(), KeyCheck::Empty);

        let mut tx = s.begin().await.unwrap();
        tx.put_secret("jwt", b"hmac secret", &key).await.unwrap();
        tx.put_signing_key("did:plc:abc", "k256", b"private bytes", &key)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        assert_eq!(s.check_master_key(&key).await.unwrap(), KeyCheck::Matches);
        assert_eq!(
            s.get_secret("jwt", &key).await.unwrap().unwrap().as_slice(),
            b"hmac secret"
        );
        let (kind, sk) = s
            .get_signing_key("did:plc:abc", &key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (kind.as_str(), sk.as_slice()),
            ("k256", &b"private bytes"[..])
        );

        // The database never holds the secret in the clear.
        let raw: Vec<u8> = match s {
            Store::Sqlite(p) => sqlx::query_scalar("SELECT sealed FROM secret WHERE name = 'jwt'")
                .fetch_one(p)
                .await
                .unwrap(),
            Store::Postgres(p) => {
                sqlx::query_scalar("SELECT sealed FROM secret WHERE name = 'jwt'")
                    .fetch_one(p)
                    .await
                    .unwrap()
            }
        };
        assert!(!raw.windows(11).any(|w| w == b"hmac secret"), "{}", b.name);

        // A node with another key: refused at boot and on any secret write.
        assert!(matches!(
            s.check_master_key(&other).await,
            Err(CoreError::Config(_))
        ));
        let mut tx = s.begin().await.unwrap();
        assert!(
            tx.put_secret("smtp", b"x", &other).await.is_err(),
            "{}",
            b.name
        );
        tx.rollback().await.unwrap();
        assert!(s.get_secret("smtp", &key).await.unwrap().is_none());
        // And it can't read the secrets either.
        assert!(s.get_secret("jwt", &other).await.is_err());
    }
}

#[tokio::test]
async fn job_runs_are_recorded() {
    for b in backends().await {
        let s = &b.store;
        s.record_job_run("gc", None, Some(123)).await.unwrap();
        s.record_job_run("gc", Some("disk full"), Some(456))
            .await
            .unwrap();
        let states = s.job_states().await.unwrap();
        assert_eq!(states.len(), 1, "{}", b.name);
        let gc = &states[0];
        assert_eq!(gc.runs, 2, "{}", b.name);
        assert_eq!(gc.last_error.as_deref(), Some("disk full"), "{}", b.name);
        assert_eq!(gc.next_run_at, Some(456), "{}", b.name);
        // The failed run doesn't erase when it last succeeded.
        assert!(gc.last_ok_at.is_some(), "{}", b.name);
        assert!(gc.last_run_at >= gc.last_ok_at, "{}", b.name);
    }
}

#[tokio::test]
async fn identity_tables_on_both_backends() {
    use crate::identity::{PlcKeyRole, RepairKind};
    for b in backends().await {
        let s = &b.store;
        let n = b.name;

        // One current PLC key at most; a second is a conflict.
        let mut tx = s.begin().await.unwrap();
        tx.insert_plc_rotation_key("did:key:zA", PlcKeyRole::Current)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let mut tx = s.begin().await.unwrap();
        match tx
            .insert_plc_rotation_key("did:key:zB", PlcKeyRole::Current)
            .await
        {
            Err(CoreError::Conflict(_)) => {}
            other => panic!("{n}: second current key: {other:?}"),
        }
        // A failed statement ends a Postgres transaction; start another.
        tx.rollback().await.unwrap();
        let mut tx = s.begin().await.unwrap();
        tx.insert_plc_rotation_key("did:key:zB", PlcKeyRole::Secondary)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let keys = s.plc_rotation_keys().await.unwrap();
        assert_eq!(keys.len(), 2, "{n}");
        assert_eq!(keys[0].role, PlcKeyRole::Current, "{n}");

        s.set_plc_key_listing("did:plc:x", &["did:key:zB".into()])
            .await
            .unwrap();
        s.set_plc_key_listing("did:plc:x", &["did:key:zA".into(), "did:key:zB".into()])
            .await
            .unwrap();
        assert_eq!(
            s.plc_key_listing("did:plc:x").await.unwrap(),
            Some(vec!["did:key:zA".to_owned(), "did:key:zB".to_owned()]),
            "{n}"
        );

        // Caches report their age on the database clock.
        s.cache_did_doc("did:plc:x", "{}").await.unwrap();
        let c = s.cached_did_doc("did:plc:x").await.unwrap().unwrap();
        assert!((0..5_000).contains(&c.age_ms), "{n}: age {}", c.age_ms);
        s.clear_did_doc("did:plc:x").await.unwrap();
        assert!(
            s.cached_did_doc("did:plc:x").await.unwrap().is_none(),
            "{n}"
        );
        s.cache_handle("a.test", None).await.unwrap();
        assert_eq!(
            s.cached_handle("a.test").await.unwrap().unwrap().value,
            None,
            "{n}"
        );
        s.cache_handle("a.test", Some("did:plc:x")).await.unwrap();
        assert_eq!(
            s.cached_handle("a.test")
                .await
                .unwrap()
                .unwrap()
                .value
                .as_deref(),
            Some("did:plc:x"),
            "{n}"
        );

        // A did:web host is tied to one DID; the row outlives the document.
        let mut tx = s.begin().await.unwrap();
        tx.insert_hosted_did_web("did:web:w.test", "w.test", "{}")
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let mut tx = s.begin().await.unwrap();
        match tx
            .insert_hosted_did_web("did:web:other", "w.test", "{}")
            .await
        {
            Err(CoreError::Conflict(_)) => {}
            other => panic!("{n}: second DID on a host: {other:?}"),
        }
        tx.rollback().await.unwrap();
        let mut tx = s.begin().await.unwrap();
        assert!(
            tx.set_hosted_did_web_doc("did:web:w.test", None)
                .await
                .unwrap()
        );
        tx.commit().await.unwrap();
        let row = s.hosted_did_web_by_host("w.test").await.unwrap().unwrap();
        assert_eq!((row.did.as_str(), row.doc), ("did:web:w.test", None), "{n}");

        // Repairs: due at once, retried with a backoff, finished only for
        // the value they hold.
        s.queue_identity_repair("did:plc:x", RepairKind::Handle, "h1.test")
            .await
            .unwrap();
        s.queue_identity_repair("did:plc:x", RepairKind::Handle, "h2.test")
            .await
            .unwrap();
        let due = s.due_identity_repairs(10).await.unwrap();
        assert_eq!(due.len(), 1, "{n}: replaced, not added");
        assert_eq!(due[0].value, "h2.test", "{n}");
        s.retry_identity_repair("did:plc:x", RepairKind::Handle, "timeout", 60_000)
            .await
            .unwrap();
        assert!(
            s.due_identity_repairs(10).await.unwrap().is_empty(),
            "{n}: backing off"
        );
        s.finish_identity_repair("did:plc:x", RepairKind::Handle, "h1.test")
            .await
            .unwrap();
        s.queue_identity_repair("did:plc:x", RepairKind::Handle, "h2.test")
            .await
            .unwrap();
        assert_eq!(
            s.due_identity_repairs(10).await.unwrap().len(),
            1,
            "{n}: stale finish kept it"
        );
        s.finish_identity_repair("did:plc:x", RepairKind::Handle, "h2.test")
            .await
            .unwrap();
        assert!(s.due_identity_repairs(10).await.unwrap().is_empty(), "{n}");
    }
}

#[tokio::test]
async fn bulk_jobs_block_writes_fence_batches_and_are_taken_over() {
    use crate::accounts::{NewAccount, Status};
    for b in backends().await {
        let s = &b.store;
        let n = b.name;
        let did = "did:plc:bulktest";
        let mut tx = s.begin().await.unwrap();
        tx.insert_account(&NewAccount {
            did,
            handle: "bulk.test",
            email: Some("bulk@example.com"),
            password_hash: None,
            status: Status::Active,
            did_method: "plc",
        })
        .await
        .unwrap();
        tx.put_signing_key(did, "k256", b"key", &test_master_key())
            .await
            .unwrap();
        tx.commit().await.unwrap();

        let mut tx = s.begin().await.unwrap();
        let job = tx
            .start_bulk_job("job-1", did, "delete", "deleting", "node-a")
            .await
            .unwrap();
        assert!(tx.write_blocked(did).await.unwrap(), "{n}");
        tx.commit().await.unwrap();
        // A second job for the account is refused.
        let mut tx = s.begin().await.unwrap();
        assert!(
            matches!(
                tx.start_bulk_job("job-2", did, "import", "staging", "node-b")
                    .await,
                Err(CoreError::Conflict(_))
            ),
            "{n}"
        );
        tx.rollback().await.unwrap();

        // Nothing to take over while the holder's lease runs.
        assert!(
            s.take_over_bulk_job("node-b").await.unwrap().is_none(),
            "{n}"
        );
        // The holder stops; its lease runs out.
        let expire = "UPDATE bulk_job SET expires_at = 0 WHERE id = 'job-1'";
        match s {
            Store::Sqlite(p) => sqlx::query(expire).execute(p).await.map(|_| ()),
            Store::Postgres(p) => sqlx::query(expire).execute(p).await.map(|_| ()),
        }
        .unwrap();
        let taken = s
            .take_over_bulk_job("node-b")
            .await
            .unwrap()
            .expect("taken over");
        assert_eq!((taken.holder.as_str(), taken.epoch), ("node-b", 2), "{n}");
        // The old holder can't commit a batch any more.
        let mut tx = s.begin().await.unwrap();
        assert!(
            matches!(tx.fence_bulk_job(&job).await, Err(CoreError::LeaseLost(_))),
            "{n}"
        );
        tx.rollback().await.unwrap();

        s.run_delete_job(&taken, 1).await.unwrap();
        assert!(s.account_by_did(did).await.unwrap().is_none(), "{n}");
        assert!(
            s.get_signing_key(did, &test_master_key())
                .await
                .unwrap()
                .is_none(),
            "{n}"
        );
        assert_eq!(
            s.bulk_job("job-1").await.unwrap().unwrap().phase,
            "done",
            "{n}"
        );
        // The handle and email are free again.
        let mut tx = s.begin().await.unwrap();
        tx.insert_account(&NewAccount {
            did: "did:plc:bulktest2",
            handle: "bulk.test",
            email: Some("bulk@example.com"),
            password_hash: None,
            status: Status::Active,
            did_method: "plc",
        })
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }
}

fn test_master_key() -> locus_core::keys::MasterKey {
    use std::sync::OnceLock;
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    let bytes = KEY.get_or_init(rand::random);
    locus_core::keys::MasterKey::from_bytes(*bytes, locus_core::keys::SourceKind::Env)
}
