//! v0.9 Arc D (#236) — kryphocron at-rest content encode/decode seam.
//!
//! This module is Aurora-Locus's host integration of kryphocron 0.3's
//! `§8.3` at-rest content seam ([`kryphocron::encode_record_content`] /
//! [`kryphocron::decode_record_content`]). It enforces the constitutional
//! **encoding-at-default floor** (kryphocron 0.3 §1.1: "the substrate's
//! typed write path produces no plaintext private-tier records at rest"):
//! every private-tier record written through Aurora-Locus's dedicated
//! kryphocron endpoints has its `text` content run through the installed
//! [`ContentCodec`] (Laquna by default) and lands as the
//! `encodedContent` / `encodedContentCodec` / `encodedContentGeneration`
//! shape, never as plaintext `text`.
//!
//! ## Encode-on-write (this commit, #236 half 1)
//!
//! [`encode_private_content`] is called by `create_post_private` and
//! `participate_private` (the two endpoints that author
//! `tools.kryphocron.feed.postPrivate` records) **before** the write
//! reaches `apply_writes`. It:
//!
//! 1. Reads the plaintext `text` field (records without `text` — already
//!    encoded, or a non-text shape — pass through untouched: the §1.4
//!    legacy/federation-interop carveout).
//! 2. Builds a per-write [`kryphocron::RecordContentContext`] from the
//!    writer DID, NSID, rkey, and `audienceList` reference.
//! 3. Calls [`kryphocron::encode_record_content`], which resolves the
//!    rotation generation (freshness-checked against the #223
//!    `aurora-locus-standard` oracle), invokes the codec, and stamps the
//!    substrate-authoritative [`EncodedRecord`].
//! 4. Replaces `text` with `encodedContent` (`{"$bytes": <base64>}`),
//!    `encodedContentCodec`, and (when present) `encodedContentGeneration`.
//!    The `$bytes` base64 is STANDARD-alphabet **no-pad** — the shape
//!    `proto_blue::lex_json::json_to_lex` decodes to `LexValue::Bytes`
//!    (the CBOR byte string) at the `repository.rs` write seam.
//!
//! The transformed record then flows through the existing write path
//! unchanged; the structural XOR rule (kryphocron 0.3 §5.4: exactly one
//! of `text` | `encodedContent`, and `encodedContent` requires a codec
//! stamp) holds by construction.
//!
//! [`ContentCodec`]: kryphocron::encryption::ContentCodec
//! [`EncodedRecord`]: kryphocron::encryption::EncodedRecord

use base64::Engine as _;
use kryphocron::audit::{AuditError, UserAuditEvent, UserAuditSink};
use kryphocron::encryption::AtRestHooks;

use crate::context::AppContext;
use crate::error::{PdsError, PdsResult};

/// The deadline budget allotted to a single at-rest content encode. The
/// codec's `encode` is given this much wall-clock before it must yield;
/// it mirrors the bind pipeline's per-operation deadline convention.
const ENCODE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// Minimal Aurora-Locus [`UserAuditSink`] for the at-rest content seam.
///
/// kryphocron ships no public `UserAuditSink`; the substrate emits the
/// `ContentEncoded` / `ContentEncodeFailed` / `ContentDecodeFailed`
/// events fire-and-forget at the encode/decode seam and a host sink must
/// receive them. This sink logs them at `tracing` level (success at
/// `debug`, failures at `warn`) and never fails — the substrate's
/// fire-and-forget contract means a failing sink would not block the
/// operation anyway, but a no-fail sink keeps the logs clean.
///
/// The events carry **no plaintext and no content bytes** — only the
/// structural subject (DID + NSID), the codec id, the rotation
/// generation, and a coarse error class — so logging them leaks nothing
/// about record content. The Overview / Tier-activity operator feeds
/// (later Arc D tickets) will consume these via the audit trail; this
/// sink is the seam they hang off.
pub(crate) struct ContentAuditSink;

impl UserAuditSink for ContentAuditSink {
    fn record(&self, event: UserAuditEvent) -> Result<(), AuditError> {
        match &event {
            UserAuditEvent::ContentEncoded {
                trace_id,
                requester,
                codec,
                generation,
                ..
            } => {
                tracing::debug!(
                    target: "aurora_locus::kryphocron",
                    event = "content_encoded",
                    requester = %requester,
                    codec = %codec,
                    generation = ?generation,
                    trace_id = ?trace_id,
                    "private-tier record content encoded at rest",
                );
            }
            UserAuditEvent::ContentEncodeFailed {
                trace_id,
                requester,
                codec,
                error_class,
                ..
            } => {
                tracing::warn!(
                    target: "aurora_locus::kryphocron",
                    event = "content_encode_failed",
                    requester = %requester,
                    codec = %codec,
                    error_class = ?error_class,
                    trace_id = ?trace_id,
                    "private-tier record content failed to encode at rest",
                );
            }
            UserAuditEvent::ContentDecodeFailed {
                trace_id,
                requester,
                codec,
                stored_codec,
                error_class,
                ..
            } => {
                tracing::warn!(
                    target: "aurora_locus::kryphocron",
                    event = "content_decode_failed",
                    requester = %requester,
                    codec = ?codec,
                    stored_codec = ?stored_codec,
                    error_class = ?error_class,
                    trace_id = ?trace_id,
                    "private-tier record content failed to decode at rest",
                );
            }
            other => {
                tracing::debug!(
                    target: "aurora_locus::kryphocron",
                    event = ?other,
                    "kryphocron user audit event",
                );
            }
        }
        Ok(())
    }
}

/// Encode a private-tier record's `text` content at rest via the
/// installed kryphocron at-rest hooks held in [`AppContext`].
///
/// When `config.kryphocron.enabled` is false, no at-rest hooks are
/// installed (this is not a kryphocron deployment), so the
/// encoding-at-default floor does not apply and the record passes
/// through unchanged. When the hooks are present, the record's `text`
/// field is transformed in place to the `encodedContent` shape per the
/// module docs.
///
/// # Errors
///
/// Returns [`PdsError::Internal`] when the at-rest encode fails (codec
/// failure, stale rotation oracle) or a record field cannot be parsed
/// into its typed kryphocron form, and [`PdsError::Validation`] when the
/// record is not a JSON object.
pub(crate) async fn encode_private_content(
    ctx: &AppContext,
    writer_did: &str,
    nsid: &str,
    rkey: &str,
    record: &mut serde_json::Value,
) -> PdsResult<()> {
    let hooks = match &ctx.kryphocron_at_rest_hooks {
        Some(hooks) => hooks.clone(),
        // kryphocron disabled: no at-rest seam installed. The
        // encoding-at-default floor is a property of a kryphocron
        // deployment; with the substrate off, the record passes through
        // as legacy plaintext.
        None => return Ok(()),
    };
    let sink = ContentAuditSink;
    let encoded =
        encode_private_content_with_hooks(hooks.as_ref(), &sink, writer_did, nsid, rkey, record)
            .await?;
    if !encoded {
        tracing::debug!(
            target: "aurora_locus::kryphocron",
            writer_did,
            nsid,
            rkey,
            "private-tier write carried no `text` field; nothing to encode \
             (already-encoded or legacy/interop shape)",
        );
    }
    Ok(())
}

/// The testable core of [`encode_private_content`]: given the at-rest
/// hooks and an audit sink directly (rather than via [`AppContext`]),
/// transform the record's `text` field to the encoded shape.
///
/// Returns `Ok(true)` when a `text` field was found and encoded,
/// `Ok(false)` when the record carried no `text` (passed through
/// untouched).
///
/// # Errors
///
/// See [`encode_private_content`].
pub(crate) async fn encode_private_content_with_hooks(
    hooks: &dyn AtRestHooks,
    sink: &dyn UserAuditSink,
    writer_did: &str,
    nsid: &str,
    rkey: &str,
    record: &mut serde_json::Value,
) -> PdsResult<bool> {
    // Only the plaintext `text` field is encoded. A record without a
    // `text` field (already encoded, or a non-text shape) passes through
    // untouched — the §1.4 legacy / federation-interop carveout.
    let text = match record.get("text").and_then(|v| v.as_str()) {
        Some(text) => text.to_string(),
        None => return Ok(false),
    };

    let nsid_typed = kryphocron::Nsid::new(nsid).map_err(|e| {
        PdsError::Internal(format!("kryphocron encode: invalid NSID {nsid}: {e}"))
    })?;
    let rkey_typed = kryphocron::RecordKey::new(rkey).map_err(|e| {
        PdsError::Internal(format!("kryphocron encode: invalid rkey {rkey}: {e}"))
    })?;
    let writer = kryphocron::Did::new(writer_did).map_err(|e| {
        PdsError::Internal(format!(
            "kryphocron encode: invalid writer DID {writer_did}: {e}"
        ))
    })?;

    // The postPrivate `audienceList` is a string at-URI under lexicons
    // 0.3; tolerate the legacy `{uri}` object shape too, matching
    // `check_participate_audience`. A missing / unparseable reference is
    // simply omitted from the content context — the oracle ignores it.
    let audience_list = record
        .get("audienceList")
        .and_then(|v| v.get("uri").or(Some(v)))
        .and_then(|v| v.as_str())
        .and_then(|s| kryphocron::AtUri::new(s).ok());

    let subject_repr = kryphocron::TargetRepresentation::structural_only(
        kryphocron::StructuralRepresentation::Resource {
            did: writer.clone(),
            nsid: nsid_typed.clone(),
        },
    );

    let content_ctx = kryphocron::RecordContentContext::new(
        nsid_typed,
        rkey_typed,
        writer.clone(),
        audience_list,
        writer,
        subject_repr,
        // kryphocron's `TraceId` exposes no public CSPRNG generator
        // (only `from_bytes`); a fresh UUIDv4's 16 bytes are a
        // well-distributed forensic trace id, matching the host-side
        // trace convention in `kryphocron_audit::synthesize_trace_id`.
        kryphocron::TraceId::from_bytes(uuid::Uuid::new_v4().into_bytes()),
        Default::default(),
    );

    let deadline = std::time::Instant::now() + ENCODE_DEADLINE;
    let now = std::time::SystemTime::now();

    let encoded = kryphocron::encode_record_content(
        hooks,
        sink,
        text.as_bytes(),
        &content_ctx,
        deadline,
        now,
    )
    .await
    .map_err(|e| PdsError::Internal(format!("kryphocron at-rest encode failed: {e}")))?;

    let obj = record.as_object_mut().ok_or_else(|| {
        PdsError::Validation("postPrivate record must be a JSON object".to_string())
    })?;
    // The XOR rule (§5.4): exactly one of `text` | `encodedContent`.
    obj.remove("text");
    let b64 = base64::engine::general_purpose::STANDARD_NO_PAD.encode(&encoded.content);
    obj.insert(
        "encodedContent".to_string(),
        serde_json::json!({ "$bytes": b64 }),
    );
    obj.insert(
        "encodedContentCodec".to_string(),
        serde_json::Value::String(encoded.codec.to_string()),
    );
    if let Some(generation) = &encoded.generation {
        obj.insert(
            "encodedContentGeneration".to_string(),
            serde_json::Value::String(generation.to_string()),
        );
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the real default at-rest hooks (Laquna codec + the
    /// substrate `DefaultRotationOracle`) over a throwaway data dir. The
    /// dir is returned so the caller keeps it alive for the test's
    /// duration (and the `TempGuard` removes it on drop).
    fn default_hooks(tag: &str) -> (Box<dyn AtRestHooks>, TempGuard) {
        let dir = std::env::temp_dir().join(format!(
            "aurora-locus-kc-content-{}-{}",
            std::process::id(),
            tag
        ));
        let hooks = kryphocron::encryption::DefaultAtRestHooks::for_data_dir(dir.clone())
            .expect("default at-rest hooks build");
        (Box::new(hooks), TempGuard(dir))
    }

    struct TempGuard(std::path::PathBuf);
    impl Drop for TempGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const DID: &str = "did:plc:exampleexampleexample";
    const RKEY: &str = "3kabcdefghij2";

    #[tokio::test]
    async fn encodes_text_into_encoded_content_and_removes_text() {
        let (hooks, _guard) = default_hooks("encodes");
        let sink = ContentAuditSink;
        let mut record = serde_json::json!({
            "$type": "tools.kryphocron.feed.postPrivate",
            "text": "the quick brown fox",
            "createdAt": "2026-06-16T00:00:00Z",
        });

        let did_encode = encode_private_content_with_hooks(
            hooks.as_ref(),
            &sink,
            DID,
            "tools.kryphocron.feed.postPrivate",
            RKEY,
            &mut record,
        )
        .await
        .expect("encode succeeds");
        assert!(did_encode, "a record with `text` must report encoded");

        let obj = record.as_object().unwrap();
        // The XOR holds: `text` gone, `encodedContent` present.
        assert!(!obj.contains_key("text"), "plaintext `text` must be removed");
        assert!(obj.contains_key("encodedContent"));
        assert!(obj.contains_key("encodedContentCodec"));
        // The default baseline is Laquna.
        assert_eq!(
            obj.get("encodedContentCodec").and_then(|v| v.as_str()),
            Some("laquna/0.2")
        );
        // `encodedContent` is a `$bytes` wrapper.
        let bytes_b64 = obj
            .get("encodedContent")
            .and_then(|v| v.get("$bytes"))
            .and_then(|v| v.as_str())
            .expect("encodedContent carries a $bytes string");

        // Floor assertion: the encoded bytes are NOT the plaintext.
        let decoded = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(bytes_b64)
            .expect("encodedContent base64 is STANDARD no-pad");
        let plaintext = b"the quick brown fox";
        assert!(
            !decoded
                .windows(plaintext.len())
                .any(|w| w == plaintext),
            "encoded bytes contain the plaintext — encoding-at-default floor violated"
        );
        // Other fields are preserved.
        assert_eq!(
            obj.get("createdAt").and_then(|v| v.as_str()),
            Some("2026-06-16T00:00:00Z")
        );
    }

    #[tokio::test]
    async fn encoded_content_roundtrips_through_lex_json_to_cbor_bytes() {
        // The on-disk shape must survive `json_to_lex` → CBOR. The
        // `$bytes` base64 the encode helper writes must be STANDARD
        // no-pad (what proto-blue's lex-json decodes), landing as
        // `LexValue::Bytes` carrying exactly the codec output.
        let (hooks, _guard) = default_hooks("roundtrip");
        let sink = ContentAuditSink;
        let mut record = serde_json::json!({
            "$type": "tools.kryphocron.feed.postPrivate",
            "text": "round trip me",
        });
        encode_private_content_with_hooks(
            hooks.as_ref(),
            &sink,
            DID,
            "tools.kryphocron.feed.postPrivate",
            RKEY,
            &mut record,
        )
        .await
        .unwrap();

        let b64 = record["encodedContent"]["$bytes"].as_str().unwrap().to_string();
        let expected = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(&b64)
            .unwrap();

        // Through the same seam the write path uses.
        let lex = proto_blue::lex_json::json_to_lex(&record);
        let back = proto_blue::lex_json::lex_to_json(&lex);
        // The `$bytes` survives the round-trip identically.
        let back_b64 = back["encodedContent"]["$bytes"].as_str().unwrap();
        let roundtripped = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(back_b64)
            .unwrap();
        assert_eq!(roundtripped, expected);
    }

    #[tokio::test]
    async fn record_without_text_passes_through_untouched() {
        let (hooks, _guard) = default_hooks("notext");
        let sink = ContentAuditSink;
        // A record already in the encoded shape (no `text`).
        let mut record = serde_json::json!({
            "$type": "tools.kryphocron.feed.postPrivate",
            "encodedContent": { "$bytes": "QUJD" },
            "encodedContentCodec": "laquna/0.2",
        });
        let before = record.clone();
        let did_encode = encode_private_content_with_hooks(
            hooks.as_ref(),
            &sink,
            DID,
            "tools.kryphocron.feed.postPrivate",
            RKEY,
            &mut record,
        )
        .await
        .unwrap();
        assert!(!did_encode, "no `text` field means nothing to encode");
        assert_eq!(record, before, "record must be untouched");
    }
}
