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
use kryphocron::encryption::{
    AtRestHooks, CodecId, DecodeContext, EncodedRecord, RotationGenerationMark,
};

use crate::context::AppContext;
use crate::error::{PdsError, PdsResult};

/// The deadline budget allotted to a single at-rest content encode. The
/// codec's `encode` is given this much wall-clock before it must yield;
/// it mirrors the bind pipeline's per-operation deadline convention.
const ENCODE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// The deadline budget for a single at-rest content decode (#237a). Same
/// shape and value as [`ENCODE_DEADLINE`] — the codec's `decode` gets this
/// much wall-clock before it must yield.
const DECODE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

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

/// Outcome of a decode-on-read attempt on a private-tier record (#237a).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DecodeOutcome {
    /// The record carried no `encodedContent` (a legacy `text` record, or a
    /// non-encoded shape) — left untouched. The §1.4 legacy/interop carveout.
    NotEncoded,
    /// `encodedContent` was decoded in place: the encoded fields were removed
    /// and `text` restored from the recovered plaintext.
    Decoded,
}

/// Decode a private-tier record's `encodedContent` back to plaintext `text`
/// via the installed kryphocron at-rest hooks held in [`AppContext`] — the
/// inverse of [`encode_private_content`].
///
/// **Authorization is the caller's responsibility.** Per the #237a (Y)
/// architecture — *kryphocron is the pathway, not the authority; Aurora-Locus
/// owns the trust model* — callers run [`authorize_private_read`] first and
/// only decode for authorized readers. The substrate codec is the at-rest
/// layer (friction), not the authorization layer; this function does no
/// access control of its own.
///
/// When kryphocron is disabled (no hooks installed), returns
/// [`DecodeOutcome::NotEncoded`] — there is no at-rest seam to decode through.
///
/// # Errors
///
/// [`PdsError::KryphocronCodecUnavailable`] (→ HTTP 410) when the record's
/// stored codec is not the one installed here (cross-peer/version skew);
/// [`PdsError::Internal`] on a codec decode failure or unparseable stored
/// field; [`PdsError::Validation`] when the record is not a JSON object.
pub(crate) async fn decode_private_content(
    ctx: &AppContext,
    originator_did: &str,
    nsid: &str,
    rkey: &str,
    record: &mut serde_json::Value,
) -> PdsResult<DecodeOutcome> {
    let hooks = match &ctx.kryphocron_at_rest_hooks {
        Some(hooks) => hooks.clone(),
        None => return Ok(DecodeOutcome::NotEncoded),
    };
    decode_private_content_with_hooks(hooks.as_ref(), originator_did, nsid, rkey, record).await
}

/// The testable core of [`decode_private_content`]: given the at-rest hooks
/// directly (rather than via [`AppContext`]), rebuild the [`EncodedRecord`]
/// from the stored `encodedContent*` fields and decode it in place.
///
/// # Errors
///
/// See [`decode_private_content`].
pub(crate) async fn decode_private_content_with_hooks(
    hooks: &dyn AtRestHooks,
    originator_did: &str,
    nsid: &str,
    rkey: &str,
    record: &mut serde_json::Value,
) -> PdsResult<DecodeOutcome> {
    // No `encodedContent` -> nothing to decode (legacy `text` record or a
    // non-encoded shape) — the §1.4 legacy / federation-interop carveout.
    let Some(b64) = record
        .get("encodedContent")
        .and_then(|v| v.get("$bytes"))
        .and_then(|v| v.as_str())
    else {
        return Ok(DecodeOutcome::NotEncoded);
    };
    let b64 = b64.to_string();
    let stored_codec = record
        .get("encodedContentCodec")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            PdsError::Internal(
                "private-tier record has encodedContent but no encodedContentCodec".to_string(),
            )
        })?
        .to_string();

    // Codec-skew check (kryphocron 0.3 §6.2): the record's stored codec must
    // match the one installed here, or this deployment cannot decode it.
    let codec = hooks.content_codec();
    let installed = codec.codec_id();
    if stored_codec != installed.as_str() {
        tracing::warn!(
            target: "aurora_locus::kryphocron",
            event = "content_decode_codec_skew",
            originator = originator_did,
            nsid,
            rkey,
            stored = %stored_codec,
            installed = %installed,
            "private-tier record encoded under a codec not installed on this deployment",
        );
        return Err(PdsError::KryphocronCodecUnavailable {
            stored: stored_codec,
            installed: installed.as_str().to_string(),
        });
    }

    let content = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(&b64)
        .map_err(|e| PdsError::Internal(format!("encodedContent base64 decode failed: {e}")))?;
    let generation = record
        .get("encodedContentGeneration")
        .and_then(|v| v.as_str())
        .map(RotationGenerationMark::new)
        .transpose()
        .map_err(|e| PdsError::Internal(format!("invalid encodedContentGeneration: {e}")))?;
    let codec_id = CodecId::new(stored_codec)
        .map_err(|e| PdsError::Internal(format!("invalid stored codec id: {e}")))?;
    let encoded = EncodedRecord::new(codec_id, content, generation);

    let nsid_typed = kryphocron::Nsid::new(nsid).map_err(|e| {
        PdsError::Internal(format!("kryphocron decode: invalid NSID {nsid}: {e}"))
    })?;
    let rkey_typed = kryphocron::RecordKey::new(rkey).map_err(|e| {
        PdsError::Internal(format!("kryphocron decode: invalid rkey {rkey}: {e}"))
    })?;
    let originator = kryphocron::Did::new(originator_did).map_err(|e| {
        PdsError::Internal(format!(
            "kryphocron decode: invalid originator DID {originator_did}: {e}"
        ))
    })?;
    let audience_list = record
        .get("audienceList")
        .and_then(|v| v.get("uri").or(Some(v)))
        .and_then(|v| v.as_str())
        .and_then(|s| kryphocron::AtUri::new(s).ok());

    let decode_ctx = DecodeContext::new(
        nsid_typed,
        rkey_typed,
        originator,
        audience_list,
        kryphocron::TraceId::from_bytes(uuid::Uuid::new_v4().into_bytes()),
        Default::default(),
    );

    let deadline = std::time::Instant::now() + DECODE_DEADLINE;
    let plaintext = codec.decode(&encoded, &decode_ctx, deadline).await.map_err(|e| {
        tracing::warn!(
            target: "aurora_locus::kryphocron",
            event = "content_decode_failed",
            originator = originator_did,
            nsid,
            rkey,
            error = %e,
            "kryphocron at-rest decode failed",
        );
        PdsError::Internal(format!("kryphocron at-rest decode failed: {e}"))
    })?;
    let text = String::from_utf8(plaintext)
        .map_err(|e| PdsError::Internal(format!("decoded content is not valid UTF-8: {e}")))?;

    let obj = record.as_object_mut().ok_or_else(|| {
        PdsError::Validation("postPrivate record must be a JSON object".to_string())
    })?;
    // Inverse of the encode transform: the XOR flips back to `text`.
    obj.remove("encodedContent");
    obj.remove("encodedContentCodec");
    obj.remove("encodedContentGeneration");
    obj.insert("text".to_string(), serde_json::Value::String(text));

    tracing::debug!(
        target: "aurora_locus::kryphocron",
        event = "content_decoded",
        originator = originator_did,
        nsid,
        rkey,
        codec = %installed,
        "private-tier record content decoded for an authorized read",
    );
    Ok(DecodeOutcome::Decoded)
}

/// Aurora-Locus's read-side authorization decision for a private-tier record
/// (#237a). **Not** kryphocron's sealed `ReadAuthorization` — architecture (Y)
/// has Aurora-Locus own the trust model, so this is Aurora-Locus's own type.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReadAuthz {
    /// The reader may see decoded plaintext.
    Authorized,
    /// The reader is not authorized; the encoded form is returned unchanged.
    NotAuthorized,
}

/// The pure, store-free portion of [`authorize_private_read`] (#237a):
///
/// - `Some(Authorized)` — self-read (`reader_did == owner_did`);
/// - `Some(NotAuthorized)` — the record has no `audienceList` (private to the
///   owner only), or the reader is anonymous;
/// - `None` — an audience-membership lookup against the store is required.
///
/// Split out so the non-store branches are unit-testable without an
/// [`AppContext`].
fn read_authz_precheck(
    reader_did: Option<&str>,
    owner_did: &str,
    record: &serde_json::Value,
) -> Option<ReadAuthz> {
    // Self-read: the account holder always reads their own private records.
    if reader_did == Some(owner_did) {
        return Some(ReadAuthz::Authorized);
    }
    // No audience reference -> private to the owner only; a non-owner denies.
    let audience_uri = record
        .get("audienceList")
        .and_then(|v| v.get("uri").or(Some(v)))
        .and_then(|v| v.as_str());
    if audience_uri.is_none() {
        return Some(ReadAuthz::NotAuthorized);
    }
    // Anonymous readers are never audience members.
    if reader_did.is_none() {
        return Some(ReadAuthz::NotAuthorized);
    }
    None
}

/// Authorize a reader for a private-tier record on the read path — the
/// symmetric mirror of `check_participate_audience` (the v0.7 write-side
/// audience check), **fail-closed**:
///
/// - **self-read** (`reader_did == owner_did`) is always authorized (the
///   account holder reads their own private records);
/// - otherwise the record's `audienceList` is resolved and **list-mode
///   membership** is checked;
/// - a record with no `audienceList`, an anonymous reader, a cross-DID /
///   unverifiable audience, a non-`list` mode (not yet implemented), or any
///   resolution error all **deny** (encoded form is returned).
///
/// Unlike the write-side `participate` check (which *defers-allow* cross-DID
/// parents pending federation read-through), the read path fails closed — a
/// read we cannot positively authorize must not leak plaintext.
pub(crate) async fn authorize_private_read(
    ctx: &AppContext,
    reader_did: Option<&str>,
    owner_did: &str,
    record: &serde_json::Value,
) -> ReadAuthz {
    // Store-free branches (self-read / no-audience / anonymous) decide first.
    if let Some(decided) = read_authz_precheck(reader_did, owner_did, record) {
        return decided;
    }
    // Past the precheck: there is an audienceList and a non-anonymous,
    // non-owner reader. Resolve list-mode membership against the store.
    let audience_uri = record
        .get("audienceList")
        .and_then(|v| v.get("uri").or(Some(v)))
        .and_then(|v| v.as_str())
        .expect("precheck returned None only when audienceList is present");
    let reader = reader_did.expect("precheck returned None only for a non-anonymous reader");

    // #335 — this membership resolution IS the read-side audience-oracle
    // consultation (the store-free prechecks above are not). Record the
    // aggregate outcome for getOracleActivity (§6.4.1).
    use crate::kryphocron_oracle_activity::OracleConsultation;
    match resolve_list_audience_membership(ctx, audience_uri, reader).await {
        Ok(true) => {
            ctx.audience_oracle_activity.record(OracleConsultation::ReadAuthorized);
            ReadAuthz::Authorized
        }
        Ok(false) => {
            ctx.audience_oracle_activity.record(OracleConsultation::ReadDenied);
            ReadAuthz::NotAuthorized
        }
        Err(e) => {
            ctx.audience_oracle_activity.record(OracleConsultation::ReadDenied);
            // Fail closed: an audience-resolution error denies the read.
            tracing::warn!(
                target: "aurora_locus::kryphocron",
                event = "read_authz_resolution_error",
                owner = owner_did,
                reader,
                audience_uri,
                error = %e,
                "audience resolution failed; denying private read (fail-closed)",
            );
            ReadAuthz::NotAuthorized
        }
    }
}

/// Fetch a `policy.audience` record and check `list`-mode membership of
/// `reader_did`. Fail-closed: a cross-DID audience owner (not a local actor),
/// a missing record/block, or a non-`list` mode all return `Ok(false)`.
///
/// `policy.audience` records are structural and stored in the clear (not
/// encoded at rest), so they are read directly via the actor store — no
/// decode involved.
async fn resolve_list_audience_membership(
    ctx: &AppContext,
    audience_uri: &str,
    reader_did: &str,
) -> PdsResult<bool> {
    let Some(audience_owner) =
        crate::api::kryphocron_endpoints::parse_at_uri_did(audience_uri)
    else {
        return Ok(false);
    };
    // Cross-DID audience (owner not a local actor) -> fail closed. The read
    // path does not defer-allow the way the write-side participate check does.
    if !ctx.actor_store.exists(&audience_owner).await {
        return Ok(false);
    }
    let Some(record) = ctx.actor_store.get_record(&audience_owner, audience_uri).await? else {
        return Ok(false);
    };
    let Some(block) = ctx.actor_store.get_block(&audience_owner, &record.cid).await? else {
        return Ok(false);
    };
    let lex = proto_blue::lex_cbor::decode(&block)
        .map_err(|e| PdsError::Internal(format!("decode audience record block: {e}")))?;
    let json = proto_blue::lex_json::lex_to_json(&lex);
    // Mode defaults to `list` per the lexicon; only `list` is wired (other
    // modes need follow-graph oracles that haven't landed) -> non-list denies.
    let mode = json.get("mode").and_then(|v| v.as_str()).unwrap_or("list");
    if mode != "list" {
        return Ok(false);
    }
    let is_member = json
        .get("members")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().any(|m| m.as_str() == Some(reader_did)))
        .unwrap_or(false);
    Ok(is_member)
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

    // ---- decode-on-read (#237a) ----

    const NSID: &str = "tools.kryphocron.feed.postPrivate";

    #[tokio::test]
    async fn encode_then_decode_round_trips_to_original_plaintext() {
        // The full at-rest cycle: a `text` record encoded (#236) then decoded
        // (#237a) recovers the exact original plaintext.
        let (hooks, _guard) = default_hooks("roundtrip-decode");
        let sink = ContentAuditSink;
        let original = "the quick brown fox jumps over the lazy dog";
        let mut record = serde_json::json!({
            "$type": NSID,
            "text": original,
            "createdAt": "2026-06-16T00:00:00Z",
        });

        encode_private_content_with_hooks(hooks.as_ref(), &sink, DID, NSID, RKEY, &mut record)
            .await
            .expect("encode");
        // Post-encode: no `text`, encoded fields present.
        assert!(record.get("text").is_none());
        assert!(record.get("encodedContent").is_some());

        let outcome = decode_private_content_with_hooks(hooks.as_ref(), DID, NSID, RKEY, &mut record)
            .await
            .expect("decode");
        assert_eq!(outcome, DecodeOutcome::Decoded);

        let obj = record.as_object().unwrap();
        // The XOR flipped back: `text` restored, encoded fields gone.
        assert_eq!(obj.get("text").and_then(|v| v.as_str()), Some(original));
        assert!(!obj.contains_key("encodedContent"));
        assert!(!obj.contains_key("encodedContentCodec"));
        assert!(!obj.contains_key("encodedContentGeneration"));
        // Sibling fields preserved across the round trip.
        assert_eq!(
            obj.get("createdAt").and_then(|v| v.as_str()),
            Some("2026-06-16T00:00:00Z")
        );
    }

    #[tokio::test]
    async fn decode_codec_skew_returns_codec_unavailable() {
        // A record stamped with a codec this deployment doesn't have installed
        // surfaces as KryphocronCodecUnavailable (→ 410), not a decode attempt.
        let (hooks, _guard) = default_hooks("skew");
        let mut record = serde_json::json!({
            "$type": NSID,
            "encodedContent": { "$bytes": "QUJD" },
            "encodedContentCodec": "laquna/0.99",
        });
        let err = decode_private_content_with_hooks(hooks.as_ref(), DID, NSID, RKEY, &mut record)
            .await
            .unwrap_err();
        match err {
            PdsError::KryphocronCodecUnavailable { stored, installed } => {
                assert_eq!(stored, "laquna/0.99");
                assert_eq!(installed, "laquna/0.2");
            }
            other => panic!("expected KryphocronCodecUnavailable, got {other:?}"),
        }
        // The record is left untouched (encoded form still returnable).
        assert!(record.get("encodedContent").is_some());
    }

    #[tokio::test]
    async fn decode_non_encoded_record_passes_through() {
        // A legacy `text`-only record (no encodedContent) is a no-op.
        let (hooks, _guard) = default_hooks("decode-notext");
        let mut record = serde_json::json!({ "$type": NSID, "text": "legacy plaintext" });
        let before = record.clone();
        let outcome = decode_private_content_with_hooks(hooks.as_ref(), DID, NSID, RKEY, &mut record)
            .await
            .unwrap();
        assert_eq!(outcome, DecodeOutcome::NotEncoded);
        assert_eq!(record, before, "non-encoded record must be untouched");
    }

    // ---- read authorization precheck (store-free branches) ----

    const OWNER: &str = "did:plc:owner";
    const OTHER: &str = "did:plc:other";

    fn private_record_with_audience() -> serde_json::Value {
        serde_json::json!({
            "$type": NSID,
            "encodedContent": { "$bytes": "QUJD" },
            "encodedContentCodec": "laquna/0.2",
            "audienceList": "at://did:plc:owner/tools.kryphocron.policy.audience/3kaud",
        })
    }

    #[test]
    fn precheck_self_read_is_authorized() {
        // The account holder always reads their own private records.
        let rec = private_record_with_audience();
        assert_eq!(
            read_authz_precheck(Some(OWNER), OWNER, &rec),
            Some(ReadAuthz::Authorized)
        );
    }

    #[test]
    fn precheck_anonymous_reader_is_not_authorized() {
        let rec = private_record_with_audience();
        assert_eq!(
            read_authz_precheck(None, OWNER, &rec),
            Some(ReadAuthz::NotAuthorized)
        );
    }

    #[test]
    fn precheck_no_audience_non_owner_is_not_authorized() {
        // No audienceList -> private to the owner only.
        let rec = serde_json::json!({
            "$type": NSID,
            "encodedContent": { "$bytes": "QUJD" },
            "encodedContentCodec": "laquna/0.2",
        });
        assert_eq!(
            read_authz_precheck(Some(OTHER), OWNER, &rec),
            Some(ReadAuthz::NotAuthorized)
        );
    }

    #[test]
    fn precheck_non_owner_with_audience_defers_to_membership_lookup() {
        // A non-owner, non-anonymous reader against a record WITH an audience
        // needs the store lookup — precheck returns None (no short-circuit).
        let rec = private_record_with_audience();
        assert_eq!(read_authz_precheck(Some(OTHER), OWNER, &rec), None);
    }

    /// Wiring tripwire (#237a): the encode→decode primitives are symmetric —
    /// decoding an encoded record restores exactly the bytes encode consumed,
    /// proving the seam is wired end-to-end and not a silent no-op. Pairs with
    /// the #236 encode-side wiring assertion.
    #[tokio::test]
    async fn decode_is_wired_inverse_of_encode() {
        let (hooks, _guard) = default_hooks("wiring");
        let sink = ContentAuditSink;
        let mut record = serde_json::json!({ "$type": NSID, "text": "wired?" });
        let did_encode =
            encode_private_content_with_hooks(hooks.as_ref(), &sink, DID, NSID, RKEY, &mut record)
                .await
                .unwrap();
        assert!(did_encode, "encode must fire");
        // If decode were a no-op, this would return NotEncoded and the text
        // would not come back.
        let outcome =
            decode_private_content_with_hooks(hooks.as_ref(), DID, NSID, RKEY, &mut record)
                .await
                .unwrap();
        assert_eq!(outcome, DecodeOutcome::Decoded);
        assert_eq!(record.get("text").and_then(|v| v.as_str()), Some("wired?"));
    }
}
