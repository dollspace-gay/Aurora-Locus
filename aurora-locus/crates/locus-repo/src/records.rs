//! Records: the lexicon registry and record preparation (`$type`, data
//! model, rkey, validation, blob references, encoding), as the
//! reference's `prepareCreate` / `prepareUpdate`.
//!
//! Records live in memory as `LexValue` and are encoded only through
//! `proto_blue_lex_cbor::encode`.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use proto_blue_lex_data::{Cid, LexValue};
use proto_blue_lexicon::Lexicons;
use proto_blue_lexicon::types::{LexRecord, LexUserType};
use serde_json::Value;
use thiserror::Error;

use crate::known_lexicons::KNOWN_LEXICONS;

/// A record the write path refuses (400 `InvalidRequest`).
#[derive(Debug, Error, PartialEq, Eq)]
#[error("{0}")]
pub struct InvalidRecord(pub String);

fn bad(m: impl Into<String>) -> InvalidRecord {
    InvalidRecord(m.into())
}

/// Every schema locus validates records against: the reference's curated
/// known-schema list and the `tools.kryphocron.*` set.
pub struct Registry {
    lexicons: Lexicons,
}

static REGISTRY: OnceLock<Registry> = OnceLock::new();

impl Registry {
    /// The process-wide registry (built once; the documents are vendored,
    /// so a failure is a build defect).
    pub fn get() -> &'static Self {
        REGISTRY.get_or_init(|| {
            let mut lexicons = Lexicons::new();
            for (nsid, json) in KNOWN_LEXICONS {
                lexicons
                    .add_from_json(json)
                    .unwrap_or_else(|e| panic!("vendored lexicon {nsid} doesn't load: {e}"));
            }
            for doc in kryphocron_lexicons::lexicons().iter_docs() {
                if lexicons.get(&doc.id).is_none() {
                    lexicons.add(doc.clone()).unwrap_or_else(|e| {
                        panic!("kryphocron lexicon {} doesn't load: {e}", doc.id)
                    });
                }
            }
            Self { lexicons }
        })
    }

    /// The record schema for a collection, if it's known.
    #[must_use]
    pub fn record(&self, nsid: &str) -> Option<&LexRecord> {
        match self.lexicons.get_def(nsid)? {
            LexUserType::Record(r) => Some(r),
            _ => None,
        }
    }

    #[must_use]
    pub fn lexicons(&self) -> &Lexicons {
        &self.lexicons
    }
}

/// What validation concluded (`validationStatus` in responses).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationStatus {
    Valid,
    Unknown,
}

impl ValidationStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Unknown => "unknown",
        }
    }
}

/// A blob a record references.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct BlobUse {
    pub cid: Cid,
    pub mime_type: String,
    pub size: i64,
}

/// A record ready to commit.
#[derive(Debug, Clone)]
pub struct PreparedRecord {
    pub value: LexValue,
    pub cid: Cid,
    pub bytes: Vec<u8>,
    /// None when validation was skipped (`validate: false`).
    pub status: Option<ValidationStatus>,
    pub blobs: Vec<BlobUse>,
}

/// Parses a record from request JSON in strict mode: no floats, safe
/// integers only, well-formed `$link`/`$bytes`/blob refs.
pub fn parse_json(json: &Value) -> Result<LexValue, InvalidRecord> {
    proto_blue_lex_json::json_to_lex_with(json, proto_blue_lex_json::LexParseOptions::strict())
        .map_err(|e| bad(format!("Invalid record: {e}")))
}

/// Checks an rkey's syntax and the reference's slur filter.
pub fn check_rkey(rkey: &str) -> Result<(), InvalidRecord> {
    if proto_blue_syntax::RecordKey::new(rkey).is_err() {
        return Err(bad(format!("Invalid record key: {rkey}")));
    }
    if locus_identity::handle::has_explicit_slur(rkey) {
        return Err(bad("Unacceptable slur in record key"));
    }
    Ok(())
}

/// Prepares a create or update: `$type` (inserted if missing, must equal
/// the collection), the rkey, validation per `validate` (as the
/// reference: `false` skips, unset validates known types and accepts
/// unknown ones, `true` refuses unknown ones), blob references (legacy
/// refs refused), then the DAG-CBOR and its CID.
pub fn prepare_record(
    collection: &str,
    rkey: &str,
    value: LexValue,
    validate: Option<bool>,
    max_bytes: usize,
) -> Result<PreparedRecord, InvalidRecord> {
    let LexValue::Map(mut map) = value else {
        return Err(bad("Invalid record: expected an object"));
    };
    match map.get("$type") {
        None => {
            map.insert("$type".into(), LexValue::String(collection.into()));
        }
        Some(LexValue::String(t)) if t == collection => {}
        Some(other) => {
            let got = other
                .as_str()
                .map_or_else(|| "a non-string".to_owned(), str::to_owned);
            return Err(bad(format!(
                "Invalid $type: expected {collection}, got {got}"
            )));
        }
    }
    check_rkey(rkey)?;
    let value = LexValue::Map(map);
    let status = validate_record(collection, rkey, &value, validate)?;
    let blobs = blob_uses(&value)?;
    let bytes =
        proto_blue_lex_cbor::encode(&value).map_err(|e| bad(format!("Invalid record: {e}")))?;
    if bytes.len() > max_bytes {
        return Err(bad(format!(
            "Record too large: {} bytes, the limit is {max_bytes}",
            bytes.len()
        )));
    }
    let cid = Cid::for_cbor(&bytes);
    Ok(PreparedRecord {
        value,
        cid,
        bytes,
        status,
        blobs,
    })
}

fn validate_record(
    collection: &str,
    rkey: &str,
    value: &LexValue,
    validate: Option<bool>,
) -> Result<Option<ValidationStatus>, InvalidRecord> {
    if validate == Some(false) {
        return Ok(None);
    }
    let Some(schema) = Registry::get().record(collection) else {
        return if validate == Some(true) {
            Err(bad(format!("Unknown lexicon type: {collection}")))
        } else {
            Ok(Some(ValidationStatus::Unknown))
        };
    };
    check_key_type(collection, schema.key.as_deref(), rkey)?;
    proto_blue_lexicon::validation::validate_record(Registry::get().lexicons(), schema, value)
        .map_err(|e| bad(format!("Invalid {collection} record: {e}")))?;
    Ok(Some(ValidationStatus::Valid))
}

/// The record schema's key type: `tid`, `nsid`, `literal:<value>` or `any`.
fn check_key_type(collection: &str, key: Option<&str>, rkey: &str) -> Result<(), InvalidRecord> {
    let ok = match key.unwrap_or("any") {
        "tid" => proto_blue_syntax::Tid::new(rkey).is_ok(),
        "nsid" => proto_blue_syntax::Nsid::new(rkey).is_ok(),
        "any" => true,
        other => match other.strip_prefix("literal:") {
            Some(literal) => rkey == literal,
            None => true,
        },
    };
    if ok {
        Ok(())
    } else {
        Err(bad(format!(
            "Invalid record key for {collection}: {rkey} doesn't match the key type {}",
            key.unwrap_or("any")
        )))
    }
}

/// What a record points at, as the reference's `getBacklinks`: a follow
/// or block's `subject` DID, a like or repost's `subject.uri`.
#[must_use]
pub fn backlinks(collection: &str, value: &LexValue) -> Vec<(String, String)> {
    let Some(map) = value.as_map() else {
        return Vec::new();
    };
    match collection {
        "app.bsky.graph.follow" | "app.bsky.graph.block" | "tools.kryphocron.graph.block" => map
            .get("subject")
            .and_then(LexValue::as_str)
            .filter(|s| proto_blue_syntax::Did::new(s).is_ok())
            .map(|s| vec![("subject".to_owned(), s.to_owned())])
            .unwrap_or_default(),
        "app.bsky.feed.like" | "app.bsky.feed.repost" => map
            .get("subject")
            .and_then(LexValue::as_map)
            .and_then(|s| s.get("uri"))
            .and_then(LexValue::as_str)
            .filter(|s| proto_blue_syntax::AtUri::new(s).is_ok())
            .map(|s| vec![("subject.uri".to_owned(), s.to_owned())])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The collection whose records are indexed for rotation and re-pointing.
pub const POST_PRIVATE: &str = "tools.kryphocron.feed.postPrivate";

/// A private post's index entry: its generation mark, whether it still
/// carries `text`, and its audience. Other collections have none.
#[must_use]
pub fn private_entry(
    did: &str,
    collection: &str,
    rkey: &str,
    value: &LexValue,
) -> Option<locus_store::kryphocron::PrivateRecord> {
    if collection != POST_PRIVATE {
        return None;
    }
    let map = value.as_map()?;
    let audience = map
        .get("audienceList")
        .and_then(LexValue::as_str)
        .and_then(|u| u.strip_prefix("at://"))
        .map(|rest| {
            let mut parts = rest.splitn(3, '/');
            let did = parts.next().unwrap_or("").to_owned();
            let _collection = parts.next();
            (did, parts.next().unwrap_or("").to_owned())
        });
    Some(locus_store::kryphocron::PrivateRecord {
        did: did.to_owned(),
        collection: collection.to_owned(),
        rkey: rkey.to_owned(),
        generation: map
            .get("encodedContentGeneration")
            .and_then(LexValue::as_str)
            .map(str::to_owned),
        legacy_text: map.contains_key("text"),
        flagged: None,
        audience_did: audience.as_ref().map(|(d, _)| d.clone()),
        audience_rkey: audience.map(|(_, r)| r),
    })
}

/// Every blob the record references (typed refs anywhere in it, deduped).
/// A legacy `{cid, mimeType}` ref is refused, as the reference.
pub fn blob_uses(value: &LexValue) -> Result<Vec<BlobUse>, InvalidRecord> {
    let mut out = Vec::new();
    walk_blobs(value, &mut out)?;
    out.sort();
    out.dedup();
    Ok(out)
}

fn walk_blobs(value: &LexValue, out: &mut Vec<BlobUse>) -> Result<(), InvalidRecord> {
    match value {
        LexValue::Map(m) => {
            if let Some(b) = typed_blob(m) {
                out.push(b);
                return Ok(());
            }
            if is_legacy_blob(m) {
                let cid = m.get("cid").and_then(LexValue::as_str).unwrap_or_default();
                return Err(bad(format!("Legacy blobs are not allowed ({cid})")));
            }
            for v in m.values() {
                walk_blobs(v, out)?;
            }
        }
        LexValue::Array(a) => {
            for v in a {
                walk_blobs(v, out)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn typed_blob(m: &BTreeMap<String, LexValue>) -> Option<BlobUse> {
    if m.get("$type").and_then(LexValue::as_str) != Some("blob") {
        return None;
    }
    Some(BlobUse {
        cid: m.get("ref")?.as_cid()?.clone(),
        mime_type: m.get("mimeType")?.as_str()?.to_owned(),
        size: m.get("size")?.as_integer()?,
    })
}

fn is_legacy_blob(m: &BTreeMap<String, LexValue>) -> bool {
    m.len() == 2
        && m.get("cid").and_then(LexValue::as_str).is_some()
        && m.get("mimeType").and_then(LexValue::as_str).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn prep(
        collection: &str,
        rkey: &str,
        v: Value,
        validate: Option<bool>,
    ) -> Result<PreparedRecord, InvalidRecord> {
        prepare_record(collection, rkey, parse_json(&v)?, validate, 1_100_000)
    }

    const TID: &str = "3jzfcijpj2z2a";

    #[test]
    fn the_registry_loads_every_vendored_and_kryphocron_schema() {
        let r = Registry::get();
        assert!(r.record("app.bsky.feed.post").is_some());
        assert!(r.record("app.bsky.actor.profile").is_some());
        assert!(r.record("tools.kryphocron.feed.postPrivate").is_some());
        assert!(r.record("com.example.nothing").is_none());
        assert!(
            r.record("com.atproto.repo.createRecord").is_none(),
            "not a record type"
        );
    }

    #[test]
    fn validation_follows_the_reference_table() {
        let post = json!({"text": "hi", "createdAt": "2024-01-01T00:00:00Z"});
        let p = prep("app.bsky.feed.post", TID, post.clone(), None).unwrap();
        assert_eq!(p.status, Some(ValidationStatus::Valid));
        assert_eq!(
            p.value.as_map().unwrap()["$type"],
            LexValue::String("app.bsky.feed.post".into())
        );
        let missing = json!({"text": "hi"});
        assert!(prep("app.bsky.feed.post", TID, missing.clone(), None).is_err());
        assert_eq!(
            prep("app.bsky.feed.post", TID, missing, Some(false))
                .unwrap()
                .status,
            None
        );
        let other = json!({"anything": 1});
        assert_eq!(
            prep("com.example.thing", "x", other.clone(), None)
                .unwrap()
                .status,
            Some(ValidationStatus::Unknown)
        );
        assert_eq!(
            prep("com.example.thing", "x", other, Some(true))
                .unwrap_err()
                .0,
            "Unknown lexicon type: com.example.thing"
        );
    }

    #[test]
    fn type_rkey_and_data_model_rules() {
        let post = json!({"$type": "app.bsky.feed.like", "text": "hi", "createdAt": "2024-01-01T00:00:00Z"});
        assert!(
            prep("app.bsky.feed.post", TID, post, Some(false))
                .unwrap_err()
                .0
                .starts_with("Invalid $type")
        );
        let ok = json!({"text": "hi", "createdAt": "2024-01-01T00:00:00Z"});
        assert!(
            prep("app.bsky.feed.post", "not-a-tid", ok.clone(), None).is_err(),
            "posts need TID keys"
        );
        assert!(
            prep("app.bsky.actor.profile", "self", json!({}), None).is_ok(),
            "literal:self"
        );
        assert!(prep("app.bsky.actor.profile", "other", json!({}), None).is_err());
        assert!(prep("com.example.thing", "bad key!", json!({}), None).is_err());
        assert!(
            prep("com.example.thing", "x", json!({"n": 1.5}), None).is_err(),
            "no floats"
        );
        assert!(
            prep(
                "com.example.thing",
                "x",
                json!({"n": 9_007_199_254_740_993_i64}),
                None
            )
            .is_err()
        );
        assert!(
            prep("com.example.thing", "x", json!([1]), None).is_err(),
            "top level must be an object"
        );
    }

    #[test]
    fn blob_refs_are_collected_and_legacy_ones_refused() {
        let cid = Cid::for_raw(b"image bytes").to_string_base32();
        let blob =
            json!({"$type": "blob", "ref": {"$link": cid}, "mimeType": "image/png", "size": 11});
        let rec = json!({"a": blob, "b": [{"nested": blob}]});
        let p = prep("com.example.thing", "x", rec, None).unwrap();
        assert_eq!(p.blobs.len(), 1, "deduped");
        assert_eq!(
            (p.blobs[0].mime_type.as_str(), p.blobs[0].size),
            ("image/png", 11)
        );
        let legacy = json!({"img": {"cid": cid, "mimeType": "image/png"}});
        assert!(
            prep("com.example.thing", "x", legacy, None)
                .unwrap_err()
                .0
                .starts_with("Legacy blobs")
        );
    }

    #[test]
    fn records_have_a_size_limit() {
        let v = parse_json(&json!({"text": "x".repeat(2000)})).unwrap();
        let err = prepare_record("com.example.thing", "x", v.clone(), None, 1024).unwrap_err();
        assert!(err.0.starts_with("Record too large"), "{}", err.0);
        assert!(prepare_record("com.example.thing", "x", v, None, 4096).is_ok());
    }

    #[test]
    fn records_encode_deterministically() {
        let a = prep("com.example.thing", "x", json!({"b": 1, "a": 2}), None).unwrap();
        let b = prep("com.example.thing", "x", json!({"a": 2, "b": 1}), None).unwrap();
        assert_eq!(a.cid, b.cid);
        assert_eq!(proto_blue_lex_cbor::decode(&a.bytes).unwrap(), a.value);
    }
}
