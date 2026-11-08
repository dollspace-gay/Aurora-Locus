//! Streaming CAR output: the header and each block written separately, so
//! a repository is sent in pages instead of being built in memory. The
//! bytes are the same as proto-blue's `blocks_to_car`.

use std::collections::BTreeMap;

use proto_blue_lex_data::{Cid, LexValue};

fn varint(out: &mut Vec<u8>, mut n: u64) {
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// The CAR header: `{version: 1, roots: [root]}` (no root: `roots: []`).
#[must_use]
pub fn header(root: Option<&Cid>) -> Vec<u8> {
    let mut m = BTreeMap::new();
    m.insert("version".to_owned(), LexValue::Integer(1));
    m.insert(
        "roots".to_owned(),
        LexValue::Array(
            root.map(|c| vec![LexValue::Cid(c.clone())])
                .unwrap_or_default(),
        ),
    );
    let bytes = proto_blue_lex_cbor::encode(&LexValue::Map(m)).expect("a CAR header encodes");
    let mut out = Vec::with_capacity(bytes.len() + 2);
    varint(&mut out, bytes.len() as u64);
    out.extend_from_slice(&bytes);
    out
}

/// One CAR block section: `varint(len) + CID bytes + data`.
#[must_use]
pub fn block(cid: &Cid, data: &[u8]) -> Vec<u8> {
    let cid_bytes = cid.to_bytes();
    let mut out = Vec::with_capacity(cid_bytes.len() + data.len() + 4);
    varint(&mut out, (cid_bytes.len() + data.len()) as u64);
    out.extend_from_slice(&cid_bytes);
    out.extend_from_slice(data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto_blue_repo::block_map::BlockMap;

    #[test]
    fn matches_proto_blue_and_reads_back() {
        let root = Cid::for_cbor(b"root");
        let mut blocks = BlockMap::new();
        for data in [b"one".as_slice(), b"two", &[7u8; 300]] {
            blocks.set(Cid::for_raw(data), data.to_vec());
        }
        let mut ours = header(Some(&root));
        for (cid, data) in blocks.iter() {
            ours.extend(block(cid, data));
        }
        let theirs = proto_blue_repo::car::blocks_to_car(Some(&root), &blocks).unwrap();
        assert_eq!(ours, theirs);
        let (roots, read) = proto_blue_repo::car::read_car(&ours).unwrap();
        assert_eq!(roots, vec![root]);
        assert_eq!(read.iter().count(), 3);
    }
}
