//! CAR (Content Addressable aRchive) file format implementation
//!
//! This module implements reading and writing of CAR v1 files, which are used
//! by ATProto for repository exports and imports.
//!
//! # CAR Format
//!
//! A CAR file consists of:
//! 1. Header: Contains version and root CIDs
//! 2. Sections: Each section contains a length prefix, CID, and block data
//!
//! # Specification
//!
//! - Official spec: https://ipld.io/specs/transport/car/carv1/
//! - Uses length-prefix framing (varint encoding)
//! - Blocks are raw IPLD data
//! - No internal indexing (sequential read only)
//!
//! # Example
//!
//! ```no_run
//! use atproto::car::{CarWriter, CarReader};
//! use libipld::cid::Cid;
//!
//! // Writing a CAR file
//! let mut writer = CarWriter::new(vec![]);
//! // writer.add_block(cid, block_data)?;
//! // let car_bytes = writer.finish()?;
//!
//! // Reading a CAR file
//! // let reader = CarReader::new(&car_bytes)?;
//! // for block in reader.blocks() {
//! //     let (cid, data) = block?;
//! //     // process block
//! // }
//! ```

use libipld::cid::Cid;
use libipld::codec::Codec;
use libipld::Ipld;
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use thiserror::Error;

/// Error types for CAR operations
#[derive(Error, Debug)]
pub enum CarError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Invalid CAR format: {0}")]
    InvalidFormat(String),

    #[error("CID error: {0}")]
    Cid(String),

    #[error("CBOR error: {0}")]
    Cbor(String),

    #[error("Varint decode error: {0}")]
    Varint(String),

    #[error("Invalid header: {0}")]
    InvalidHeader(String),

    #[error("End of file")]
    Eof,
}

/// Result type for CAR operations
pub type Result<T> = std::result::Result<T, CarError>;

/// CAR file header
#[derive(Debug, Clone)]
pub struct CarHeader {
    /// CAR format version (always 1 for CARv1)
    pub version: u64,

    /// Root CIDs referenced in this CAR
    pub roots: Vec<Cid>,
}

impl CarHeader {
    /// Create a new CAR header with the given root CIDs
    pub fn new(roots: Vec<Cid>) -> Self {
        Self { version: 1, roots }
    }

    /// Convert to IPLD representation
    fn to_ipld(&self) -> Ipld {
        let mut map = BTreeMap::new();
        map.insert("version".to_string(), Ipld::Integer(self.version as i128));
        map.insert(
            "roots".to_string(),
            Ipld::List(self.roots.iter().map(|cid| Ipld::Link(*cid)).collect()),
        );
        Ipld::Map(map)
    }

    /// Create from IPLD representation
    fn from_ipld(ipld: &Ipld) -> Result<Self> {
        if let Ipld::Map(map) = ipld {
            let version = match map.get("version") {
                Some(Ipld::Integer(i)) => *i as u64,
                _ => return Err(CarError::InvalidHeader("Missing or invalid version field".to_string())),
            };

            let roots_ipld = match map.get("roots") {
                Some(Ipld::List(list)) => list,
                _ => return Err(CarError::InvalidHeader("Missing or invalid roots field".to_string())),
            };

            let mut roots = Vec::new();
            for root_ipld in roots_ipld {
                if let Ipld::Link(cid) = root_ipld {
                    roots.push(*cid);
                } else {
                    return Err(CarError::InvalidHeader("Root is not a CID".to_string()));
                }
            }

            Ok(Self { version, roots })
        } else {
            Err(CarError::InvalidHeader("Header is not a map".to_string()))
        }
    }

    /// Encode the header to CBOR bytes
    pub fn to_cbor(&self) -> Result<Vec<u8>> {
        let ipld = self.to_ipld();
        libipld_cbor::DagCborCodec.encode(&ipld)
            .map_err(|e| CarError::Cbor(format!("Failed to encode header: {}", e)))
    }

    /// Decode the header from CBOR bytes
    pub fn from_cbor(bytes: &[u8]) -> Result<Self> {
        let ipld = libipld_cbor::DagCborCodec.decode(bytes)
            .map_err(|e| CarError::Cbor(format!("Failed to decode header: {}", e)))?;
        Self::from_ipld(&ipld)
    }
}

/// CAR file writer
///
/// Writes blocks to a CAR file with proper framing.
pub struct CarWriter<W: Write> {
    writer: W,
    header_written: bool,
    roots: Vec<Cid>,
}

impl<W: Write> CarWriter<W> {
    /// Create a new CAR writer
    ///
    /// # Arguments
    ///
    /// * `writer` - The output writer
    ///
    /// # Example
    ///
    /// ```
    /// use atproto::car::CarWriter;
    ///
    /// let mut writer = CarWriter::new(Vec::new());
    /// ```
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            header_written: false,
            roots: Vec::new(),
        }
    }

    /// Create a new CAR writer with specified root CIDs
    pub fn with_roots(writer: W, roots: Vec<Cid>) -> Self {
        Self {
            writer,
            header_written: false,
            roots,
        }
    }

    /// Write the CAR header (called automatically on first block write)
    fn write_header(&mut self) -> Result<()> {
        if self.header_written {
            return Ok(());
        }

        let header = CarHeader::new(self.roots.clone());
        let header_bytes = header.to_cbor()?;

        // Write header length as varint
        write_varint(&mut self.writer, header_bytes.len() as u64)?;

        // Write header bytes
        self.writer.write_all(&header_bytes)?;

        self.header_written = true;
        Ok(())
    }

    /// Add a root CID (must be called before writing any blocks)
    pub fn add_root(&mut self, cid: Cid) -> Result<()> {
        if self.header_written {
            return Err(CarError::InvalidFormat(
                "Cannot add roots after header is written".to_string(),
            ));
        }
        self.roots.push(cid);
        Ok(())
    }

    /// Write a block to the CAR file
    ///
    /// # Arguments
    ///
    /// * `cid` - The CID of the block
    /// * `data` - The block data
    pub fn write_block(&mut self, cid: &Cid, data: &[u8]) -> Result<()> {
        // Ensure header is written
        self.write_header()?;

        // Serialize CID to bytes
        let cid_bytes = cid.to_bytes();

        // Calculate total section length (CID + data)
        let section_len = cid_bytes.len() + data.len();

        // Write section length as varint
        write_varint(&mut self.writer, section_len as u64)?;

        // Write CID bytes
        self.writer.write_all(&cid_bytes)?;

        // Write block data
        self.writer.write_all(data)?;

        Ok(())
    }

    /// Finish writing and return the underlying writer
    pub fn finish(mut self) -> Result<W> {
        // Ensure header is written even if no blocks
        self.write_header()?;
        self.writer.flush()?;
        Ok(self.writer)
    }
}

/// CAR file reader
///
/// Reads blocks sequentially from a CAR file.
pub struct CarReader<R: Read> {
    reader: R,
    header: CarHeader,
}

impl<R: Read> CarReader<R> {
    /// Create a new CAR reader
    ///
    /// This reads and validates the header immediately.
    pub fn new(mut reader: R) -> Result<Self> {
        // Read header length
        let header_len = read_varint(&mut reader)?;

        // Read header bytes
        let mut header_bytes = vec![0u8; header_len as usize];
        reader.read_exact(&mut header_bytes)?;

        // Parse header
        let header = CarHeader::from_cbor(&header_bytes)?;

        // Validate version
        if header.version != 1 {
            return Err(CarError::InvalidHeader(format!(
                "Unsupported CAR version: {}",
                header.version
            )));
        }

        Ok(Self { reader, header })
    }

    /// Get the header
    pub fn header(&self) -> &CarHeader {
        &self.header
    }

    /// Get the root CIDs
    pub fn roots(&self) -> &[Cid] {
        &self.header.roots
    }

    /// Read the next block from the CAR file
    ///
    /// Returns `None` when EOF is reached.
    pub fn read_block(&mut self) -> Result<Option<(Cid, Vec<u8>)>> {
        // Try to read section length
        let section_len = match read_varint(&mut self.reader) {
            Ok(len) => len,
            Err(CarError::Eof) => return Ok(None),
            Err(e) => return Err(e),
        };

        // Read the entire section
        let mut section_data = vec![0u8; section_len as usize];
        self.reader.read_exact(&mut section_data)?;

        // Parse CID from the section
        let (cid, cid_len) = parse_cid_from_bytes(&section_data)?;

        // The rest is the block data
        let block_data = section_data[cid_len..].to_vec();

        Ok(Some((cid, block_data)))
    }

    /// Iterate over all blocks in the CAR file
    pub fn blocks(self) -> CarBlockIterator<R> {
        CarBlockIterator { reader: self }
    }
}

/// Iterator over blocks in a CAR file
pub struct CarBlockIterator<R: Read> {
    reader: CarReader<R>,
}

impl<R: Read> Iterator for CarBlockIterator<R> {
    type Item = Result<(Cid, Vec<u8>)>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.reader.read_block() {
            Ok(Some(block)) => Some(Ok(block)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}

/// Write an unsigned varint to a writer
fn write_varint<W: Write>(writer: &mut W, mut value: u64) -> Result<()> {
    while value >= 0x80 {
        writer.write_all(&[(value as u8) | 0x80])?;
        value >>= 7;
    }
    writer.write_all(&[value as u8])?;
    Ok(())
}

/// Read an unsigned varint from a reader
fn read_varint<R: Read>(reader: &mut R) -> Result<u64> {
    let mut value = 0u64;
    let mut shift = 0u32;
    let mut buf = [0u8; 1];

    loop {
        match reader.read_exact(&mut buf) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(CarError::Eof);
            }
            Err(e) => return Err(CarError::Io(e)),
        }

        let byte = buf[0];
        value |= ((byte & 0x7F) as u64) << shift;

        if byte & 0x80 == 0 {
            break;
        }

        shift += 7;

        if shift >= 64 {
            return Err(CarError::Varint("Varint too large".to_string()));
        }
    }

    Ok(value)
}

/// Parse a CID from bytes, returning the CID and the number of bytes consumed
fn parse_cid_from_bytes(bytes: &[u8]) -> Result<(Cid, usize)> {
    // CID parsing using libipld
    let cid = Cid::read_bytes(bytes).map_err(|e| CarError::Cid(e.to_string()))?;

    // Calculate how many bytes were consumed
    let cid_bytes = cid.to_bytes();
    let cid_len = cid_bytes.len();

    Ok((cid, cid_len))
}

#[cfg(test)]
mod tests {
    use super::*;
    use libipld::multihash::Multihash;
    use sha2::{Digest, Sha256};

    fn create_test_cid(data: &[u8]) -> Cid {
        let hash = Sha256::digest(data);
        let multihash = Multihash::wrap(0x12, &hash).unwrap();
        Cid::new_v1(0x71, multihash)
    }

    #[test]
    fn test_car_header_creation() {
        let cid = create_test_cid(b"test");
        let header = CarHeader::new(vec![cid]);

        assert_eq!(header.version, 1);
        assert_eq!(header.roots.len(), 1);
    }

    #[test]
    fn test_car_header_serialization() {
        let cid = create_test_cid(b"test");
        let header = CarHeader::new(vec![cid.clone()]);

        let cbor = header.to_cbor().unwrap();
        assert!(!cbor.is_empty());

        let decoded = CarHeader::from_cbor(&cbor).unwrap();
        assert_eq!(decoded.version, header.version);
        assert_eq!(decoded.roots.len(), header.roots.len());
        assert_eq!(decoded.roots[0], cid);
    }

    #[test]
    fn test_varint_encoding() {
        let test_cases = vec![0u64, 1, 127, 128, 255, 256, 65535, 1000000];

        for value in test_cases {
            let mut buf = Vec::new();
            write_varint(&mut buf, value).unwrap();

            let mut reader = &buf[..];
            let decoded = read_varint(&mut reader).unwrap();

            assert_eq!(decoded, value, "Failed for value {}", value);
        }
    }

    #[test]
    fn test_car_write_and_read_single_block() {
        let cid = create_test_cid(b"test");
        let block_data = b"Hello, World!";

        // Write CAR
        let mut writer = CarWriter::with_roots(Vec::new(), vec![cid.clone()]);
        writer.write_block(&cid, block_data).unwrap();
        let car_bytes = writer.finish().unwrap();

        // Read CAR
        let mut reader = CarReader::new(&car_bytes[..]).unwrap();

        assert_eq!(reader.roots().len(), 1);
        assert_eq!(reader.roots()[0], cid);

        let (read_cid, read_data) = reader.read_block().unwrap().unwrap();
        assert_eq!(read_cid, cid);
        assert_eq!(&read_data, block_data);

        // Should be EOF
        assert!(reader.read_block().unwrap().is_none());
    }

    #[test]
    fn test_car_write_and_read_multiple_blocks() {
        let cid1 = create_test_cid(b"block1");
        let cid2 = create_test_cid(b"block2");
        let cid3 = create_test_cid(b"block3");

        let data1 = b"First block";
        let data2 = b"Second block";
        let data3 = b"Third block";

        // Write CAR
        let mut writer = CarWriter::with_roots(Vec::new(), vec![cid1.clone()]);
        writer.write_block(&cid1, data1).unwrap();
        writer.write_block(&cid2, data2).unwrap();
        writer.write_block(&cid3, data3).unwrap();
        let car_bytes = writer.finish().unwrap();

        // Read CAR using iterator
        let reader = CarReader::new(&car_bytes[..]).unwrap();
        let blocks: Vec<_> = reader.blocks().collect();

        assert_eq!(blocks.len(), 3);

        let (c1, d1) = blocks[0].as_ref().unwrap();
        assert_eq!(c1, &cid1);
        assert_eq!(d1, data1);

        let (c2, d2) = blocks[1].as_ref().unwrap();
        assert_eq!(c2, &cid2);
        assert_eq!(d2, data2);

        let (c3, d3) = blocks[2].as_ref().unwrap();
        assert_eq!(c3, &cid3);
        assert_eq!(d3, data3);
    }

    #[test]
    fn test_car_empty() {
        let cid = create_test_cid(b"root");

        // Write empty CAR (header only)
        let writer = CarWriter::with_roots(Vec::new(), vec![cid.clone()]);
        let car_bytes = writer.finish().unwrap();

        // Read CAR
        let mut reader = CarReader::new(&car_bytes[..]).unwrap();
        assert_eq!(reader.roots().len(), 1);

        // Should be EOF immediately
        assert!(reader.read_block().unwrap().is_none());
    }

    #[test]
    fn test_car_multiple_roots() {
        let cid1 = create_test_cid(b"root1");
        let cid2 = create_test_cid(b"root2");
        let cid3 = create_test_cid(b"root3");

        let roots = vec![cid1.clone(), cid2.clone(), cid3.clone()];

        // Write CAR with multiple roots
        let writer = CarWriter::with_roots(Vec::new(), roots.clone());
        let car_bytes = writer.finish().unwrap();

        // Read CAR
        let reader = CarReader::new(&car_bytes[..]).unwrap();
        assert_eq!(reader.roots().len(), 3);
        assert_eq!(reader.roots()[0], cid1);
        assert_eq!(reader.roots()[1], cid2);
        assert_eq!(reader.roots()[2], cid3);
    }

    #[test]
    fn test_car_add_root_after_write_fails() {
        let cid1 = create_test_cid(b"test");
        let cid2 = create_test_cid(b"root");

        let mut writer = CarWriter::new(Vec::new());
        writer.write_block(&cid1, b"data").unwrap();

        // Should fail because header is already written
        let result = writer.add_root(cid2);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_cid_from_bytes() {
        let cid = create_test_cid(b"test");
        let cid_bytes = cid.to_bytes();

        let (parsed_cid, len) = parse_cid_from_bytes(&cid_bytes).unwrap();

        assert_eq!(parsed_cid, cid);
        assert_eq!(len, cid_bytes.len());
    }
}
