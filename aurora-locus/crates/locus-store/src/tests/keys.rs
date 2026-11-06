//! Rolling-restart env apply: node ids, stale reports, lapsed leases and
//! concurrent re-checks.

use super::*;

/// How many times the environment set `key` (audit rows).
async fn env_applies(s: &Store, key: &str) -> usize {
    s.audit_entries(None, 1000)
        .await
        .unwrap()
        .iter()
        .filter(|e| e.actor == "environment" && e.target.as_deref() == Some(key))
        .count()
}

#[tokio::test]
async fn rolling_env_apply_follows_live_nodes_only() {
    const KEY: &str = "server.service_name";
    for b in backends().await {
        let s = &b.store;
        let n = b.name;
        let ttl = Duration::from_secs(30);
        let claim = |id: &'static str| async move {
            s.claim_slot(CLOCK_SLOT_FAMILY, 16, id, ttl)
                .await
                .unwrap()
                .unwrap()
                .1
        };
        let value = |v: &str| collect_env(&env_of(&[("PDS_SERVICE_NAME", v)])).0;
        let name = || async { s.load_snapshot().await.unwrap().text(KEY).to_owned() };
        let (v0, v1, v2) = (value("Zero"), value("One"), value("Two"));

        // Three nodes boot with v0: applied once, by the last to report.
        let a = claim("a").await;
        let b_ = claim("b").await;
        let c = claim("c").await;
        assert!(s.report_and_apply_env("a", &v0).await.unwrap().is_empty());
        assert!(s.report_and_apply_env("b", &v0).await.unwrap().is_empty());
        assert_eq!(s.report_and_apply_env("c", &v0).await.unwrap(), vec![KEY]);
        assert_eq!(env_applies(s, KEY).await, 1, "{n}");

        // A stopped node's stale report (it holds no slot) neither blocks
        // nor counts: a reporter without a live slot changes nothing.
        assert!(
            s.report_and_apply_env("ghost", &v1)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(name().await, "Zero", "{n}");

        // Rolling restart onto v1, each node coming back with a new id.
        s.release_lease(&a).await.unwrap();
        let _a2 = claim("a2").await;
        assert!(s.report_and_apply_env("a2", &v1).await.unwrap().is_empty());
        s.release_lease(&b_).await.unwrap();
        let _b2 = claim("b2").await;
        assert!(s.report_and_apply_env("b2", &v1).await.unwrap().is_empty());
        assert_eq!(name().await, "Zero", "{n}: nothing until the last restarts");
        s.release_lease(&c).await.unwrap();
        let c2 = claim("c2").await;
        assert_eq!(
            s.report_and_apply_env("c2", &v1).await.unwrap(),
            vec![KEY],
            "{n}: the old ids' reports don't block"
        );
        assert_eq!(name().await, "One");
        assert_eq!(env_applies(s, KEY).await, 2, "{n}");

        // Another restart with a new id and the same value: no re-apply,
        // even after a console change.
        s.set_setting(KEY, json!("Console"), "op").await.unwrap();
        s.release_lease(&c2).await.unwrap();
        let _c3 = claim("c3").await;
        assert!(s.report_and_apply_env("c3", &v1).await.unwrap().is_empty());
        assert_eq!(name().await, "Console", "{n}");
        assert_eq!(env_applies(s, KEY).await, 2, "{n}");

        // A node that stops without releasing its slot blocks a change
        // until its lease lapses; the next timer re-check then applies it.
        let short = s
            .claim_slot(CLOCK_SLOT_FAMILY, 16, "d", Duration::from_secs(1))
            .await
            .unwrap()
            .unwrap()
            .1;
        assert!(s.report_and_apply_env("d", &v1).await.unwrap().is_empty());
        for node in ["a2", "b2", "c3"] {
            assert!(s.report_and_apply_env(node, &v2).await.unwrap().is_empty());
        }
        assert_eq!(name().await, "Console", "{n}: d still live with v1");
        drop(short);
        tokio::time::sleep(Duration::from_millis(1500)).await;

        // Every node re-checks at once: applied (and audited) exactly once.
        let mut checks = Vec::new();
        for _ in 0..3 {
            for _ in 0..4 {
                let (s, v2) = (s.clone(), v2.clone());
                checks.push(tokio::spawn(async move { s.apply_env(&v2).await }));
            }
        }
        let mut applied = 0;
        for c in checks {
            applied += c.await.unwrap().unwrap().len();
        }
        assert_eq!(applied, 1, "{n}: the compare-and-set lets one apply");
        assert_eq!(name().await, "Two", "{n}");
        assert_eq!(env_applies(s, KEY).await, 3, "{n}");
    }
}

#[tokio::test]
async fn a_reserved_key_is_a_checked_secret_write() {
    use locus_core::keys::{MasterKey, SourceKind};
    for b in backends().await {
        let s = &b.store;
        let n = b.name;
        let key = MasterKey::generate(SourceKind::Env);
        let other = MasterKey::generate(SourceKind::Env);
        // The first secret write records the install's key.
        s.reserve_key("did:plc:first", "did:key:zA", b"first", &key, 60_000)
            .await
            .unwrap();
        assert!(!s.key_store_empty().await.unwrap(), "{n}");
        // Another key: refused, and nothing sealed under it is stored.
        assert!(
            s.reserve_key("did:plc:other", "did:key:zB", b"other", &other, 60_000)
                .await
                .is_err(),
            "{n}"
        );
        assert!(
            s.reserved_key("did:plc:other", &key)
                .await
                .unwrap()
                .is_none(),
            "{n}"
        );
        assert!(
            s.reserve_key("did:plc:first", "did:key:zC", b"swap", &other, 60_000)
                .await
                .is_err(),
            "{n}"
        );
        let (did_key, private) = s
            .reserved_key("did:plc:first", &key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (did_key.as_str(), private.as_slice()),
            ("did:key:zA", &b"first"[..]),
            "{n}"
        );
    }
}
