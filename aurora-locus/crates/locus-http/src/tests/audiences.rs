//! The audience copy: listings that stop early, concurrent listings,
//! duplicate-free creates, re-pointing, membership readiness, deletes with
//! the tier off and outage-created audiences, against a stub prism that
//! can answer late or not at all.

use std::time::{Duration, Instant};

use crate::audience::{Prism, Trigger, refresh};

use super::*;

const LIST: &str = "app.nearhorizon.private.listAudiences";
const CREATE: &str = "app.nearhorizon.private.createAudience";

/// One scripted answer, sent after `delay_ms`.
#[derive(Clone)]
struct Answer {
    status: u16,
    body: Value,
    delay_ms: u64,
}

/// A stub prism: answers by method (`nsid#cursor` for a listing page),
/// chosen when the request arrives and sent after its delay. A listing
/// scripted as `{"$echo": n, "did": d, "tag": t}` lists every create seen
/// so far, n times each (prism made it, maybe more than once), under rkeys
/// starting `3m6<t>`. `did.json` answers.
#[derive(Clone, Default)]
struct Stub {
    url: String,
    answers: Arc<Mutex<HashMap<String, Answer>>>,
    seen: SeenLog,
}

impl Stub {
    fn set(&self, key: &str, status: u16, body: Value) {
        self.set_slow(key, status, body, 0);
    }

    fn set_slow(&self, key: &str, status: u16, body: Value, delay_ms: u64) {
        self.answers.lock().unwrap().insert(
            key.into(),
            Answer {
                status,
                body,
                delay_ms,
            },
        );
    }

    fn calls(&self, nsid: &str) -> usize {
        let prefix = format!("/xrpc/{nsid}");
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.uri.starts_with(&prefix))
            .count()
    }

    fn bodies(&self, nsid: &str) -> Vec<Value> {
        let prefix = format!("/xrpc/{nsid}");
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.uri.starts_with(&prefix))
            .map(|s| serde_json::from_slice(&s.body).unwrap_or(Value::Null))
            .collect()
    }

    async fn answer(&self, nsid: &str, req: Request<Body>) -> axum::response::Response {
        let (parts, body) = req.into_parts();
        let body = body.collect().await.unwrap().to_bytes().to_vec();
        let cursor = parts.uri.query().and_then(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .find(|(k, _)| k == "cursor")
                .map(|(_, v)| v.into_owned())
        });
        let entry = {
            let a = self.answers.lock().unwrap();
            cursor
                .and_then(|c| a.get(&format!("{nsid}#{c}")).cloned())
                .or_else(|| a.get(nsid).cloned())
        };
        let Some(mut entry) = entry else {
            self.push(&parts, body);
            return (
                StatusCode::NOT_IMPLEMENTED,
                axum::Json(json!({"error": "MethodNotImplemented"})),
            )
                .into_response();
        };
        if let Some(n) = entry.body.get("$echo").and_then(Value::as_u64) {
            let did = entry.body["did"].as_str().unwrap_or_default().to_owned();
            let tag = entry.body["tag"].as_str().unwrap_or("echo").to_owned();
            let mut audiences = Vec::new();
            for (c, sent) in self.bodies(CREATE).iter().enumerate() {
                for i in 0..n {
                    audiences.push(json!({
                        "uri": format!("at://{did}/{AUD}/3m6{tag}{c:03}{i:03}"),
                        "cid": format!("{tag}{c}{i}"),
                        "record": sent["record"],
                    }));
                }
            }
            entry.body = json!({"audiences": audiences});
        }
        self.push(&parts, body);
        tokio::time::sleep(Duration::from_millis(entry.delay_ms)).await;
        (
            StatusCode::from_u16(entry.status).unwrap(),
            axum::Json(entry.body),
        )
            .into_response()
    }

    fn push(&self, parts: &axum::http::request::Parts, body: Vec<u8>) {
        self.seen.lock().unwrap().push(Seen {
            method: parts.method.to_string(),
            uri: parts.uri.to_string(),
            headers: parts.headers.clone(),
            body,
        });
    }
}

async fn stub_prism() -> Stub {
    let stub = Stub::default();
    let st = stub.clone();
    let app = Router::new()
        .route(
            "/.well-known/did.json",
            get(|| async { axum::Json(json!({"id": "did:web:prism.test"})) }),
        )
        .route(
            "/xrpc/{nsid}",
            axum::routing::any(move |Path(nsid): Path<String>, req: Request<Body>| {
                let st = st.clone();
                async move { st.answer(&nsid, req).await }
            }),
        );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    Stub { url, ..stub }
}

impl Harness {
    /// The stub as the configured prism, admitting every caller.
    async fn with_stub(&self) -> Stub {
        let stub = stub_prism().await;
        for (k, v) in [
            ("appview.url", stub.url.as_str()),
            ("appview.did", "did:web:prism.test"),
        ] {
            self.state.settings.set(k, json!(v), "test").await.unwrap();
        }
        stub.set(
            "app.nearhorizon.actor.getMembership",
            200,
            json!({"status": "member", "features": [], "validForSecs": 3600,
                   "permissions": {"privateIndexed": true, "privateRead": true}}),
        );
        stub
    }

    async fn set(&self, key: &str, v: Value) {
        self.state.settings.set(key, v, "test").await.unwrap();
    }

    async fn rkeys(&self, did: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .state
            .store
            .kr_audiences(did)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.rkey)
            .collect();
        v.sort();
        v
    }

    fn pool(&self) -> &sqlx::SqlitePool {
        let locus_store::Store::Sqlite(p) = &self.state.store else {
            panic!("the harness is SQLite");
        };
        p
    }
}

fn far() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

const A: &str = "3m6aaaaaaaaaa";
const B: &str = "3m6bbbbbbbbbb";
const C: &str = "3m6cccccccccc";
const D: &str = "3m6dddddddddd";

// ---------------------------------------------------------------- listings

#[tokio::test]
async fn a_listing_that_stops_early_deletes_nothing() {
    let h = harness(false).await;
    let (did, _) = h.user("lia.pds.example.com").await;
    let stub = h.with_stub().await;
    h.add_audience(&did, A).await;
    h.add_audience(&did, B).await;
    let page_a = prism_audience(&did, A, "3m6prismcid22", &[]);

    // Page 1 lists A only; page 2 fails (5xx, then 429): B survives both.
    stub.set(LIST, 200, json!({"cursor": "p2", "audiences": [page_a]}));
    for (status, error) in [(500, "InternalServerError"), (429, "RateLimitExceeded")] {
        stub.set(&format!("{LIST}#p2"), status, json!({"error": error}));
        let r = refresh(&h.state, &did, Trigger::Background, None, far())
            .await
            .unwrap();
        assert!(!r.complete && r.error.is_some(), "{r:?}");
        assert_eq!(h.rkeys(&did).await, [A, B], "a failed page {status}");
    }

    // A store-miss refresh stops at the page holding the URI: C is adopted,
    // B (not on that page) survives.
    stub.set(
        LIST,
        200,
        json!({"cursor": "p2", "audiences": [page_a, prism_audience(&did, C, "v1", &[])]}),
    );
    let r = refresh(&h.state, &did, Trigger::Background, Some(C), far())
        .await
        .unwrap();
    assert!(r.found && !r.complete && r.pages == 1, "{r:?}");
    assert_eq!(h.rkeys(&did).await, [A, B, C]);

    // A last page answering past the deadline applies nothing: D isn't
    // adopted and B and C aren't deleted.
    stub.set_slow(
        LIST,
        200,
        json!({"audiences": [page_a, prism_audience(&did, D, "v1", &[])]}),
        600,
    );
    let soon = Instant::now() + Duration::from_millis(200);
    let r = refresh(&h.state, &did, Trigger::Background, None, soon)
        .await
        .unwrap();
    assert_eq!(
        (r.complete, r.error.as_deref()),
        (false, Some("deadline")),
        "{r:?}"
    );
    assert_eq!(h.rkeys(&did).await, [A, B, C], "past the deadline");

    // A complete listing deletes what it lacks, with tombstones.
    stub.set(LIST, 200, json!({"audiences": [page_a]}));
    let r = refresh(&h.state, &did, Trigger::Background, None, far())
        .await
        .unwrap();
    assert!(r.complete && r.error.is_none(), "{r:?}");
    assert_eq!(h.rkeys(&did).await, [A]);
    for rkey in [B, C] {
        assert!(
            h.state
                .store
                .audience_tombstone(&did, rkey)
                .await
                .unwrap()
                .is_some()
        );
    }

    // Tombstones outlive the listing deadline: with a 15 min deadline a
    // 12 min old tombstone is kept; with the default 5 min it goes (10 min).
    sqlx::query("UPDATE kr_audience_tombstone SET deleted_at = deleted_at - 720000 WHERE rkey = ?")
        .bind(B)
        .execute(h.pool())
        .await
        .unwrap();
    h.set("kryphocron.audience_listing_deadline_s", json!(900))
        .await;
    let cancel = tokio_util::sync::CancellationToken::new();
    crate::audience_sync::poll_once(&h.state, &cancel)
        .await
        .unwrap();
    let kept = |rkey: &'static str| {
        let h = &h;
        let did = did.clone();
        async move {
            h.state
                .store
                .audience_tombstone(&did, rkey)
                .await
                .unwrap()
                .is_some()
        }
    };
    assert!(kept(B).await, "younger than the deadline");
    h.set("kryphocron.audience_listing_deadline_s", json!(300))
        .await;
    crate::audience_sync::poll_once(&h.state, &cancel)
        .await
        .unwrap();
    assert!(!kept(B).await, "older than 10 min and the deadline");
    assert!(kept(C).await, "a fresh one stays");
}

#[tokio::test]
async fn concurrent_listings_keep_their_own_snapshots_and_tombstones_win() {
    let h = harness(false).await;
    let (did, access) = h.user("max.pds.example.com").await;
    let stub = h.with_stub().await;
    // C existed before both listings.
    h.add_audience(&did, C).await;

    // Listing 1 starts; its only page is slow and lists A and C.
    stub.set_slow(
        LIST,
        200,
        json!({"audiences": [prism_audience(&did, A, "v1", &[]), prism_audience(&did, C, "other", &[])]}),
        2500,
    );
    let st = Arc::clone(&h.state);
    let d = did.clone();
    let first = tokio::spawn(async move {
        refresh(&st, &d, Trigger::Background, None, far())
            .await
            .unwrap()
    });
    for _ in 0..100 {
        if stub.calls(LIST) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(stub.calls(LIST), 1);

    // Listing 2, quick and complete: adopts A and B, deletes C.
    stub.set(
        LIST,
        200,
        json!({"audiences": [prism_audience(&did, A, "v1", &[]), prism_audience(&did, B, "v1", &[])]}),
    );
    let r = refresh(&h.state, &did, Trigger::Background, None, far())
        .await
        .unwrap();
    assert!(r.complete, "{r:?}");
    assert_eq!(h.rkeys(&did).await, [A, B]);
    // The owner deletes A.
    stub.set("app.nearhorizon.private.deleteAudience", 200, json!({}));
    let (st, v) = h
        .post(
            "tools.kryphocron.policy.deleteAudience",
            Some(&access),
            json!({"repo": did, "rkey": A}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");

    // Listing 1's page arrives: A (deleted after the page was made) and C
    // (removed by listing 2) aren't brought back, and B, which listing 1
    // never saw before it started, isn't deleted by its complete listing.
    let r = first.await.unwrap();
    assert!(r.complete && r.error.is_none(), "{r:?}");
    assert_eq!(h.rkeys(&did).await, [B]);
}

// ---------------------------------------------------------------- duplicates

#[tokio::test]
async fn ambiguous_creates_link_only_a_single_match() {
    let h = harness(false).await;
    let (did, access) = h.user("noa.pds.example.com").await;
    let stub = h.with_stub().await;
    h.set("kryphocron.prism_request_timeout_ms", json!(1000))
        .await;
    // Prism takes the create but answers too late.
    stub.set_slow(
        CREATE,
        200,
        json!({"uri": format!("at://{did}/{AUD}/3m6late0000000"), "cid": "x"}),
        3000,
    );
    let create = || async {
        h.post(
            "tools.kryphocron.policy.manageAudience",
            Some(&access),
            json!({"repo": did, "record": {"members": ["did:plc:friend"]}}),
        )
        .await
    };
    let open = |h: &Harness| {
        let did = did.clone();
        let st = Arc::clone(&h.state);
        async move { st.store.open_attempts(&did).await.unwrap() }
    };

    // The listing after the timeout has no match: retryable, the attempt
    // kept open (unknown), never sent again.
    stub.set(LIST, 200, json!({"$echo": 0, "did": did, "tag": "none"}));
    let (st, v) = create().await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{v}");
    let attempts = open(&h).await;
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].outcome, "unknown");
    assert_eq!(stub.calls(CREATE), 1);
    assert!(!h.state.kr.prism.is_down(), "the probe answered");

    // Two matches (prism made it twice?): none linked, still retryable.
    stub.seen.lock().unwrap().clear();
    stub.set(LIST, 200, json!({"$echo": 2, "did": did, "tag": "twin"}));
    let (st, v) = create().await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE, "{v}");
    let two = open(&h).await;
    assert_eq!(two.len(), 2, "neither attempt linked");
    for a in &two {
        assert!(
            h.state
                .store
                .attempt_row(&did, &a.id)
                .await
                .unwrap()
                .is_none()
        );
    }

    // Exactly one match: linked to the attempt and returned.
    stub.seen.lock().unwrap().clear();
    stub.set(LIST, 200, json!({"$echo": 1, "did": did, "tag": "once"}));
    let (st, v) = create().await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["uri"], json!(format!("at://{did}/{AUD}/3m6once000000")));
    assert_eq!(open(&h).await.len(), 2, "the earlier two stay open");
}

#[tokio::test]
async fn sending_attempts_and_earlier_adoptions_match() {
    let h = harness(false).await;
    let (did, _) = h.user("oli.pds.example.com").await;
    let stub = h.with_stub().await;
    h.set("kryphocron.prism_request_timeout_ms", json!(1000))
        .await;
    let stamp = "2024-05-01T10:00:00.000Z";
    let sent = json!({"$type": AUD, "members": ["did:plc:a"], "createdAt": stamp, "updatedAt": stamp});
    let mut entry = prism_audience(&did, A, "v1", &["did:plc:a"]);
    entry["record"]["createdAt"] = json!(stamp);
    entry["record"]["updatedAt"] = json!(stamp);
    stub.set(LIST, 200, json!({"audiences": [entry]}));

    // A `sending` attempt younger than the request timeout may still be in
    // flight: the refresh adopts the row but doesn't link it.
    h.state
        .store
        .create_audience_attempt("att1", &did, &sent.to_string(), stamp, stamp, None)
        .await
        .unwrap();
    refresh(&h.state, &did, Trigger::Background, None, far())
        .await
        .unwrap();
    assert!(h.rkeys(&did).await.contains(&A.to_owned()));
    assert!(
        h.state
            .store
            .attempt_row(&did, "att1")
            .await
            .unwrap()
            .is_none(),
        "in flight"
    );

    // Older than the timeout it's a crash leftover, matched like `unknown`,
    // and it matches the row the earlier refresh adopted.
    sqlx::query("UPDATE kr_audience_attempt SET created_at = created_at - 1500 WHERE id = 'att1'")
        .execute(h.pool())
        .await
        .unwrap();
    refresh(&h.state, &did, Trigger::Background, None, far())
        .await
        .unwrap();
    assert_eq!(
        h.state
            .store
            .attempt_row(&did, "att1")
            .await
            .unwrap()
            .map(|r| r.rkey),
        Some(A.to_owned())
    );
}

/// An outage-made local audience and prism back up.
async fn local_audience(h: &Harness, did: &str, access: &str) -> String {
    h.state.kr.prism.set(true);
    let (st, v) = h
        .post(
            "tools.kryphocron.policy.manageAudience",
            Some(access),
            json!({"repo": did, "record": {"members": ["did:plc:friend"]}}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    h.state.kr.prism.set(false);
    v["uri"].as_str().unwrap().rsplit('/').next().unwrap().to_owned()
}

#[tokio::test]
async fn a_local_audience_is_created_in_prism_once() {
    let h = harness(false).await;
    let (did, access) = h.user("pia.pds.example.com").await;
    let stub = h.with_stub().await;
    h.set("kryphocron.prism_request_timeout_ms", json!(1000))
        .await;
    // A first write while prism is up (a membership answer to fall back on).
    let mine = h.add_audience(&did, A).await;
    let (st, _) = h
        .private_post_as(&did, &access, private_post("x", &mine))
        .await;
    assert_eq!(st, StatusCode::OK);
    let local = local_audience(&h, &did, &access).await;
    let row = h.state.store.kr_audience(&did, &local).await.unwrap().unwrap();
    let listing = |started_at| crate::audience::Refreshed {
        complete: true,
        started_at,
        ..Default::default()
    };

    // The unique open attempt: a second open attempt for the same local
    // audience can't be written, so two nodes never both send.
    h.state
        .store
        .create_audience_attempt("n1", &did, "{}", "", "", Some((&local, row.row_version)))
        .await
        .unwrap();
    assert!(
        h.state
            .store
            .create_audience_attempt("n2", &did, "{}", "", "", Some((&local, row.row_version)))
            .await
            .is_err(),
        "one open attempt per local audience"
    );
    // While it's open the sync doesn't send.
    let now = h.state.store.db_now().await.unwrap();
    crate::audience_sync::sync_account(&h.state, &did, &listing(now))
        .await
        .unwrap();
    assert_eq!(stub.calls(CREATE), 0, "another node's create is open");

    // Expired, but the listing started before it expired: still not sent.
    sqlx::query("UPDATE kr_audience_attempt SET outcome = 'expired', expires_at = ? WHERE id = 'n1'")
        .bind(now - 1000)
        .execute(h.pool())
        .await
        .unwrap();
    crate::audience_sync::sync_account(&h.state, &did, &listing(now - 2000))
        .await
        .unwrap();
    assert_eq!(stub.calls(CREATE), 0, "a listing older than the expiry");
    // A complete listing that started after the expiry lacks a match: sent.
    // Prism takes it, and the node dies before the mapping.
    stub.set(
        CREATE,
        200,
        json!({"uri": format!("at://{did}/{AUD}/3m6pmappedmap"), "cid": "c1"}),
    );
    crate::faults::set(
        &h.state,
        "audience.created",
        Some(Arc::new(|| {
            Box::pin(async { Err(locus_core::CoreError::Storage("killed".into())) })
        })),
    );
    assert!(
        crate::audience_sync::sync_account(&h.state, &did, &listing(now))
            .await
            .is_err()
    );
    crate::faults::set(&h.state, "audience.created", None);
    assert_eq!(stub.calls(CREATE), 1);
    assert!(h.state.store.audience_alias(&did, &local).await.unwrap().is_none());

    // After the restart, a refresh lists prism's audience; the attempt
    // (a `sending` leftover once the timeout passes) matches it and maps.
    let mut made = prism_audience(&did, "3m6pmappedmap", "c1", &["did:plc:friend"]);
    made["record"] = stub.bodies(CREATE)[0]["record"].clone();
    stub.set(
        LIST,
        200,
        json!({"audiences": [prism_audience(&did, A, "3m6prismcid22", &[]), made]}),
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    h.state.store.poll_queued_audiences_now().await.unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    crate::audience_sync::poll_once(&h.state, &cancel)
        .await
        .unwrap();
    assert_eq!(
        h.state
            .store
            .audience_alias(&did, &local)
            .await
            .unwrap()
            .as_deref(),
        Some("3m6pmappedmap"),
        "mapped by the matching"
    );
    assert!(h.state.store.kr_audience(&did, &local).await.unwrap().is_none());
    h.state.store.set_next_poll(&did, 0).await.unwrap();
    crate::audience_sync::poll_once(&h.state, &cancel)
        .await
        .unwrap();
    assert_eq!(stub.calls(CREATE), 1, "never created twice");
}

// ---------------------------------------------------------------- re-pointing

/// Changes a private post outside the re-point job (a client edit).
async fn touch_post(s: &Arc<AppState>, did: &str, rkey: &str) {
    use proto_blue_lex_data::LexValue;
    let (cid, _) = s
        .store
        .record_entry(did, POST_PRIVATE, rkey)
        .await
        .unwrap()
        .unwrap();
    let block = s.store.block(did, &cid).await.unwrap().unwrap();
    let Ok(LexValue::Map(mut map)) = proto_blue_lex_cbor::decode(&block) else {
        panic!("a stored post decodes");
    };
    map.insert(
        "createdAt".into(),
        LexValue::String("2024-02-02T00:00:00.000Z".into()),
    );
    let rec = locus_repo::records::prepare_record(
        POST_PRIVATE,
        rkey,
        LexValue::Map(map),
        None,
        usize::MAX,
    )
    .unwrap();
    let account = s.store.account_by_did(did).await.unwrap().unwrap();
    let w = crate::repo::prepared(
        locus_repo::Action::Update,
        POST_PRIVATE,
        rkey,
        Some(&rec),
        None,
    );
    crate::repo::commit(s, &account, vec![w], None, false)
        .await
        .unwrap();
}

#[tokio::test]
async fn re_pointing_commits_once_per_repo_batch_and_waits_its_turn() {
    let h = harness(false).await;
    let (owner, owner_access) = h.user("quin.pds.example.com").await;
    let (member, member_access) = h.user("rae.pds.example.com").await;
    let stub = h.with_stub().await;
    h.set("kryphocron.repoint_max_commits_per_minute", json!(10_000))
        .await;
    // First writes while prism is up: membership answers to fall back on.
    for (did, access, rkey) in [(&owner, &owner_access, A), (&member, &member_access, B)] {
        let mine = h.add_audience(did, rkey).await;
        let (st, v) = h
            .private_post_as(did, access, private_post("x", &mine))
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
    }

    // The outage: a local audience with the member, 250 posts under it and
    // the member's reply to the first.
    h.state.kr.prism.set(true);
    let (st, v) = h
        .post(
            "tools.kryphocron.policy.manageAudience",
            Some(&owner_access),
            json!({"repo": owner, "record": {"members": [member]}}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let local_uri = v["uri"].as_str().unwrap().to_owned();
    let local = local_uri.rsplit('/').next().unwrap().to_owned();
    let mut rkeys = Vec::new();
    for batch in 0..2 {
        let writes: Vec<Value> = (0..125)
            .map(|i| {
                let rkey = h.state.repo.tids().next();
                rkeys.push(rkey.clone());
                json!({"$type": "com.atproto.repo.applyWrites#create", "collection": POST_PRIVATE,
                       "rkey": rkey, "value": private_post(&format!("post {batch}/{i}"), &local_uri)})
            })
            .collect();
        let (st, v) = h
            .post(
                "com.atproto.repo.applyWrites",
                Some(&owner_access),
                json!({"repo": owner, "writes": writes}),
            )
            .await;
        assert_eq!(st, StatusCode::OK, "{v}");
    }
    let parent_cid = h
        .state
        .store
        .record_entry(&owner, POST_PRIVATE, &rkeys[0])
        .await
        .unwrap()
        .unwrap()
        .0;
    let parent = json!({"uri": format!("at://{owner}/{POST_PRIVATE}/{}", rkeys[0]), "cid": parent_cid});
    let (st, reply) = h
        .reply_as(&member, &member_access, &parent, &local_uri)
        .await;
    assert_eq!(
        st,
        StatusCode::OK,
        "a member's reply under the local URI: {reply}"
    );
    let reply_rkey = reply["uri"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_owned();
    let reply_before = h.record_value(&member, POST_PRIVATE, &reply_rkey).await;
    let before = h.record_value(&owner, POST_PRIVATE, &rkeys[7]).await;

    // Prism is back: the local audience is created there and mapped.
    h.state.kr.prism.set(false);
    let prism_uri = format!("at://{owner}/{AUD}/3m6pmappedmap");
    stub.set(
        LIST,
        200,
        json!({"audiences": [prism_audience(&owner, A, "3m6prismcid22", &[])]}),
    );
    stub.set(CREATE, 200, json!({"uri": prism_uri, "cid": "c1"}));
    h.state.store.poll_queued_audiences_now().await.unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    crate::audience_sync::poll_once(&h.state, &cancel)
        .await
        .unwrap();
    assert_eq!(
        h.state
            .store
            .audience_alias(&owner, &local)
            .await
            .unwrap()
            .as_deref(),
        Some("3m6pmappedmap")
    );
    // Since then the owner removed the member from the audience.
    sqlx::query("UPDATE kr_audience SET members_json = '[]' WHERE rkey = '3m6pmappedmap'")
        .execute(h.pool())
        .await
        .unwrap();
    let commits = |did: &str| {
        let (h, did) = (&h, did.to_owned());
        async move {
            h.outbox_kinds(&did)
                .await
                .iter()
                .filter(|k| *k == "commit")
                .count()
        }
    };
    let repoint = || crate::audience_sync::repoint_pass(&h.state, &cancel);

    // The owner's repo is write-blocked and the member deactivated: nothing
    // moves.
    sqlx::query("UPDATE account SET write_block_op = 'test' WHERE did = ?")
        .bind(&owner)
        .execute(h.pool())
        .await
        .unwrap();
    let (st, v) = h
        .post(
            "com.atproto.server.deactivateAccount",
            Some(&member_access),
            json!({}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let owner_commits = commits(&owner).await;
    assert_eq!(repoint().await.unwrap(), 0);
    assert_eq!(commits(&owner).await, owner_commits);

    // Unblocked, but a post changes between the job's read and its commit:
    // the CAS-guarded commit is refused and the repo waits for the next run.
    sqlx::query("UPDATE account SET write_block_op = NULL WHERE did = ?")
        .bind(&owner)
        .execute(h.pool())
        .await
        .unwrap();
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let (st, o, r) = (Arc::clone(&h.state), owner.clone(), rkeys[0].clone());
        let fired = Arc::clone(&fired);
        crate::faults::set(
            &h.state,
            "repoint.commit",
            Some(Arc::new(move || {
                let (st, o, r) = (Arc::clone(&st), o.clone(), r.clone());
                let first = !fired.swap(true, std::sync::atomic::Ordering::SeqCst);
                Box::pin(async move {
                    if first {
                        touch_post(&st, &o, &r).await;
                    }
                    Ok(())
                })
            })),
        );
    }
    assert_eq!(repoint().await.unwrap(), 0, "skipped");
    crate::faults::set(&h.state, "repoint.commit", None);
    assert!(fired.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(
        commits(&owner).await,
        owner_commits + 1,
        "only the client's edit"
    );
    assert_eq!(
        h.record_value(&owner, POST_PRIVATE, &rkeys[1]).await["audienceList"],
        json!(local_uri)
    );

    // The next run re-points all 250 (the changed one too) in two commits,
    // changing only `audienceList`.
    assert_eq!(repoint().await.unwrap(), 250);
    assert_eq!(
        commits(&owner).await,
        owner_commits + 3,
        "two re-point commits"
    );
    for rkey in [&rkeys[0], &rkeys[249]] {
        assert_eq!(
            h.record_value(&owner, POST_PRIVATE, rkey).await["audienceList"],
            json!(prism_uri)
        );
    }
    let mut after = h.record_value(&owner, POST_PRIVATE, &rkeys[7]).await;
    let mut was = before.clone();
    assert_eq!(after["audienceList"], json!(prism_uri));
    after.as_object_mut().unwrap().remove("audienceList");
    was.as_object_mut().unwrap().remove("audienceList");
    assert_eq!(after, was, "nothing else changed");
    // (Read from the store: the deactivated repo isn't served.)
    let (cid, _) = h
        .state
        .store
        .record_entry(&member, POST_PRIVATE, &reply_rkey)
        .await
        .unwrap()
        .unwrap();
    let block = h.state.store.block(&member, &cid).await.unwrap().unwrap();
    let Ok(proto_blue_lex_data::LexValue::Map(map)) = proto_blue_lex_cbor::decode(&block) else {
        panic!("a stored reply decodes");
    };
    assert_eq!(
        map["audienceList"].as_str(),
        Some(local_uri.as_str()),
        "the deactivated member's reply waits"
    );

    // Reactivated: the reply is re-pointed though its author was removed
    // from the audience meanwhile (no re-check, no re-encode).
    let (st, v) = h
        .post(
            "com.atproto.server.activateAccount",
            Some(&member_access),
            json!({}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(repoint().await.unwrap(), 1);
    let reply_after = h.record_value(&member, POST_PRIVATE, &reply_rkey).await;
    assert_eq!(reply_after["audienceList"], json!(prism_uri));
    for k in ["encodedContent", "encodedContentGeneration", "reply"] {
        assert_eq!(reply_after[k], reply_before[k], "{k}");
    }
}

// ---------------------------------------------------------------- readiness

/// A fresh permission row for `did` from prism, checked `age_ms` ago.
fn perm(
    did: &str,
    source: &str,
    status: &str,
    read: bool,
    age_ms: i64,
) -> locus_store::kryphocron::PermissionRow {
    let now = locus_core::time::now_ms();
    locus_store::kryphocron::PermissionRow {
        did: did.into(),
        source: source.into(),
        prism_did: "did:web:prism.test".into(),
        status: status.into(),
        private_indexed: true,
        private_read: read,
        expires_at: now + 3_600_000,
        checked_at: now - age_ms,
    }
}

#[tokio::test]
async fn readiness_takes_the_newer_row_and_failed_probes_change_nothing() {
    let h = harness(false).await;
    let (owner, _) = h.user("sal.pds.example.com").await;
    let (member, member_access) = h.user("tam.pds.example.com").await;
    let stub = h.with_stub().await;
    let audience = h.add_audience_with(&owner, A, "list", &[&member]).await;
    let posts = vec![format!("at://{owner}/{POST_PRIVATE}/3m6ppppppppppp")];
    let check = || crate::membership::is_member(&h.state, &member, &audience, &posts);
    let row = |source: &'static str| {
        let (h, member) = (&h, member.clone());
        async move { h.state.store.kr_permission(&member, source).await.unwrap() }
    };
    let put = |r: locus_store::kryphocron::PermissionRow| {
        let h = &h;
        async move { h.state.store.put_kr_permission(&r).await.unwrap() }
    };
    let posts_answer = |status: u16, error: &str| {
        stub.set(
            "app.nearhorizon.private.getPosts",
            status,
            json!({"error": error}),
        );
    };

    // The newer of the two rows decides.
    put(perm(&member, "membership", "member", true, 2000)).await;
    put(perm(&member, "probe", "notReady", false, 1000)).await;
    assert!(!check().await.unwrap(), "the newer probe row says not ready");
    put(perm(&member, "probe", "notReady", false, 3000)).await;
    assert!(check().await.unwrap(), "the newer membership row says ready");

    // With no rows the probe asks prism; a 429 or a 5xx writes no row.
    h.state.store.delete_kr_permissions(&member).await.unwrap();
    for (status, error) in [(429, "RateLimitExceeded"), (502, "UpstreamFailure")] {
        posts_answer(status, error);
        assert!(!check().await.unwrap(), "{status}");
        assert!(
            row("probe").await.is_none() && row("membership").await.is_none(),
            "{status}"
        );
    }
    tokio::time::sleep(Duration::from_millis(1100)).await;

    // NotReady and Reindexing: indexed but not readable.
    for (error, status) in [("NotReady", "notReady"), ("Reindexing", "reindexing")] {
        h.state.store.delete_kr_permissions(&member).await.unwrap();
        posts_answer(400, error);
        assert!(!check().await.unwrap(), "{error}");
        let r = row("probe").await.expect(error);
        assert_eq!(
            (r.status.as_str(), r.private_indexed, r.private_read),
            (status, true, false)
        );
        assert!(r.expires_at - r.checked_at >= 5_000);
    }

    // NotEnrolled: a 5 s probe row, which doesn't block the member's own
    // write (the write gate asks getMembership).
    h.state.store.delete_kr_permissions(&member).await.unwrap();
    posts_answer(400, "NotEnrolled");
    assert!(!check().await.unwrap());
    let r = row("probe").await.expect("notEnrolled");
    assert_eq!(
        (r.status.as_str(), r.private_read, r.expires_at - r.checked_at),
        ("notEnrolled", false, 5_000)
    );
    let mine = h.add_audience(&member, B).await;
    let (st, v) = h
        .private_post_as(&member, &member_access, private_post("x", &mine))
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
}

#[tokio::test]
async fn takedown_and_not_enrolled_answers_rewrite_both_rows() {
    let h = harness(false).await;
    let (did, access) = h.user("uma.pds.example.com").await;
    let stub = h.with_stub().await;
    let mine = h.add_audience(&did, A).await;
    let (st, _) = h
        .private_post_as(&did, &access, private_post("x", &mine))
        .await;
    assert_eq!(st, StatusCode::OK);
    let rows = || {
        let (h, did) = (&h, did.clone());
        async move {
            let mut v = Vec::new();
            for source in ["membership", "probe"] {
                v.push(h.state.store.kr_permission(&did, source).await.unwrap());
            }
            v
        }
    };
    let both = || {
        let (h, did) = (&h, did.clone());
        async move {
            for source in ["membership", "probe"] {
                h.state
                    .store
                    .put_kr_permission(&perm(&did, source, "member", true, 0))
                    .await
                    .unwrap();
            }
        }
    };

    // AccountTakedown from an audience call: both rows, both flags false,
    // and the write gate refuses.
    stub.set(LIST, 400, json!({"error": "AccountTakedown"}));
    refresh(&h.state, &did, Trigger::Background, None, far())
        .await
        .unwrap();
    for r in rows().await {
        let r = r.expect("takedown row");
        assert_eq!(
            (r.status.as_str(), r.private_indexed, r.private_read),
            ("takedown", false, false)
        );
    }
    let (st, _) = h
        .private_post_as(&did, &access, private_post("x", &mine))
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // An audience call's NotEnrolled purges both rows and the copy.
    stub.set(LIST, 400, json!({"error": "NotEnrolled"}));
    both().await;
    refresh(&h.state, &did, Trigger::Background, None, far())
        .await
        .unwrap();
    assert!(rows().await.iter().all(Option::is_none));
    assert!(h.rkeys(&did).await.is_empty());

    // So does the sync rule's, for a queued edit pushed to prism.
    h.add_audience(&did, A).await;
    both().await;
    h.state.kr.prism.set(true);
    let (st, v) = h
        .post(
            "tools.kryphocron.policy.manageAudience",
            Some(&access),
            json!({"repo": did, "rkey": A, "record": {"members": ["did:plc:x"]}}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    h.state.kr.prism.set(false);
    stub.set(
        LIST,
        200,
        json!({"audiences": [prism_audience(&did, A, "3m6prismcid22", &[])]}),
    );
    stub.set(
        "app.nearhorizon.private.updateAudience",
        400,
        json!({"error": "NotEnrolled"}),
    );
    h.state.store.poll_queued_audiences_now().await.unwrap();
    crate::audience_sync::poll_once(&h.state, &tokio_util::sync::CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(stub.calls("app.nearhorizon.private.updateAudience"), 1);
    assert!(rows().await.iter().all(Option::is_none));
    assert!(h.rkeys(&did).await.is_empty());
}

#[tokio::test]
async fn another_prism_did_discards_the_copy_aliases_and_both_rows() {
    let h = harness(false).await;
    let (did, access) = h.user("val.pds.example.com").await;
    let stub = h.with_stub().await;
    let mine = h.add_audience(&did, A).await;
    let (st, _) = h
        .private_post_as(&did, &access, private_post("x", &mine))
        .await;
    assert_eq!(st, StatusCode::OK);
    let local = local_audience(&h, &did, &access).await;
    h.state
        .store
        .put_audience_alias(&did, "3m6oldlocalkey", A)
        .await
        .unwrap();
    h.state
        .store
        .put_kr_permission(&perm(&did, "probe", "member", true, 0))
        .await
        .unwrap();

    // The same stub under another DID.
    h.set("appview.did", json!("did:web:prism2.test")).await;
    stub.set(LIST, 200, json!({"audiences": []}));
    refresh(&h.state, &did, Trigger::Background, None, far())
        .await
        .unwrap();
    assert!(
        h.state
            .store
            .audience_versions(&did)
            .await
            .unwrap()
            .is_empty(),
        "the local {local} too"
    );
    assert!(h.state.store.audience_aliases(&did).await.unwrap().is_empty());
    for source in ["membership", "probe"] {
        assert!(
            h.state
                .store
                .kr_permission(&did, source)
                .await
                .unwrap()
                .is_none(),
            "{source}"
        );
    }
}

// ---------------------------------------------------------------- deletes with the tier off

#[tokio::test]
async fn deletes_succeed_with_the_tier_off_while_writes_are_refused() {
    let h = harness(false).await;
    let (did, access) = h.user("wes.pds.example.com").await;
    let stub = h.with_stub().await;
    stub.set("app.nearhorizon.private.deleteAudience", 200, json!({}));
    let mine = h.add_audience(&did, A).await;
    let subject = "did:plc:someoneelse";
    let call = |method: &'static str, body: Value| {
        let (h, access) = (&h, access.clone());
        async move { h.post(method, Some(&access), body).await }
    };
    let record = |collection: &str| -> Value {
        if collection == POST_PRIVATE {
            private_post("x", &format!("at://{did}/{AUD}/{A}"))
        } else {
            json!({"$type": collection, "subject": subject, "createdAt": "2024-01-01T00:00:00.000Z"})
        }
    };
    let collections = [
        POST_PRIVATE,
        "tools.kryphocron.graph.block",
        "tools.kryphocron.graph.mute",
    ];

    for (n, condition) in ["no AppView", "tier off", "permission lapsed", "local block"]
        .into_iter()
        .enumerate()
    {
        // Made while the tier is on: a record of each collection, a post to
        // delete with deletePostPrivate and an audience.
        let mut made = Vec::new();
        for c in collections {
            let (st, v) = call(
                "com.atproto.repo.createRecord",
                json!({"repo": did, "collection": c, "record": record(c)}),
            )
            .await;
            assert_eq!(st, StatusCode::OK, "{condition} {c}: {v}");
            made.push((c, v["uri"].as_str().unwrap().rsplit('/').next().unwrap().to_owned()));
        }
        let (st, v) = h.private_post_as(&did, &access, private_post("x", &mine)).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        let post = v["uri"].as_str().unwrap().rsplit('/').next().unwrap().to_owned();
        let aud_rkey = format!("3m6gone{n}aaaaa");
        h.add_audience(&did, &aud_rkey).await;

        let (refusal, status) = match condition {
            "no AppView" => {
                h.set("appview.url", json!("")).await;
                h.set("appview.did", json!("")).await;
                ("UnsupportedNamespace", StatusCode::BAD_REQUEST)
            }
            "tier off" => {
                h.set("kryphocron.private_tier_enabled", json!(false)).await;
                ("UnsupportedNamespace", StatusCode::BAD_REQUEST)
            }
            "permission lapsed" => {
                h.state.store.delete_kr_permissions(&did).await.unwrap();
                stub.set(
                    "app.nearhorizon.actor.getMembership",
                    200,
                    json!({"status": "member", "features": [], "validForSecs": 3600,
                           "permissions": {"privateIndexed": false, "privateRead": false}}),
                );
                ("Forbidden", StatusCode::FORBIDDEN)
            }
            _ => {
                h.state.store.set_private_tier_blocked(&did, true).await.unwrap();
                ("Forbidden", StatusCode::FORBIDDEN)
            }
        };
        let refused = |(st, v): (StatusCode, Value), what: &str| {
            assert_eq!(
                (st, v["error"].as_str()),
                (status, Some(refusal)),
                "{condition}: {what}: {v}"
            );
        };

        // Creates and updates are refused.
        for (c, rkey) in &made {
            refused(
                call(
                    "com.atproto.repo.createRecord",
                    json!({"repo": did, "collection": c, "record": record(c)}),
                )
                .await,
                &format!("create {c}"),
            );
            refused(
                call(
                    "com.atproto.repo.putRecord",
                    json!({"repo": did, "collection": c, "rkey": rkey, "record": record(c)}),
                )
                .await,
                &format!("update {c}"),
            );
        }
        refused(
            h.private_post_as(&did, &access, private_post("x", &mine)).await,
            "createPostPrivate",
        );
        refused(
            call(
                "tools.kryphocron.feed.updatePostPrivate",
                json!({"repo": did, "rkey": post, "audienceList": mine}),
            )
            .await,
            "updatePostPrivate",
        );
        refused(
            call(
                "tools.kryphocron.policy.manageAudience",
                json!({"repo": did, "record": {"members": []}}),
            )
            .await,
            "create audience",
        );
        refused(
            call(
                "tools.kryphocron.policy.manageAudience",
                json!({"repo": did, "rkey": aud_rkey, "record": {"members": []}}),
            )
            .await,
            "update audience",
        );

        // Deletes succeed.
        for (c, rkey) in &made {
            let (st, v) = call(
                "com.atproto.repo.deleteRecord",
                json!({"repo": did, "collection": c, "rkey": rkey}),
            )
            .await;
            assert_eq!(st, StatusCode::OK, "{condition}: delete {c}: {v}");
            assert!(
                h.state.store.record_entry(&did, c, rkey).await.unwrap().is_none(),
                "{condition}: {c} gone"
            );
        }
        let (st, v) = call(
            "tools.kryphocron.feed.deletePostPrivate",
            json!({"repo": did, "rkey": post}),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{condition}: deletePostPrivate: {v}");
        assert!(
            h.state.store.record_entry(&did, POST_PRIVATE, &post).await.unwrap().is_none()
        );
        let (st, v) = call(
            "tools.kryphocron.policy.deleteAudience",
            json!({"repo": did, "rkey": aud_rkey}),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{condition}: deleteAudience: {v}");
        assert!(
            h.state.store.kr_audience(&did, &aud_rkey).await.unwrap().is_none(),
            "{condition}: the audience is gone (or a pending delete)"
        );
        // An audience the store never held: a no-op (with the tier on,
        // once a complete listing lacks it), and with the tier off prism
        // isn't asked.
        stub.set(
            LIST,
            200,
            json!({"audiences": [prism_audience(&did, A, "3m6prismcid22", &[])]}),
        );
        sqlx::query("UPDATE kr_audience_sync SET last_owner_refresh_at = 0")
            .execute(h.pool())
            .await
            .unwrap();
        let calls = stub.seen.lock().unwrap().len();
        let (st, v) = call(
            "tools.kryphocron.policy.deleteAudience",
            json!({"repo": did, "rkey": "3m6neverheldxx"}),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{condition}: unknown audience: {v}");
        if n < 2 {
            assert_eq!(stub.seen.lock().unwrap().len(), calls, "{condition}: no prism call");
        }

        // Back on.
        h.set("appview.url", json!(stub.url)).await;
        h.set("appview.did", json!("did:web:prism.test")).await;
        h.set("kryphocron.private_tier_enabled", json!(true)).await;
        h.state.store.set_private_tier_blocked(&did, false).await.unwrap();
        h.state.store.delete_kr_permissions(&did).await.unwrap();
        stub.set(
            "app.nearhorizon.actor.getMembership",
            200,
            json!({"status": "member", "features": [], "validForSecs": 3600,
                   "permissions": {"privateIndexed": true, "privateRead": true}}),
        );
    }
}

// ---------------------------------------------------------------- outages

#[tokio::test]
async fn a_hung_prism_is_an_outage_and_errors_are_not() {
    let h = harness(false).await;
    let (did, access) = h.user("xia.pds.example.com").await;
    let stub = h.with_stub().await;
    h.set("kryphocron.prism_request_timeout_ms", json!(1000))
        .await;
    let mine = h.add_audience(&did, A).await;
    let (st, _) = h
        .private_post_as(&did, &access, private_post("x", &mine))
        .await;
    assert_eq!(st, StatusCode::OK);
    let list = || crate::audience::call(&h.state, &did, LIST, &[], None);
    let wait_for = |down: bool, ms: u64| {
        let h = &h;
        async move {
            let until = Instant::now() + Duration::from_millis(ms);
            while h.state.kr.prism.is_down() != down && Instant::now() < until {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            h.state.kr.prism.is_down() == down
        }
    };

    // A 5xx or a 429 is an answer, not an outage.
    for status in [500, 503, 429] {
        stub.set(LIST, status, json!({"error": "Nope"}));
        assert!(matches!(list().await, Prism::Answer { .. }));
        assert!(!wait_for(true, 300).await, "{status} doesn't mark it down");
    }

    // Hung (accepts, never answers): the call times out (ambiguous) and the
    // probe times out too, so it's down.
    let hung = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let hung_url = format!("http://{}", hung.local_addr().unwrap());
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((s, _)) = hung.accept().await {
            held.push(s);
        }
    });
    h.set("appview.url", json!(hung_url)).await;
    assert!(matches!(list().await, Prism::Timeout));
    assert!(wait_for(true, 3000).await, "a timed-out probe marks it down");

    // Down: proxied audience calls are answered from the store, prism's
    // version token from before the outage passing the swap once.
    let proxy = [
        ("atproto-proxy", "did:web:prism.test#bsky_appview"),
        ("content-type", "application/json"),
    ];
    let update = |swap: &'static str| {
        let (h, access, did) = (&h, access.clone(), did.clone());
        async move {
            let body = json!({"uri": format!("at://{did}/{AUD}/{A}"), "swapCid": swap,
                              "record": {"members": ["did:plc:new"]}});
            let (st, _, b) = h
                .raw(
                    "POST",
                    "/xrpc/app.nearhorizon.private.updateAudience",
                    Some(&access),
                    serde_json::to_vec(&body).unwrap(),
                    &proxy,
                )
                .await;
            (st, serde_json::from_slice::<Value>(&b).unwrap_or(Value::Null))
        }
    };
    let (st, v) = update("3m6prismcid22").await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let row = h.state.store.kr_audience(&did, A).await.unwrap().unwrap();
    assert_eq!(
        (row.pending.as_str(), v["cid"].as_str()),
        ("update", Some(row.local_cid.as_str()))
    );
    let (st, v) = update("3m6prismcid22").await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("InvalidSwap")),
        "a stale token: {v}"
    );
    assert_eq!(stub.calls("app.nearhorizon.private.updateAudience"), 0);

    // The audience cap counts local audiences.
    h.set("kryphocron.audience_max_per_account", json!(2)).await;
    let create = || {
        h.post(
            "tools.kryphocron.policy.manageAudience",
            Some(&access),
            json!({"repo": did, "record": {"members": []}}),
        )
    };
    let (st, v) = create().await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (st, v) = create().await;
    assert_eq!(
        (st, v["error"].as_str()),
        (StatusCode::BAD_REQUEST, Some("LimitReached")),
        "{v}"
    );

    // The next answered probe marks it up.
    h.set("appview.url", json!(stub.url)).await;
    assert!(wait_for(false, 12_000).await, "probed back up");
}

#[tokio::test]
async fn local_audiences_are_flagged_never_deleted_and_paced() {
    let h = harness(false).await;
    let (did, access) = h.user("yan.pds.example.com").await;
    let stub = h.with_stub().await;
    let mine = h.add_audience(&did, A).await;
    let (st, _) = h
        .private_post_as(&did, &access, private_post("x", &mine))
        .await;
    assert_eq!(st, StatusCode::OK);
    let collide = local_audience(&h, &did, &access).await;
    let flagged = |rkey: &str| {
        let (h, did, rkey) = (&h, did.clone(), rkey.to_owned());
        async move {
            let q = "SELECT flagged FROM kr_audience WHERE did = ? AND rkey = ?";
            sqlx::query_scalar::<_, Option<String>>(q)
                .bind(&did)
                .bind(&rkey)
                .fetch_one(h.pool())
                .await
                .unwrap()
        }
    };
    let page_a = prism_audience(&did, A, "3m6prismcid22", &[]);
    let sync = || {
        let (h, did) = (&h, did.clone());
        async move {
            let r = refresh(&h.state, &did, Trigger::Background, None, far())
                .await
                .unwrap();
            assert!(r.complete, "{r:?}");
            crate::audience_sync::sync_account(&h.state, &did, &r)
                .await
                .unwrap()
        }
    };

    // A complete listing without a local audience leaves it; one with its
    // rkey is a collision: flagged, not adopted, never sent.
    stub.set(LIST, 200, json!({"audiences": [page_a]}));
    stub.set(CREATE, 400, json!({"error": "InvalidRequest"}));
    h.set("kryphocron.audience_max_per_account", json!(1)).await;
    assert_eq!(sync().await, 0, "no room in prism");
    let row = h.state.store.kr_audience(&did, &collide).await.unwrap().unwrap();
    assert_eq!(row.origin, "local", "never deleted by a listing");
    assert_eq!(flagged(&collide).await.as_deref(), Some("limit"));
    h.set("kryphocron.audience_max_per_account", json!(200)).await;
    let mut theirs = prism_audience(&did, &collide, "v9", &["did:plc:theirs"]);
    theirs["record"]["name"] = json!("theirs");
    stub.set(LIST, 200, json!({"audiences": [page_a, theirs]}));
    // (The limit flag clears with room, then the collision flags it.)
    sync().await;
    let row = h.state.store.kr_audience(&did, &collide).await.unwrap().unwrap();
    assert_eq!(
        (row.origin.as_str(), row.members_json.as_str()),
        ("local", "[\"did:plc:friend\"]"),
        "not adopted"
    );
    assert!(
        flagged(&collide).await.is_some_and(|f| f.contains("collision")),
        "{:?}",
        flagged(&collide).await
    );
    stub.seen.lock().unwrap().clear();

    // InvalidRequest flags until the audience is edited.
    let bad = local_audience(&h, &did, &access).await;
    stub.set(LIST, 200, json!({"audiences": [page_a]}));
    assert_eq!(sync().await, 1);
    assert_eq!(flagged(&bad).await.as_deref(), Some("refused"));
    assert_eq!(sync().await, 0, "flagged: not sent again");
    h.state.kr.prism.set(true);
    let (st, v) = h
        .post(
            "tools.kryphocron.policy.manageAudience",
            Some(&access),
            json!({"repo": did, "rkey": bad, "record": {"members": ["did:plc:fixed"]}}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    h.state.kr.prism.set(false);
    assert_eq!(flagged(&bad).await, None, "an edit clears the flag");

    // Prism's LimitReached flags it too.
    stub.set(CREATE, 400, json!({"error": "LimitReached"}));
    assert_eq!(sync().await, 1);
    assert_eq!(flagged(&bad).await.as_deref(), Some("limit"));

    // With room, the limit flag clears and both are sent (a 5xx leaves them
    // for later), with the local stamps, at most 2 writes/s.
    let other = local_audience(&h, &did, &access).await;
    let created = h
        .state
        .store
        .kr_audience(&did, &other)
        .await
        .unwrap()
        .unwrap();
    stub.seen.lock().unwrap().clear();
    stub.set(CREATE, 500, json!({"error": "InternalServerError"}));
    let started = Instant::now();
    assert_eq!(sync().await, 2, "both sent");
    assert!(
        started.elapsed() >= Duration::from_millis(1000),
        "paced: {:?}",
        started.elapsed()
    );
    let bodies = stub.bodies(CREATE);
    let sent = bodies
        .iter()
        .find(|b| b["record"]["members"] == json!(["did:plc:friend"]))
        .expect("the other local audience was sent");
    assert_eq!(
        (
            sent["record"]["createdAt"].as_str(),
            sent["record"]["updatedAt"].as_str()
        ),
        (Some(created.created_at.as_str()), Some(created.updated_at.as_str())),
        "the local stamps"
    );
}

#[tokio::test]
async fn the_background_poll_keeps_within_the_call_budget() {
    let h = harness(false).await;
    let stub = h.with_stub().await;
    stub.set(LIST, 200, json!({"audiences": []}));
    for name in ["zed", "zia", "zoe"] {
        let (did, _) = h.user(&format!("{name}.pds.example.com")).await;
        h.state.store.ensure_audience_sync(&did).await.unwrap();
    }
    // Three accounts, one page each, at 2 calls/s: at least 1.5 s.
    h.set("kryphocron.prism_call_budget", json!(2)).await;
    let cancel = tokio_util::sync::CancellationToken::new();
    let started = Instant::now();
    assert_eq!(
        crate::audience_sync::poll_once(&h.state, &cancel)
            .await
            .unwrap(),
        3
    );
    assert!(
        started.elapsed() >= Duration::from_millis(1500),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(stub.calls(LIST), 3);
}

// ---------------------------------------------------------------- store misses

#[tokio::test]
async fn store_miss_refreshes_are_retryable_until_a_listing_settles_them() {
    let h = harness(false).await;
    let (did, access) = h.user("ada.pds.example.com").await;
    let stub = h.with_stub().await;
    h.set("kryphocron.prism_request_timeout_ms", json!(1000))
        .await;
    let mine = h.add_audience(&did, A).await;
    let (st, _) = h
        .private_post_as(&did, &access, private_post("x", &mine))
        .await;
    assert_eq!(st, StatusCode::OK);
    // The owner limiter allows one refresh per 5 s; each step starts clear.
    let clear = || {
        let (h, did) = (&h, did.clone());
        async move {
            sqlx::query("UPDATE kr_audience_sync SET last_owner_refresh_at = 0 WHERE did = ?")
                .bind(&did)
                .execute(h.pool())
                .await
                .unwrap();
        }
    };
    let post = |rkey: &'static str| {
        let (h, did, access) = (&h, did.clone(), access.clone());
        async move {
            h.private_post_as(&did, &access, private_post("x", &format!("at://{did}/{AUD}/{rkey}")))
                .await
        }
    };
    let retryable = |(st, v): (StatusCode, Value), what: &str| {
        assert_eq!(
            (st, v["error"].as_str()),
            (StatusCode::SERVICE_UNAVAILABLE, Some("TemporarilyUnavailable")),
            "{what}: {v}"
        );
    };

    // A 5xx, then a timeout: retryable, never InvalidRequest.
    stub.set(LIST, 502, json!({"error": "BadGateway"}));
    clear().await;
    retryable(post(B).await, "5xx");
    stub.set_slow(
        LIST,
        200,
        json!({"audiences": [prism_audience(&did, B, "v1", &[])]}),
        1500,
    );
    clear().await;
    retryable(post(B).await, "timeout");
    // A later retry finds it.
    stub.set(LIST, 200, json!({"audiences": [prism_audience(&did, B, "v1", &[])]}));
    clear().await;
    let (st, v) = post(B).await;
    assert_eq!(st, StatusCode::OK, "{v}");

    // A synchronous refresh past the request deadline is retryable.
    h.set("kryphocron.prism_request_timeout_ms", json!(10_000))
        .await;
    h.set("http.request_deadline_ms", json!(2000)).await;
    stub.set_slow(
        LIST,
        200,
        json!({"audiences": [prism_audience(&did, C, "v1", &[])]}),
        1500,
    );
    clear().await;
    retryable(post(C).await, "past the deadline");
    h.set("http.request_deadline_ms", json!(60_000)).await;

    // An unknown-rkey update refreshes first, then goes to prism.
    stub.set(
        LIST,
        200,
        json!({"audiences": [prism_audience(&did, D, "v1", &[])]}),
    );
    stub.set(
        "app.nearhorizon.private.updateAudience",
        200,
        json!({"uri": format!("at://{did}/{AUD}/{D}"), "cid": "v2"}),
    );
    stub.seen.lock().unwrap().clear();
    clear().await;
    let (st, v) = h
        .post(
            "tools.kryphocron.policy.manageAudience",
            Some(&access),
            json!({"repo": did, "rkey": D, "record": {"members": []}}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let order: Vec<String> = stub
        .seen
        .lock()
        .unwrap()
        .iter()
        .filter(|s| s.uri.contains("Audience"))
        .map(|s| s.uri.split('?').next().unwrap().to_owned())
        .collect();
    assert_eq!(
        order,
        [
            format!("/xrpc/{LIST}"),
            "/xrpc/app.nearhorizon.private.updateAudience".to_owned()
        ]
    );

    // InvalidSwap from prism wants a refresh; the owner's limiter was just
    // spent, so it's retryable rather than InvalidSwap.
    stub.set(
        "app.nearhorizon.private.updateAudience",
        400,
        json!({"error": "InvalidSwap"}),
    );
    let (st, v) = h
        .post(
            "tools.kryphocron.policy.manageAudience",
            Some(&access),
            json!({"repo": did, "rkey": D, "record": {"members": ["did:plc:x"]}}),
        )
        .await;
    retryable((st, v), "throttled InvalidSwap refresh");

    // Deleting an rkey a complete listing lacks: a no-op, prism not asked
    // to delete.
    clear().await;
    let (st, v) = h
        .post(
            "tools.kryphocron.policy.deleteAudience",
            Some(&access),
            json!({"repo": did, "rkey": "3m6notinprism0"}),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(stub.calls("app.nearhorizon.private.deleteAudience"), 0);
}
