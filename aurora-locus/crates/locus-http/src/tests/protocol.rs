//! Robustness of the XRPC and OAuth endpoints: every route, signed in as a
//! user (and as the admin for admin routes), given garbage query strings
//! and bodies, answers with a 4xx (or a 2xx when the garbage happens to be
//! acceptable), never a 5xx or a panic.

use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};

use super::*;

/// Every XRPC method a plain PDS serves itself (proxied `app.bsky.*` reads
/// excluded: their upstream answers for them; entryway methods aren't
/// mounted here).
const XRPC: &[&str] = &[
    "com.atproto.temp.checkSignupQueue",
    "com.atproto.server.describeServer",
    "com.atproto.server.createAccount",
    "com.atproto.server.createSession",
    "com.atproto.server.reserveSigningKey",
    "com.atproto.server.getSession",
    "com.atproto.server.refreshSession",
    "com.atproto.server.createAppPassword",
    "com.atproto.server.listAppPasswords",
    "com.atproto.server.revokeAppPassword",
    "com.atproto.server.requestEmailConfirmation",
    "com.atproto.server.confirmEmail",
    "com.atproto.server.requestEmailUpdate",
    "com.atproto.server.updateEmail",
    "com.atproto.server.requestPasswordReset",
    "com.atproto.server.resetPassword",
    "com.atproto.server.getAccountInviteCodes",
    "com.atproto.server.getServiceAuth",
    "com.atproto.server.deactivateAccount",
    "com.atproto.server.activateAccount",
    "com.atproto.server.checkAccountStatus",
    "com.atproto.server.requestAccountDelete",
    "com.atproto.server.deleteAccount",
    "com.atproto.server.createInviteCode",
    "com.atproto.server.createInviteCodes",
    "com.atproto.repo.createRecord",
    "com.atproto.repo.putRecord",
    "com.atproto.repo.deleteRecord",
    "com.atproto.repo.applyWrites",
    "com.atproto.repo.getRecord",
    "com.atproto.repo.listRecords",
    "com.atproto.repo.describeRepo",
    "com.atproto.repo.uploadBlob",
    "com.atproto.repo.importRepo",
    "com.atproto.repo.listMissingBlobs",
    "com.atproto.sync.getBlob",
    "com.atproto.sync.subscribeRepos",
    "com.atproto.sync.getRepo",
    "com.atproto.sync.getCheckout",
    "com.atproto.sync.getRecord",
    "com.atproto.sync.getBlocks",
    "com.atproto.sync.getLatestCommit",
    "com.atproto.sync.getHead",
    "com.atproto.sync.getRepoStatus",
    "com.atproto.sync.listRepos",
    "com.atproto.sync.listBlobs",
    "com.atproto.identity.resolveHandle",
    "com.atproto.identity.updateHandle",
    "com.atproto.identity.getRecommendedDidCredentials",
    "com.atproto.identity.requestPlcOperationSignature",
    "com.atproto.identity.signPlcOperation",
    "com.atproto.identity.submitPlcOperation",
    "app.bsky.actor.getPreferences",
    "app.bsky.actor.putPreferences",
    "app.bsky.notification.registerPush",
    "app.bsky.notification.unregisterPush",
    "tools.kryphocron.feed.createPostPrivate",
    "tools.kryphocron.feed.updatePostPrivate",
    "tools.kryphocron.feed.deletePostPrivate",
    "tools.kryphocron.actor.participatePrivate",
    "tools.kryphocron.policy.manageAudience",
    "tools.kryphocron.policy.deleteAudience",
    "com.atproto.admin.getAccountInfo",
    "com.atproto.admin.getAccountInfos",
    "com.atproto.admin.searchAccounts",
    "com.atproto.admin.getSubjectStatus",
    "com.atproto.admin.updateSubjectStatus",
    "com.atproto.admin.sendEmail",
    "com.atproto.admin.updateAccountPassword",
    "com.atproto.admin.updateAccountEmail",
    "com.atproto.admin.updateAccountHandle",
    "com.atproto.admin.getInviteCodes",
    "com.atproto.admin.disableAccountInvites",
    "com.atproto.admin.enableAccountInvites",
    "com.atproto.admin.disableInviteCodes",
];

/// The OAuth endpoints that read requests.
const OAUTH: &[&str] = &[
    "/oauth/par",
    "/oauth/token",
    "/oauth/authorize",
    "/oauth/authorize/sign-in",
    "/oauth/authorize/accept",
    "/oauth/authorize/reject",
];

/// Parameter names the routes read, so garbage reaches their parsing.
const PARAMS: &[&str] = &[
    "repo", "collection", "rkey", "cid", "cids", "did", "limit", "cursor", "since", "handle",
    "uri", "uris", "actor", "dids", "subject", "reverse", "includeTakedowns", "email",
    "request_uri", "client_id", "response_type", "redirect_uri", "code", "grant_type",
    "code_verifier", "refresh_token", "scope", "state", "code_challenge", "lxm", "aud", "exp",
];

fn value() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        Just("-1".to_owned()),
        Just("0".to_owned()),
        Just("99999999999999999999999".to_owned()),
        Just("did:plc:".to_owned()),
        Just("at://did:plc:x/app.bsky.feed.post/3jzfcijpj2z2a".to_owned()),
        Just("%FF%FE".to_owned()),
        Just("%00".to_owned()),
        Just("bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454".to_owned()),
        Just("2024-01-01T00:00:00.11\u{e9}Z".to_owned()),
        "[a-zA-Z0-9:./_%-]{0,40}",
        ".{0,20}",
    ]
}

fn query() -> impl Strategy<Value = String> {
    let pair = (
        prop_oneof![4 => prop::sample::select(PARAMS).prop_map(str::to_owned), 1 => "[a-z]{1,5}"],
        value(),
    );
    prop::collection::vec(pair, 0..6).prop_map(|ps| {
        ps.iter()
            .map(|(k, v)| {
                // Keep the URI parseable; percent-encode what isn't safe.
                let v: String = v
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || ":./_%-".contains(c) {
                            c.to_string()
                        } else {
                            let mut b = [0u8; 4];
                            c.encode_utf8(&mut b)
                                .bytes()
                                .map(|x| format!("%{x:02X}"))
                                .collect()
                        }
                    })
                    .collect();
                format!("{k}={v}")
            })
            .collect::<Vec<_>>()
            .join("&")
    })
}

fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        Just(json!(1.5)),
        value().prop_map(Value::from),
    ];
    leaf.prop_recursive(3, 24, 5, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::btree_map(
                prop_oneof![
                    3 => prop::sample::select(PARAMS).prop_map(str::to_owned),
                    1 => prop::sample::select(&["record", "writes", "$type", "value", "action",
                        "password", "token", "preferences", "status", "takedown", "validate", "deleteAfter", "birthDate"][..])
                        .prop_map(str::to_owned),
                ],
                inner,
                0..5,
            )
            .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

#[derive(Debug, Clone)]
enum Payload {
    None,
    Json(Value),
    Raw(Vec<u8>, &'static str),
}

fn body() -> impl Strategy<Value = Payload> {
    prop_oneof![
        1 => Just(Payload::None),
        4 => json_value().prop_map(Payload::Json),
        1 => (
            prop::collection::vec(any::<u8>(), 0..64),
            prop::sample::select(&[
                "application/json",
                "application/x-www-form-urlencoded",
                "application/vnd.ipld.car",
                "image/png",
                "text/plain",
                "",
            ][..]),
        )
            .prop_map(|(b, t)| Payload::Raw(b, t)),
    ]
}

impl Harness {
    async fn garbage(&self, method: &str, path: &str, auth: Option<&str>, body: &Payload) -> StatusCode {
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if let Some(a) = auth {
            headers.push(("authorization", a));
        }
        let bytes = match body {
            Payload::None => Vec::new(),
            Payload::Json(v) => {
                headers.push(("content-type", "application/json"));
                v.to_string().into_bytes()
            }
            Payload::Raw(b, t) => {
                if !t.is_empty() {
                    headers.push(("content-type", t));
                }
                b.clone()
            }
        };
        self.raw(method, path, None, bytes, &headers).await.0
    }
}

/// Garbage at every route, from a user and (for admin routes) the admin.
#[tokio::test(flavor = "multi_thread")]
async fn xrpc_and_oauth_routes_answer_garbage_with_4xx() {
    let h = harness(false).await;
    let (_did, access) = h.user("fuzz.pds.example.com").await;
    let bearer = format!("Bearer {access}");
    let admin = h.basic_admin();
    let mut runner = TestRunner::new(Config {
        cases: 2000,
        ..Config::default()
    });
    let routes: Vec<String> = XRPC
        .iter()
        .map(|n| format!("/xrpc/{n}"))
        .chain(OAUTH.iter().map(|p| (*p).to_owned()))
        .collect();
    let strategy = (
        prop::sample::select(routes),
        // Mostly the route's own method (queries GET, procedures POST).
        prop::bool::weighted(0.1),
        query(),
        body(),
        any::<bool>(),
    )
        .prop_map(|(route, wrong, q, b, signed)| {
            let nsid = route.rsplit('/').next().unwrap_or_default();
            let name = nsid.rsplit('.').next().unwrap_or_default();
            let query = ["get", "list", "describe", "resolve", "check", "search", "subscribe"]
                .iter()
                .any(|p| name.starts_with(p))
                || route == "/oauth/authorize";
            let method = if query != wrong { "GET" } else { "POST" };
            (route, method, q, b, signed)
        });
    let handle = tokio::runtime::Handle::current();
    let failures = std::sync::Mutex::new(Vec::new());
    let seen = std::sync::Mutex::new(std::collections::BTreeMap::<u16, usize>::new());
    // The runner is synchronous; each case runs on this test's runtime.
    tokio::task::block_in_place(|| {
        runner
            .run(&strategy, |(route, method, q, b, signed)| {
                let auth = match (signed, route.contains(".admin.")) {
                    (false, _) => None,
                    (true, true) => Some(admin.as_str()),
                    (true, false) => Some(bearer.as_str()),
                };
                let path = if q.is_empty() { route.clone() } else { format!("{route}?{q}") };
                let st = handle.block_on(h.garbage(method, &path, auth, &b));
                *seen.lock().unwrap().entry(st.as_u16()).or_insert(0) += 1;
                // Collected rather than shrunk, so one run lists them all.
                if st.is_server_error() {
                    failures.lock().unwrap().push(format!("{st} {method} {path} {b:?}"));
                }
                Ok(())
            })
            .unwrap();
    });
    let failures = failures.into_inner().unwrap();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    // The garbage got past sign-in and routing to the handlers' own checks.
    let seen = seen.into_inner().unwrap();
    assert!(seen.get(&400).copied().unwrap_or(0) > 100, "{seen:?}");
    assert!(seen.get(&200).copied().unwrap_or(0) > 10, "{seen:?}");
}

/// Datetimes with a multi-byte character in the fraction panicked the
/// handler: `deleteAfter` at once, a stored birth date on every later
/// `getPreferences`. Now a 400, and preferences read back.
#[tokio::test]
async fn hostile_datetimes_in_requests_are_refused_not_panicked_on() {
    let h = harness(false).await;
    let (_did, access) = h.user("dates.pds.example.com").await;
    let bad = "2024-01-01T00:00:00.11\u{e9}Z";
    let (st, v) = h
        .post(
            "com.atproto.server.deactivateAccount",
            Some(&access),
            json!({"deleteAfter": bad}),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    let pref = json!({"$type": "app.bsky.actor.defs#personalDetailsPref", "birthDate": bad});
    let (st, v) = h
        .post(
            "app.bsky.actor.putPreferences",
            Some(&access),
            json!({ "preferences": [pref] }),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (st, v) = h
        .call("GET", "/xrpc/app.bsky.actor.getPreferences", Some(&access), None)
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let age = v["preferences"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["$type"] == "app.bsky.actor.defs#declaredAgePref")
        .cloned()
        .unwrap();
    assert_eq!(age["isOverAge13"], json!(false), "unparseable: false");
}
