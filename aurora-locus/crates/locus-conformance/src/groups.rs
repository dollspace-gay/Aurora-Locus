//! The later scenario groups: bodies on no-input procedures, sync and the
//! firehose, migration, email, OAuth, pipethrough, operator methods.

use serde_json::json;

use crate::runner::{Auth, Dpop, Step};
use crate::scenarios::{GIF, PNG, post};

const UNKNOWN: &str = "did:plc:aaaaaaaaaaaaaaaaaaaaaaaa";

/// Procedures whose lexicon has no input, sent a body anyway (the token is
/// checked before the body).
pub fn no_input_bodies() -> Vec<Step> {
    let me = Auth::User("alice");
    let body = || json!({});
    vec![
        Step::post(
            "noInput.refreshSession",
            "com.atproto.server.refreshSession",
            body(),
        )
        .auth(Auth::Refresh("alice")),
        Step::post(
            "noInput.refreshSession.bogus",
            "com.atproto.server.refreshSession",
            body(),
        )
        .auth(Auth::Bogus),
        Step::post(
            "noInput.deleteSession",
            "com.atproto.server.deleteSession",
            body(),
        )
        .auth(Auth::Refresh("alice")),
        Step::post(
            "noInput.activateAccount",
            "com.atproto.server.activateAccount",
            body(),
        )
        .auth(me),
        Step::post(
            "noInput.activateAccount.noAuth",
            "com.atproto.server.activateAccount",
            body(),
        ),
        Step::post(
            "noInput.requestAccountDelete",
            "com.atproto.server.requestAccountDelete",
            body(),
        )
        .auth(me),
        Step::post(
            "noInput.requestEmailConfirmation",
            "com.atproto.server.requestEmailConfirmation",
            body(),
        )
        .auth(me),
        Step::post(
            "noInput.requestEmailUpdate",
            "com.atproto.server.requestEmailUpdate",
            body(),
        )
        .auth(me),
        Step::post(
            "noInput.requestPlcOperationSignature",
            "com.atproto.identity.requestPlcOperationSignature",
            body(),
        )
        .auth(me),
        // The refresh token still works: nothing above used it.
        Step::call(
            "noInput.refreshSession.after",
            "com.atproto.server.refreshSession",
        )
        .auth(Auth::Refresh("alice"))
        .keep("alice.access", "/accessJwt")
        .keep("alice.refresh", "/refreshJwt"),
    ]
}

/// The firehose with cursors, and sync reads' edge cases.
#[allow(clippy::too_many_lines)]
pub fn sync() -> Vec<Step> {
    let me = Auth::User("alice");
    vec![
        // Backfill from the start: every event so far, in order.
        Step::subscribe("firehose.backfill", "0").keep("fh.last", ""),
        Step::post("sync.createRecord.forFirehose", "com.atproto.repo.createRecord",
            json!({"repo": "{{alice.did}}", "collection": "app.bsky.feed.post", "rkey": "3kaaaaaaaaaa3", "record": post("three")}))
            .auth(me).keep("post3.cid", "/cid"),
        // From the middle: only what came after.
        Step::subscribe("firehose.fromCursor", "{{fh.last}}"),
        Step::subscribe("firehose.futureCursor", "99999999"),
        Step::subscribe("firehose.badCursor", "abc"),
        Step::subscribe("firehose.noCursor", ""),
        // getRepo, getBlocks, getRecord.
        Step::get("sync.getRepo.unknown", "com.atproto.sync.getRepo").q("did", UNKNOWN),
        Step::get("sync.getRepo.badDid", "com.atproto.sync.getRepo").q("did", "not-a-did"),
        Step::get("sync.getRepo.badSince", "com.atproto.sync.getRepo").q("did", "{{alice.did}}").q("since", "nope"),
        Step::get("sync.getRepo.futureSince", "com.atproto.sync.getRepo").q("did", "{{alice.did}}").q("since", "7zzzzzzzzzzzz"),
        Step::get("sync.getBlocks", "com.atproto.sync.getBlocks").q("did", "{{alice.did}}").q("cids", "{{post3.cid}}"),
        Step::get("sync.getBlocks.two", "com.atproto.sync.getBlocks")
            .q("did", "{{alice.did}}").q("cids", "{{post3.cid}}").q("cids", "{{post1.cid}}"),
        Step::get("sync.getBlocks.noCids", "com.atproto.sync.getBlocks").q("did", "{{alice.did}}"),
        Step::get("sync.getBlocks.badCid", "com.atproto.sync.getBlocks").q("did", "{{alice.did}}").q("cids", "nope"),
        Step::get("sync.getBlocks.unknownRepo", "com.atproto.sync.getBlocks").q("did", UNKNOWN).q("cids", "{{post3.cid}}"),
        Step::get("sync.getRecord.missing", "com.atproto.sync.getRecord")
            .q("did", "{{alice.did}}").q("collection", "app.bsky.feed.post").q("rkey", "3zzzzzzzzzzzz"),
        Step::get("sync.getRecord.unknownRepo", "com.atproto.sync.getRecord")
            .q("did", UNKNOWN).q("collection", "app.bsky.feed.post").q("rkey", "3zzzzzzzzzzzz"),
        Step::get("sync.getRecord.badCollection", "com.atproto.sync.getRecord")
            .q("did", "{{alice.did}}").q("collection", "nope").q("rkey", "self"),
        Step::get("sync.getHead", "com.atproto.sync.getHead").q("did", "{{alice.did}}"),
        Step::get("sync.getCheckout", "com.atproto.sync.getCheckout").q("did", "{{alice.did}}"),
        // listRepos paging.
        Step::get("sync.listRepos.page1", "com.atproto.sync.listRepos").q("limit", "1").keep("lr.cursor", "/cursor"),
        Step::get("sync.listRepos.page2", "com.atproto.sync.listRepos").q("limit", "1").q("cursor", "{{lr.cursor}}"),
        Step::get("sync.listRepos.badLimit", "com.atproto.sync.listRepos").q("limit", "0"),
        Step::get("sync.listRepos.bigLimit", "com.atproto.sync.listRepos").q("limit", "1001"),
        Step::get("sync.getRepoStatus.unknown", "com.atproto.sync.getRepoStatus").q("did", UNKNOWN),
        // listBlobs since and paging.
        Step::post("sync.uploadBlob.second", "com.atproto.repo.uploadBlob", json!(null)).bytes(GIF, "image/gif").auth(me)
            .keep("blob2.cid", "/blob/ref/$link"),
        Step::post("sync.createRecord.withSecondBlob", "com.atproto.repo.createRecord",
            json!({"repo": "{{alice.did}}", "collection": "app.bsky.feed.post", "record": {
                "$type": "app.bsky.feed.post", "text": "gif", "createdAt": "2026-11-06T00:00:00.000Z",
                "embed": {"$type": "app.bsky.embed.images", "images": [{"alt": "", "image": {
                    "$type": "blob", "ref": {"$link": "{{blob2.cid}}"}, "mimeType": "image/gif", "size": GIF.len()}}]}}}))
            .auth(me),
        Step::get("sync.listBlobs.since", "com.atproto.sync.listBlobs").q("did", "{{alice.did}}").q("since", "{{post1.rev}}"),
        Step::get("sync.listBlobs.page1", "com.atproto.sync.listBlobs").q("did", "{{alice.did}}").q("limit", "1")
            .keep("lb.cursor", "/cursor"),
        Step::get("sync.listBlobs.page2", "com.atproto.sync.listBlobs").q("did", "{{alice.did}}").q("limit", "1")
            .q("cursor", "{{lb.cursor}}"),
        Step::get("sync.listBlobs.unknown", "com.atproto.sync.listBlobs").q("did", UNKNOWN),
        // Relay methods.
        Step::post("sync.requestCrawl", "com.atproto.sync.requestCrawl", json!({"hostname": "localhost"})),
        Step::post("sync.notifyOfUpdate", "com.atproto.sync.notifyOfUpdate", json!({"hostname": "localhost"})),
    ]
}

/// A migration in from the other server: each server imports the other's
/// `dave` (service auth from the source, `createAccount` with the DID, the
/// repo CAR, the missing blobs, the PLC update, activation).
#[allow(clippy::too_many_lines)]
pub fn migration() -> Vec<Step> {
    let dave = Auth::User("dave");
    let mig = Auth::User("mig");
    let svc = Auth::Var("svc.jwt");
    let account = |handle: &str, did: &str| {
        json!({"handle": handle, "email": format!("{}@example.com", handle.trim_end_matches(".test")),
               "password": "mig-password", "did": did})
    };
    vec![
        Step::post("migration.source.createAccount", "com.atproto.server.createAccount",
            json!({"handle": "dave.test", "email": "dave@example.com", "password": "dave-password"}))
            .keep("dave.did", "/did").keep("dave.access", "/accessJwt"),
        Step::post("migration.source.uploadBlob", "com.atproto.repo.uploadBlob", json!(null)).bytes(PNG, "image/png").auth(dave)
            .keep("dave.blob", "/blob/ref/$link"),
        Step::post("migration.source.createRecord", "com.atproto.repo.createRecord",
            json!({"repo": "{{dave.did}}", "collection": "app.bsky.feed.post", "record": {
                "$type": "app.bsky.feed.post", "text": "moving", "createdAt": "2026-11-06T00:00:00.000Z",
                "embed": {"$type": "app.bsky.embed.images", "images": [{"alt": "", "image": {
                    "$type": "blob", "ref": {"$link": "{{dave.blob}}"}, "mimeType": "image/png", "size": PNG.len()}}]}}}))
            .auth(dave),
        Step::post("migration.source.profile", "com.atproto.repo.putRecord",
            json!({"repo": "{{dave.did}}", "collection": "app.bsky.actor.profile", "rkey": "self",
                   "record": {"$type": "app.bsky.actor.profile", "displayName": "Dave"}})).auth(dave),
        // Service auth from the source for createAccount here.
        Step::get("migration.peer.getServiceAuth", "com.atproto.server.getServiceAuth").on_peer().auth(dave)
            .q("aud", "did:web:localhost").q("lxm", "com.atproto.server.createAccount").keep("svc.jwt", "/token"),
        Step::post("migration.createAccount.noAuth", "com.atproto.server.createAccount", account("migrant.test", "{{peer.dave.did}}")),
        Step::post("migration.createAccount.wrongDid", "com.atproto.server.createAccount", account("migrant.test", "{{alice.did}}"))
            .auth(svc),
        Step::post("migration.createAccount", "com.atproto.server.createAccount", account("migrant.test", "{{peer.dave.did}}"))
            .auth(svc).keep("mig.access", "/accessJwt").keep("mig.refresh", "/refreshJwt"),
        Step::get("migration.checkAccountStatus.new", "com.atproto.server.checkAccountStatus").auth(mig),
        Step::get("migration.getSession.new", "com.atproto.server.getSession").auth(mig),
        Step::post("migration.createRecord.inactive", "com.atproto.repo.createRecord",
            json!({"repo": "{{peer.dave.did}}", "collection": "app.bsky.feed.post", "record": post("x")})).auth(mig),
        // The repo from the source.
        Step::get("migration.peer.getRepo", "com.atproto.sync.getRepo").on_peer().q("did", "{{peer.dave.did}}").keep_bytes("car"),
        Step::post("migration.importRepo.noAuth", "com.atproto.repo.importRepo", json!(null))
            .bytes_var("car", "application/vnd.ipld.car"),
        Step::post("migration.importRepo.garbage", "com.atproto.repo.importRepo", json!(null))
            .bytes(b"not a car", "application/vnd.ipld.car").auth(mig),
        Step::post("migration.importRepo", "com.atproto.repo.importRepo", json!(null))
            .bytes_var("car", "application/vnd.ipld.car").auth(mig),
        Step::get("migration.checkAccountStatus.imported", "com.atproto.server.checkAccountStatus").auth(mig),
        Step::get("migration.listMissingBlobs", "com.atproto.repo.listMissingBlobs").auth(mig).keep("missing.cid", "/blobs/0/cid"),
        Step::get("migration.listMissingBlobs.page", "com.atproto.repo.listMissingBlobs").auth(mig).q("limit", "1"),
        Step::get("migration.peer.getBlob", "com.atproto.sync.getBlob").on_peer()
            .q("did", "{{peer.dave.did}}").q("cid", "{{missing.cid}}").keep_bytes("blob"),
        Step::post("migration.uploadBlob", "com.atproto.repo.uploadBlob", json!(null)).bytes_var("blob", "image/png").auth(mig),
        Step::get("migration.listMissingBlobs.after", "com.atproto.repo.listMissingBlobs").auth(mig),
        Step::get("migration.checkAccountStatus.blobs", "com.atproto.server.checkAccountStatus").auth(mig),
        Step::get("migration.listRecords", "com.atproto.repo.listRecords").q("repo", "{{peer.dave.did}}").q("collection", "app.bsky.feed.post"),
        Step::get("migration.getRepoStatus.inactive", "com.atproto.sync.getRepoStatus").q("did", "{{peer.dave.did}}"),
        // The DID document still names the source.
        Step::call("migration.activateAccount.early", "com.atproto.server.activateAccount").auth(mig),
        Step::get("migration.getRecommendedDidCredentials", "com.atproto.identity.getRecommendedDidCredentials").auth(mig)
            .keep("creds.rotationKeys", "/rotationKeys").keep("creds.alsoKnownAs", "/alsoKnownAs")
            .keep("creds.verificationMethods", "/verificationMethods").keep("creds.services", "/services"),
        Step::get("migration.getRecommendedDidCredentials.noAuth", "com.atproto.identity.getRecommendedDidCredentials"),
        Step::plc("migration.plc.update", "{{peer.dave.did}}", json!({
            "type": "plc_operation", "prev": null, "sig": "mock",
            "rotationKeys": "{{creds.rotationKeys}}", "alsoKnownAs": "{{creds.alsoKnownAs}}",
            "verificationMethods": "{{creds.verificationMethods}}", "services": "{{creds.services}}"})),
        Step::call("migration.activateAccount", "com.atproto.server.activateAccount").auth(mig),
        Step::get("migration.checkAccountStatus.active", "com.atproto.server.checkAccountStatus").auth(mig),
        Step::get("migration.getRepoStatus.active", "com.atproto.sync.getRepoStatus").q("did", "{{peer.dave.did}}"),
        Step::get("migration.describeRepo", "com.atproto.repo.describeRepo").q("repo", "{{peer.dave.did}}"),
        Step::post("migration.createRecord.active", "com.atproto.repo.createRecord",
            json!({"repo": "{{peer.dave.did}}", "collection": "app.bsky.feed.post", "record": post("arrived")})).auth(mig),
        // The source deactivates its copy.
        Step::post("migration.peer.deactivate", "com.atproto.server.deactivateAccount", json!({})).on_peer().auth(dave),
        Step::get("migration.source.getRepoStatus", "com.atproto.sync.getRepoStatus").q("did", "{{dave.did}}"),
        Step::get("migration.source.checkAccountStatus", "com.atproto.server.checkAccountStatus").auth(dave),
        // A DID that already has an account here.
        Step::post("migration.createAccount.again", "com.atproto.server.createAccount", account("migrant2.test", "{{peer.dave.did}}"))
            .auth(svc),
    ]
}

/// Email flows on a dedicated account (in `no-mail` mode neither server
/// has SMTP and there are no tokens to read).
#[allow(clippy::too_many_lines)]
pub fn email() -> Vec<Step> {
    let erin = Auth::User("erin");
    vec![
        Step::post("email.createAccount", "com.atproto.server.createAccount",
            json!({"handle": "erin.test", "email": "erin@example.com", "password": "erin-password"}))
            .keep("erin.did", "/did").keep("erin.access", "/accessJwt"),
        Step::call("email.requestEmailConfirmation", "com.atproto.server.requestEmailConfirmation").auth(erin),
        Step::mail("email.confirm.mail", "erin@example.com").keep("tok.confirm", ""),
        Step::post("email.confirmEmail.wrongEmail", "com.atproto.server.confirmEmail",
            json!({"email": "other@example.com", "token": "{{tok.confirm}}"})).auth(erin),
        Step::post("email.confirmEmail.badToken", "com.atproto.server.confirmEmail",
            json!({"email": "erin@example.com", "token": "AAAAA-BBBBB"})).auth(erin),
        Step::post("email.confirmEmail", "com.atproto.server.confirmEmail",
            json!({"email": "erin@example.com", "token": "{{tok.confirm}}"})).auth(erin),
        Step::get("email.getSession.confirmed", "com.atproto.server.getSession").auth(erin),
        // An update needs a token once the email is confirmed.
        Step::post("email.updateEmail.noToken", "com.atproto.server.updateEmail",
            json!({"email": "erin2@example.com"})).auth(erin),
        Step::call("email.requestEmailUpdate", "com.atproto.server.requestEmailUpdate").auth(erin),
        Step::mail("email.update.mail", "erin@example.com").keep("tok.update", ""),
        Step::post("email.updateEmail", "com.atproto.server.updateEmail",
            json!({"email": "erin2@example.com", "token": "{{tok.update}}"})).auth(erin),
        Step::get("email.getSession.updated", "com.atproto.server.getSession").auth(erin),
        Step::post("email.updateEmail.taken", "com.atproto.server.updateEmail",
            json!({"email": "alice@example.com", "token": "{{tok.update}}"})).auth(erin),
        // Password reset.
        Step::post("email.requestPasswordReset", "com.atproto.server.requestPasswordReset", json!({"email": "erin2@example.com"})),
        Step::mail("email.reset.mail", "erin2@example.com").keep("tok.reset", ""),
        Step::post("email.requestPasswordReset.unknown", "com.atproto.server.requestPasswordReset",
            json!({"email": "nobody@example.com"})),
        Step::post("email.resetPassword.badToken", "com.atproto.server.resetPassword",
            json!({"token": "AAAAA-BBBBB", "password": "erin-new-password"})),
        Step::post("email.resetPassword", "com.atproto.server.resetPassword",
            json!({"token": "{{tok.reset}}", "password": "erin-new-password"})),
        Step::post("email.createSession.newPassword", "com.atproto.server.createSession",
            json!({"identifier": "erin.test", "password": "erin-new-password"})).keep("erin.access", "/accessJwt"),
        Step::post("email.createSession.oldPassword", "com.atproto.server.createSession",
            json!({"identifier": "erin.test", "password": "erin-password"})),
        // Account deletion.
        Step::wait("email.wait.beforeDelete", 1500),
        Step::call("email.requestAccountDelete", "com.atproto.server.requestAccountDelete").auth(erin),
        Step::mail("email.delete.mail", "erin2@example.com").keep("tok.delete", ""),
        Step::post("email.deleteAccount.badToken", "com.atproto.server.deleteAccount",
            json!({"did": "{{erin.did}}", "password": "erin-new-password", "token": "AAAAA-BBBBB"})),
        Step::post("email.deleteAccount.badPassword", "com.atproto.server.deleteAccount",
            json!({"did": "{{erin.did}}", "password": "wrong", "token": "{{tok.delete}}"})),
        Step::post("email.deleteAccount", "com.atproto.server.deleteAccount",
            json!({"did": "{{erin.did}}", "password": "erin-new-password", "token": "{{tok.delete}}"})),
        Step::get("email.getRepoStatus.deleted", "com.atproto.sync.getRepoStatus").q("did", "{{erin.did}}"),
    ]
}

/// A loopback client (no metadata document needed).
const CLIENT_ID: &str = "http://localhost?redirect_uri=http%3A%2F%2F127.0.0.1%2Fcallback&scope=atproto%20transition%3Ageneric";

/// OAuth as far as it goes without a browser: metadata, PAR, error shapes,
/// DPoP nonces.
#[allow(clippy::too_many_lines)]
pub fn oauth() -> Vec<Step> {
    let par = |change: &[(&'static str, &'static str)]| {
        let mut f = vec![
            ("client_id", CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", "http://127.0.0.1/callback"),
            ("scope", "atproto transition:generic"),
            ("state", "state-1"),
            (
                "code_challenge",
                "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            ),
            ("code_challenge_method", "S256"),
        ];
        for (k, v) in change {
            f.retain(|(x, _)| x != k);
            if !v.is_empty() {
                f.push((k, v));
            }
        }
        f
    };
    let token = |grant: &[(&'static str, &'static str)]| {
        let mut f = vec![("client_id", CLIENT_ID)];
        f.extend_from_slice(grant);
        f
    };
    vec![
        Step::page(
            "oauth.authorizationServer",
            "/.well-known/oauth-authorization-server",
        ),
        Step::page(
            "oauth.protectedResource",
            "/.well-known/oauth-protected-resource",
        ),
        Step::page("oauth.jwks", "/oauth/jwks"),
        Step::form("oauth.par.noDpop", "/oauth/par", &par(&[])),
        Step::form("oauth.par.dpopNoNonce", "/oauth/par", &par(&[]))
            .dpop(Dpop::Fresh)
            .show("dpop-nonce"),
        Step::form("oauth.par", "/oauth/par", &par(&[]))
            .dpop(Dpop::Nonce)
            .show("dpop-nonce")
            .keep("par.uri", "/request_uri"),
        Step::form(
            "oauth.par.badClient",
            "/oauth/par",
            &par(&[("client_id", "not-a-client")]),
        )
        .dpop(Dpop::Nonce),
        Step::form(
            "oauth.par.badRedirect",
            "/oauth/par",
            &par(&[("redirect_uri", "http://127.0.0.1/elsewhere")]),
        )
        .dpop(Dpop::Nonce),
        Step::form(
            "oauth.par.noAtprotoScope",
            "/oauth/par",
            &par(&[("scope", "transition:generic")]),
        )
        .dpop(Dpop::Nonce),
        Step::form(
            "oauth.par.noChallenge",
            "/oauth/par",
            &par(&[("code_challenge", ""), ("code_challenge_method", "")]),
        )
        .dpop(Dpop::Nonce),
        Step::form(
            "oauth.par.badResponseType",
            "/oauth/par",
            &par(&[("response_type", "token")]),
        )
        .dpop(Dpop::Nonce),
        Step::form(
            "oauth.token.badCode",
            "/oauth/token",
            &token(&[
                ("grant_type", "authorization_code"),
                ("code", "cod-bogus"),
                ("redirect_uri", "http://127.0.0.1/callback"),
                (
                    "code_verifier",
                    "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
                ),
            ]),
        )
        .dpop(Dpop::Nonce),
        Step::form(
            "oauth.token.badGrant",
            "/oauth/token",
            &token(&[("grant_type", "password")]),
        )
        .dpop(Dpop::Nonce),
        Step::form(
            "oauth.token.badRefresh",
            "/oauth/token",
            &token(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", "ref-bogus"),
            ]),
        )
        .dpop(Dpop::Nonce),
        Step::form(
            "oauth.token.noDpop",
            "/oauth/token",
            &token(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", "ref-bogus"),
            ]),
        ),
        Step::form(
            "oauth.revoke.bogus",
            "/oauth/revoke",
            &token(&[("token", "bogus")]),
        )
        .dpop(Dpop::Nonce),
        Step::page("oauth.authorize.unknownRequest", "/oauth/authorize")
            .q("client_id", CLIENT_ID)
            .q(
                "request_uri",
                "urn:ietf:params:oauth:request_uri:req-0000000000000000",
            ),
        // A DPoP-bound token no server issued.
        Step::get("oauth.xrpc.bogusDpopToken", "com.atproto.server.getSession")
            .header(
                "authorization",
                "DPoP eyJhbGciOiJFUzI1NiJ9.eyJzdWIiOiJ4In0.bm90LWEtc2lnbmF0dXJl",
            )
            .dpop(Dpop::Nonce)
            .show("www-authenticate"),
    ]
}

/// app.bsky.* and other unknown methods proxied to the mock AppView, and
/// the preferences served locally.
pub fn pipethrough() -> Vec<Step> {
    let me = Auth::User("alice");
    let appview = "did:web:appview.test#bsky_appview";
    let adult = json!({"$type": "app.bsky.actor.defs#adultContentPref", "enabled": true});
    let search = |name: &str| {
        Step::get(name, "app.bsky.actor.searchActors")
            .auth(me)
            .q("q", "a")
    };
    vec![
        Step::get("proxy.getProfile", "app.bsky.actor.getProfile").auth(me).q("actor", "{{alice.did}}"),
        Step::get("proxy.getProfile.noAuth", "app.bsky.actor.getProfile").q("actor", "{{alice.did}}"),
        search("proxy.searchActors.headers")
            .header("accept-language", "fr").header("atproto-accept-labelers", UNKNOWN)
            .header("x-bsky-topics", "t1").show("content-language"),
        Step::get("proxy.getTimeline", "app.bsky.feed.getTimeline").auth(me),
        search("proxy.header.appview").header("atproto-proxy", appview),
        search("proxy.header.noFragment").header("atproto-proxy", "did:web:appview.test"),
        search("proxy.header.badDid").header("atproto-proxy", "not-a-did#bsky_appview"),
        search("proxy.header.unknownDid").header("atproto-proxy", &format!("{UNKNOWN}#bsky_appview")),
        Step::get("proxy.unknownMethod", "com.example.unknown.method").auth(me),
        Step::get("proxy.unknownMethod.noAuth", "com.example.unknown.method"),
        search("proxy.upstream500").q("fail", "500"),
        search("proxy.upstream400").q("fail", "400"),
        Step::post("proxy.procedure", "app.bsky.graph.muteActor", json!({"actor": "{{bob.did}}"})).auth(me),
        Step::get("proxy.chat", "chat.bsky.convo.listConvos").auth(me),
        // Preferences.
        Step::get("prefs.get.empty", "app.bsky.actor.getPreferences").auth(me),
        Step::get("prefs.get.noAuth", "app.bsky.actor.getPreferences"),
        Step::post("prefs.put", "app.bsky.actor.putPreferences", json!({"preferences": [adult]})).auth(me),
        Step::get("prefs.get", "app.bsky.actor.getPreferences").auth(me),
        Step::post("prefs.put.noType", "app.bsky.actor.putPreferences", json!({"preferences": [{"enabled": true}]})).auth(me),
        Step::post("prefs.put.otherNamespace", "app.bsky.actor.putPreferences",
            json!({"preferences": [{"$type": "com.example.pref", "x": 1}]})).auth(me),
        Step::post("prefs.put.personalDetails", "app.bsky.actor.putPreferences",
            json!({"preferences": [adult, {"$type": "app.bsky.actor.defs#personalDetailsPref", "birthDate": "2000-01-01T00:00:00.000Z"}]}))
            .auth(me),
        Step::get("prefs.get.after", "app.bsky.actor.getPreferences").auth(me),
        Step::get("prefs.get.proxied", "app.bsky.actor.getPreferences").auth(me).header("atproto-proxy", appview),
    ]
}

/// Operator methods, record and blob takedowns and what reads return
/// afterwards.
#[allow(clippy::too_many_lines)]
pub fn admin() -> Vec<Step> {
    let a = Auth::Admin;
    let me = Auth::User("alice");
    let record =
        json!({"$type": "com.atproto.repo.strongRef", "uri": "{{tk.uri}}", "cid": "{{tk.cid}}"});
    let blob = json!({"$type": "com.atproto.admin.defs#repoBlobRef", "did": "{{alice.did}}", "cid": "{{blob.cid}}"});
    let read = |name: &str, nsid: &'static str| {
        Step::get(name, nsid)
            .q("collection", "app.bsky.feed.post")
            .q("rkey", "3kaaaaaaaaaa4")
    };
    vec![
        Step::post("admin.frank.createAccount", "com.atproto.server.createAccount",
            json!({"handle": "frank.test", "email": "frank@example.com", "password": "frank-password"}))
            .keep("frank.did", "/did").keep("frank.access", "/accessJwt"),
        Step::get("admin.getAccountInfos", "com.atproto.admin.getAccountInfos").auth(a)
            .q("dids", "{{alice.did}}").q("dids", "{{frank.did}}").unordered("/infos"),
        Step::get("admin.getAccountInfos.unknown", "com.atproto.admin.getAccountInfos").auth(a).q("dids", UNKNOWN),
        Step::get("admin.getAccountInfo.unknown", "com.atproto.admin.getAccountInfo").auth(a).q("did", UNKNOWN),
        Step::get("admin.getAccountInfo.userAuth", "com.atproto.admin.getAccountInfo").auth(me).q("did", "{{frank.did}}"),
        Step::get("admin.searchAccounts", "com.atproto.admin.searchAccounts").auth(a).q("email", "frank@example.com"),
        Step::get("admin.getInviteCodes", "com.atproto.admin.getInviteCodes").auth(a),
        Step::post("admin.disableAccountInvites", "com.atproto.admin.disableAccountInvites",
            json!({"account": "{{frank.did}}", "note": "x"})).auth(a),
        Step::get("admin.getAccountInfo.invitesDisabled", "com.atproto.admin.getAccountInfo").auth(a).q("did", "{{frank.did}}"),
        Step::post("admin.enableAccountInvites", "com.atproto.admin.enableAccountInvites", json!({"account": "{{frank.did}}"})).auth(a),
        Step::post("admin.disableInviteCodes", "com.atproto.admin.disableInviteCodes", json!({"codes": ["nope"], "accounts": []})).auth(a),
        Step::post("admin.updateAccountEmail", "com.atproto.admin.updateAccountEmail",
            json!({"account": "{{frank.did}}", "email": "frank2@example.com"})).auth(a),
        Step::post("admin.updateAccountEmail.taken", "com.atproto.admin.updateAccountEmail",
            json!({"account": "{{frank.did}}", "email": "alice@example.com"})).auth(a),
        Step::post("admin.updateAccountPassword", "com.atproto.admin.updateAccountPassword",
            json!({"did": "{{frank.did}}", "password": "frank-new-password"})).auth(a),
        Step::post("admin.createSession.newPassword", "com.atproto.server.createSession",
            json!({"identifier": "frank.test", "password": "frank-new-password"})),
        Step::post("admin.updateAccountHandle", "com.atproto.admin.updateAccountHandle",
            json!({"did": "{{frank.did}}", "handle": "frank2.test"})).auth(a),
        Step::post("admin.updateAccountHandle.taken", "com.atproto.admin.updateAccountHandle",
            json!({"did": "{{frank.did}}", "handle": "alice.test"})).auth(a),
        Step::post("admin.updateAccountHandle.invalid", "com.atproto.admin.updateAccountHandle",
            json!({"did": "{{frank.did}}", "handle": "no"})).auth(a),
        Step::get("admin.resolveHandle.updated", "com.atproto.identity.resolveHandle").q("handle", "frank2.test"),
        Step::post("admin.sendEmail", "com.atproto.admin.sendEmail",
            json!({"recipientDid": "{{frank.did}}", "senderDid": "{{alice.did}}", "content": "hello", "subject": "hi"})).auth(a),
        // A record takedown.
        Step::post("admin.createRecord.toTakeDown", "com.atproto.repo.createRecord",
            json!({"repo": "{{alice.did}}", "collection": "app.bsky.feed.post", "rkey": "3kaaaaaaaaaa4", "record": post("four")}))
            .auth(me).keep("tk.uri", "/uri").keep("tk.cid", "/cid"),
        Step::post("admin.takedownRecord", "com.atproto.admin.updateSubjectStatus",
            json!({"subject": record, "takedown": {"applied": true, "ref": "case-2"}})).auth(a),
        Step::get("admin.getSubjectStatus.record", "com.atproto.admin.getSubjectStatus").auth(a).q("uri", "{{tk.uri}}"),
        read("admin.takenDownRecord.getRecord", "com.atproto.repo.getRecord").q("repo", "{{alice.did}}"),
        Step::get("admin.takenDownRecord.listRecords", "com.atproto.repo.listRecords")
            .q("repo", "{{alice.did}}").q("collection", "app.bsky.feed.post"),
        read("admin.takenDownRecord.syncGetRecord", "com.atproto.sync.getRecord").q("did", "{{alice.did}}"),
        Step::post("admin.restoreRecord", "com.atproto.admin.updateSubjectStatus",
            json!({"subject": record, "takedown": {"applied": false}})).auth(a),
        read("admin.restoredRecord.getRecord", "com.atproto.repo.getRecord").q("repo", "{{alice.did}}"),
        // A blob takedown.
        Step::post("admin.takedownBlob", "com.atproto.admin.updateSubjectStatus",
            json!({"subject": blob, "takedown": {"applied": true, "ref": "case-3"}})).auth(a),
        Step::get("admin.getSubjectStatus.blob", "com.atproto.admin.getSubjectStatus").auth(a)
            .q("did", "{{alice.did}}").q("blob", "{{blob.cid}}"),
        Step::get("admin.takenDownBlob.getBlob", "com.atproto.sync.getBlob").q("did", "{{alice.did}}").q("cid", "{{blob.cid}}"),
        Step::get("admin.takenDownBlob.listBlobs", "com.atproto.sync.listBlobs").q("did", "{{alice.did}}"),
        Step::post("admin.restoreBlob", "com.atproto.admin.updateSubjectStatus",
            json!({"subject": blob, "takedown": {"applied": false}})).auth(a),
        Step::get("admin.restoredBlob.getBlob", "com.atproto.sync.getBlob").q("did", "{{alice.did}}").q("cid", "{{blob.cid}}"),
        Step::get("admin.getSubjectStatus.none", "com.atproto.admin.getSubjectStatus").auth(a),
        // Deletion by an operator.
        Step::wait("admin.wait.beforeDelete", 1500),
        Step::post("admin.deleteAccount", "com.atproto.admin.deleteAccount", json!({"did": "{{frank.did}}"})).auth(a),
        Step::get("admin.deleted.getRepoStatus", "com.atproto.sync.getRepoStatus").q("did", "{{frank.did}}"),
        Step::get("admin.deleted.getAccountInfo", "com.atproto.admin.getAccountInfo").auth(a).q("did", "{{frank.did}}"),
        Step::post("admin.deleted.createSession", "com.atproto.server.createSession",
            json!({"identifier": "frank2.test", "password": "frank-new-password"})),
        // A second delete announces the deletion again. The reference
        // deletes a DID's earlier events as it sequences a deletion, and
        // its subscribers poll: without the wait its first deletion event
        // can be gone before it's sent.
        Step::wait("admin.wait.beforeDeleteAgain", 1500),
        Step::post("admin.deleteAccount.again", "com.atproto.admin.deleteAccount", json!({"did": "{{frank.did}}"})).auth(a),
        Step::wait("admin.wait.afterDeleteAgain", 1500),
    ]
}
