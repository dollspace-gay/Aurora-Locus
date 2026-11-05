//! Read-after-write, as the reference: for the requester's own views
//! (`getProfile`, `getProfiles`, `getTimeline`, `getAuthorFeed`,
//! `getPostThread`, `getActorLikes`), the AppView's answer is merged with
//! the records written here after the rev the AppView says it has seen
//! (`atproto-repo-rev`), so a new post or profile edit shows at once. Also
//! `getFeed` (to the feed generator) and `registerPush`/`unregisterPush`.

use std::sync::Arc;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, Method, Uri};
use axum::response::{IntoResponse, Response};
use locus_auth::session::SCOPE_SIGNUP_QUEUED;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth;
use crate::error::{XrpcError, XrpcResult};
use crate::pipethrough::{self as pt, Target};
use crate::state::AppState;

type St = State<Arc<AppState>>;

const PROFILE: &str = "app.bsky.actor.profile";
const POST: &str = "app.bsky.feed.post";
/// How many records after the rev are merged, as the reference.
const SINCE_LIMIT: i64 = 10;

// ---------------------------------------------------------------- local records

/// A record written here: its URI, CID, `indexedAt` and JSON value.
#[derive(Debug, Clone)]
struct Descript {
    uri: String,
    cid: String,
    indexed_at: String,
    record: Value,
}

#[derive(Debug, Default)]
struct Local {
    count: usize,
    profile: Option<Descript>,
    posts: Vec<Descript>,
}

impl Local {
    /// Ms since the oldest merged record was indexed.
    fn lag(&self) -> Option<i64> {
        let oldest = self
            .profile
            .iter()
            .chain(&self.posts)
            .map(|d| d.indexed_at.as_str())
            .min()?;
        let ms = crate::account::parse_datetime_ms(oldest)?;
        Some(locus_core::time::now_ms() - ms)
    }
}

fn to_json(bytes: &[u8]) -> Option<Value> {
    let v = proto_blue_lex_cbor::decode(bytes).ok()?;
    Some(proto_blue_lex_json::lex_to_json(&v))
}

async fn records_since(s: &AppState, did: &str, rev: &str) -> XrpcResult<Local> {
    let mut out = Local::default();
    let rows = s.store.records_since_rev(did, rev, SINCE_LIMIT).await?;
    // A rev older than every local record (an account that moved here)
    // means the AppView knows nothing this PDS wrote: nothing to merge.
    if !rows.is_empty() && !s.store.has_record_at_or_before(did, rev).await? {
        return Ok(out);
    }
    for (collection, rkey, cid, indexed_at, content) in rows {
        out.count += 1;
        let Some(record) = to_json(&content) else {
            continue;
        };
        let d = Descript {
            uri: format!("at://{did}/{collection}/{rkey}"),
            cid,
            indexed_at: locus_core::time::iso8601(indexed_at),
            record,
        };
        if collection == PROFILE && rkey == "self" {
            out.profile = Some(d);
        } else if collection == POST {
            out.posts.push(d);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- the local viewer

/// Builds views of the requester's own records, as the reference's
/// `LocalViewer`.
struct Viewer<'a> {
    s: &'a AppState,
    did: &'a str,
}

/// A blob's CID from its JSON form (typed or legacy).
fn blob_cid(b: &Value) -> Option<String> {
    b["ref"]["$link"]
        .as_str()
        .or_else(|| b["cid"].as_str())
        .map(str::to_owned)
}

/// Sets `key` to `value`, or removes it when there's none (as a spread of
/// `undefined` drops it from the JSON).
fn set_or_remove(map: &mut Value, key: &str, value: Option<Value>) {
    if let Some(m) = map.as_object_mut() {
        match value {
            Some(v) => {
                m.insert(key.to_owned(), v);
            }
            None => {
                m.remove(key);
            }
        }
    }
}

impl Viewer<'_> {
    fn image_url(&self, pattern: &str, cid: &str) -> String {
        let cdn = self.s.snapshot().text("appview.cdn_url_pattern").to_owned();
        if !cdn.is_empty() && pt::appview(self.s).is_some() {
            // util.format: each %s in turn.
            let mut out = String::new();
            let mut args = [pattern, self.did, cid].into_iter();
            let mut rest = cdn.as_str();
            while let Some(i) = rest.find("%s") {
                out.push_str(&rest[..i]);
                match args.next() {
                    Some(a) => out.push_str(a),
                    None => out.push_str("%s"),
                }
                rest = &rest[i + 2..];
            }
            out.push_str(rest);
            return out;
        }
        format!(
            "https://{}/xrpc/com.atproto.sync.getBlob?did={}&cid={cid}",
            self.s.hostname, self.did
        )
    }

    fn image(&self, pattern: &str, blob: &Value) -> Option<Value> {
        blob_cid(blob).map(|c| Value::String(self.image_url(pattern, &c)))
    }

    fn update_basic(&self, view: &Value, record: &Value) -> Value {
        let mut v = view.clone();
        set_or_remove(&mut v, "displayName", record.get("displayName").cloned());
        let avatar = record.get("avatar").and_then(|a| self.image("avatar", a));
        set_or_remove(&mut v, "avatar", avatar);
        v
    }

    fn update_view(&self, view: &Value, record: &Value) -> Value {
        let mut v = self.update_basic(view, record);
        set_or_remove(&mut v, "description", record.get("description").cloned());
        v
    }

    fn update_detailed(&self, view: &Value, record: &Value) -> Value {
        let mut v = self.update_view(view, record);
        let banner = record.get("banner").and_then(|b| self.image("banner", b));
        set_or_remove(&mut v, "banner", banner);
        v
    }

    async fn profile_basic(&self) -> XrpcResult<Option<Value>> {
        let Some(account) = self.s.store.account_by_did(self.did).await? else {
            return Ok(None);
        };
        let profile = match self.s.store.record_entry(self.did, PROFILE, "self").await? {
            Some((cid, _)) => self
                .s
                .store
                .block(self.did, &cid)
                .await?
                .and_then(|b| to_json(&b)),
            None => None,
        };
        let mut out = json!({"did": self.did, "handle": account.handle});
        if let Some(p) = profile {
            set_or_remove(&mut out, "displayName", p.get("displayName").cloned());
            set_or_remove(
                &mut out,
                "avatar",
                p.get("avatar").and_then(|a| self.image("avatar", a)),
            );
        }
        Ok(Some(out))
    }

    async fn post_view(&self, d: &Descript) -> XrpcResult<Option<Value>> {
        let Some(author) = self.profile_basic().await? else {
            return Ok(None);
        };
        let mut view = json!({
            "uri": d.uri,
            "cid": d.cid,
            "likeCount": 0,
            "replyCount": 0,
            "repostCount": 0,
            "quoteCount": 0,
            "author": author,
            "record": d.record,
            "indexedAt": d.indexed_at,
        });
        let embed = self.embed(&d.record).await;
        set_or_remove(&mut view, "embed", embed);
        Ok(Some(view))
    }

    async fn embed(&self, record: &Value) -> Option<Value> {
        let embed = record.get("embed")?;
        match embed["$type"].as_str()? {
            "app.bsky.embed.images" => Some(self.images_view(embed)),
            "app.bsky.embed.external" => Some(self.external_view(embed)),
            "app.bsky.embed.record" => Some(self.record_view(embed).await),
            "app.bsky.embed.recordWithMedia" => {
                let media = &embed["media"];
                let media = match media["$type"].as_str() {
                    Some("app.bsky.embed.images") => self.images_view(media),
                    Some("app.bsky.embed.external") => self.external_view(media),
                    _ => return None,
                };
                let record = self.record_view(&embed["record"]).await;
                Some(json!({
                    "$type": "app.bsky.embed.recordWithMedia#view",
                    "record": record,
                    "media": media,
                }))
            }
            _ => None,
        }
    }

    fn images_view(&self, embed: &Value) -> Value {
        let images: Vec<Value> = embed["images"]
            .as_array()
            .map(|imgs| {
                imgs.iter()
                    .map(|img| {
                        let mut v = json!({
                            "thumb": self.image("feed_thumbnail", &img["image"]),
                            "fullsize": self.image("feed_fullsize", &img["image"]),
                            "alt": img["alt"],
                        });
                        set_or_remove(&mut v, "aspectRatio", img.get("aspectRatio").cloned());
                        v
                    })
                    .collect()
            })
            .unwrap_or_default();
        json!({"$type": "app.bsky.embed.images#view", "images": images})
    }

    fn external_view(&self, embed: &Value) -> Value {
        let ext = &embed["external"];
        let mut external = json!({
            "uri": ext["uri"],
            "title": ext["title"],
            "description": ext["description"],
        });
        set_or_remove(
            &mut external,
            "thumb",
            ext.get("thumb")
                .and_then(|t| self.image("feed_thumbnail", t)),
        );
        json!({"$type": "app.bsky.embed.external#view", "external": external})
    }

    async fn record_view(&self, embed: &Value) -> Value {
        let uri = embed["record"]["uri"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let record = match self.record_view_inner(&uri).await {
            Some(v) => v,
            None => json!({
                "$type": "app.bsky.embed.record#viewNotFound",
                "uri": uri,
                "notFound": true,
            }),
        };
        json!({"$type": "app.bsky.embed.record#view", "record": record})
    }

    async fn record_view_inner(&self, uri: &str) -> Option<Value> {
        let collection = uri.strip_prefix("at://")?.split('/').nth(1)?;
        match collection {
            POST => {
                let data =
                    appview_get(self.s, self.did, "app.bsky.feed.getPosts", &[("uris", uri)])
                        .await?;
                let post = data["posts"].get(0)?;
                let mut v = json!({
                    "$type": "app.bsky.embed.record#viewRecord",
                    "uri": post["uri"],
                    "cid": post["cid"],
                    "author": post["author"],
                    "value": post["record"],
                    "indexedAt": post["indexedAt"],
                });
                set_or_remove(&mut v, "labels", post.get("labels").cloned());
                set_or_remove(&mut v, "embeds", post.get("embed").map(|e| json!([e])));
                Some(v)
            }
            "app.bsky.feed.generator" => {
                let data = appview_get(
                    self.s,
                    self.did,
                    "app.bsky.feed.getFeedGenerator",
                    &[("feed", uri)],
                )
                .await?;
                let mut v = data.get("view")?.clone();
                v.as_object_mut()?
                    .insert("$type".into(), json!("app.bsky.feed.defs#generatorView"));
                Some(v)
            }
            "app.bsky.graph.list" => {
                let data =
                    appview_get(self.s, self.did, "app.bsky.graph.getList", &[("list", uri)])
                        .await?;
                let mut v = data.get("list")?.clone();
                v.as_object_mut()?
                    .insert("$type".into(), json!("app.bsky.graph.defs#listView"));
                Some(v)
            }
            _ => None,
        }
    }

    /// Inserts the local posts newer than the feed's last item, newest
    /// first, each before the first older item (as the reference).
    async fn insert_posts(
        &self,
        mut feed: Vec<Value>,
        posts: &[Descript],
    ) -> XrpcResult<Vec<Value>> {
        if posts.is_empty() {
            return Ok(feed);
        }
        let last = feed
            .last()
            .and_then(|f| f["post"]["indexedAt"].as_str())
            .map_or_else(|| locus_core::time::iso8601(0), str::to_owned);
        for p in posts.iter().filter(|p| p.indexed_at > last).rev() {
            let Some(post) = self.post_view(p).await? else {
                continue;
            };
            let at = post["indexedAt"].as_str().unwrap_or_default().to_owned();
            let idx = feed.iter().position(|fi| {
                fi["post"]["indexedAt"]
                    .as_str()
                    .is_some_and(|t| t < at.as_str())
            });
            let item = json!({ "post": post });
            match idx {
                Some(i) => feed.insert(i, item),
                None => feed.push(item),
            }
        }
        Ok(feed)
    }

    /// Feed items' authors updated where the author is the requester.
    fn update_authors(&self, feed: Vec<Value>, profile: &Descript) -> Vec<Value> {
        feed.into_iter()
            .map(|mut item| {
                if item["post"]["author"]["did"] == self.did {
                    let author = self.update_basic(&item["post"]["author"], &profile.record);
                    item["post"]["author"] = author;
                }
                item
            })
            .collect()
    }
}

/// A GET to the configured AppView as `requester` (for record embeds and
/// thread parents). None on any failure.
async fn appview_get(
    s: &AppState,
    requester: &str,
    lxm: &str,
    params: &[(&str, &str)],
) -> Option<Value> {
    let (url, did) = pt::appview(s)?;
    let target = Target {
        url,
        did,
        service_id: "bsky_appview".into(),
        user_influenced: false,
    };
    let query: String = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(params.iter().copied())
        .finish();
    let path = format!("/xrpc/{lxm}?{query}");
    let headers = HeaderMap::new();
    let up = pt::send(
        s,
        pt::Outgoing {
            method: Method::GET,
            lxm,
            path_and_query: &path,
            headers: &headers,
            body: None,
            iss: Some(requester),
            aud: None,
            target: &target,
            accept_encoding: None,
            strict: false,
        },
    )
    .await
    .ok()?;
    let max = usize::try_from(s.snapshot().int("proxy.max_response_size")).unwrap_or(usize::MAX);
    let bytes = pt::read_capped(up.bytes_stream(), max).await.ok()?;
    serde_json::from_slice(&bytes).ok()
}

// ---------------------------------------------------------------- the pipeline

/// Which method's merge to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Munge {
    Profile,
    Profiles,
    Timeline,
    AuthorFeed,
    ActorLikes,
    PostThread,
}

fn is_json(headers: &reqwest::header::HeaderMap) -> Option<bool> {
    let ct = headers
        .get("content-type")?
        .to_str()
        .ok()?
        .to_ascii_lowercase();
    let mime = ct.split(';').next().unwrap_or("").trim().to_owned();
    Some(
        mime.strip_prefix("application/")
            .is_some_and(|sub| sub == "json" || sub.ends_with("+json")),
    )
}

fn munged(body: Value, lag: Option<i64>) -> Response {
    let mut res = Json(body).into_response();
    if let Some(l) = lag
        && let Ok(v) = HeaderValue::from_str(&l.to_string())
    {
        res.headers_mut().insert("atproto-upstream-lag", v);
    }
    res
}

/// Without an AppView these methods are plain pipethrough (which says no
/// service is configured, unless `atproto-proxy` names one).
async fn without_appview(
    s: &AppState,
    lxm: &str,
    method: Method,
    uri: &Uri,
    headers: &HeaderMap,
    body: axum::body::Body,
) -> Response {
    match pt::proxy_inner(s, lxm, method, uri, headers, body).await {
        Ok(r) | Err(r) => r,
    }
}

async fn read_after_write(
    s: &AppState,
    lxm: &str,
    uri: &Uri,
    headers: &HeaderMap,
    munge: Munge,
) -> Response {
    if pt::appview(s).is_none() {
        return without_appview(s, lxm, Method::GET, uri, headers, axum::body::Body::empty()).await;
    }
    match read_after_write_inner(s, lxm, uri, headers, munge).await {
        Ok(r) | Err(r) => r,
    }
}

async fn read_after_write_inner(
    s: &AppState,
    lxm: &str,
    uri: &Uri,
    headers: &HeaderMap,
    munge: Munge,
) -> Result<Response, Response> {
    let authed = auth::access(s, headers)
        .await
        .map_err(IntoResponse::into_response)?;
    let up = forward(s, lxm, uri, headers, &authed.did).await?;
    if up.status().as_u16() >= 400 {
        let e = pt::UpstreamError::read(s, up).await;
        if munge == Munge::PostThread
            && e.error == "NotFound"
            && let Some(rev) = e.header("atproto-repo-rev").map(str::to_owned)
            && let Some(r) = thread_not_found(s, &authed.did, uri, &rev).await
        {
            return Ok(r);
        }
        return Err(e.into_response());
    }
    after_upstream(s, &authed.did, up, munge).await
}

async fn forward(
    s: &AppState,
    lxm: &str,
    uri: &Uri,
    headers: &HeaderMap,
    iss: &str,
) -> Result<reqwest::Response, Response> {
    let target = pt::target(s, headers, lxm)
        .await
        .map_err(IntoResponse::into_response)?;
    pt::send_any(
        s,
        pt::Outgoing {
            method: Method::GET,
            lxm,
            path_and_query: uri.path_and_query().map_or(uri.path(), |q| q.as_str()),
            headers,
            body: None,
            iss: Some(iss),
            aud: None,
            target: &target,
            // Locus may read the answer, so it asks for it uncompressed.
            accept_encoding: None,
            strict: false,
        },
    )
    .await
}

async fn after_upstream(
    s: &AppState,
    did: &str,
    up: reqwest::Response,
    munge: Munge,
) -> Result<Response, Response> {
    let Some(rev) = up
        .headers()
        .get("atproto-repo-rev")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
    else {
        return Ok(pt::stream_back(s, up));
    };
    if is_json(up.headers()) == Some(false) {
        return Ok(pt::stream_back(s, up));
    }
    let local = records_since(s, did, &rev)
        .await
        .map_err(IntoResponse::into_response)?;
    if local.count == 0 {
        return Ok(pt::stream_back(s, up));
    }
    let status = axum::http::StatusCode::from_u16(up.status().as_u16()).unwrap_or_default();
    let content_type = up
        .headers()
        .get("content-type")
        .and_then(|c| HeaderValue::from_bytes(c.as_bytes()).ok());
    let max = usize::try_from(s.snapshot().int("proxy.max_response_size")).unwrap_or(usize::MAX);
    let bytes = pt::read_capped(up.bytes_stream(), max)
        .await
        .map_err(IntoResponse::into_response)?;
    let buffered = |bytes: Vec<u8>| {
        let mut r = Response::new(axum::body::Body::from(bytes));
        *r.status_mut() = status;
        if let Some(ct) = content_type.clone() {
            r.headers_mut().insert("content-type", ct);
        }
        r
    };
    let Ok(original) = serde_json::from_slice::<Value>(&bytes) else {
        return Ok(buffered(bytes));
    };
    let viewer = Viewer { s, did };
    match apply(&viewer, munge, original, &local).await {
        Ok(Some(body)) => Ok(munged(body, local.lag())),
        Ok(None) => Ok(buffered(bytes)),
        Err(e) => {
            tracing::warn!(did, "error in read after write munge: {}", e.message);
            Ok(buffered(bytes))
        }
    }
}

/// Applies a merge. None when the answer isn't shaped as expected (it is
/// then returned as it came, as the reference does with invalid output).
async fn apply(
    v: &Viewer<'_>,
    munge: Munge,
    mut original: Value,
    local: &Local,
) -> XrpcResult<Option<Value>> {
    if !original.is_object() {
        return Ok(None);
    }
    let feed_of = |o: &Value| o["feed"].as_array().cloned();
    match munge {
        Munge::Profile => {
            if let Some(p) = &local.profile
                && original["did"] == v.did
            {
                return Ok(Some(v.update_detailed(&original, &p.record)));
            }
            Ok(Some(original))
        }
        Munge::Profiles => {
            let Some(profiles) = original["profiles"].as_array().cloned() else {
                return Ok(None);
            };
            if let Some(p) = &local.profile {
                original["profiles"] = Value::Array(
                    profiles
                        .into_iter()
                        .map(|prof| {
                            if prof["did"] == v.did {
                                v.update_detailed(&prof, &p.record)
                            } else {
                                prof
                            }
                        })
                        .collect(),
                );
            }
            Ok(Some(original))
        }
        Munge::Timeline => {
            let Some(feed) = feed_of(&original) else {
                return Ok(None);
            };
            original["feed"] = Value::Array(v.insert_posts(feed, &local.posts).await?);
            Ok(Some(original))
        }
        Munge::AuthorFeed => {
            let Some(mut feed) = feed_of(&original) else {
                return Ok(None);
            };
            // Only the requester's own feed.
            let own = feed.first().is_some_and(|first| match first.get("reason") {
                None | Some(Value::Null) => first["post"]["author"]["did"] == v.did,
                Some(r) => {
                    r["$type"] == "app.bsky.feed.defs#reasonRepost" && r["by"]["did"] == v.did
                }
            });
            if !own {
                return Ok(Some(original));
            }
            if let Some(p) = &local.profile {
                feed = v.update_authors(feed, p);
            }
            original["feed"] = Value::Array(v.insert_posts(feed, &local.posts).await?);
            Ok(Some(original))
        }
        Munge::ActorLikes => {
            let Some(feed) = feed_of(&original) else {
                return Ok(None);
            };
            if let Some(p) = &local.profile {
                original["feed"] = Value::Array(v.update_authors(feed, p));
            }
            Ok(Some(original))
        }
        Munge::PostThread => {
            if original["thread"]["$type"] != "app.bsky.feed.defs#threadViewPost" {
                return Ok(Some(original));
            }
            let thread = add_to_thread(v, original["thread"].clone(), &local.posts).await?;
            original["thread"] = thread;
            Ok(Some(original))
        }
    }
}

// ---------------------------------------------------------------- threads

async fn thread_post(v: &Viewer<'_>, d: &Descript) -> XrpcResult<Option<Value>> {
    Ok(v.post_view(d)
        .await?
        .map(|post| json!({"$type": "app.bsky.feed.defs#threadViewPost", "post": post})))
}

/// The local posts that belong in this thread (same root).
fn in_thread<'a>(thread: &Value, posts: &'a [Descript]) -> Vec<&'a Descript> {
    let uri = thread["post"]["uri"].as_str();
    let thread_root = thread["post"]["record"]["reply"]["root"]["uri"].as_str();
    posts
        .iter()
        .filter(|p| {
            let Some(root) = p.record["reply"]["root"]["uri"].as_str() else {
                return false;
            };
            Some(root) == uri || Some(root) == thread_root
        })
        .collect()
}

async fn add_to_thread(v: &Viewer<'_>, mut thread: Value, posts: &[Descript]) -> XrpcResult<Value> {
    for d in in_thread(&thread, posts) {
        thread = insert_reply(v, thread, d).await?;
    }
    Ok(thread)
}

fn insert_reply<'a>(
    v: &'a Viewer<'a>,
    mut view: Value,
    d: &'a Descript,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = XrpcResult<Value>> + Send + 'a>> {
    Box::pin(async move {
        if d.record["reply"]["parent"]["uri"] == view["post"]["uri"] {
            let Some(post) = thread_post(v, d).await? else {
                return Ok(view);
            };
            let mut replies = vec![post];
            if let Some(existing) = view["replies"].as_array() {
                replies.extend(existing.iter().cloned());
            }
            view["replies"] = Value::Array(replies);
            return Ok(view);
        }
        let Some(replies) = view["replies"].as_array().cloned() else {
            return Ok(view);
        };
        let mut out = Vec::with_capacity(replies.len());
        for r in replies {
            if r["$type"] == "app.bsky.feed.defs#threadViewPost" {
                out.push(insert_reply(v, r, d).await?);
            } else {
                out.push(r);
            }
        }
        view["replies"] = Value::Array(out);
        Ok(view)
    })
}

/// The AppView doesn't have the requester's post yet: build the thread from
/// local records, with its parents from the AppView (as the reference).
async fn thread_not_found(s: &AppState, did: &str, uri: &Uri, rev: &str) -> Option<Response> {
    let mut thread_uri = None;
    let mut parent_height = None;
    for (k, v) in url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes()) {
        match k.as_ref() {
            "uri" => thread_uri = Some(v.into_owned()),
            "parentHeight" => parent_height = Some(v.into_owned()),
            _ => {}
        }
    }
    let thread_uri = thread_uri?;
    let rest = thread_uri.strip_prefix("at://")?;
    let (host, path) = rest.split_once('/')?;
    let host_did = if host.starts_with("did:") {
        host.to_owned()
    } else {
        s.store
            .account_by_handle(host)
            .await
            .ok()
            .flatten()
            .map_or_else(|| host.to_owned(), |a| a.did)
    };
    if host_did != did {
        return None;
    }
    let resolved = format!("at://{host_did}/{path}");
    let local = records_since(s, did, rev).await.ok()?;
    let found = local.posts.iter().find(|p| p.uri == resolved)?;
    let viewer = Viewer { s, did };
    let mut thread = thread_post(&viewer, found).await.ok()??;
    let rest: Vec<Descript> = local
        .posts
        .iter()
        .filter(|p| p.uri != resolved)
        .cloned()
        .collect();
    thread = add_to_thread(&viewer, thread, &rest).await.ok()?;
    if let Some(parent) = found.record["reply"]["parent"]["uri"].as_str() {
        let mut params = vec![("uri", parent), ("depth", "0")];
        if let Some(h) = &parent_height {
            params.push(("parentHeight", h.as_str()));
        }
        if let Some(res) = appview_get(s, did, "app.bsky.feed.getPostThread", &params).await {
            thread["parent"] = res["thread"].clone();
        }
    }
    Some(munged(json!({ "thread": thread }), local.lag()))
}

// ---------------------------------------------------------------- handlers

macro_rules! raw_handler {
    ($name:ident, $lxm:literal, $munge:expr) => {
        pub async fn $name(State(s): St, uri: Uri, headers: HeaderMap) -> Response {
            read_after_write(&s, $lxm, &uri, &headers, $munge).await
        }
    };
}

raw_handler!(get_profile, "app.bsky.actor.getProfile", Munge::Profile);
raw_handler!(get_profiles, "app.bsky.actor.getProfiles", Munge::Profiles);
raw_handler!(get_timeline, "app.bsky.feed.getTimeline", Munge::Timeline);
raw_handler!(
    get_author_feed,
    "app.bsky.feed.getAuthorFeed",
    Munge::AuthorFeed
);
raw_handler!(
    get_actor_likes,
    "app.bsky.feed.getActorLikes",
    Munge::ActorLikes
);
raw_handler!(
    get_post_thread,
    "app.bsky.feed.getPostThread",
    Munge::PostThread
);

#[derive(Deserialize)]
pub struct GetFeedParams {
    feed: String,
}

/// `app.bsky.feed.getFeed`: the feed generator's DID is read from its
/// record (through the AppView), and the request goes on with a token for
/// that generator (`getFeedSkeleton`), as the reference.
pub async fn get_feed(
    State(s): St,
    uri: Uri,
    headers: HeaderMap,
    Query(p): Query<GetFeedParams>,
) -> Response {
    let lxm = "app.bsky.feed.getFeed";
    if pt::appview(&s).is_none() {
        return without_appview(
            &s,
            lxm,
            Method::GET,
            &uri,
            &headers,
            axum::body::Body::empty(),
        )
        .await;
    }
    match get_feed_inner(&s, &uri, &headers, &p.feed).await {
        Ok(r) | Err(r) => r,
    }
}

async fn get_feed_inner(
    s: &AppState,
    uri: &Uri,
    headers: &HeaderMap,
    feed: &str,
) -> Result<Response, Response> {
    let authed = auth::access(s, headers)
        .await
        .map_err(IntoResponse::into_response)?;
    let parts: Vec<&str> = feed
        .strip_prefix("at://")
        .unwrap_or(feed)
        .splitn(3, '/')
        .collect();
    let (repo, collection, rkey) = (
        parts.first().copied().unwrap_or(""),
        parts.get(1).copied().unwrap_or(""),
        parts.get(2).copied().unwrap_or(""),
    );
    let (url, did) =
        pt::appview(s).ok_or_else(|| XrpcError::internal("no AppView").into_response())?;
    let appview = Target {
        url,
        did,
        service_id: "bsky_appview".into(),
        user_influenced: false,
    };
    let query: String = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs([("repo", repo), ("collection", collection), ("rkey", rkey)])
        .finish();
    let path = format!("/xrpc/com.atproto.repo.getRecord?{query}");
    let none = HeaderMap::new();
    let up = pt::send(
        s,
        pt::Outgoing {
            method: Method::GET,
            lxm: "com.atproto.repo.getRecord",
            path_and_query: &path,
            headers: &none,
            body: None,
            iss: None,
            aud: None,
            target: &appview,
            accept_encoding: None,
            strict: false,
        },
    )
    .await?;
    let max = usize::try_from(s.snapshot().int("proxy.max_response_size")).unwrap_or(usize::MAX);
    let bytes = pt::read_capped(up.bytes_stream(), max)
        .await
        .map_err(IntoResponse::into_response)?;
    let record: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let Some(feed_did) = record["value"]["did"].as_str().map(str::to_owned) else {
        return Err(XrpcError::named("UnknownFeed", "could not resolve feed did").into_response());
    };
    let target = pt::target(s, headers, "app.bsky.feed.getFeed")
        .await
        .map_err(IntoResponse::into_response)?;
    let up = pt::send(
        s,
        pt::Outgoing {
            method: Method::GET,
            lxm: "app.bsky.feed.getFeedSkeleton",
            path_and_query: uri.path_and_query().map_or(uri.path(), |q| q.as_str()),
            headers,
            body: None,
            iss: Some(&authed.did),
            aud: Some(&feed_did),
            target: &target,
            accept_encoding: headers
                .get(axum::http::header::ACCEPT_ENCODING)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned),
            strict: false,
        },
    )
    .await?;
    Ok(pt::stream_back(s, up))
}

/// `app.bsky.notification.registerPush` / `unregisterPush`: to the
/// configured AppView when `serviceDid` is its DID, otherwise to the
/// `#bsky_notif` endpoint in that DID's document (stricter URL rule).
async fn push(
    s: &AppState,
    lxm: &str,
    uri: &Uri,
    headers: &HeaderMap,
    body: Bytes,
    extra: &[&str],
) -> Response {
    if pt::appview(s).is_none() {
        return without_appview(
            s,
            lxm,
            Method::POST,
            uri,
            headers,
            axum::body::Body::from(body),
        )
        .await;
    }
    match push_inner(s, lxm, headers, body, extra).await {
        Ok(r) | Err(r) => r,
    }
}

async fn push_inner(
    s: &AppState,
    lxm: &str,
    headers: &HeaderMap,
    body: Bytes,
    extra: &[&str],
) -> Result<Response, Response> {
    let authed = auth::access_with(s, headers, auth::STANDARD, extra)
        .await
        .map_err(IntoResponse::into_response)?;
    let input: Value = serde_json::from_slice(&body)
        .map_err(|_| XrpcError::invalid_request("Request body must be JSON").into_response())?;
    let service_did = input["serviceDid"]
        .as_str()
        .ok_or_else(|| {
            XrpcError::invalid_request("Input must have the property \"serviceDid\"")
                .into_response()
        })?
        .to_owned();
    let (url, av_did) =
        pt::appview(s).ok_or_else(|| XrpcError::internal("no AppView").into_response())?;
    let (target, strict) = if service_did == av_did {
        (
            Target {
                url,
                did: av_did,
                service_id: "bsky_appview".into(),
                user_influenced: false,
            },
            false,
        )
    } else {
        let doc = s
            .resolve_did_doc(&service_did, false)
            .await
            .ok()
            .flatten()
            .ok_or_else(|| {
                XrpcError::invalid_request(format!("could not resolve did document: {service_did}"))
                    .into_response()
            })?;
        let endpoint = proto_blue_common::did_doc::get_notif_endpoint(&doc).ok_or_else(|| {
            XrpcError::invalid_request(format!(
                "invalid notification service details in did document: {service_did}"
            ))
            .into_response()
        })?;
        (
            Target {
                url: endpoint,
                did: service_did.clone(),
                service_id: "bsky_notif".into(),
                user_influenced: true,
            },
            true,
        )
    };
    let mut fwd = HeaderMap::new();
    fwd.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    let path = format!("/xrpc/{lxm}");
    let up = pt::send(
        s,
        pt::Outgoing {
            method: Method::POST,
            lxm,
            path_and_query: &path,
            headers: &fwd,
            body: Some(axum::body::Body::from(body)),
            iss: Some(&authed.did),
            aud: Some(&service_did),
            target: &target,
            accept_encoding: None,
            strict,
        },
    )
    .await?;
    drop(up);
    Ok(axum::http::StatusCode::OK.into_response())
}

pub async fn register_push(State(s): St, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    push(
        &s,
        "app.bsky.notification.registerPush",
        &uri,
        &headers,
        body,
        &[SCOPE_SIGNUP_QUEUED],
    )
    .await
}

pub async fn unregister_push(State(s): St, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    push(
        &s,
        "app.bsky.notification.unregisterPush",
        &uri,
        &headers,
        body,
        &[],
    )
    .await
}
