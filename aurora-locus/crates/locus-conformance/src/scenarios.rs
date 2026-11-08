//! The scenarios, in order (later steps use what earlier ones captured).

use serde_json::{Value, json};

use crate::groups;
use crate::runner::{Auth, Step};

pub fn post(text: &str) -> Value {
    json!({"$type": "app.bsky.feed.post", "text": text, "createdAt": "2026-11-06T00:00:00.000Z"})
}

/// A one-pixel PNG.
pub const PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

/// A one-pixel GIF.
pub const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";

/// Every group, in order.
pub fn all() -> Vec<Step> {
    [
        basics(),
        groups::no_input_bodies(),
        records(),
        groups::sync(),
        groups::migration(),
        groups::email(),
        groups::oauth(),
        groups::pipethrough(),
        groups::admin(),
        lifecycle(),
    ]
    .concat()
}

/// Accounts, sessions, app passwords, service auth, handles.
fn basics() -> Vec<Step> {
    let me = Auth::User("alice");
    vec![
        // Server.
        Step::get("server.describeServer", "com.atproto.server.describeServer"),
        Step::post("server.createAccount.badHandle", "com.atproto.server.createAccount",
            json!({"handle": "no", "email": "x@example.com", "password": "password-123"})),
        Step::post("server.createAccount.otherDomain", "com.atproto.server.createAccount",
            json!({"handle": "alice.elsewhere.example", "email": "x@example.com", "password": "password-123"})),
        Step::post("server.createAccount.noEmail", "com.atproto.server.createAccount",
            json!({"handle": "carol.test", "password": "password-123"})),
        Step::post("server.createAccount", "com.atproto.server.createAccount",
            json!({"handle": "alice.test", "email": "alice@example.com", "password": "alice-password"}))
            .keep("alice.did", "/did").keep("alice.access", "/accessJwt").keep("alice.refresh", "/refreshJwt"),
        Step::post("server.createAccount.taken", "com.atproto.server.createAccount",
            json!({"handle": "alice.test", "email": "alice2@example.com", "password": "alice-password"})),
        Step::post("server.createAccount.bob", "com.atproto.server.createAccount",
            json!({"handle": "bob.test", "email": "bob@example.com", "password": "bob-password"}))
            .keep("bob.did", "/did").keep("bob.access", "/accessJwt"),
        Step::post("server.createSession.wrongPassword", "com.atproto.server.createSession",
            json!({"identifier": "alice.test", "password": "nope"})),
        Step::post("server.createSession.unknown", "com.atproto.server.createSession",
            json!({"identifier": "nobody.test", "password": "nope"})),
        Step::post("server.createSession", "com.atproto.server.createSession",
            json!({"identifier": "alice.test", "password": "alice-password"}))
            .keep("alice.access", "/accessJwt").keep("alice.refresh", "/refreshJwt"),
        Step::post("server.createSession.byEmail", "com.atproto.server.createSession",
            json!({"identifier": "alice@example.com", "password": "alice-password"})),
        Step::get("server.getSession", "com.atproto.server.getSession").auth(me),
        Step::get("server.getSession.none", "com.atproto.server.getSession"),
        Step::get("server.getSession.bogus", "com.atproto.server.getSession").auth(Auth::Bogus),
        Step::get("server.getSession.refreshToken", "com.atproto.server.getSession").auth(Auth::Refresh("alice")),
        Step::call("server.refreshSession", "com.atproto.server.refreshSession")
            .auth(Auth::Refresh("alice")).keep("alice.access", "/accessJwt").keep("alice.refresh", "/refreshJwt"),
        Step::call("server.refreshSession.withAccess", "com.atproto.server.refreshSession").auth(me),
        Step::post("server.createAppPassword", "com.atproto.server.createAppPassword", json!({"name": "client one"})).auth(me),
        Step::post("server.createAppPassword.again", "com.atproto.server.createAppPassword", json!({"name": "client one"})).auth(me),
        Step::get("server.listAppPasswords", "com.atproto.server.listAppPasswords").auth(me),
        Step::post("server.revokeAppPassword", "com.atproto.server.revokeAppPassword", json!({"name": "client one"})).auth(me),
        Step::get("server.listAppPasswords.after", "com.atproto.server.listAppPasswords").auth(me),
        Step::get("server.checkAccountStatus", "com.atproto.server.checkAccountStatus").auth(me),
        Step::get("server.getServiceAuth", "com.atproto.server.getServiceAuth").auth(me).q("aud", "did:web:example.com"),
        Step::get("server.getServiceAuth.noAud", "com.atproto.server.getServiceAuth").auth(me),
        // Identity.
        Step::get("identity.resolveHandle", "com.atproto.identity.resolveHandle").q("handle", "alice.test"),
        Step::get("identity.resolveHandle.unknown", "com.atproto.identity.resolveHandle").q("handle", "nobody.test"),
        Step::get("identity.resolveHandle.invalid", "com.atproto.identity.resolveHandle").q("handle", "not a handle"),
        Step::post("identity.updateHandle.taken", "com.atproto.identity.updateHandle", json!({"handle": "bob.test"})).auth(me),
        Step::post("identity.updateHandle.otherDomain", "com.atproto.identity.updateHandle",
            json!({"handle": "alice.elsewhere.example"})).auth(me),
    ]
}

/// Records, blobs and the first sync reads.
#[allow(clippy::too_many_lines)]
fn records() -> Vec<Step> {
    let me = Auth::User("alice");
    vec![
        // Records.
        Step::post("repo.createRecord", "com.atproto.repo.createRecord",
            json!({"repo": "{{alice.did}}", "collection": "app.bsky.feed.post", "record": post("hello")}))
            .auth(me).keep("post1.uri", "/uri").keep("post1.cid", "/cid").keep("post1.rev", "/commit/rev"),
        Step::post("repo.createRecord.noAuth", "com.atproto.repo.createRecord",
            json!({"repo": "{{alice.did}}", "collection": "app.bsky.feed.post", "record": post("x")})),
        Step::post("repo.createRecord.otherRepo", "com.atproto.repo.createRecord",
            json!({"repo": "{{bob.did}}", "collection": "app.bsky.feed.post", "record": post("x")})).auth(me),
        Step::post("repo.createRecord.invalid", "com.atproto.repo.createRecord",
            json!({"repo": "{{alice.did}}", "collection": "app.bsky.feed.post", "record": {"$type": "app.bsky.feed.post"}})).auth(me),
        Step::post("repo.createRecord.badCollection", "com.atproto.repo.createRecord",
            json!({"repo": "{{alice.did}}", "collection": "not-an-nsid", "record": post("x")})).auth(me),
        Step::post("repo.createRecord.unknownType", "com.atproto.repo.createRecord",
            json!({"repo": "{{alice.did}}", "collection": "com.example.thing", "record": {"$type": "com.example.thing", "a": 1}})).auth(me),
        Step::post("repo.createRecord.unknownTypeValidate", "com.atproto.repo.createRecord",
            json!({"repo": "{{alice.did}}", "collection": "com.example.thing", "validate": true, "record": {"$type": "com.example.thing", "a": 1}})).auth(me),
        Step::post("repo.putRecord.profile", "com.atproto.repo.putRecord",
            json!({"repo": "{{alice.did}}", "collection": "app.bsky.actor.profile", "rkey": "self",
                   "record": {"$type": "app.bsky.actor.profile", "displayName": "Alice"}})).auth(me),
        Step::post("repo.putRecord.swapMismatch", "com.atproto.repo.putRecord",
            json!({"repo": "{{alice.did}}", "collection": "app.bsky.actor.profile", "rkey": "self",
                   "swapRecord": "{{post1.cid}}", "record": {"$type": "app.bsky.actor.profile", "displayName": "A"}})).auth(me),
        Step::post("repo.createRecord.swapCommitMismatch", "com.atproto.repo.createRecord",
            json!({"repo": "{{alice.did}}", "collection": "app.bsky.feed.post", "swapCommit": "{{post1.cid}}", "record": post("x")})).auth(me),
        Step::get("repo.getRecord", "com.atproto.repo.getRecord")
            .q("repo", "{{alice.did}}").q("collection", "app.bsky.actor.profile").q("rkey", "self"),
        Step::get("repo.getRecord.byHandle", "com.atproto.repo.getRecord")
            .q("repo", "alice.test").q("collection", "app.bsky.actor.profile").q("rkey", "self"),
        Step::get("repo.getRecord.missing", "com.atproto.repo.getRecord")
            .q("repo", "{{alice.did}}").q("collection", "app.bsky.feed.post").q("rkey", "3zzzzzzzzzzzz"),
        Step::get("repo.getRecord.unknownRepo", "com.atproto.repo.getRecord")
            .q("repo", "did:plc:aaaaaaaaaaaaaaaaaaaaaaaa").q("collection", "app.bsky.feed.post").q("rkey", "x"),
        Step::post("repo.applyWrites", "com.atproto.repo.applyWrites", json!({"repo": "{{alice.did}}", "writes": [
            {"$type": "com.atproto.repo.applyWrites#create", "collection": "app.bsky.feed.post", "rkey": "3kaaaaaaaaaa2", "value": post("two")},
            {"$type": "com.atproto.repo.applyWrites#update", "collection": "app.bsky.actor.profile", "rkey": "self",
             "value": {"$type": "app.bsky.actor.profile", "displayName": "Alice B"}},
        ]})).auth(me),
        Step::post("repo.applyWrites.deleteMissing", "com.atproto.repo.applyWrites", json!({"repo": "{{alice.did}}", "writes": [
            {"$type": "com.atproto.repo.applyWrites#delete", "collection": "app.bsky.feed.post", "rkey": "3zzzzzzzzzzzz"},
        ]})).auth(me),
        Step::get("repo.listRecords", "com.atproto.repo.listRecords")
            .q("repo", "{{alice.did}}").q("collection", "app.bsky.feed.post"),
        Step::get("repo.listRecords.reverse", "com.atproto.repo.listRecords")
            .q("repo", "{{alice.did}}").q("collection", "app.bsky.feed.post").q("reverse", "true").q("limit", "1"),
        Step::get("repo.describeRepo", "com.atproto.repo.describeRepo").q("repo", "{{alice.did}}"),
        Step::post("repo.deleteRecord", "com.atproto.repo.deleteRecord",
            json!({"repo": "{{alice.did}}", "collection": "app.bsky.feed.post", "rkey": "3kaaaaaaaaaa2"})).auth(me),
        Step::post("repo.deleteRecord.again", "com.atproto.repo.deleteRecord",
            json!({"repo": "{{alice.did}}", "collection": "app.bsky.feed.post", "rkey": "3kaaaaaaaaaa2"})).auth(me),
        // Blobs.
        Step::post("repo.uploadBlob", "com.atproto.repo.uploadBlob", json!(null)).bytes(PNG, "image/png").auth(me)
            .keep("blob.cid", "/blob/ref/$link"),
        Step::post("repo.uploadBlob.noAuth", "com.atproto.repo.uploadBlob", json!(null)).bytes(PNG, "image/png"),
        Step::post("repo.createRecord.withImage", "com.atproto.repo.createRecord",
            json!({"repo": "{{alice.did}}", "collection": "app.bsky.feed.post", "record": {
                "$type": "app.bsky.feed.post", "text": "pic", "createdAt": "2026-11-06T00:00:00.000Z",
                "embed": {"$type": "app.bsky.embed.images", "images": [{"alt": "", "image": {
                    "$type": "blob", "ref": {"$link": "{{blob.cid}}"}, "mimeType": "image/png", "size": PNG.len()}}]}}}))
            .auth(me),
        Step::post("repo.createRecord.missingBlob", "com.atproto.repo.createRecord",
            json!({"repo": "{{alice.did}}", "collection": "app.bsky.feed.post", "record": {
                "$type": "app.bsky.feed.post", "text": "pic", "createdAt": "2026-11-06T00:00:00.000Z",
                "embed": {"$type": "app.bsky.embed.images", "images": [{"alt": "", "image": {
                    "$type": "blob", "ref": {"$link": "bafkreigh2akiscaildcqabsyg3dfr6chu3fgpregiymsck7e7aqa4s52zy"},
                    "mimeType": "image/png", "size": 10}}]}}}))
            .auth(me),
        Step::get("repo.listMissingBlobs", "com.atproto.repo.listMissingBlobs").auth(me),
        Step::get("sync.getBlob", "com.atproto.sync.getBlob").q("did", "{{alice.did}}").q("cid", "{{blob.cid}}"),
        Step::get("sync.getBlob.missing", "com.atproto.sync.getBlob")
            .q("did", "{{alice.did}}").q("cid", "bafkreigh2akiscaildcqabsyg3dfr6chu3fgpregiymsck7e7aqa4s52zy"),
        Step::get("sync.listBlobs", "com.atproto.sync.listBlobs").q("did", "{{alice.did}}"),
        // Sync reads.
        Step::get("sync.getLatestCommit", "com.atproto.sync.getLatestCommit").q("did", "{{alice.did}}"),
        Step::get("sync.getLatestCommit.unknown", "com.atproto.sync.getLatestCommit").q("did", "did:plc:aaaaaaaaaaaaaaaaaaaaaaaa"),
        Step::get("sync.getRepo", "com.atproto.sync.getRepo").q("did", "{{alice.did}}"),
        Step::get("sync.getRepo.since", "com.atproto.sync.getRepo").q("did", "{{alice.did}}").q("since", "{{post1.rev}}"),
        Step::get("sync.getRecord", "com.atproto.sync.getRecord")
            .q("did", "{{alice.did}}").q("collection", "app.bsky.actor.profile").q("rkey", "self"),
        Step::get("sync.getRepoStatus", "com.atproto.sync.getRepoStatus").q("did", "{{alice.did}}"),
        Step::get("sync.listRepos", "com.atproto.sync.listRepos"),
        Step::get("sync.getBlocks.missing", "com.atproto.sync.getBlocks")
            .q("did", "{{alice.did}}").q("cids", "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifvx5xytvske7mr3hpm"),
    ]
}

/// Takedown of bob, then the account lifecycle of alice (last: it ends
/// alice's session).
fn lifecycle() -> Vec<Step> {
    let me = Auth::User("alice");
    vec![
        // Admin.
        Step::get("admin.getAccountInfo", "com.atproto.admin.getAccountInfo").q("did", "{{bob.did}}").auth(Auth::Admin),
        Step::get("admin.getAccountInfo.noAuth", "com.atproto.admin.getAccountInfo").q("did", "{{bob.did}}"),
        Step::post("admin.updateSubjectStatus", "com.atproto.admin.updateSubjectStatus", json!({
            "subject": {"$type": "com.atproto.admin.defs#repoRef", "did": "{{bob.did}}"},
            "takedown": {"applied": true, "ref": "case-1"}})).auth(Auth::Admin),
        Step::get("admin.getSubjectStatus", "com.atproto.admin.getSubjectStatus").q("did", "{{bob.did}}").auth(Auth::Admin),
        Step::get("sync.getRepo.takenDown", "com.atproto.sync.getRepo").q("did", "{{bob.did}}"),
        Step::get("sync.getRepoStatus.takenDown", "com.atproto.sync.getRepoStatus").q("did", "{{bob.did}}"),
        Step::post("server.createSession.takenDown", "com.atproto.server.createSession",
            json!({"identifier": "bob.test", "password": "bob-password"})),
        // Account lifecycle.
        Step::post("server.deactivateAccount", "com.atproto.server.deactivateAccount", json!({})).auth(me),
        Step::get("sync.getRepoStatus.deactivated", "com.atproto.sync.getRepoStatus").q("did", "{{alice.did}}"),
        Step::post("repo.createRecord.deactivated", "com.atproto.repo.createRecord",
            json!({"repo": "{{alice.did}}", "collection": "app.bsky.feed.post", "record": post("x")})).auth(me),
        Step::call("server.activateAccount", "com.atproto.server.activateAccount").auth(me),
        Step::call("server.deleteSession", "com.atproto.server.deleteSession").auth(Auth::Refresh("alice")),
        Step::get("server.getSession.afterDelete", "com.atproto.server.getSession").auth(Auth::Refresh("alice")),
    ]
}
