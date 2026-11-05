//! Host rules the kryphocron lexicons can't express, applied on every write
//! path whatever `validate` says.

use std::collections::BTreeMap;

use proto_blue_lex_data::LexValue;

use crate::nsid;

/// A record a host rule refuses (400 `InvalidRequest`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RuleError(pub String);

fn bad(m: impl Into<String>) -> RuleError {
    RuleError(m.into())
}

/// An AT-URI's (authority, collection, rkey); a URI with fewer parts has
/// empty ones.
#[must_use]
pub fn split_uri(uri: &str) -> Option<(&str, &str, &str)> {
    let rest = uri.strip_prefix("at://")?;
    let mut parts = rest.splitn(3, '/');
    let authority = parts.next().filter(|a| !a.is_empty())?;
    Some((
        authority,
        parts.next().unwrap_or(""),
        parts.next().unwrap_or(""),
    ))
}

/// Whether `uri` names a record in `did`'s repo (in `collection`, if given).
#[must_use]
pub fn in_repo(uri: &str, did: &str, collection: Option<&str>) -> bool {
    match split_uri(uri) {
        Some((a, c, r)) => a == did && !r.is_empty() && collection.is_none_or(|want| want == c),
        None => false,
    }
}

/// Every kryphocron record lexicon declares `key: tid`.
pub fn check_rkey(rkey: &str) -> Result<(), RuleError> {
    if proto_blue_syntax::Tid::new(rkey).is_ok() {
        Ok(())
    } else {
        Err(bad(format!(
            "Invalid record key: kryphocron records use TID keys, got {rkey}"
        )))
    }
}

fn str_field<'a>(map: &'a BTreeMap<String, LexValue>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(LexValue::as_str)
}

/// The rules for one record, before any pipeline runs: the rkey, and per
/// collection the fields that must point into the writer's repo.
pub fn check_record(
    did: &str,
    collection: &str,
    rkey: &str,
    map: &BTreeMap<String, LexValue>,
) -> Result<(), RuleError> {
    check_rkey(rkey)?;
    match collection {
        nsid::THREADGATE => check_threadgate(did, rkey, map),
        nsid::POST_PRIVATE => {
            if let Some(c) = str_field(map, "publicCompanion")
                && !in_repo(c, did, None)
            {
                return Err(bad(
                    "publicCompanion must point into the writer's repository",
                ));
            }
            let audience =
                str_field(map, "audienceList").ok_or_else(|| bad("audienceList is required"))?;
            // A reply names its parent's audience (checked against the
            // parent, participatePrivate).
            if !map.contains_key("reply") && !in_repo(audience, did, Some(nsid::AUDIENCE)) {
                return Err(bad(
                    "audienceList must name one of the writer's own audiences",
                ));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// A threadgate's rkey is its post's, the post is the writer's, and a
/// `listRule` names an `app.bsky.graph.list` (an audience URI matches
/// nobody).
fn check_threadgate(
    did: &str,
    rkey: &str,
    map: &BTreeMap<String, LexValue>,
) -> Result<(), RuleError> {
    let post = map
        .get("post")
        .and_then(LexValue::as_map)
        .and_then(|p| p.get("uri"))
        .and_then(LexValue::as_str)
        .ok_or_else(|| bad("threadgate post is required"))?;
    let Some((authority, _, post_rkey)) = split_uri(post) else {
        return Err(bad("Invalid threadgate post URI"));
    };
    if authority != did {
        return Err(bad(
            "A threadgate's post must be in the writer's repository",
        ));
    }
    if post_rkey != rkey {
        return Err(bad("A threadgate's record key must be its post's"));
    }
    for rule in map.get("allow").and_then(LexValue::as_array).unwrap_or(&[]) {
        let Some(rule) = rule.as_map() else { continue };
        let is_list = rule
            .get("$type")
            .and_then(LexValue::as_str)
            .is_some_and(|t| t.ends_with("#listRule"));
        if !is_list {
            continue;
        }
        let list = rule.get("list").and_then(LexValue::as_str).unwrap_or("");
        let ok =
            split_uri(list).is_some_and(|(_, c, r)| c == "app.bsky.graph.list" && !r.is_empty());
        if !ok {
            return Err(bad(
                "A threadgate listRule must name an app.bsky.graph.list",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DID: &str = "did:plc:writer";
    const TID: &str = "3jzfcijpj2z2a";

    fn map(v: serde_json::Value) -> BTreeMap<String, LexValue> {
        match proto_blue_lex_json::json_to_lex_with(
            &v,
            proto_blue_lex_json::LexParseOptions::strict(),
        )
        .unwrap()
        {
            LexValue::Map(m) => m,
            _ => unreachable!(),
        }
    }

    #[test]
    fn rkeys_are_tids() {
        assert!(check_record(DID, nsid::LIKE, "self", &map(serde_json::json!({}))).is_err());
        assert!(check_record(DID, nsid::LIKE, TID, &map(serde_json::json!({}))).is_ok());
    }

    #[test]
    fn private_posts_point_into_the_writers_repo() {
        let aud = format!("at://{DID}/{}/3jzfcijpj2z2b", nsid::AUDIENCE);
        let ok = map(serde_json::json!({"audienceList": aud}));
        assert!(check_record(DID, nsid::POST_PRIVATE, TID, &ok).is_ok());
        for bad in [
            serde_json::json!({}),
            serde_json::json!({"audienceList": format!("at://did:plc:other/{}/3jzfcijpj2z2b", nsid::AUDIENCE)}),
            serde_json::json!({"audienceList": format!("at://{DID}/app.bsky.graph.list/3jzfcijpj2z2b")}),
            serde_json::json!({"audienceList": aud, "publicCompanion": "at://did:plc:other/app.bsky.feed.post/1"}),
        ] {
            assert!(
                check_record(DID, nsid::POST_PRIVATE, TID, &map(bad.clone())).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn threadgates_follow_their_post() {
        let gate = |uri: &str, list: &str| {
            map(serde_json::json!({
                "post": {"uri": uri, "cid": "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifvx5xytvske7mr3hpm"},
                "allow": [{"$type": "tools.kryphocron.feed.threadgate#listRule", "list": list}],
                "createdAt": "2024-01-01T00:00:00Z"
            }))
        };
        let post = format!("at://{DID}/{}/{TID}", nsid::POST_PRIVATE);
        let list = format!("at://{DID}/app.bsky.graph.list/3jzfcijpj2z2b");
        assert!(check_record(DID, nsid::THREADGATE, TID, &gate(&post, &list)).is_ok());
        assert!(check_record(DID, nsid::THREADGATE, "3jzfcijpj2z2c", &gate(&post, &list)).is_err());
        let other = format!("at://did:plc:other/{}/{TID}", nsid::POST_PRIVATE);
        assert!(check_record(DID, nsid::THREADGATE, TID, &gate(&other, &list)).is_err());
        let audience = format!("at://{DID}/{}/3jzfcijpj2z2b", nsid::AUDIENCE);
        assert!(check_record(DID, nsid::THREADGATE, TID, &gate(&post, &audience)).is_err());
    }
}
