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
