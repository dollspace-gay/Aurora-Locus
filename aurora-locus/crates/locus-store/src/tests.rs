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

#[tokio::test]
async fn rate_limit_counters_use_fixed_windows_and_prune() {
    for b in backends().await {
        let s = &b.store;
        let (n, end) = s.consume_rate_limit("k", 2, 60_000).await.unwrap();
        assert_eq!(n, 2, "{}", b.name);
        let (n, end2) = s.consume_rate_limit("k", 3, 60_000).await.unwrap();
        assert_eq!((n, end2), (5, end), "{}: same window", b.name);
        assert_eq!(s.consume_rate_limit("other", 1, 60_000).await.unwrap().0, 1);
        // A window that has ended starts over.
        let (_, short_end) = s.consume_rate_limit("short", 4, 1).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let (n, end3) = s.consume_rate_limit("short", 1, 60_000).await.unwrap();
        assert_eq!(n, 1, "{}: new window", b.name);
        assert!(end3 > short_end);
        // Only ended windows are pruned.
        s.consume_rate_limit("gone", 1, 1).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(s.prune_rate_limits(100).await.unwrap(), 1, "{}", b.name);
        assert_eq!(s.consume_rate_limit("k", 1, 60_000).await.unwrap().0, 6);
    }
}

#[tokio::test]
async fn oauth_replay_windows() {
    for b in backends().await {
        let s = &b.store;
        assert!(
            s.oauth_unique("DPoP", "j1", 60_000).await.unwrap(),
            "{}",
            b.name
        );
        assert!(
            !s.oauth_unique("DPoP", "j1", 60_000).await.unwrap(),
            "{}: replay",
            b.name
        );
        assert!(
            s.oauth_unique("Other", "j1", 60_000).await.unwrap(),
            "namespaces are separate"
        );
        assert!(s.oauth_unique("DPoP", "short", 1).await.unwrap());
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(
            s.oauth_unique("DPoP", "short", 60_000).await.unwrap(),
            "{}: window ended",
            b.name
        );
        assert!(!s.oauth_unique("DPoP", "short", 60_000).await.unwrap());
        s.oauth_unique("DPoP", "gone", 1).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(s.prune_oauth(100).await.unwrap(), 1, "{}", b.name);
    }
}

#[tokio::test]
async fn oauth_requests_tokens_and_refresh_rotation() {
    use crate::oauth::{RequestRow, TokenRow};
    for b in backends().await {
        let s = &b.store;
        let mut r = RequestRow {
            id: "req-1".into(),
            did: None,
            device_id: None,
            client_id: "c".into(),
            client_auth: None,
            parameters: "{}".into(),
            expires_at: 1,
            code: None,
        };
        s.oauth_create_request(&r).await.unwrap();
        r.did = Some("did:plc:a".into());
        r.code = Some("cod-1".into());
        s.oauth_update_request(&r).await.unwrap();
        assert_eq!(
            s.oauth_read_request("req-1")
                .await
                .unwrap()
                .unwrap()
                .code
                .as_deref(),
            Some("cod-1")
        );
        assert_eq!(
            s.oauth_consume_code("cod-1").await.unwrap().unwrap().id,
            "req-1",
            "{}",
            b.name
        );
        assert!(
            s.oauth_consume_code("cod-1").await.unwrap().is_none(),
            "{}: a code works once",
            b.name
        );

        let t = TokenRow {
            id: 0,
            did: "did:plc:a".into(),
            token_id: "tok-1".into(),
            created_at: 10,
            updated_at: 10,
            expires_at: 20,
            client_id: "c".into(),
            client_auth: "{\"method\":\"none\"}".into(),
            device_id: None,
            parameters: "{}".into(),
            details: None,
            code: Some("cod-1".into()),
            current_refresh_token: Some("ref-1".into()),
            scope: Some("atproto".into()),
        };
        s.oauth_create_token(&t).await.unwrap();
        let row = s.oauth_token("tok-1").await.unwrap().unwrap();
        assert_eq!(
            s.oauth_token_by_code("cod-1").await.unwrap().unwrap().id,
            row.id
        );
        s.oauth_rotate_token(
            row.id,
            "tok-2",
            "ref-2",
            30,
            40,
            "{\"method\":\"none\"}",
            Some("atproto"),
        )
        .await
        .unwrap();
        assert!(s.oauth_token("tok-1").await.unwrap().is_none());
        let now = s.oauth_token("tok-2").await.unwrap().unwrap();
        assert_eq!(
            (now.current_refresh_token.as_deref(), now.expires_at),
            (Some("ref-2"), 40)
        );
        // The spent refresh token still finds the grant (a replay).
        let spent = s.oauth_token_by_refresh("ref-1").await.unwrap().unwrap();
        assert_eq!(
            (spent.id, spent.current_refresh_token.as_deref()),
            (now.id, Some("ref-2")),
            "{}",
            b.name
        );
        s.oauth_delete_token(now.id).await.unwrap();
        assert!(s.oauth_token_by_refresh("ref-1").await.unwrap().is_none());
        assert!(s.oauth_token_by_refresh("ref-2").await.unwrap().is_none());

        s.oauth_upsert_device("dev-1", "ses-1", Some("ua"), "1.2.3.4")
            .await
            .unwrap();
        assert_eq!(
            s.oauth_device_session("dev-1").await.unwrap().as_deref(),
            Some("ses-1")
        );
        s.oauth_touch_account_device("did:plc:a", "dev-1")
            .await
            .unwrap();
        assert_eq!(s.oauth_device_accounts("dev-1").await.unwrap().len(), 1);
        s.oauth_set_authorized_client("did:plc:a", "c", "{\"authorizedScopes\":[\"atproto\"]}")
            .await
            .unwrap();
        assert!(
            s.oauth_authorized_client("did:plc:a", "c")
                .await
                .unwrap()
                .unwrap()
                .contains("atproto")
        );
    }
}

#[tokio::test]
async fn kryphocron_tables_on_both_backends() {
    use crate::kryphocron::{AudienceRow, AuditRow, PermissionRow, PrivateRecord};
    for b in backends().await {
        let s = &b.store;
        let n = b.name;

        // Permission rows, one per source.
        let row = PermissionRow {
            did: "did:plc:a".into(),
            source: "membership".into(),
            prism_did: "did:web:prism".into(),
            status: "member".into(),
            private_indexed: true,
            private_read: false,
            expires_at: 10,
            checked_at: 1,
        };
        s.put_kr_permission(&row).await.unwrap();
        s.put_kr_permission(&PermissionRow {
            status: "indexing".into(),
            ..row.clone()
        })
        .await
        .unwrap();
        let got = s
            .kr_permission("did:plc:a", "membership")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (got.status.as_str(), got.private_indexed, got.private_read),
            ("indexing", true, false),
            "{n}"
        );
        assert!(
            s.kr_permission("did:plc:a", "probe")
                .await
                .unwrap()
                .is_none(),
            "{n}"
        );
        assert_eq!(
            s.delete_kr_permissions_not_from("did:web:prism")
                .await
                .unwrap(),
            0,
            "{n}"
        );
        assert_eq!(
            s.delete_kr_permissions_not_from("did:web:other")
                .await
                .unwrap(),
            1,
            "{n}"
        );
        assert!(
            !s.private_tier_blocked("did:plc:nobody").await.unwrap(),
            "{n}"
        );

        // Audiences: a pending delete counts as missing.
        let aud = AudienceRow {
            did: "did:plc:a".into(),
            rkey: "3m6aaaaaaaaaa".into(),
            name: Some("friends".into()),
            mode: "list".into(),
            members_json: "[]".into(),
            created_at: "2024-01-01T00:00:00.000Z".into(),
            updated_at: "2024-01-01T00:00:00.000Z".into(),
            origin: "prism".into(),
            prism_did: "did:web:prism".into(),
            prism_cid: None,
            local_cid: "bafy".into(),
            value_cbor: vec![0xa0],
            pending: "none".into(),
            row_version: 1,
        };
        s.put_kr_audience(&aud).await.unwrap();
        s.put_kr_audience(&aud).await.unwrap();
        let got = s
            .kr_audience("did:plc:a", "3m6aaaaaaaaaa")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.row_version, 2, "{n}: bumped on every write");
        s.put_kr_audience(&AudienceRow {
            pending: "delete".into(),
            ..aud.clone()
        })
        .await
        .unwrap();
        assert!(
            s.kr_audience("did:plc:a", "3m6aaaaaaaaaa")
                .await
                .unwrap()
                .is_none(),
            "{n}"
        );

        // The private-record index and rotation selection.
        let entry = |rkey: &str, generation: Option<&str>, legacy: bool| PrivateRecord {
            did: "did:plc:a".into(),
            collection: "tools.kryphocron.feed.postPrivate".into(),
            rkey: rkey.into(),
            generation: generation.map(str::to_owned),
            legacy_text: legacy,
            flagged: None,
            audience_did: Some("did:plc:a".into()),
            audience_rkey: Some("3m6aaaaaaaaaa".into()),
        };
        let mut tx = s.begin().await.unwrap();
        for e in [
            entry(
                "3m6aaaaaaaaa2",
                Some("laquna/00000000000000000001/aa"),
                false,
            ),
            entry(
                "3m6aaaaaaaaa3",
                Some("laquna/00000000000000000003/aa"),
                false,
            ),
            entry("3m6aaaaaaaaa4", None, true),
            entry(
                "3m6aaaaaaaaa5",
                Some("laquna/00000000000000000009/aa"),
                false,
            ),
        ] {
            tx.replace_private_record("did:plc:a", &e.collection, &e.rkey, Some(&e))
                .await
                .unwrap();
        }
        tx.commit().await.unwrap();
        let current = "laquna/00000000000000000002/aa";
        let due = |urgent: bool| async move {
            s.rotation_due_records("did:plc:a", current, urgent, 10)
                .await
                .unwrap()
                .into_iter()
                .map(|r| r.rkey)
                .collect::<Vec<String>>()
        };
        assert_eq!(
            due(false).await,
            ["3m6aaaaaaaaa2", "3m6aaaaaaaaa4"],
            "{n}: older, or legacy text"
        );
        assert_eq!(due(true).await, ["3m6aaaaaaaaa4"], "{n}: legacy text only");
        let far = "laquna/00000000000000000099/ff";
        assert_eq!(
            s.rotation_due_accounts(current, far, "", 10).await.unwrap(),
            ["did:plc:a"],
            "{n}"
        );
        assert!(
            s.rotation_due_accounts(current, far, "did:plc:a", 10)
                .await
                .unwrap()
                .is_empty(),
            "{n}: paged by DID"
        );
        let future = s
            .rotation_future_dated("did:plc:a", "laquna/00000000000000000005/ff", 10)
            .await
            .unwrap();
        assert_eq!(future.len(), 1, "{n}");
        s.flag_private_record(
            "did:plc:a",
            "tools.kryphocron.feed.postPrivate",
            "3m6aaaaaaaaa2",
            "undecodable",
        )
        .await
        .unwrap();
        assert_eq!(
            s.rotation_due_records("did:plc:a", current, false, 10)
                .await
                .unwrap()
                .len(),
            1,
            "{n}: flagged skipped"
        );
        assert_eq!(s.flagged_private_records(10).await.unwrap().len(), 1, "{n}");
        let mut tx = s.begin().await.unwrap();
        tx.replace_private_record(
            "did:plc:a",
            "tools.kryphocron.feed.postPrivate",
            "3m6aaaaaaaaa4",
            None,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert!(
            s.private_record(
                "did:plc:a",
                "tools.kryphocron.feed.postPrivate",
                "3m6aaaaaaaaa4"
            )
            .await
            .unwrap()
            .is_none()
        );

        // Generations: the newest is current.
        assert!(s.current_generation().await.unwrap().is_none(), "{n}");
        s.add_generation("laquna/00000000000000000001/aa", 1)
            .await
            .unwrap();
        s.add_generation("laquna/00000000000000000002/bb", 2)
            .await
            .unwrap();
        s.add_generation("laquna/00000000000000000002/bb", 3)
            .await
            .unwrap();
        assert_eq!(
            s.current_generation().await.unwrap(),
            Some(("laquna/00000000000000000002/bb".to_owned(), 2)),
            "{n}"
        );

        // Audit rows, appended and pruned by age.
        let ev = |at: i64| AuditRow {
            at,
            sink: "user".into(),
            event: "ContentEncoded".into(),
            correlation_id: Some("abc".into()),
            detail_json: "\"x\"".into(),
        };
        s.append_kr_audit(&[ev(1), ev(100)]).await.unwrap();
        assert_eq!(
            s.kr_audit(10).await.unwrap()[0].at,
            100,
            "{n}: newest first"
        );
        assert_eq!(s.prune_kr_audit(50, 100).await.unwrap(), 1, "{n}");
        assert_eq!(s.kr_audit(10).await.unwrap().len(), 1, "{n}");
    }
}

#[tokio::test]
async fn the_audience_copy_on_both_backends() {
    use crate::kryphocron::{AudienceRow, RefreshLimiter};
    for b in backends().await {
        let s = &b.store;
        let n = b.name;
        let row = |rkey: &str, origin: &str, pending: &str| AudienceRow {
            did: "did:plc:o".into(),
            rkey: rkey.into(),
            name: None,
            mode: "list".into(),
            members_json: "[]".into(),
            created_at: "2024-01-01T00:00:00.000Z".into(),
            updated_at: "2024-01-01T00:00:00.000Z".into(),
            origin: origin.into(),
            prism_did: "did:web:prism".into(),
            prism_cid: Some("v1".into()),
            local_cid: "bafy".into(),
            value_cbor: vec![0xa0],
            pending: pending.into(),
            row_version: 1,
        };
        assert!(s.db_now().await.unwrap() > 1_600_000_000_000, "{n}");
        assert!(
            s.adopt_audience(&row("a", "prism", "none")).await.unwrap(),
            "{n}"
        );
        assert!(
            !s.adopt_audience(&row("a", "prism", "none")).await.unwrap(),
            "{n}: already there"
        );
        let v = s.audience_versions("did:plc:o").await.unwrap();
        assert_eq!((v.len(), v[0].row_version), (1, 1), "{n}");
        // Conditional on the version the listing saw.
        let newer = AudienceRow {
            prism_cid: Some("v2".into()),
            ..row("a", "prism", "none")
        };
        assert!(
            !s.replace_audience_if(&newer, 7).await.unwrap(),
            "{n}: stale version"
        );
        assert!(s.replace_audience_if(&newer, 1).await.unwrap(), "{n}");
        assert_eq!(
            s.kr_audience("did:plc:o", "a")
                .await
                .unwrap()
                .unwrap()
                .row_version,
            2
        );
        assert!(
            !s.delete_audience("did:plc:o", "a", Some(1)).await.unwrap(),
            "{n}: changed since"
        );
        assert!(
            s.audience_tombstone("did:plc:o", "a")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            s.delete_audience("did:plc:o", "a", Some(2)).await.unwrap(),
            "{n}"
        );
        assert!(
            s.audience_tombstone("did:plc:o", "a")
                .await
                .unwrap()
                .is_some(),
            "{n}: tombstoned"
        );
        assert_eq!(s.prune_audience_tombstones(0).await.unwrap(), 0);
        assert_eq!(
            s.prune_audience_tombstones(i64::MAX).await.unwrap(),
            1,
            "{n}"
        );

        // Limiters.
        assert!(
            s.take_refresh("did:plc:o", RefreshLimiter::Owner, 60_000)
                .await
                .unwrap(),
            "{n}"
        );
        assert!(
            !s.take_refresh("did:plc:o", RefreshLimiter::Owner, 60_000)
                .await
                .unwrap(),
            "{n}"
        );
        assert!(
            s.take_refresh("did:plc:o", RefreshLimiter::ThirdParty, 60_000)
                .await
                .unwrap(),
            "{n}: separate"
        );
        assert!(
            s.take_refresh("did:plc:o", RefreshLimiter::Owner, 0)
                .await
                .unwrap(),
            "{n}"
        );
        s.note_listing("did:plc:o", 5, true, None).await.unwrap();
        s.note_listing("did:plc:o", 6, false, Some("timeout"))
            .await
            .unwrap();
        let sync = s.audience_sync("did:plc:o").await.unwrap();
        assert_eq!(sync.last_listing_started_at, Some(6), "{n}");
        assert!(
            sync.last_complete_at.is_some(),
            "{n}: kept from the complete one"
        );
        assert_eq!(sync.last_error.as_deref(), Some("timeout"));

        // Aliases, polling and purging.
        s.put_audience_alias("did:plc:o", "local1", "prism1")
            .await
            .unwrap();
        assert_eq!(
            s.audience_alias("did:plc:o", "local1")
                .await
                .unwrap()
                .as_deref(),
            Some("prism1")
        );
        s.adopt_audience(&row("b", "local", "none")).await.unwrap();
        s.set_next_poll("did:plc:o", i64::MAX).await.unwrap();
        assert!(
            s.audience_polls_due(1_000, 10).await.unwrap().is_empty(),
            "{n}"
        );
        assert_eq!(s.poll_queued_audiences_now().await.unwrap(), 1, "{n}");
        assert_eq!(
            s.audience_polls_due(1_000, 10).await.unwrap(),
            ["did:plc:o"]
        );
        s.purge_audience_copy("did:plc:o").await.unwrap();
        assert!(s.kr_audiences("did:plc:o").await.unwrap().is_empty(), "{n}");
        assert!(
            s.audience_alias("did:plc:o", "local1")
                .await
                .unwrap()
                .is_none(),
            "{n}"
        );
    }
}

#[tokio::test]
async fn the_mapping_transaction_on_both_backends() {
    use crate::kryphocron::{AudienceRow, PrivateRecord};
    for b in backends().await {
        let s = &b.store;
        let n = b.name;
        let row = |rkey: &str, origin: &str, members: &str| AudienceRow {
            did: "did:plc:o".into(),
            rkey: rkey.into(),
            name: None,
            mode: "list".into(),
            members_json: members.into(),
            created_at: "2024-01-01T00:00:00.000Z".into(),
            updated_at: "2024-01-02T00:00:00.000Z".into(),
            origin: origin.into(),
            prism_did: "did:web:prism".into(),
            prism_cid: None,
            local_cid: "bafy".into(),
            value_cbor: vec![0xa0],
            pending: "none".into(),
            row_version: 1,
        };
        // Edited after its value was sent (version 1 sent, now 2).
        s.put_kr_audience(&row("local1", "local", "[]"))
            .await
            .unwrap();
        s.put_kr_audience(&row("local1", "local", "[\"did:plc:late\"]"))
            .await
            .unwrap();
        let prism = AudienceRow {
            prism_cid: Some("c1".into()),
            ..row("p1", "prism", "[]")
        };
        assert!(
            s.map_local_audience("did:plc:o", "local1", Some(1), "p1", Some(&prism))
                .await
                .unwrap(),
            "{n}"
        );
        let p = s.kr_audience("did:plc:o", "p1").await.unwrap().unwrap();
        assert_eq!(
            (p.pending.as_str(), p.members_json.as_str()),
            ("update", "[\"did:plc:late\"]"),
            "{n}: the late edit queued"
        );
        assert!(
            s.kr_audience("did:plc:o", "local1")
                .await
                .unwrap()
                .is_none(),
            "{n}"
        );
        assert!(
            s.audience_tombstone("did:plc:o", "local1")
                .await
                .unwrap()
                .is_some(),
            "{n}"
        );
        assert!(
            !s.map_local_audience("did:plc:o", "local1", Some(1), "p1", Some(&prism))
                .await
                .unwrap(),
            "{n}: twice is a no-op"
        );

        // Deleted meanwhile: prism's row becomes a pending delete.
        let prism2 = AudienceRow {
            prism_cid: Some("c2".into()),
            ..row("p2", "prism", "[]")
        };
        assert!(
            s.map_local_audience("did:plc:o", "gone", Some(1), "p2", Some(&prism2))
                .await
                .unwrap()
        );
        assert_eq!(
            s.audience_versions("did:plc:o")
                .await
                .unwrap()
                .iter()
                .find(|v| v.rkey == "p2")
                .unwrap()
                .pending,
            "delete",
            "{n}"
        );

        // Re-point candidates: private records naming a mapped local URI.
        let mut tx = s.begin().await.unwrap();
        for (rkey, aud) in [("3m6aaaaaaaaa2", "local1"), ("3m6aaaaaaaaa3", "p1")] {
            let e = PrivateRecord {
                did: "did:plc:writer".into(),
                collection: "tools.kryphocron.feed.postPrivate".into(),
                rkey: rkey.into(),
                generation: None,
                legacy_text: false,
                flagged: None,
                audience_did: Some("did:plc:o".into()),
                audience_rkey: Some(aud.into()),
            };
            tx.replace_private_record("did:plc:writer", &e.collection, rkey, Some(&e))
                .await
                .unwrap();
        }
        tx.commit().await.unwrap();
        let c = s.repoint_candidates(10).await.unwrap();
        assert_eq!(c.len(), 1, "{n}");
        assert_eq!(
            (
                c[0].did.as_str(),
                c[0].rkey.as_str(),
                c[0].prism_rkey.as_str()
            ),
            ("did:plc:writer", "3m6aaaaaaaaa2", "p1")
        );
        assert_eq!(
            s.pending_audiences("did:plc:o").await.unwrap().len(),
            2,
            "{n}"
        );
        assert!(
            s.local_audiences("did:plc:o").await.unwrap().is_empty(),
            "{n}"
        );
    }
}

#[tokio::test]
async fn the_review_queue_on_both_backends() {
    for b in backends().await {
        let s = &b.store;
        let n = b.name;
        let id = s
            .add_review("did:plc:a", "bafy", "upload", "quarantine", Some("hash"))
            .await
            .unwrap();
        s.add_review("did:plc:a", "bafz", "rescan", "reject", None)
            .await
            .unwrap();
        let open = s.reviews("open", 10).await.unwrap();
        assert_eq!(open.len(), 2, "{n}");
        assert_eq!(open[0].reason.as_deref(), Some("hash"));
        assert!(
            s.resolve_review(id, "reported", "admin", Some("sent"))
                .await
                .unwrap(),
            "{n}"
        );
        assert!(
            !s.resolve_review(id, "cleared", "admin", None)
                .await
                .unwrap(),
            "{n}: once"
        );
        assert_eq!(
            s.reviews("reported", 10).await.unwrap()[0].note.as_deref(),
            Some("sent")
        );
        assert!(s.unscanned_blobs(10).await.unwrap().is_empty(), "{n}");
        assert!(s.scan_status("did:plc:a", "bafy").await.unwrap().is_none());
    }
}

#[tokio::test]
async fn private_only_blobs_on_both_backends() {
    use crate::kryphocron::PrivateRecord;
    for b in backends().await {
        let s = &b.store;
        let n = b.name;
        let mut tx = s.begin().await.unwrap();
        tx.insert_temp_blob("did:plc:a", "bafyimg", "image/png", 10, "tmp/x")
            .await
            .unwrap();
        tx.replace_record_blobs(
            "did:plc:a",
            "tools.kryphocron.feed.postPrivate",
            "3m6aaaaaaaaa2",
            &["bafyimg".into()],
        )
        .await
        .unwrap();
        let e = PrivateRecord {
            did: "did:plc:a".into(),
            collection: "tools.kryphocron.feed.postPrivate".into(),
            rkey: "3m6aaaaaaaaa2".into(),
            generation: None,
            legacy_text: false,
            flagged: None,
            audience_did: Some("did:plc:a".into()),
            audience_rkey: Some("3m6aud".into()),
        };
        tx.replace_private_record("did:plc:a", &e.collection, &e.rkey, Some(&e))
            .await
            .unwrap();
        tx.recompute_private_only("did:plc:a", "bafyimg")
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert!(
            s.blob_private_only("did:plc:a", "bafyimg").await.unwrap(),
            "{n}"
        );
        let refs = s.blob_private_posts("did:plc:a", "bafyimg").await.unwrap();
        assert_eq!(
            (refs.len(), refs[0].audience_rkey.as_deref()),
            (1, Some("3m6aud")),
            "{n}"
        );
        // A public reference makes it public.
        let mut tx = s.begin().await.unwrap();
        tx.replace_record_blobs(
            "did:plc:a",
            "app.bsky.feed.post",
            "3m6aaaaaaaaa3",
            &["bafyimg".into()],
        )
        .await
        .unwrap();
        tx.recompute_private_only("did:plc:a", "bafyimg")
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert!(
            !s.blob_private_only("did:plc:a", "bafyimg").await.unwrap(),
            "{n}"
        );
        // Never-scanned private-only blobs are picked up by the rescan.
        let mut tx = s.begin().await.unwrap();
        tx.replace_record_blobs("did:plc:a", "app.bsky.feed.post", "3m6aaaaaaaaa3", &[])
            .await
            .unwrap();
        tx.recompute_private_only("did:plc:a", "bafyimg")
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(s.unscanned_blobs(10).await.unwrap().len(), 1, "{n}");
    }
}

#[tokio::test]
async fn operators_and_admin_queries_on_both_backends() {
    for b in backends().await {
        let s = &b.store;
        let n = b.name;
        assert!(
            s.create_operator("ana", "hash", "owner").await.unwrap(),
            "{n}"
        );
        assert!(
            !s.create_operator("ana", "hash", "viewer").await.unwrap(),
            "{n}: taken"
        );
        assert_eq!(s.count_owners().await.unwrap(), 1);
        s.create_operator_token("t1", "h1", "ana", "cli", None)
            .await
            .unwrap();
        s.create_operator_token("t2", "h2", "ana", "old", Some(1))
            .await
            .unwrap();
        assert_eq!(
            s.operator_by_token("h1").await.unwrap().unwrap().role,
            "owner",
            "{n}"
        );
        assert!(
            s.operator_by_token("h2").await.unwrap().is_none(),
            "{n}: expired"
        );
        assert!(
            s.update_operator("ana", Some("admin"), None, Some(true))
                .await
                .unwrap()
        );
        assert!(
            s.operator_by_token("h1").await.unwrap().is_none(),
            "{n}: disabled"
        );
        assert_eq!(s.operator("ana").await.unwrap().unwrap().role, "admin");
        assert_eq!(s.operator_tokens("ana").await.unwrap().len(), 2);
        assert!(s.revoke_operator_token("t1").await.unwrap());
        s.audit("operator:ana", "x.one", Some("t"), None)
            .await
            .unwrap();
        s.audit(
            "operator:ana",
            "x.two",
            None,
            Some(&serde_json::json!({"a": 1})),
        )
        .await
        .unwrap();
        let entries = s.audit_entries(None, 10).await.unwrap();
        assert_eq!(entries[0].action, "x.two", "{n}: newest first");
        assert_eq!(
            s.audit_entries(Some(entries[0].id), 10).await.unwrap()[0].action,
            "x.one"
        );

        // Invites, paged by recency and usage.
        let mut tx = s.begin().await.unwrap();
        for (i, code) in ["c1", "c2", "c3"].iter().enumerate() {
            tx.insert_invite_code(code, 2, "admin", "admin")
                .await
                .unwrap();
            let _ = i;
            tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        }
        tx.commit().await.unwrap();
        let page: Vec<String> = s
            .all_invite_codes(false, None, 2)
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.code)
            .collect();
        assert_eq!(page.len(), 2, "{n}");
        let rest = s.all_invite_codes(false, Some(&page[1]), 10).await.unwrap();
        assert_eq!(rest.len(), 1, "{n}");
        assert_eq!(s.all_invite_codes(true, None, 10).await.unwrap().len(), 3);
        s.disable_invite_codes(&["c1".into()], &[]).await.unwrap();
        assert!(s.invite_code("c1").await.unwrap().unwrap().disabled, "{n}");
        assert_eq!(s.invite_code("c2").await.unwrap().unwrap().available, 2);
        assert!(
            s.search_accounts("nobody", None, 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            !s.set_record_takedown("did:plc:x", "c", "r", Some("t"))
                .await
                .unwrap()
        );
        assert!(!s.set_blob_takedown("did:plc:x", "c", None).await.unwrap());
        assert!(s.invited_by("did:plc:x").await.unwrap().is_none());
    }
}

#[tokio::test]
async fn setup_state_on_both_backends() {
    for b in backends().await {
        let s = &b.store;
        assert!(s.setup_state().await.unwrap().is_none(), "{}", b.name);
        assert!(!s.setup_completed().await.unwrap());
        assert!(s.create_setup_token("h1").await.unwrap());
        assert!(
            !s.create_setup_token("h2").await.unwrap(),
            "{}: first wins",
            b.name
        );
        assert!(s.setup_token_matches("h1").await.unwrap());
        assert!(!s.setup_token_matches("h2").await.unwrap());

        s.add_setup_session("live", 60_000).await.unwrap();
        s.add_setup_session("old", -1).await.unwrap();
        assert!(s.setup_session_valid("live").await.unwrap(), "{}", b.name);
        assert!(
            !s.setup_session_valid("old").await.unwrap(),
            "{}: expired",
            b.name
        );
        assert!(s.save_setup_draft(r#"{"seeded":true}"#).await.unwrap());
        s.set_setup_probe("nonce").await.unwrap();
        s.record_setup_probe("https").await.unwrap();
        let row = s.setup_state().await.unwrap().unwrap();
        assert_eq!(
            (
                row.probe_nonce.as_deref(),
                row.probe_seen.as_deref(),
                row.draft_json.as_str()
            ),
            (Some("nonce"), Some("https"), r#"{"seeded":true}"#)
        );

        // A new token ends the old one's sessions.
        assert!(s.replace_setup_token("h3").await.unwrap());
        assert!(!s.setup_session_valid("live").await.unwrap(), "{}", b.name);
        s.add_setup_session("live2", 60_000).await.unwrap();

        // Completion is one conditional write, with the operator beside it.
        let mut tx = s.begin().await.unwrap();
        assert!(tx.complete_setup("pds.example.com", "setup").await.unwrap());
        assert!(tx.create_operator("alice", "hash", "owner").await.unwrap());
        assert!(!tx.create_operator("alice", "hash", "owner").await.unwrap());
        tx.commit().await.unwrap();
        let mut tx = s.begin().await.unwrap();
        assert!(
            !tx.complete_setup("other.example.com", "setup")
                .await
                .unwrap(),
            "{}: once",
            b.name
        );
        tx.rollback().await.unwrap();
        let row = s.setup_state().await.unwrap().unwrap();
        assert!(row.completed_at.is_some());
        assert_eq!(
            (row.hostname.as_deref(), row.draft_json.as_str()),
            (Some("pds.example.com"), "{}")
        );
        assert!(
            !s.setup_session_valid("live2").await.unwrap(),
            "{}: sessions end",
            b.name
        );
        assert!(!s.setup_token_matches("h3").await.unwrap());
        assert!(!s.save_setup_draft("{}").await.unwrap());
        assert!(!s.replace_setup_token("h4").await.unwrap());
        assert_eq!(s.operator("alice").await.unwrap().unwrap().role, "owner");
    }
}
