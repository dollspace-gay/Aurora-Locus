//! Robustness of firehose framing: whatever a stored event holds, framing
//! answers with a frame or an error, and a framed event decodes back to
//! its body with `seq` and `time` added.

use std::collections::BTreeMap;

use proptest::prelude::*;
use proto_blue_lex_data::{Cid, LexValue};
use proto_blue_ws::frame::Frame;

use super::*;

fn lex_value() -> impl Strategy<Value = LexValue> {
    let leaf = prop_oneof![
        Just(LexValue::Null),
        any::<bool>().prop_map(LexValue::Bool),
        any::<i64>().prop_map(LexValue::Integer),
        ".{0,12}".prop_map(LexValue::String),
        prop::collection::vec(any::<u8>(), 0..16).prop_map(LexValue::Bytes),
        any::<u8>().prop_map(|n| LexValue::Cid(Cid::for_cbor(&[n]))),
    ];
    leaf.prop_recursive(3, 32, 5, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(LexValue::Array),
            prop::collection::btree_map(".{0,6}", inner, 0..4).prop_map(LexValue::Map),
        ]
    })
}

fn event(payload: Vec<u8>, seq: i64, at: i64) -> SeqEvent {
    SeqEvent {
        seq,
        did: "did:plc:x".into(),
        kind: "commit".into(),
        payload,
        sequenced_at: at,
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2048, ..ProptestConfig::default() })]

    #[test]
    fn framing_arbitrary_payloads_never_panics(
        payload in prop::collection::vec(any::<u8>(), 0..200),
        seq in any::<i64>(),
        at in any::<i64>(),
    ) {
        let _ = frame(&event(payload, seq, at));
    }

    #[test]
    fn framed_events_decode_to_their_body(
        body in prop::collection::btree_map("[a-z]{1,6}", lex_value(), 0..6),
        seq in 0i64..i64::MAX,
        at in 0i64..253_402_300_799_000,
    ) {
        let payload = proto_blue_lex_cbor::encode(&LexValue::Map(body.clone())).unwrap();
        let f = frame(&event(payload, seq, at)).unwrap();
        prop_assert_eq!(f.seq, seq);
        let Frame::Message(m) = Frame::decode(&f.bytes).unwrap() else {
            panic!("not a message")
        };
        prop_assert_eq!(m.r#type.as_deref(), Some("#commit"));
        let mut want: BTreeMap<String, LexValue> = body;
        want.insert("seq".into(), LexValue::Integer(seq));
        want.insert("time".into(), LexValue::String(locus_core::time::iso8601(at)));
        prop_assert_eq!(m.body, LexValue::Map(want));
    }

    #[test]
    fn decoding_arbitrary_frames_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..200)) {
        let _ = Frame::decode(&bytes);
    }
}
