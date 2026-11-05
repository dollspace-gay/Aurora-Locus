//! Permission sets for `include:` scopes, as the reference's lexicon
//! resolver: the NSID's authority → `_lexicon` TXT → DID → its PDS →
//! `com.atproto.lexicon.schema/<nsid>` with a proof under the DID's signed
//! commit (or straight from the store when the account is hosted here).
//! Results are cached in `oauth_lexicon` and refreshed after 5 minutes; a
//! failed refresh keeps the last good document.

use locus_auth::oauth::OAuthError;
use locus_auth::scopes::IncludeScope;
use proto_blue_lex_data::Cid;
use serde_json::Value;

use crate::state::AppState;

const SCHEMA_NSID: &str = "com.atproto.lexicon.schema";
const REFRESH_MS: i64 = 5 * 60_000;

/// The DNS authority of an NSID (`com.example.calendar.auth` →
/// `calendar.example.com`).
#[must_use]
pub fn authority(nsid: &str) -> String {
    let parts: Vec<&str> = nsid.split('.').collect();
    let mut domain: Vec<&str> = parts[..parts.len().saturating_sub(1)].to_vec();
    domain.reverse();
    domain.join(".")
}

async fn did_authority(s: &AppState, nsid: &str) -> Result<String, String> {
    let set = s.snapshot().text("oauth.lexicon_authority_did").to_owned();
    if !set.is_empty() {
        return Ok(set);
    }
    s.resolvers()
        .map_err(|e| e.message)?
        .lexicon_did_authority(&authority(nsid))
        .await
        .ok_or_else(|| format!("Could not resolve a DID authority for NSID ({nsid})"))
}

/// The record from the DID's PDS, verified against its signed commit.
async fn fetch_record(s: &AppState, did: &str, rkey: &str) -> Result<Value, String> {
    use proto_blue_common::fetch::{FetchHandler, HttpRequest};
    let doc = s
        .resolve_did_doc(did, false)
        .await
        .map_err(|e| e.message)?
        .ok_or("could not resolve the DID")?;
    let pds = proto_blue_common::get_pds_endpoint(&doc).ok_or("the DID has no PDS")?;
    let key = proto_blue_common::get_signing_did_key(&doc).ok_or("the DID has no signing key")?;
    let url = format!(
        "{}/xrpc/com.atproto.sync.getRecord?did={}&collection={SCHEMA_NSID}&rkey={}",
        pds.trim_end_matches('/'),
        url::form_urlencoded::byte_serialize(did.as_bytes()).collect::<String>(),
        url::form_urlencoded::byte_serialize(rkey.as_bytes()).collect::<String>()
    );
    let fetcher = locus_identity::fetch::SafeFetcher::new(locus_identity::fetch::FetchPolicy {
        ssrf_protection: s.ssrf_protection,
        strict: false,
    })
    .map_err(|e| e.to_string())?;
    let res = fetcher
        .fetch(HttpRequest::get(url))
        .await
        .map_err(|e| e.to_string())?;
    if res.status != 200 {
        return Err(format!("getRecord answered {}", res.status));
    }
    let (root, blocks) =
        proto_blue_repo::car::read_car_with_root(&res.body).map_err(|e| e.to_string())?;
    let commit_value = proto_blue_lex_cbor::decode(blocks.get(&root).ok_or("missing commit")?)
        .map_err(|e| e.to_string())?;
    let commit = proto_blue_repo::commit::SignedCommit::from_lex_value(&commit_value)
        .map_err(|e| e.to_string())?;
    if commit.did != did
        || !proto_blue_repo::commit::verify_commit_sig(&commit, &key).unwrap_or(false)
    {
        return Err("the record's commit isn't signed by its DID".into());
    }
    let path = format!("{SCHEMA_NSID}/{rkey}");
    let tree = proto_blue_repo::mst::Mst::load(commit.data.clone());
    let cid: Cid = tree
        .get(&path, &blocks)
        .map_err(|e| e.to_string())?
        .ok_or("the record isn't in the repository")?;
    let bytes = blocks.get(&cid).ok_or("missing record block")?;
    if !cid.verify(bytes).unwrap_or(false) {
        return Err("record block doesn't match its CID".into());
    }
    let value = proto_blue_lex_cbor::decode(bytes).map_err(|e| e.to_string())?;
    Ok(proto_blue_lex_json::lex_to_json(&value))
}

/// The record, from here when the authority's account is hosted here.
async fn lexicon_record(s: &AppState, did: &str, nsid: &str) -> Result<Value, String> {
    if s.store.account_by_did(did).await.ok().flatten().is_some() {
        let (cid, _) = s
            .store
            .record_entry(did, SCHEMA_NSID, nsid)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Could not resolve Lexicon schema record")?;
        let bytes = s
            .store
            .block(did, &cid)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("missing record block")?;
        let v = proto_blue_lex_cbor::decode(&bytes).map_err(|e| e.to_string())?;
        return Ok(proto_blue_lex_json::lex_to_json(&v));
    }
    fetch_record(s, did, nsid).await
}

async fn resolve(s: &AppState, nsid: &str) -> Result<(String, Value), String> {
    let did = did_authority(s, nsid).await?;
    let doc = lexicon_record(s, &did, nsid)
        .await
        .map_err(|e| format!("Could not resolve Lexicon schema record ({nsid}): {e}"))?;
    if doc.get("$type").and_then(Value::as_str) != Some(SCHEMA_NSID) {
        return Err(format!("Invalid Lexicon schema record ({nsid})"));
    }
    if doc.get("id").and_then(Value::as_str) != Some(nsid) {
        return Err(format!(
            "Lexicon schema record id does not match NSID ({nsid})"
        ));
    }
    Ok((format!("at://{did}/{SCHEMA_NSID}/{nsid}"), doc))
}

/// A lexicon document by NSID (cached, refreshed after 5 minutes).
async fn lexicon(s: &AppState, nsid: &str) -> Result<Value, String> {
    let cached = s
        .store
        .oauth_lexicon(nsid)
        .await
        .map_err(|e| e.to_string())?;
    let now = locus_core::time::now_ms();
    if let Some((updated, Some(doc))) = &cached
        && now - updated < REFRESH_MS
    {
        return serde_json::from_str(doc).map_err(|e| e.to_string());
    }
    match resolve(s, nsid).await {
        Ok((uri, doc)) => {
            let _ = s
                .store
                .oauth_put_lexicon(nsid, Some(&uri), Some(&doc.to_string()))
                .await;
            Ok(doc)
        }
        Err(e) => {
            let _ = s.store.oauth_put_lexicon(nsid, None, None).await;
            match cached {
                Some((_, Some(doc))) => serde_json::from_str(&doc).map_err(|e| e.to_string()),
                _ => Err(e),
            }
        }
    }
}

/// The permission set an NSID names.
pub async fn permission_set(s: &AppState, nsid: &str) -> Result<Value, String> {
    let doc = lexicon(s, nsid).await?;
    let main = doc.pointer("/defs/main").cloned().unwrap_or(Value::Null);
    if main.get("type").and_then(Value::as_str) != Some("permission-set") {
        return Err(format!("Lexicon document is not a permission set ({nsid})"));
    }
    Ok(main)
}

/// The `include:` scopes of a scope string with their permission sets.
pub async fn permission_sets(
    s: &AppState,
    scope: Option<&str>,
) -> Result<Vec<(IncludeScope, Value)>, OAuthError> {
    let mut out = Vec::new();
    for v in scope.unwrap_or("").split(' ') {
        if let Some(inc) = IncludeScope::parse(v) {
            let set = permission_set(s, &inc.nsid)
                .await
                .map_err(OAuthError::invalid_scope)?;
            out.push((inc, set));
        }
    }
    Ok(out)
}

/// The scope a token carries: `include:` scopes replaced by the permissions
/// their sets grant (as `buildTokenScope`).
pub async fn token_scope(s: &AppState, scope: &str) -> Result<String, OAuthError> {
    let sets = permission_sets(s, Some(scope)).await?;
    if sets.is_empty() {
        return Ok(scope.to_owned());
    }
    let mut out: Vec<String> = sets.iter().flat_map(|(inc, set)| inc.expand(set)).collect();
    out.extend(
        scope
            .split(' ')
            .filter(|v| IncludeScope::parse(v).is_none())
            .map(str::to_owned),
    );
    Ok(out.join(" "))
}

#[cfg(test)]
mod tests {
    #[test]
    fn authorities() {
        assert_eq!(
            super::authority("com.example.calendar.auth"),
            "calendar.example.com"
        );
        assert_eq!(super::authority("app.bsky.authFull"), "bsky.app");
    }
}
