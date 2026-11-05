//! The `feed.postPrivate` write pipeline: plaintext submissions are encoded
//! with laquna, pre-encoded ones are trial-decoded (bounded), checked as
//! plaintext would be, and re-encoded; the clear fields are always derived
//! here. Plaintext exists only in memory, and never reaches a log or an
//! error message.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use kryphocron::codec::laquna::Codec;
use kryphocron::post_content::{PostContent, derive_clear_fields};
use kryphocron::{
    AtRestHooks, AtUri, AuditEncryptionResolver, CodecError, ContentCodec, DecodeContext, Did,
    EncodedRecord, Nsid, RecordContentContext, RecordKey, RecordValidation, RotationOracle,
    StructuralRepresentation, TargetRepresentation, TraceId,
};
use proto_blue_lex_data::LexValue;
use serde_json::Value;
use smallvec::SmallVec;

use crate::audit::AuditSinks;
use crate::nsid;
use crate::rotation::SharedOracle;

/// The lexicon's `encodedContent` maxLength, and the decode bound.
pub const MAX_CONTENT: usize = 1_000_000;

/// How long one encode or decode may take.
const DEADLINE: Duration = Duration::from_secs(10);

/// Fields locus derives; whatever a client sent is replaced.
const CLEAR_FIELDS: [&str; 4] = ["embedBlobs", "mentionDids", "quote", "mediaKinds"];
const ENCODED_FIELDS: [&str; 3] = [
    "encodedContent",
    "encodedContentCodec",
    "encodedContentGeneration",
];
const CONTENT_FIELDS: [&str; 3] = ["text", "facets", "embed"];

/// Why a private post can't be written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PipelineError {
    /// 400 `InvalidRequest`.
    #[error("{0}")]
    Invalid(String),
    /// A retryable failure (the rotation state is stale, the codec's
    /// backend is down).
    #[error("{0}")]
    Retry(String),
}

fn invalid(m: impl Into<String>) -> PipelineError {
    PipelineError::Invalid(m.into())
}

/// Checks a record without plaintext (the lexicon validation the caller
/// runs on the plaintext form).
pub type Validate<'a> = &'a (dyn Fn(&BTreeMap<String, LexValue>) -> Result<(), String> + Sync);

/// The installed codec, the rotation oracle and a bounded decoder.
pub struct Encoder {
    codec: Arc<dyn ContentCodec>,
    decoder: Codec,
    oracle: Arc<SharedOracle>,
}

impl std::fmt::Debug for Encoder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Encoder").finish_non_exhaustive()
    }
}

impl AtRestHooks for Encoder {
    fn audit(&self) -> Option<Arc<dyn AuditEncryptionResolver>> {
        None
    }
    fn content_codec(&self) -> Arc<dyn ContentCodec> {
        self.codec.clone()
    }
    fn rotation_oracle(&self) -> Option<Arc<dyn RotationOracle>> {
        Some(self.oracle.clone())
    }
}

fn ids(did: &str, collection: &str, rkey: &str) -> Result<(Did, Nsid, RecordKey), PipelineError> {
    Ok((
        Did::new(did).map_err(|_| invalid("Invalid DID"))?,
        Nsid::new(collection).map_err(|_| invalid("Invalid collection"))?,
        RecordKey::new(rkey).map_err(|_| invalid("Invalid record key"))?,
    ))
}

fn to_lex(v: &Value) -> Result<LexValue, PipelineError> {
    proto_blue_lex_json::json_to_lex_with(v, proto_blue_lex_json::LexParseOptions::strict())
        .map_err(|_| invalid("Invalid postPrivate content"))
}

impl Encoder {
    #[must_use]
    pub fn new(oracle: Arc<SharedOracle>) -> Self {
        Self {
            codec: Arc::new(Codec::default()),
            decoder: Codec::default().with_max_decoded_len(MAX_CONTENT),
            oracle,
        }
    }

    #[must_use]
    pub fn oracle(&self) -> &Arc<SharedOracle> {
        &self.oracle
    }

    /// Decodes stored content, bounded, for locus's own checks (a
    /// pre-encoded submission, a rotation rewrite). Never for a reader.
    pub async fn trial_decode(
        &self,
        did: &str,
        collection: &str,
        rkey: &str,
        content: Vec<u8>,
    ) -> Result<Vec<u8>, CodecError> {
        let (did, nsid, rkey) = ids(did, collection, rkey).map_err(|_| CodecError::Malformed {
            codec: kryphocron::CodecId::new(nsid::CODEC).expect("a valid codec id"),
        })?;
        let ctx = DecodeContext::new(
            nsid,
            rkey,
            did,
            None,
            crate::audit::new_trace(),
            SmallVec::new(),
        );
        let codec_id = kryphocron::CodecId::new(nsid::CODEC).expect("a valid codec id");
        let encoded = EncodedRecord::new(codec_id, content, None);
        self.decoder
            .decode(&encoded, &ctx, Instant::now() + DEADLINE)
            .await
    }

    /// Encodes `payload` for the record at (did, collection, rkey), stamped
    /// with the current generation.
    #[allow(clippy::too_many_arguments)]
    pub async fn encode(
        &self,
        sinks: &AuditSinks,
        trace: TraceId,
        did: &str,
        collection: &str,
        rkey: &str,
        audience: Option<&str>,
        payload: &[u8],
    ) -> Result<EncodedRecord, PipelineError> {
        let (d, n, r) = ids(did, collection, rkey)?;
        let audience = audience.and_then(|a| AtUri::new(a).ok());
        let subject = TargetRepresentation::structural_only(StructuralRepresentation::Resource {
            did: d.clone(),
            nsid: n.clone(),
        });
        let ctx = RecordContentContext::new(
            n,
            r,
            d.clone(),
            audience,
            d,
            subject,
            trace,
            SmallVec::new(),
        );
        let encoded = kryphocron::encode_record_content(
            self,
            sinks,
            payload,
            &ctx,
            Instant::now() + DEADLINE,
            SystemTime::now(),
        )
        .await
        .map_err(|e| match e {
            CodecError::RotationStateUnavailable { .. } => PipelineError::Retry(
                "Private posting is briefly unavailable (rotation state); try again".into(),
            ),
            _ => PipelineError::Retry("Private posting is briefly unavailable; try again".into()),
        })?;
        if encoded.content.len() > MAX_CONTENT {
            return Err(invalid("Post content is too large"));
        }
        Ok(encoded)
    }

    /// The §8.4 pipeline for a create or update of a `feed.postPrivate`.
    /// Returns the record to store: no `text`, `facets` or `embed`; the
    /// clear fields derived; `encodedContent` and its stamps set.
    pub async fn prepare_post(
        &self,
        sinks: &AuditSinks,
        did: &str,
        rkey: &str,
        mut record: BTreeMap<String, LexValue>,
        validate: Validate<'_>,
    ) -> Result<BTreeMap<String, LexValue>, PipelineError> {
        let trace = crate::audit::new_trace();
        let has = |k: &str| record.contains_key(k);
        let shape = RecordValidation::post_private(
            has("text"),
            has("encodedContent"),
            has("encodedContentCodec"),
            has("encodedContentGeneration"),
        );
        let (d, n, _) = ids(did, nsid::POST_PRIVATE, rkey)?;
        kryphocron::validate_record_for_write(&shape, n, d, trace, sinks, SystemTime::now())
            .map_err(|reason| invalid(format!("Invalid postPrivate record: {reason:?}")))?;

        let content = if let Some(encoded) = record.get("encodedContent") {
            if record.get("encodedContentCodec").and_then(LexValue::as_str) != Some(nsid::CODEC) {
                return Err(invalid(format!(
                    "Unsupported encodedContentCodec: locus accepts {}",
                    nsid::CODEC
                )));
            }
            let bytes = encoded
                .as_bytes()
                .ok_or_else(|| invalid("encodedContent must be bytes"))?;
            if bytes.len() > MAX_CONTENT {
                return Err(invalid("encodedContent is too large"));
            }
            let plain = self
                .trial_decode(did, nsid::POST_PRIVATE, rkey, bytes.to_vec())
                .await
                .map_err(|_| invalid("encodedContent doesn't decode for this record"))?;
            PostContent::from_payload(&plain).map_err(|_| invalid("Invalid postPrivate content"))?
        } else {
            let json = |k: &str| record.get(k).map(proto_blue_lex_json::lex_to_json);
            PostContent {
                text: record
                    .get("text")
                    .and_then(LexValue::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                facets: json("facets"),
                embed: json("embed"),
            }
        };

        for k in ENCODED_FIELDS
            .iter()
            .chain(&CLEAR_FIELDS)
            .chain(&CONTENT_FIELDS)
        {
            record.remove(*k);
        }

        // The plaintext form, checked as a plaintext submission is.
        let mut plain = record.clone();
        plain.insert("text".into(), LexValue::String(content.text.clone()));
        if let Some(f) = &content.facets {
            plain.insert("facets".into(), to_lex(f)?);
        }
        if let Some(e) = &content.embed {
            plain.insert("embed".into(), to_lex(e)?);
        }
        validate(&plain).map_err(invalid)?;

        let mut clear = serde_json::Map::new();
        derive_clear_fields(content.facets.as_ref(), content.embed.as_ref()).apply_to(&mut clear);
        for (k, v) in &clear {
            record.insert(k.clone(), to_lex(v)?);
        }

        let audience = record
            .get("audienceList")
            .and_then(LexValue::as_str)
            .map(str::to_owned);
        let encoded = self
            .encode(
                sinks,
                trace,
                did,
                nsid::POST_PRIVATE,
                rkey,
                audience.as_deref(),
                &content.to_payload(),
            )
            .await?;
        stamp(&mut record, encoded);
        Ok(record)
    }

    /// Rotation's rewrite of an encoded record: the same plaintext under the
    /// current generation and a fresh slug; only `encodedContent` and
    /// `encodedContentGeneration` change.
    pub async fn reencode(
        &self,
        sinks: &AuditSinks,
        did: &str,
        rkey: &str,
        mut record: BTreeMap<String, LexValue>,
    ) -> Result<BTreeMap<String, LexValue>, PipelineError> {
        if record.get("encodedContentCodec").and_then(LexValue::as_str) != Some(nsid::CODEC) {
            return Err(invalid("unknown codec"));
        }
        let bytes = record
            .get("encodedContent")
            .and_then(LexValue::as_bytes)
            .ok_or_else(|| invalid("no encodedContent"))?
            .to_vec();
        let plain = self
            .trial_decode(did, nsid::POST_PRIVATE, rkey, bytes)
            .await
            .map_err(|_| invalid("encodedContent doesn't decode"))?;
        let audience = record
            .get("audienceList")
            .and_then(LexValue::as_str)
            .map(str::to_owned);
        let encoded = self
            .encode(
                sinks,
                crate::audit::new_trace(),
                did,
                nsid::POST_PRIVATE,
                rkey,
                audience.as_deref(),
                &plain,
            )
            .await?;
        record.insert("encodedContent".into(), LexValue::Bytes(encoded.content));
        match encoded.generation {
            Some(g) => {
                record.insert(
                    "encodedContentGeneration".into(),
                    LexValue::String(g.as_str().to_owned()),
                );
            }
            None => {
                record.remove("encodedContentGeneration");
            }
        }
        Ok(record)
    }
}

fn stamp(record: &mut BTreeMap<String, LexValue>, encoded: EncodedRecord) {
    record.insert(
        "encodedContentCodec".into(),
        LexValue::String(encoded.codec.as_str().to_owned()),
    );
    if let Some(g) = &encoded.generation {
        record.insert(
            "encodedContentGeneration".into(),
            LexValue::String(g.as_str().to_owned()),
        );
    }
    record.insert("encodedContent".into(), LexValue::Bytes(encoded.content));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DID: &str = "did:plc:writer";
    const RKEY: &str = "3jzfcijpj2z2a";

    fn map(v: Value) -> BTreeMap<String, LexValue> {
        match to_lex(&v).unwrap() {
            LexValue::Map(m) => m,
            _ => unreachable!(),
        }
    }

    fn encoder() -> Encoder {
        let oracle = Arc::new(SharedOracle::default());
        oracle.set(&crate::rotation::new_mark(1_700_000_000));
        Encoder::new(oracle)
    }

    fn ok(_: &BTreeMap<String, LexValue>) -> Result<(), String> {
        Ok(())
    }

    fn post(extra: Value) -> BTreeMap<String, LexValue> {
        let mut v = json!({
            "$type": nsid::POST_PRIVATE,
            "createdAt": "2024-01-01T00:00:00Z",
            "audienceList": format!("at://{DID}/{}/3jzfcijpj2z2b", nsid::AUDIENCE),
        });
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        map(v)
    }

    #[tokio::test]
    async fn plaintext_is_encoded_and_the_clear_fields_derived() {
        let (sinks, _rx) = AuditSinks::new([0; 32]);
        let enc = encoder();
        let mention = json!([{"index": {"byteStart": 0, "byteEnd": 2},
            "features": [{"$type": "app.bsky.richtext.facet#mention", "did": "did:plc:friend"}]}]);
        let out = enc
            .prepare_post(
                &sinks,
                DID,
                RKEY,
                post(json!({"text": "hi secret sentinel", "facets": mention, "mentionDids": ["did:plc:liar"]})),
                &ok,
            )
            .await
            .unwrap();
        assert!(!out.contains_key("text") && !out.contains_key("facets"));
        assert_eq!(out["encodedContentCodec"].as_str(), Some(nsid::CODEC));
        assert!(
            out["encodedContentGeneration"]
                .as_str()
                .unwrap()
                .starts_with("laquna/")
        );
        assert_eq!(
            proto_blue_lex_json::lex_to_json(&out["mentionDids"]),
            json!(["did:plc:friend"])
        );
        let bytes = out["encodedContent"].as_bytes().unwrap();
        assert!(
            !bytes.windows(8).any(|w| w == b"sentinel"),
            "encoded, not plain"
        );
        let plain = enc
            .trial_decode(DID, nsid::POST_PRIVATE, RKEY, bytes.to_vec())
            .await
            .unwrap();
        let c = PostContent::from_payload(&plain).unwrap();
        assert_eq!(c.text, "hi secret sentinel");
    }

    #[tokio::test]
    async fn pre_encoded_content_is_checked_and_re_encoded() {
        let (sinks, _rx) = AuditSinks::new([0; 32]);
        let enc = encoder();
        let client = kryphocron::codec::laquna::encode_standalone(
            &PostContent {
                text: "from the client".into(),
                ..Default::default()
            }
            .to_payload(),
            DID,
            nsid::POST_PRIVATE,
            RKEY,
            1_600_000_000,
        )
        .unwrap();
        let submitted = post(json!({}));
        let mut submitted = submitted;
        submitted.insert(
            "encodedContent".into(),
            LexValue::Bytes(client.content.clone()),
        );
        submitted.insert(
            "encodedContentCodec".into(),
            LexValue::String(nsid::CODEC.into()),
        );
        let out = enc
            .prepare_post(&sinks, DID, RKEY, submitted.clone(), &ok)
            .await
            .unwrap();
        assert_ne!(
            out["encodedContent"].as_bytes().unwrap(),
            client.content.as_slice(),
            "re-encoded"
        );

        // Bound to its identity: another rkey doesn't decode.
        let err = enc
            .prepare_post(&sinks, DID, "3jzfcijpj2z2c", submitted.clone(), &ok)
            .await
            .unwrap_err();
        assert!(matches!(err, PipelineError::Invalid(_)));

        // Validation of the decoded content applies.
        let refuse = |_: &BTreeMap<String, LexValue>| Err("text too long".to_owned());
        assert_eq!(
            enc.prepare_post(&sinks, DID, RKEY, submitted.clone(), &refuse)
                .await
                .unwrap_err(),
            PipelineError::Invalid("text too long".into())
        );

        let mut other_codec = submitted;
        other_codec.insert(
            "encodedContentCodec".into(),
            LexValue::String("rot13/1".into()),
        );
        assert!(
            enc.prepare_post(&sinks, DID, RKEY, other_codec, &ok)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn orphan_stamps_and_a_stale_oracle_are_refused() {
        let (sinks, _rx) = AuditSinks::new([0; 32]);
        let enc = encoder();
        for bad in [
            json!({}),
            json!({"text": "a", "encodedContentGeneration": "laquna/x"}),
            json!({"encodedContentCodec": nsid::CODEC}),
        ] {
            assert!(
                matches!(
                    enc.prepare_post(&sinks, DID, RKEY, post(bad.clone()), &ok)
                        .await,
                    Err(PipelineError::Invalid(_))
                ),
                "{bad}"
            );
        }
        let stale = Encoder::new(Arc::new(SharedOracle::default()));
        assert!(matches!(
            stale
                .prepare_post(&sinks, DID, RKEY, post(json!({"text": "a"})), &ok)
                .await,
            Err(PipelineError::Retry(_))
        ));
    }

    #[tokio::test]
    async fn rotation_keeps_everything_but_the_content_and_mark() {
        let (sinks, _rx) = AuditSinks::new([0; 32]);
        let enc = encoder();
        let first = enc
            .prepare_post(&sinks, DID, RKEY, post(json!({"text": "keep me"})), &ok)
            .await
            .unwrap();
        enc.oracle().set(&crate::rotation::new_mark(1_800_000_000));
        let second = enc
            .reencode(&sinks, DID, RKEY, first.clone())
            .await
            .unwrap();
        assert_ne!(first["encodedContent"], second["encodedContent"]);
        assert!(
            second["encodedContentGeneration"].as_str()
                > first["encodedContentGeneration"].as_str()
        );
        for k in ["createdAt", "audienceList", "encodedContentCodec", "$type"] {
            assert_eq!(first[k], second[k], "{k}");
        }
        let plain = enc
            .trial_decode(
                DID,
                nsid::POST_PRIVATE,
                RKEY,
                second["encodedContent"].as_bytes().unwrap().to_vec(),
            )
            .await
            .unwrap();
        assert_eq!(PostContent::from_payload(&plain).unwrap().text, "keep me");
    }
}
