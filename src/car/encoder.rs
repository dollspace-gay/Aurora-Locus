use crate::error::{PdsError, PdsResult};
use libipld::Cid;
use serde_cbor;

/// CARv1 file encoder for ATProto repositories
///
/// CAR (Content Addressable aRchive) format specification:
/// - Header: CBOR-encoded { version: 1, roots: [CID] }
/// - Blocks: Repeated { varint(cid_len), cid_bytes, varint(block_len), block_bytes }
pub struct CarEncoder {
    buffer: Vec<u8>,
}

impl CarEncoder {
    /// Create a new CAR encoder with the given root CID
    pub fn new(root: &Cid) -> PdsResult<Self> {
        let mut buffer = Vec::new();

        // Encode CAR header
        let header = serde_json::json!({
            "version": 1,
            "roots": [root.to_string()]
        });

        let header_bytes = serde_cbor::to_vec(&header)
            .map_err(|e| PdsError::Internal(format!("Failed to encode CAR header: {}", e)))?;

        // Write header length as varint
        write_varint(&mut buffer, header_bytes.len() as u64);
        buffer.extend_from_slice(&header_bytes);

        Ok(Self { buffer })
    }

    /// Add a block to the CAR file
    pub fn add_block(&mut self, cid: &Cid, data: &[u8]) -> PdsResult<()> {
        // Write CID
        let cid_bytes = cid.to_bytes();
        write_varint(&mut self.buffer, cid_bytes.len() as u64);
        self.buffer.extend_from_slice(&cid_bytes);

        // Write block data
        write_varint(&mut self.buffer, data.len() as u64);
        self.buffer.extend_from_slice(data);

        Ok(())
    }

    /// Add blocks from a collection of CID/data pairs
    pub fn add_blocks(&mut self, blocks: Vec<(Cid, Vec<u8>)>) -> PdsResult<()> {
        for (cid, data) in blocks {
            self.add_block(&cid, &data)?;
        }
        Ok(())
    }

    /// Finalize and return the CAR file bytes
    pub fn finalize(self) -> Vec<u8> {
        self.buffer
    }
}

/// Write an unsigned varint to a buffer
fn write_varint(buffer: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        buffer.push((value as u8) | 0x80);
        value >>= 7;
    }
    buffer.push(value as u8);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_varint_encoding() {
        let mut buf = Vec::new();
        write_varint(&mut buf, 0);
        assert_eq!(buf, vec![0]);

        let mut buf = Vec::new();
        write_varint(&mut buf, 127);
        assert_eq!(buf, vec![127]);

        let mut buf = Vec::new();
        write_varint(&mut buf, 128);
        assert_eq!(buf, vec![0x80, 0x01]);

        let mut buf = Vec::new();
        write_varint(&mut buf, 300);
        assert_eq!(buf, vec![0xAC, 0x02]);
    }

    #[test]
    fn test_car_encoder_creation() {
        let cid = Cid::try_from("bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454").unwrap();
        let encoder = CarEncoder::new(&cid);
        assert!(encoder.is_ok());
    }
}
