//! DAG-CBOR encoder for firehose subscription frames.
//!
//! Implements Arc 14 §7.3.1 wire-format conversion: the atproto
//! `com.atproto.sync.subscribeRepos` protocol uses DAG-CBOR-encoded
//! binary WebSocket frames consisting of TWO consecutive CBOR objects
//! (header + body) packed into a single `Message::Binary` payload.
//!
//! # Frame structure
//!
//! Each frame is `Vec<u8>` formed by concatenating:
//!
//! 1. **Header**: `{t: "<frame-type>", op: 1}` for data frames, or
//!    `{op: -1, error: "<name>"[, message: "..."]}` for error frames.
//! 2. **Body**: a `LexValue::Map` carrying the frame payload (commit
//!    operations, sync data, identity changes, account status, info
//!    messages).
//!
//! Both objects are encoded via `proto_blue::lex_cbor::encode`, which
//! enforces RFC 8949 §4.2.1 canonical map-key ordering
//! (byte-length-then-lex sort).
//!
//! # Field-absence convention
//!
//! Per Arc 14 §7.3.2, optional fields use the manual "omit-if-none"
//! pattern in `LexValue::Map` construction: if a value is `None`, the
//! key is not inserted. `lex_cbor::encode` does NOT honor
//! `#[serde(skip_serializing_if = ...)]` since `LexValue` is not
//! serde-native; the encoder works on a concrete `LexValue` tree.
//!
//! # `cid: null` discipline (delete ops)
//!
//! Per the lexicon `nullable` marking on `CommitOp.cid` for delete
//! ops: emit `LexValue::Null` (encoded byte `0xf6`), distinct from
//! field-absence. Builder responsibility — see `commit_op_to_lex_value`.

use proto_blue::lex_cbor;
use proto_blue::lex_data::{Cid, LexValue};
use std::collections::BTreeMap;
use std::str::FromStr;

use crate::error::PdsError;

/// Build a canonical `LexValue::Map` from key/value pairs.
///
/// Arc 14 §7.3.1: the macro accepts up to ~255 pairs (encoded as
/// CBOR major type 5 with length-byte encoding). Aurora-Locus call
/// sites today use ≤4 pairs, so the overflow branch is
/// `unreachable!` per v3.2 round-4 F1 closure.
///
/// # Examples
///
/// ```ignore
/// use proto_blue::lex_data::LexValue;
/// let header = canonical_cbor_map!(
///     ("t", LexValue::String("#commit".to_string())),
///     ("op", LexValue::Integer(1)),
/// );
/// // header is LexValue::Map(BTreeMap{"t" → ..., "op" → ...}).
/// // lex_cbor::encode will emit map-2 (0xa2) with keys sorted by
/// // byte-length-then-lex: "t" (len 1) before "op" (len 2).
/// ```
#[macro_export]
macro_rules! canonical_cbor_map {
    ($( ($key:expr, $value:expr) ),* $(,)?) => {{
        let mut map: ::std::collections::BTreeMap<
            ::std::string::String,
            ::proto_blue::lex_data::LexValue,
        > = ::std::collections::BTreeMap::new();
        $(
            map.insert(::std::string::ToString::to_string($key), $value);
        )*
        if map.len() > 0xff {
            unreachable!(
                "canonical_cbor_map! invoked with more than 255 pairs; \
                 Aurora-Locus call sites never exceed 4 (v3.2 round-4 F1 closure)"
            );
        }
        ::proto_blue::lex_data::LexValue::Map(map)
    }};
}

/// Encode a firehose frame: header + body, two consecutive CBOR
/// objects packed into a single `Vec<u8>`.
///
/// # Arguments
///
/// * `frame_type`: the `t` value for data frames (`"#commit"`,
///   `"#sync"`, `"#identity"`, `"#account"`, `"#info"`). For error
///   frames, pass `None` and supply `header_override`.
/// * `body`: a `LexValue::Map` carrying the frame payload.
///
/// # Errors
///
/// Returns `PdsError::CborEncoding` if proto-blue's encoder rejects
/// the value (e.g. float, non-string key, duplicate key — all
/// programmer errors caught by builder discipline).
pub fn firehose_frame_to_cbor(
    frame_type: &str,
    body: LexValue,
) -> Result<Vec<u8>, PdsError> {
    let mut header_map: BTreeMap<String, LexValue> = BTreeMap::new();
    header_map.insert("t".to_string(), LexValue::String(frame_type.to_string()));
    header_map.insert("op".to_string(), LexValue::Integer(1));
    let header = LexValue::Map(header_map);

    let mut buf = lex_cbor::encode(&header)?;
    let body_bytes = lex_cbor::encode(&body)?;
    buf.extend_from_slice(&body_bytes);
    Ok(buf)
}

/// Encode a firehose error frame: header (`op: -1`) + body
/// (`{error: <name>, message?: <msg>}`).
///
/// Per Arc 14 §7.3.4: emit named lexicon errors (e.g.
/// `"FutureCursor"`, `"ConsumerTooSlow"`) as the value of `error`.
/// `message` is optional human-readable detail.
pub fn firehose_error_frame_to_cbor(
    error_name: &str,
    message: Option<&str>,
) -> Result<Vec<u8>, PdsError> {
    let mut header_map: BTreeMap<String, LexValue> = BTreeMap::new();
    header_map.insert("op".to_string(), LexValue::Integer(-1));
    let header = LexValue::Map(header_map);

    let mut body_map: BTreeMap<String, LexValue> = BTreeMap::new();
    body_map.insert("error".to_string(), LexValue::String(error_name.to_string()));
    if let Some(msg) = message {
        body_map.insert("message".to_string(), LexValue::String(msg.to_string()));
    }
    let body = LexValue::Map(body_map);

    let mut buf = lex_cbor::encode(&header)?;
    let body_bytes = lex_cbor::encode(&body)?;
    buf.extend_from_slice(&body_bytes);
    Ok(buf)
}

/// Parse a CID string into a `Cid` for CBOR tag-42 encoding.
///
/// CID values in body maps MUST be encoded as `LexValue::Cid(...)`
/// so that `lex_cbor::encode` emits CBOR tag 42 with the binary CID
/// payload (not as a string). Use this helper to convert the
/// string form held in `CommitEvent`/`SyncEvent` to a typed CID.
///
/// If the string is malformed, returns a `CborEncoding` error
/// (matches `?` propagation in builders).
fn cid_str_to_lex(cid_str: &str) -> Result<LexValue, PdsError> {
    Cid::from_str(cid_str).map(LexValue::Cid).map_err(|e| {
        PdsError::CborEncoding(format!("invalid CID '{}': {}", cid_str, e))
    })
}

/// Build a `LexValue::Map` body for a `#commit` frame.
///
/// Field order in the map is by AT Protocol convention; canonical
/// byte-length-then-lex re-ordering happens at `lex_cbor::encode`
/// time.
///
/// # Arguments
///
/// * `seq`: monotonic sequence number from `repo_seq`.
/// * `rebase`, `too_big`: commit metadata flags.
/// * `repo`: actor DID.
/// * `commit_cid`: CID of the signed commit block (parsed for tag-42).
/// * `rev`: TID revision string.
/// * `since`: prior commit CID (Some for non-genesis); CID-typed.
/// * `prev_data`: prior MST root CID (Some for non-genesis; Step 2
///   integration — currently always `None`).
/// * `blocks`: raw CAR bytes (NOT base64; emitted as CBOR major-type-2).
/// * `ops`: vec of `CommitOp` LexValues (built via
///   `commit_op_to_lex_value`).
/// * `blobs`: vec of blob CID strings (CID-typed in the LexValue map).
/// * `time`: ISO-8601 timestamp string.
#[allow(clippy::too_many_arguments)]
pub fn commit_body_to_lex_value(
    seq: i64,
    rebase: bool,
    too_big: bool,
    repo: &str,
    commit_cid: &str,
    rev: &str,
    since: Option<&str>,
    prev_data: Option<&str>,
    blocks: Vec<u8>,
    ops: Vec<LexValue>,
    blobs: &[String],
    time: &str,
) -> Result<LexValue, PdsError> {
    let mut map: BTreeMap<String, LexValue> = BTreeMap::new();
    map.insert("seq".to_string(), LexValue::Integer(seq));
    map.insert("rebase".to_string(), LexValue::Bool(rebase));
    map.insert("tooBig".to_string(), LexValue::Bool(too_big));
    map.insert("repo".to_string(), LexValue::String(repo.to_string()));
    map.insert("commit".to_string(), cid_str_to_lex(commit_cid)?);
    map.insert("rev".to_string(), LexValue::String(rev.to_string()));
    // Optional fields: omit-if-none discipline (Arc 14 §7.3.2).
    if let Some(s) = since {
        map.insert("since".to_string(), cid_str_to_lex(s)?);
    }
    if let Some(pd) = prev_data {
        map.insert("prevData".to_string(), cid_str_to_lex(pd)?);
    }
    map.insert("blocks".to_string(), LexValue::Bytes(blocks));
    map.insert("ops".to_string(), LexValue::Array(ops));
    let blob_lex: Result<Vec<LexValue>, PdsError> =
        blobs.iter().map(|b| cid_str_to_lex(b)).collect();
    map.insert("blobs".to_string(), LexValue::Array(blob_lex?));
    map.insert("time".to_string(), LexValue::String(time.to_string()));
    Ok(LexValue::Map(map))
}

/// Build a single `CommitOp` LexValue entry for a `#commit.ops` array.
///
/// Per Arc 14 §7.3.2 + lexicon: `cid` MUST be:
/// - omitted via `LexValue::Null` (encoded as `0xf6`) for `delete` ops
///   — this is the lexicon `nullable` marking, distinct from
///   field-absence.
/// - CID-typed (`LexValue::Cid`) for `create` and `update` ops.
///
/// `prev` (prior record version CID) is field-absent on `create` ops,
/// CID-typed on `update`/`delete` ops (Step 2 integration; currently
/// always `None`).
pub fn commit_op_to_lex_value(
    action: &str,
    path: &str,
    cid: Option<&str>,
    prev: Option<&str>,
) -> Result<LexValue, PdsError> {
    let mut map: BTreeMap<String, LexValue> = BTreeMap::new();
    map.insert("action".to_string(), LexValue::String(action.to_string()));
    map.insert("path".to_string(), LexValue::String(path.to_string()));
    match cid {
        // create/update ops carry a CID (tag-42).
        Some(c) => {
            map.insert("cid".to_string(), cid_str_to_lex(c)?);
        }
        // delete ops emit CBOR null per lexicon `nullable` discipline.
        None => {
            map.insert("cid".to_string(), LexValue::Null);
        }
    }
    if let Some(p) = prev {
        map.insert("prev".to_string(), cid_str_to_lex(p)?);
    }
    Ok(LexValue::Map(map))
}

/// Build a `LexValue::Map` body for a `#sync` frame.
pub fn sync_body_to_lex_value(
    seq: i64,
    did: &str,
    rev: &str,
    blocks: Vec<u8>,
    time: &str,
) -> Result<LexValue, PdsError> {
    let mut map: BTreeMap<String, LexValue> = BTreeMap::new();
    map.insert("seq".to_string(), LexValue::Integer(seq));
    map.insert("did".to_string(), LexValue::String(did.to_string()));
    map.insert("rev".to_string(), LexValue::String(rev.to_string()));
    map.insert("blocks".to_string(), LexValue::Bytes(blocks));
    map.insert("time".to_string(), LexValue::String(time.to_string()));
    Ok(LexValue::Map(map))
}

/// Build a `LexValue::Map` body for a `#identity` frame.
pub fn identity_body_to_lex_value(
    seq: i64,
    did: &str,
    time: &str,
    handle: Option<&str>,
) -> LexValue {
    let mut map: BTreeMap<String, LexValue> = BTreeMap::new();
    map.insert("seq".to_string(), LexValue::Integer(seq));
    map.insert("did".to_string(), LexValue::String(did.to_string()));
    map.insert("time".to_string(), LexValue::String(time.to_string()));
    if let Some(h) = handle {
        map.insert("handle".to_string(), LexValue::String(h.to_string()));
    }
    LexValue::Map(map)
}

/// Build a `LexValue::Map` body for an `#account` frame.
pub fn account_body_to_lex_value(
    seq: i64,
    did: &str,
    time: &str,
    active: bool,
    status: Option<&str>,
) -> LexValue {
    let mut map: BTreeMap<String, LexValue> = BTreeMap::new();
    map.insert("seq".to_string(), LexValue::Integer(seq));
    map.insert("did".to_string(), LexValue::String(did.to_string()));
    map.insert("time".to_string(), LexValue::String(time.to_string()));
    map.insert("active".to_string(), LexValue::Bool(active));
    if let Some(s) = status {
        map.insert("status".to_string(), LexValue::String(s.to_string()));
    }
    LexValue::Map(map)
}

/// Build a `LexValue::Map` body for an `#info` frame.
///
/// Per Arc 14 §7.3.4: the lexicon allows `name` value
/// `"OutdatedCursor"` only. Spurious values (`"Connected"`,
/// `"Error"`) emitted by Aurora-Locus pre-Arc-14 are removed.
pub fn info_body_to_lex_value(name: &str, message: Option<&str>) -> LexValue {
    let mut map: BTreeMap<String, LexValue> = BTreeMap::new();
    map.insert("name".to_string(), LexValue::String(name.to_string()));
    if let Some(m) = message {
        map.insert("message".to_string(), LexValue::String(m.to_string()));
    }
    LexValue::Map(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Arc 14 §7.4 Step 1.0(c) Test 1 (round-4 F9 closure):
    /// `canonical_cbor_map!(("t", "#commit"), ("op", 1_i32))` →
    /// emitted bytes start with `0xa2 0x61 0x74` (map-2, "t" first).
    #[test]
    fn step_1_0_c_test_1_two_pair_lex_sort() {
        let m = canonical_cbor_map!(
            ("t", LexValue::String("#commit".to_string())),
            ("op", LexValue::Integer(1)),
        );
        let bytes = lex_cbor::encode(&m).expect("encode");
        // 0xa2 = map of length 2 (CBOR major type 5).
        assert_eq!(bytes[0], 0xa2, "map header byte");
        // 0x61 = text-string length 1 (major type 3).
        assert_eq!(bytes[1], 0x61, "first key length prefix");
        // 0x74 = 't' (ASCII 0x74).
        assert_eq!(bytes[2], 0x74, "first key char 't'");
    }

    /// Arc 14 §7.4 Step 1.0(c) Test 2 (round-4 F9 closure):
    /// `canonical_cbor_map!(("op", -1_i32))` → emitted bytes are
    /// `0xa1 0x62 0x6f 0x70 0x20` (map-1, "op", negative-int -1).
    #[test]
    fn step_1_0_c_test_2_one_pair_negative_int() {
        let m = canonical_cbor_map!(("op", LexValue::Integer(-1)));
        let bytes = lex_cbor::encode(&m).expect("encode");
        assert_eq!(bytes, vec![0xa1, 0x62, 0x6f, 0x70, 0x20]);
    }

    /// Header canonical ordering: per RFC 8949 §4.2.1, "t" (len 1)
    /// sorts before "op" (len 2). `firehose_frame_to_cbor` must
    /// emit a header whose encoded byte sequence starts with `0xa2
    /// 0x61 0x74` regardless of macro-insertion order.
    #[test]
    fn firehose_frame_header_canonical_order() {
        let body = LexValue::Map(BTreeMap::new());
        let bytes = firehose_frame_to_cbor("#commit", body).expect("encode");
        assert_eq!(&bytes[0..3], &[0xa2, 0x61, 0x74]);
    }

    /// Error frame: header is `{op: -1}` (single-pair map).
    #[test]
    fn error_frame_header_op_neg1() {
        let bytes = firehose_error_frame_to_cbor("FutureCursor", Some("test"))
            .expect("encode");
        // 0xa1 = map-1; 0x62 = text-2; "op" = 0x6f 0x70; -1 = 0x20.
        assert_eq!(&bytes[0..5], &[0xa1, 0x62, 0x6f, 0x70, 0x20]);
    }

    /// CommitOp delete-action emits `cid: null` (0xf6), not absent.
    #[test]
    fn commit_op_delete_emits_cid_null() {
        let op = commit_op_to_lex_value(
            "delete",
            "app.bsky.feed.post/abc",
            None,
            None,
        )
        .expect("build");
        let bytes = lex_cbor::encode(&op).expect("encode");
        // Find the 'cid' key followed by 0xf6 (CBOR null).
        // Key "cid" = 0x63 0x63 0x69 0x64; value 0xf6.
        let needle: &[u8] = &[0x63, 0x63, 0x69, 0x64, 0xf6];
        assert!(
            bytes.windows(needle.len()).any(|w| w == needle),
            "expected 'cid: null' (0xf6) in encoded delete-op bytes: {:02x?}",
            bytes
        );
    }

    /// CommitOp create-action emits `cid: <tag-42>` (CID), not null.
    #[test]
    fn commit_op_create_emits_cid_tag42() {
        let op = commit_op_to_lex_value(
            "create",
            "app.bsky.feed.post/abc",
            Some("bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454"),
            None,
        )
        .expect("build");
        let bytes = lex_cbor::encode(&op).expect("encode");
        // No 0xf6 immediately following "cid" key.
        let null_needle: &[u8] = &[0x63, 0x63, 0x69, 0x64, 0xf6];
        assert!(
            !bytes.windows(null_needle.len()).any(|w| w == null_needle),
            "create op must NOT have 'cid: null'"
        );
    }

    /// Arc 14 §7.3.2 / §7.6.2: genesis commit body MUST NOT contain
    /// the `"prevData"` key. Verify via byte-level absence check.
    #[test]
    fn genesis_commit_omits_prev_data_key() {
        let body = commit_body_to_lex_value(
            1,
            false,
            false,
            "did:plc:test",
            "bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454",
            "3l4rev",
            None,
            None, // prev_data absent for genesis
            vec![],
            vec![],
            &[],
            "2026-05-18T00:00:00Z",
        )
        .expect("build");
        let bytes = lex_cbor::encode(&body).expect("encode");
        // Key "prevData" = 8 chars → CBOR text-string-8 prefix 0x68
        // followed by ASCII bytes. Verify absence.
        let prev_data_key: &[u8] = b"\x68prevData";
        assert!(
            !bytes.windows(prev_data_key.len()).any(|w| w == prev_data_key),
            "genesis commit body MUST NOT contain 'prevData' key"
        );
    }

    /// Arc 14 §7.3.2 / §7.6.2: subsequent commit body includes the
    /// `"prevData"` key + a tag-42 CID value.
    #[test]
    fn subsequent_commit_includes_prev_data_key() {
        let body = commit_body_to_lex_value(
            2,
            false,
            false,
            "did:plc:test",
            "bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454",
            "3l4rev",
            Some("bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454"),
            Some("bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454"),
            vec![],
            vec![],
            &[],
            "2026-05-18T00:00:00Z",
        )
        .expect("build");
        let bytes = lex_cbor::encode(&body).expect("encode");
        let prev_data_key: &[u8] = b"\x68prevData";
        assert!(
            bytes.windows(prev_data_key.len()).any(|w| w == prev_data_key),
            "subsequent commit body MUST contain 'prevData' key"
        );
    }

    /// Arc 14 §7.3.2 / §7.6.2: create op MUST NOT contain a `"prev"`
    /// key in its body.
    #[test]
    fn create_op_omits_prev_key() {
        let op = commit_op_to_lex_value(
            "create",
            "app.bsky.feed.post/abc",
            Some("bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454"),
            None,
        )
        .expect("build");
        let bytes = lex_cbor::encode(&op).expect("encode");
        // "prev" key = 4 chars → text-string-4 prefix 0x64 + ASCII.
        let prev_key: &[u8] = b"\x64prev";
        assert!(
            !bytes.windows(prev_key.len()).any(|w| w == prev_key),
            "create op MUST NOT contain 'prev' key"
        );
    }

    /// Arc 14 §7.3.2 / §7.6.2: update op MUST contain a `"prev"` key.
    #[test]
    fn update_op_includes_prev_key() {
        let op = commit_op_to_lex_value(
            "update",
            "app.bsky.feed.post/abc",
            Some("bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454"),
            Some("bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454"),
        )
        .expect("build");
        let bytes = lex_cbor::encode(&op).expect("encode");
        let prev_key: &[u8] = b"\x64prev";
        assert!(
            bytes.windows(prev_key.len()).any(|w| w == prev_key),
            "update op MUST contain 'prev' key"
        );
    }

    /// Arc 14 §7.3.2 / §7.6.2: delete op MUST contain a `"prev"` key
    /// AND emit `cid: null` (0xf6).
    #[test]
    fn delete_op_includes_prev_and_cid_null() {
        let op = commit_op_to_lex_value(
            "delete",
            "app.bsky.feed.post/abc",
            None,
            Some("bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454"),
        )
        .expect("build");
        let bytes = lex_cbor::encode(&op).expect("encode");
        let prev_key: &[u8] = b"\x64prev";
        let cid_null: &[u8] = &[0x63, 0x63, 0x69, 0x64, 0xf6];
        assert!(
            bytes.windows(prev_key.len()).any(|w| w == prev_key),
            "delete op MUST contain 'prev' key"
        );
        assert!(
            bytes.windows(cid_null.len()).any(|w| w == cid_null),
            "delete op MUST emit 'cid: null' (0xf6)"
        );
    }

    /// `blocks` field emits as CBOR major-type-2 (bytes), not base64
    /// string. Verify the encoded body contains the raw byte payload.
    #[test]
    fn commit_blocks_emits_as_bytes() {
        let body = commit_body_to_lex_value(
            42,
            false,
            false,
            "did:plc:test",
            "bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454",
            "3l4rev",
            None,
            None,
            vec![0xde, 0xad, 0xbe, 0xef],
            vec![],
            &[],
            "2026-05-18T00:00:00Z",
        )
        .expect("build");
        let bytes = lex_cbor::encode(&body).expect("encode");
        // CBOR major-type-2 length-4 prefix is 0x44.
        let bytes_marker: &[u8] = &[0x44, 0xde, 0xad, 0xbe, 0xef];
        assert!(
            bytes
                .windows(bytes_marker.len())
                .any(|w| w == bytes_marker),
            "expected raw bytes (0x44 0xde 0xad 0xbe 0xef) in encoded body: {:02x?}",
            bytes
        );
    }
}
