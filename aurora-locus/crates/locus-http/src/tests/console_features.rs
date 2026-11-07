//! The console's hostname wizard, live status pages, jobs controls, the
//! import report, the account portal's straggler notice, and the landing
//! page's theme.

use std::time::Duration;

use super::*;

const OWNER_PW: &str = "an owner password";

/// Operators of every role with console passwords, and a browser signed in
/// as each: (owner, admin, moderator, viewer).
async fn crew(h: &Harness) -> [Browser; 4] {
    let roles = [
        ("olga", "owner", OWNER_PW),
        ("ada", "admin", "an admin password"),
        ("mo", "moderator", "a moderator password"),
        ("vic", "viewer", "a viewer password"),
    ];
    let mut out = Vec::new();
    for (name, role, pw) in roles {
        h.console_operator(name, role, pw).await;
        let b = Browser::default();
        h.console_sign_in(&b, name, pw).await;
        out.push(b);
    }
    out.try_into().unwrap_or_else(|_| unreachable!())
}

fn csp(headers: &axum::http::HeaderMap) -> String {
    headers["content-security-policy"]
        .to_str()
        .unwrap()
        .to_owned()
}

async fn audited(h: &Harness, action: &str, actor: &str) -> bool {
    h.state
        .store
        .audit_entries(None, 200)
        .await
        .unwrap()
        .iter()
        .any(|e| e.action == action && e.actor == actor)
}

#[tokio::test]
async fn the_hostname_wizard_prepares_checks_confirms_and_cancels() {
    let h = harness(false).await;
    let [owner, admin, _mo, viewer] = crew(&h).await;
    let new = "next.pds.example.com";

    // Every role sees the page; only an owner gets the prepare form.
    for b in [&owner, &admin, &viewer] {
        let (st, headers, page) = b.get(&h, "/admin/hostname").await;
        assert_eq!(st, StatusCode::OK, "{page}");
        assert!(page.contains("pds.example.com"), "{page}");
        assert!(
            !csp(&headers).contains("script-src"),
            "nothing runs yet, so the page isn't live"
        );
    }
    let (_, _, page) = owner.get(&h, "/admin/hostname").await;
    assert!(page.contains("Prepare a new hostname"), "{page}");
    let (_, _, page) = viewer.get(&h, "/admin/hostname").await;
    assert!(
        !page.contains("Prepare a new hostname") && page.contains("Only an owner"),
        "{page}"
    );
    assert_eq!(
        Browser::default().get(&h, "/admin/hostname").await.0,
        StatusCode::SEE_OTHER,
        "signed out"
    );

    // An admin's prepare is refused by the admin API (owners only), and a
    // forged form by the console.
    let (_, _, page) = admin.get(&h, "/admin/hostname").await;
    let admin_csrf = hidden(&page, "csrf");
    let (st, _, page) = admin
        .post(
            &h,
            "/admin/hostname/prepare",
            &[("csrf", &admin_csrf), ("host", new)],
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{page}");
    assert!(page.contains("owner"), "{page}");
    let (_, _, page) = owner.get(&h, "/admin/hostname").await;
    let csrf = hidden(&page, "csrf");
    let (st, _, _) = owner
        .post(
            &h,
            "/admin/hostname/prepare",
            &[("csrf", "forged"), ("host", new)],
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert!(
        h.state
            .store
            .open_hostname_migration()
            .await
            .unwrap()
            .is_none()
    );
    // A handle host is refused, with the API's reason.
    h.user("pia.pds.example.com").await;
    let (st, _, page) = owner
        .post(
            &h,
            "/admin/hostname/prepare",
            &[("csrf", &csrf), ("host", "pia.pds.example.com")],
        )
        .await;
    assert!(
        st == StatusCode::BAD_REQUEST && page.contains("handle or did:web host"),
        "{page}"
    );

    // Prepare: the candidate is recorded, and the page runs its checks.
    let (st, headers, _) = owner
        .post(
            &h,
            "/admin/hostname/prepare",
            &[("csrf", &csrf), ("host", new)],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(headers["location"], "/admin/hostname?done=prepared&check=1");
    let m = h.state.store.open_hostname_migration().await.unwrap().unwrap();
    assert_eq!((m.state.as_str(), m.new_host.as_str()), ("prepared", new));
    assert!(audited(&h, "hostname.prepare", "operator:olga").await);
    h.reload_hosts().await;
    let (st, _, page) = owner
        .get(&h, "/admin/hostname?done=prepared&check=1")
        .await;
    assert_eq!(st, StatusCode::OK, "{page}");
    // No HTTPS in tests: that pre-check fails, so no certificate yet and
    // confirming isn't offered; /tls-check approves the candidate.
    assert!(page.contains("Pre-checks") && page.contains("fails"), "{page}");
    assert!(page.contains("Certificate") && page.contains("not yet"), "{page}");
    assert!(page.contains("approves it"), "{page}");
    assert!(
        !page.contains("Confirm the migration") && page.contains("Confirming opens once"),
        "{page}"
    );
    assert!(page.contains("Cancel the candidate"), "{page}");
    let stored = h.state.store.open_hostname_migration().await.unwrap().unwrap();
    assert!(
        stored.prechecks.unwrap().contains("tls-check"),
        "the check ran through the API and was kept"
    );
    // The viewer sees the checks but no actions.
    let (_, _, page) = viewer.get(&h, "/admin/hostname").await;
    assert!(
        page.contains("Pre-checks") && !page.contains("Cancel the candidate"),
        "{page}"
    );
    // Every console page says a candidate waits.
    let (_, _, page) = admin.get(&h, "/admin").await;
    assert!(
        page.contains(&format!("A new hostname, {new}, is prepared")),
        "{page}"
    );

    // Confirming needs a fresh confirmation, for the page and the post.
    age_console_confirmations(&h).await;
    let (st, headers, _) = owner.get(&h, "/admin/hostname/confirm").await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(
        headers["location"],
        "/admin/confirm-identity?next=%2Fadmin%2Fhostname%2Fconfirm"
    );
    let (st, headers, _) = owner
        .post(
            &h,
            "/admin/hostname/confirm",
            &[("csrf", &csrf), ("typed", new)],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert!(
        headers["location"]
            .to_str()
            .unwrap()
            .starts_with("/admin/confirm-identity")
    );
    let (st, headers, _) = owner
        .post(
            &h,
            "/admin/confirm-identity",
            &[
                ("csrf", &csrf),
                ("next", "/admin/hostname/confirm"),
                ("password", OWNER_PW),
            ],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(headers["location"], "/admin/hostname/confirm");
    let (st, headers, page) = owner.get(&h, "/admin/hostname/confirm").await;
    assert_eq!(st, StatusCode::OK, "{page}");
    assert!(
        page.contains("Every OAuth session must sign in again")
            && page.contains("reversible in practice")
            && page.contains(&format!("<code>{new}</code>")),
        "{page}"
    );
    assert!(!csp(&headers).contains("script-src"));
    assert_eq!(
        admin.get(&h, "/admin/hostname/confirm").await.0,
        StatusCode::FORBIDDEN,
        "owners only"
    );
    // The typed name must match; then the API runs the pre-checks again
    // and refuses (no HTTPS here), and nothing changes.
    let (st, _, page) = owner
        .post(
            &h,
            "/admin/hostname/confirm",
            &[("csrf", &csrf), ("typed", "next")],
        )
        .await;
    assert!(
        st == StatusCode::BAD_REQUEST && page.contains(&format!("Type {new} to confirm")),
        "{page}"
    );
    let (st, _, page) = owner
        .post(
            &h,
            "/admin/hostname/confirm",
            &[("csrf", &csrf), ("typed", new)],
        )
        .await;
    assert!(
        st == StatusCode::BAD_REQUEST && page.contains("pre-check"),
        "{page}"
    );
    let m = h.state.store.open_hostname_migration().await.unwrap().unwrap();
    assert_eq!(m.state, "prepared");
    assert!(!audited(&h, "hostname.confirm", "operator:olga").await);

    // Cancel: an admin can't; the owner can, and the candidate goes.
    let (st, _, _) = admin
        .post(&h, "/admin/hostname/cancel", &[("csrf", &admin_csrf)])
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(h.state.store.open_hostname_migration().await.unwrap().is_some());
    let (st, headers, _) = owner
        .post(&h, "/admin/hostname/cancel", &[("csrf", &csrf)])
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(headers["location"], "/admin/hostname?done=cancelled");
    assert!(h.state.store.open_hostname_migration().await.unwrap().is_none());
    assert!(audited(&h, "hostname.cancel", "operator:olga").await);
    let (_, _, page) = owner.get(&h, "/admin").await;
    assert!(!page.contains("is prepared"), "{page}");
}

#[tokio::test]
async fn a_running_hostname_migration_shows_progress_stragglers_and_closes() {
    let h = harness(false).await;
    let [owner, admin, _mo, viewer] = crew(&h).await;
    let (moved, _) = h.user("moved.pds.example.com").await;
    let (stuck, _) = h.user("stuck.pds.example.com").await;
    let old = "old.example.net";
    // This node is already the new hostname: a migration from `old` runs,
    // its window already over, its job part way.
    h.state
        .store
        .prepare_hostname_migration("m1", old, &h.state.hostname, "operator:olga")
        .await
        .unwrap();
    assert!(
        h.state
            .store
            .confirm_hostname_migration("m1", 0)
            .await
            .unwrap()
    );
    h.reload_hosts().await;
    let store = &h.state.store;
    store
        .record_hostname_move("m1", &moved, "moved", None)
        .await
        .unwrap();
    store
        .record_hostname_move("m1", &stuck, "cannot", Some("lists none of the keys"))
        .await
        .unwrap();

    let (st, headers, page) = admin.get(&h, "/admin/hostname").await;
    assert_eq!(st, StatusCode::OK, "{page}");
    assert!(
        page.contains(&format!("Moving from {old} to pds.example.com"))
            && page.contains("moving accounts"),
        "{page}"
    );
    assert!(page.contains(&stuck) && page.contains("lists none of the keys"));
    assert!(!page.contains(&format!(">{moved}<")), "a moved account isn't listed");
    assert!(page.contains(&format!("Accounts still pointing at {old} (1)")));
    // Running, the page is live: its script and stream, under a CSP that
    // allows only them.
    let policy = csp(&headers);
    assert!(
        policy.contains("script-src 'self'") && policy.contains("connect-src 'self'"),
        "{policy}"
    );
    assert!(page.contains(r#"data-live="/admin/live?page=/admin/hostname""#));
    assert!(page.contains(&format!(
        r#"<script src="{}" defer></script>"#,
        *locus_web::assets::JS_HREF
    )));
    assert!(!page.contains("Close the migration"), "admins can't close");
    // The banner on every page.
    let (_, _, home) = viewer.get(&h, "/admin").await;
    assert!(home.contains("the job is moving accounts"), "{home}");

    // The job isn't done: closing is refused, by the API.
    let (_, _, page) = owner.get(&h, "/admin/hostname").await;
    assert!(!page.contains("Close the migration") && page.contains("can be closed once"));
    let csrf = hidden(&page, "csrf");
    let (st, _, page) = owner
        .post(&h, "/admin/hostname/close", &[("csrf", &csrf)])
        .await;
    assert!(
        st == StatusCode::BAD_REQUEST && page.contains("closes after its job"),
        "{page}"
    );

    // The job finishes with a straggler; after the window it closes.
    store.set_hostname_cursor("m1", None).await.unwrap();
    let (_, _, page) = owner.get(&h, "/admin/hostname").await;
    assert!(page.contains("Close the migration"), "{page}");
    let (_, _, home) = viewer.get(&h, "/admin").await;
    assert!(home.contains("1 account(s) still point at"), "{home}");
    let (st, headers, _) = owner
        .post(&h, "/admin/hostname/close", &[("csrf", &csrf)])
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert_eq!(headers["location"], "/admin/hostname?done=closed");
    let m = &store.hostname_migrations().await.unwrap()[0];
    assert_eq!(
        (m.state.as_str(), m.former_at.is_none()),
        ("done", true),
        "closed with its straggler, the old host isn't former"
    );
    assert!(audited(&h, "hostname.close", "operator:olga").await);
    // The straggler stays listed, and so does the banner.
    let (_, _, page) = viewer.get(&h, "/admin/hostname").await;
    assert!(
        page.contains(&stuck) && page.contains(&format!("Accounts still pointing at {old} (1)")),
        "{page}"
    );
    let (_, _, home) = viewer.get(&h, "/admin/accounts").await;
    assert!(
        home.contains(&format!("still point at the old hostname {old}")),
        "{home}"
    );

    // Once its document moves, the straggler and the banner go.
    store
        .record_hostname_move("m1", &stuck, "moved", None)
        .await
        .unwrap();
    let (_, _, home) = viewer.get(&h, "/admin").await;
    assert!(!home.contains("still point at"), "{home}");
}

#[tokio::test]
async fn the_account_portal_tells_a_straggler_what_to_do() {
    let h = harness(false).await;
    let (stuck, _) = h.user("stuck.pds.example.com").await;
    let (fine, _) = h.user("fine.pds.example.com").await;
    let old = "old.example.net";
    let store = &h.state.store;
    store
        .prepare_hostname_migration("m1", old, &h.state.hostname, "test")
        .await
        .unwrap();
    store.confirm_hostname_migration("m1", 0).await.unwrap();
    store
        .record_hostname_move("m1", &stuck, "cannot", Some("no held key"))
        .await
        .unwrap();
    store
        .record_hostname_move("m1", &fine, "moved", None)
        .await
        .unwrap();

    let b = Browser::default();
    let (st, _, _) = portal_sign_in(&h, &b, "stuck.pds.example.com", "pw", None).await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    let (_, _, page) = b.get(&h, "/account").await;
    assert!(
        page.contains(&format!("still names {old} as your PDS"))
            && page.contains(r#"href="/account/identity""#),
        "{page}"
    );
    let (_, _, page) = b.get(&h, "/account/identity").await;
    assert!(
        page.contains("Your PDS moved")
            && page.contains("sign a PLC operation with one of your own rotation keys")
            && page.contains("https://pds.example.com"),
        "{page}"
    );
    // An account whose document moved sees nothing.
    let other = Browser::default();
    portal_sign_in(&h, &other, "fine.pds.example.com", "pw", None).await;
    let (_, _, page) = other.get(&h, "/account").await;
    assert!(!page.contains("still names"), "{page}");
    // A failed move says so.
    store
        .record_hostname_move("m1", &stuck, "failed", Some("PLC said 500"))
        .await
        .unwrap();
    let (_, _, page) = b.get(&h, "/account/identity").await;
    assert!(
        page.contains("tried to update it and failed (PLC said 500)"),
        "{page}"
    );
    // Once the old host is former, no document names it: nothing to say.
    store.finish_hostname_migration("m1", true).await.unwrap();
    let (_, _, page) = b.get(&h, "/account").await;
    assert!(!page.contains("still names"), "{page}");
}

/// A live stream's request, as the browser's `EventSource` sends it.
async fn open_live(h: &Harness, b: &Browser, page: &str) -> axum::response::Response {
    let req = Request::builder()
        .method("GET")
        .uri(format!("/admin/live?page={page}"))
        .header("host", h.state.hostname.clone())
        .header("cookie", b.cookie_header())
        .header("accept", "text/event-stream")
        .body(Body::empty())
        .unwrap();
    h.app.clone().oneshot(req).await.unwrap()
}

/// The stream's next chunk: Err on a timeout, Ok(None) when it ended.
async fn chunk(body: &mut Body, wait: Duration) -> Result<Option<String>, ()> {
    match tokio::time::timeout(wait, body.frame()).await {
        Err(_) => Err(()),
        Ok(None) => Ok(None),
        Ok(Some(f)) => Ok(f
            .unwrap()
            .into_data()
            .ok()
            .map(|d| String::from_utf8_lossy(&d).into_owned())),
    }
}

/// Reads until a chunk contains `want` (or `secs` pass).
async fn until(body: &mut Body, want: &str, secs: u64) -> String {
    let end = tokio::time::Instant::now() + Duration::from_secs(secs);
    let mut seen = String::new();
    while tokio::time::Instant::now() < end {
        match chunk(body, Duration::from_millis(500)).await {
            Ok(Some(c)) => {
                seen.push_str(&c);
                if c.contains(want) {
                    return c;
                }
            }
            Ok(None) => panic!("the stream ended before {want:?}: {seen}"),
            Err(()) => {}
        }
    }
    panic!("no {want:?} in {secs}s: {seen}");
}

#[tokio::test]
async fn status_pages_refresh_over_server_sent_events() {
    let h = harness(false).await;
    let [_owner, admin, _mo, viewer] = crew(&h).await;

    // Live pages load the one script and name their stream; other pages
    // keep the strict CSP and no script.
    for (path, src) in [
        ("/admin", "/admin/live?page=/admin"),
        ("/admin/jobs", "/admin/live?page=/admin/jobs"),
        ("/admin/storage", "/admin/live?page=/admin/storage"),
        ("/admin/services", "/admin/live?page=/admin/services"),
    ] {
        let (st, headers, page) = viewer.get(&h, path).await;
        assert_eq!(st, StatusCode::OK, "{path}: {page}");
        assert!(page.contains(&format!(r#"data-live="{src}""#)), "{path}: {page}");
        assert!(page.contains("data-live-part"), "{path}");
        assert_eq!(
            csp(&headers),
            locus_web::assets::CSP_LIVE,
            "{path}: scripts and connections from this site only"
        );
    }
    for path in ["/admin/settings", "/admin/audit", "/admin/import"] {
        let (_, headers, page) = viewer.get(&h, path).await;
        assert!(!page.contains("<script") && !page.contains("data-live="), "{path}");
        assert_eq!(csp(&headers), locus_web::assets::CSP, "{path}");
    }
    let js = locus_web::assets::JS_HREF.clone();
    let (st, headers, body) = h.raw("GET", &js, None, vec![], &[]).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(headers["content-type"], "text/javascript; charset=utf-8");
    assert!(String::from_utf8(body).unwrap().contains("EventSource"));

    // The stream is the console's: no sign-in, no stream; only live pages.
    let res = open_live(&h, &Browser::default(), "/admin/jobs").await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    for page in ["/admin/settings", "/admin/operators", "/elsewhere", ""] {
        let res = open_live(&h, &viewer, page).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "{page}");
    }

    // A viewer's stream: it starts with its retry, says "change" when the
    // page would render differently, and keeps alive meanwhile.
    let res = open_live(&h, &viewer, "/admin/jobs").await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()["content-type"], "text/event-stream");
    assert_eq!(res.headers()["cache-control"], "no-store");
    let mut body = res.into_body();
    let first = until(&mut body, "retry:", 2).await;
    assert!(first.starts_with("retry: 5000\n"), "{first}");
    until(&mut body, ": keepalive", 3).await;
    h.state
        .store
        .record_job_run("gc-blocks", Some("disk full"), None)
        .await
        .unwrap();
    let change = until(&mut body, "event: change", 3).await;
    assert!(change.contains("\ndata: "), "{change}");
    // The page it points at now shows the change.
    let (_, _, page) = viewer.get(&h, "/admin/jobs").await;
    assert!(page.contains("disk full"));
    // No change, no event: only keepalives.
    let quiet = until(&mut body, ": keepalive", 3).await;
    assert!(!quiet.contains("change"), "{quiet}");

    // Signing out ends it.
    let (_, _, page) = viewer.get(&h, "/admin/jobs").await;
    let csrf = hidden(&page, "csrf");
    viewer
        .post(&h, "/admin/sign-out", &[("csrf", &csrf)])
        .await;
    until(&mut body, "event: end", 3).await;
    assert_eq!(chunk(&mut body, Duration::from_secs(2)).await, Ok(None));

    // At most a few streams per sign-in; a closed one frees its place.
    let mut held = Vec::new();
    for _ in 0..crate::console_live::PER_SESSION {
        let res = open_live(&h, &admin, "/admin").await;
        assert_eq!(res.status(), StatusCode::OK);
        held.push(res);
    }
    let res = open_live(&h, &admin, "/admin/jobs").await;
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    drop(held.pop());
    let res = open_live(&h, &admin, "/admin/jobs").await;
    assert_eq!(res.status(), StatusCode::OK);
    drop(held);

    // A stream ends after its lifetime; the browser reconnects.
    let mut body = res.into_body();
    until(&mut body, ": bye", 8).await;
    assert_eq!(chunk(&mut body, Duration::from_secs(2)).await, Ok(None));
}

#[tokio::test]
async fn jobs_run_now_and_pause_go_through_the_admin_api() {
    let h = harness(false).await;
    let [_owner, admin, mo, viewer] = crew(&h).await;
    h.state
        .store
        .record_job_run("gc-blocks", None, None)
        .await
        .unwrap();

    // Admins get the buttons; lower roles don't, and their posts are
    // refused by the API.
    let (_, _, page) = admin.get(&h, "/admin/jobs").await;
    assert!(page.contains("Run now") && page.contains("Pause"), "{page}");
    let csrf = hidden(&page, "csrf");
    for b in [&mo, &viewer] {
        let (_, _, page) = b.get(&h, "/admin/jobs").await;
        assert!(!page.contains("Run now"), "{page}");
        let c = hidden(&page, "csrf");
        let (st, _, page) = b
            .post(
                &h,
                "/admin/jobs/pause",
                &[("csrf", &c), ("name", "gc-blocks"), ("paused", "true")],
            )
            .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{page}");
    }
    assert!(!h.state.store.job_control("gc-blocks").await.unwrap().paused);
    let (st, _, _) = admin
        .post(
            &h,
            "/admin/jobs/pause",
            &[("csrf", "forged"), ("name", "gc-blocks"), ("paused", "true")],
        )
        .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Pause: stored, audited, shown with a Resume button.
    let (st, headers, _) = admin
        .post(
            &h,
            "/admin/jobs/pause",
            &[("csrf", &csrf), ("name", "gc-blocks"), ("paused", "true")],
        )
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert!(
        headers["location"]
            .to_str()
            .unwrap()
            .starts_with("/admin/jobs?done=")
    );
    let c = h.state.store.job_control("gc-blocks").await.unwrap();
    assert_eq!(
        (c.paused, c.paused_by.as_deref()),
        (true, Some("operator:ada"))
    );
    assert!(audited(&h, "jobs.setPaused", "operator:ada").await);
    let (_, _, page) = admin.get(&h, "/admin/jobs?done=x").await;
    assert!(
        page.contains("paused by operator:ada") && page.contains("Resume"),
        "{page}"
    );
    // A paused job can't be run now (the API says so).
    let (st, _, page) = admin
        .post(&h, "/admin/jobs/run", &[("csrf", &csrf), ("name", "gc-blocks")])
        .await;
    assert!(
        st == StatusCode::BAD_REQUEST && page.contains("resume it first"),
        "{page}"
    );
    // Resume, then run now.
    admin
        .post(
            &h,
            "/admin/jobs/pause",
            &[("csrf", &csrf), ("name", "gc-blocks"), ("paused", "false")],
        )
        .await;
    assert!(!h.state.store.job_control("gc-blocks").await.unwrap().paused);
    let (st, _, _) = admin
        .post(&h, "/admin/jobs/run", &[("csrf", &csrf), ("name", "gc-blocks")])
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER);
    assert!(
        h.state
            .store
            .job_control("gc-blocks")
            .await
            .unwrap()
            .run_requested
            .is_some()
    );
    assert!(audited(&h, "jobs.runNow", "operator:ada").await);
    let (_, _, page) = viewer.get(&h, "/admin/jobs").await;
    assert!(page.contains("run asked for"), "{page}");
}

#[tokio::test]
async fn the_console_shows_the_reference_import_report() {
    let h = harness(false).await;
    let [owner, admin, mo, viewer] = crew(&h).await;
    let ns = |m: &str| format!("/xrpc/{}.{m}", locus_core::LOCUS_NS);
    let (_, _, page) = viewer.get(&h, "/admin/import").await;
    assert!(page.contains("imported from a reference PDS"), "{page}");
    let token = format!("Bearer {}", h.operator("tok", "viewer").await);
    let (st, v) = h
        .admin_call("GET", &ns("viewer.getReferenceImport"), &token, None)
        .await;
    assert_eq!((st, v["imported"].clone()), (StatusCode::OK, json!(false)));

    // An import's report: counts, and what it skipped (shown escaped, and
    // bounded).
    let mut skipped: Vec<String> = (0..1004).map(|i| format!("did:plc:x{i}: no repo")).collect();
    skipped.insert(0, "env value: PDS_<b>ODD</b>".into());
    let report = json!({"counts": {"repos": 2, "records": 17, "blobs": 3}, "skipped": skipped});
    let mut tx = h.state.store.begin().await.unwrap();
    tx.record_reference_import(1_767_225_600_000, &report)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    for b in [&owner, &admin, &mo, &viewer] {
        let (st, headers, page) = b.get(&h, "/admin/import").await;
        assert_eq!(st, StatusCode::OK, "{page}");
        assert_eq!(csp(&headers), locus_web::assets::CSP);
        assert!(
            page.contains("records") && page.contains(">17<") && page.contains(">2<"),
            "{page}"
        );
        assert!(page.contains("the first 1000 of 1005"), "{page}");
        assert!(
            page.contains("PDS_&lt;b&gt;ODD&lt;/b&gt;") && !page.contains("<b>ODD"),
            "{page}"
        );
        assert!(page.contains("2026-01-01"), "{page}");
    }
    let (st, v) = h
        .admin_call("GET", &ns("viewer.getReferenceImport"), &token, None)
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["skippedTotal"], json!(1005));
    assert_eq!(v["skipped"].as_array().unwrap().len(), 1000);
    assert_eq!(v["counts"]["records"], json!(17));
    let (st, _) = h
        .admin_call("GET", &ns("viewer.getReferenceImport"), "Bearer nope", None)
        .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(
        Browser::default().get(&h, "/admin/import").await.0,
        StatusCode::SEE_OTHER
    );
}

#[tokio::test]
async fn the_landing_page_wears_the_published_theme() {
    let h = harness(false).await;
    // Off (the default): the reference's plain banner, and its robots.txt.
    let (st, headers, body) = h.raw("GET", "/", None, vec![], &[]).await;
    let text = String::from_utf8(body).unwrap();
    assert_eq!(st, StatusCode::OK);
    assert_eq!(headers["content-type"], "text/plain; charset=utf-8");
    assert!(
        text.contains("This is an AT Protocol Personal Data Server (aka, an atproto PDS)")
            && text.contains("Most API routes are under /xrpc/"),
        "{text}"
    );
    let (st, _, body) = h.raw("GET", "/robots.txt", None, vec![], &[]).await;
    assert_eq!(st, StatusCode::OK);
    assert!(String::from_utf8(body).unwrap().contains("User-agent: *\nAllow: /"));

    // On: a page, strict CSP, no theme yet.
    h.state
        .store
        .set_setting("server.landing_page", json!(true), "test")
        .await
        .unwrap();
    h.state.settings.refresh().await.unwrap();
    let (st, headers, body) = h.raw("GET", "/", None, vec![], &[]).await;
    let page = String::from_utf8(body).unwrap();
    assert_eq!(st, StatusCode::OK, "{page}");
    assert_eq!(csp(&headers), locus_web::assets::CSP);
    assert!(
        page.contains("Personal Data Server") && !page.contains("/branding/theme/"),
        "{page}"
    );
    assert!(page.contains(r#"href="/account""#), "the portal is linked");

    // Publish a theme: the landing page gets all of it (the full stylesheet
    // with its custom CSS, the name, footer and links), as the portal does.
    h.console_operator("ada", "admin", "an admin password").await;
    let b = Browser::default();
    h.console_sign_in(&b, "ada", "an admin password").await;
    let (_, headers, editor) = b.get(&h, "/admin/branding").await;
    assert!(
        editor.contains("/admin/branding/preview/landing?mode=dark"),
        "the editor previews it"
    );
    assert!(csp(&headers).contains("frame-src 'self'"));
    let csrf = hidden(&editor, "csrf");
    let form = brand_form(
        &csrf,
        &[
            ("service_name", "Example Social"),
            ("light_primary", "#1f6feb"),
            ("dark_primary", "#4d8dff"),
            ("link_terms", "https://example.com/terms"),
            ("text_footer", "Run by Example"),
            ("custom_css", ".card { border-width: 2px }"),
        ],
    );
    b.post(&h, "/admin/branding/save", &form).await;
    let (_, _, page) = b.get(&h, "/admin/branding/preview/landing?mode=dark").await;
    assert!(
        page.contains(r#"class="mode-dark"#)
            && page.contains("draft.css?kind=full")
            && page.contains("Example Social"),
        "{page}"
    );
    assert_eq!(
        Browser::default()
            .get(&h, "/admin/branding/preview/landing")
            .await
            .0,
        StatusCode::NOT_FOUND,
        "previews are for operators"
    );
    let (st, _, body) = b
        .post(&h, "/admin/branding/publish", &[("csrf", &csrf)])
        .await;
    assert_eq!(st, StatusCode::SEE_OTHER, "{body}");
    let v = h
        .state
        .store
        .branding_active()
        .await
        .unwrap()
        .unwrap()
        .version;
    h.state.branding.invalidate();
    let (_, headers, body) = h.raw("GET", "/", None, vec![], &[]).await;
    let page = String::from_utf8(body).unwrap();
    assert!(
        page.contains(&format!("/branding/theme/{v}/full.css"))
            && !page.contains("vars.css"),
        "{page}"
    );
    assert!(
        page.contains("<h1>Example Social</h1>")
            && page.contains("Run by Example")
            && page.contains(r#"href="https://example.com/terms""#),
        "{page}"
    );
    assert!(!page.contains("<script"), "{page}");
    assert_eq!(csp(&headers), locus_web::assets::CSP, "still strict");
    let (_, _, css) = h
        .raw("GET", &format!("/branding/theme/{v}/full.css"), None, vec![], &[])
        .await;
    assert!(String::from_utf8(css).unwrap().contains("border-width"));
}
