//! Key backups: every stored secret, every account signing key and the PLC
//! rotation keys' roles, encrypted under an operator-chosen passphrase.
//!
//! A backup is a JSON header line followed by frames. The header names the
//! format and the Argon2id parameters; the passphrase, stretched with them,
//! is the XChaCha20-Poly1305 key of every frame. A frame is a 4-byte
//! big-endian length, a 24-byte nonce and the sealed JSON of a batch of
//! entries. Each frame's associated data is the header's SHA-256, the
//! frame's index and whether it's the last, so frames can't be reordered,
//! dropped, moved between backups or cut off after a frame without the
//! reader noticing. Backups stream: neither side holds the whole set.

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::error::{CoreError, CoreResult};

/// The header's `format` value.
pub const FORMAT: &str = "aurora-locus-key-backup";
/// The shortest passphrase a backup takes.
pub const MIN_PASSPHRASE: usize = 12;
/// The most bytes one frame may hold.
const MAX_FRAME: usize = 64 * 1024 * 1024;
const NONCE_LEN: usize = 24;

/// Argon2id's cost parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfCost {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl KdfCost {
    /// The cost new backups use: 64 MiB, three passes.
    pub const DEFAULT: Self = Self {
        m_kib: 64 * 1024,
        t: 3,
        p: 1,
    };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Kdf {
    pub alg: String,
    #[serde(flatten)]
    pub cost: KdfCost,
    /// Base64.
    pub salt: String,
}

/// What a backup says about itself (readable without the passphrase).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub format: String,
    pub version: u32,
    pub created_at: i64,
    /// The service DID of the install it came from.
    pub service_did: String,
    /// The id of the master key the install used then.
    pub master_key_id: String,
    pub kdf: Kdf,
}

mod b64 {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};
    use zeroize::Zeroizing;

    pub fn serialize<S: Serializer>(v: &Zeroizing<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        let text = Zeroizing::new(STANDARD.encode(v.as_slice()));
        s.serialize_str(&text)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Zeroizing<Vec<u8>>, D::Error> {
        let text = Zeroizing::new(String::deserialize(d)?);
        STANDARD
            .decode(text.as_bytes())
            .map(Zeroizing::new)
            .map_err(serde::de::Error::custom)
    }
}

/// One thing a backup holds.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Entry {
    /// A named secret of the key store (PLC rotation keys' private halves
    /// among them).
    Secret {
        name: String,
        #[serde(with = "b64")]
        value: Zeroizing<Vec<u8>>,
    },
    /// An account's signing key.
    SigningKey {
        did: String,
        key_type: String,
        #[serde(with = "b64")]
        value: Zeroizing<Vec<u8>>,
    },
    /// A held PLC rotation key and its role then.
    PlcKey { did_key: String, role: String },
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Secret { name, .. } => write!(f, "Secret({name})"),
            Self::SigningKey { did, .. } => write!(f, "SigningKey({did})"),
            Self::PlcKey { did_key, role } => write!(f, "PlcKey({did_key}, {role})"),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Frame {
    entries: Vec<Entry>,
}

fn bad(msg: impl Into<String>) -> CoreError {
    CoreError::Invalid(msg.into())
}

fn derive(passphrase: &str, salt: &[u8], cost: KdfCost) -> CoreResult<Zeroizing<[u8; 32]>> {
    let params = Params::new(cost.m_kib, cost.t, cost.p, Some(32))
        .map_err(|e| bad(format!("key backup cost: {e}")))?;
    let mut out = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, out.as_mut_slice())
        .map_err(|e| bad(format!("key backup passphrase: {e}")))?;
    Ok(out)
}

fn aad(header_hash: &[u8; 32], index: u64, last: bool) -> Vec<u8> {
    let mut a = Vec::with_capacity(41);
    a.extend_from_slice(header_hash);
    a.extend_from_slice(&index.to_be_bytes());
    a.push(u8::from(last));
    a
}

/// Writes a backup frame by frame.
pub struct Sealer {
    cipher: XChaCha20Poly1305,
    header_hash: [u8; 32],
    index: u64,
    done: bool,
}

impl Sealer {
    /// A new backup; returns the sealer and the header line to write first.
    pub fn new(
        passphrase: &str,
        service_did: &str,
        master_key_id: &str,
        created_at: i64,
        cost: KdfCost,
    ) -> CoreResult<(Self, Vec<u8>)> {
        if passphrase.chars().count() < MIN_PASSPHRASE {
            return Err(bad(format!(
                "A key backup passphrase is at least {MIN_PASSPHRASE} characters"
            )));
        }
        let salt: [u8; 16] = rand::random();
        let header = Header {
            format: FORMAT.into(),
            version: 1,
            created_at,
            service_did: service_did.into(),
            master_key_id: master_key_id.into(),
            kdf: Kdf {
                alg: "argon2id".into(),
                cost,
                salt: B64.encode(salt),
            },
        };
        let mut line =
            serde_json::to_vec(&header).map_err(|e| CoreError::Storage(e.to_string()))?;
        line.push(b'\n');
        let key = derive(passphrase, &salt, cost)?;
        Ok((
            Self {
                cipher: XChaCha20Poly1305::new(key.as_slice().into()),
                header_hash: Sha256::digest(&line).into(),
                index: 0,
                done: false,
            },
            line,
        ))
    }

    /// The next frame; `last` ends the backup.
    pub fn frame(&mut self, entries: Vec<Entry>, last: bool) -> CoreResult<Vec<u8>> {
        if self.done {
            return Err(CoreError::Storage("the backup is already finished".into()));
        }
        let plain = Zeroizing::new(
            serde_json::to_vec(&Frame { entries })
                .map_err(|e| CoreError::Storage(e.to_string()))?,
        );
        let nonce: [u8; NONCE_LEN] = rand::random();
        let sealed = self
            .cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &plain,
                    aad: &aad(&self.header_hash, self.index, last),
                },
            )
            .map_err(|_| CoreError::Storage("couldn't seal a backup frame".into()))?;
        let len = u32::try_from(NONCE_LEN + sealed.len())
            .map_err(|_| CoreError::Storage("backup frame too large".into()))?;
        let mut out = Vec::with_capacity(4 + len as usize);
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        self.index += 1;
        self.done = last;
        Ok(out)
    }
}

/// Reads a backup's header without the passphrase.
pub fn header(bytes: &[u8]) -> CoreResult<Header> {
    let end = bytes
        .iter()
        .position(|b| *b == b'\n')
        .ok_or_else(|| bad("This isn't a key backup"))?;
    let h: Header =
        serde_json::from_slice(&bytes[..end]).map_err(|_| bad("This isn't a key backup"))?;
    if h.format != FORMAT || h.version != 1 || h.kdf.alg != "argon2id" {
        return Err(bad("This key backup's format isn't one this version reads"));
    }
    Ok(h)
}

/// Opens a whole backup: its header and every entry. A wrong passphrase,
/// a changed byte, a missing or extra frame all fail.
pub fn open(bytes: &[u8], passphrase: &str) -> CoreResult<(Header, Vec<Entry>)> {
    let h = header(bytes)?;
    let end = bytes.iter().position(|b| *b == b'\n').unwrap_or(0) + 1;
    let header_hash: [u8; 32] = Sha256::digest(&bytes[..end]).into();
    let salt = B64
        .decode(&h.kdf.salt)
        .map_err(|_| bad("This key backup's header is damaged"))?;
    // Refuse costs no backup this version writes would have, so a crafted
    // header can't make the reader allocate without bound.
    if h.kdf.cost.m_kib > 4 * 1024 * 1024 || h.kdf.cost.t > 64 || h.kdf.cost.p > 64 {
        return Err(bad("This key backup's header is damaged"));
    }
    let key = derive(passphrase, &salt, h.kdf.cost)?;
    let cipher = XChaCha20Poly1305::new(key.as_slice().into());
    let mut entries = Vec::new();
    let mut at = end;
    let mut index = 0u64;
    loop {
        let Some(len_bytes) = bytes.get(at..at + 4) else {
            return Err(bad("This key backup is incomplete"));
        };
        let len = u32::from_be_bytes(len_bytes.try_into().unwrap_or_default()) as usize;
        if !(NONCE_LEN + 16..=MAX_FRAME).contains(&len) {
            return Err(bad("This key backup is damaged"));
        }
        let frame = bytes
            .get(at + 4..at + 4 + len)
            .ok_or_else(|| bad("This key backup is incomplete"))?;
        at += 4 + len;
        let last = at == bytes.len();
        let plain = cipher
            .decrypt(
                XNonce::from_slice(&frame[..NONCE_LEN]),
                Payload {
                    msg: &frame[NONCE_LEN..],
                    aad: &aad(&header_hash, index, last),
                },
            )
            .map_err(|_| {
                if index == 0 {
                    bad("That passphrase doesn't open this key backup")
                } else {
                    bad("This key backup is damaged or incomplete")
                }
            })?;
        let plain = Zeroizing::new(plain);
        let f: Frame =
            serde_json::from_slice(&plain).map_err(|_| bad("This key backup is damaged"))?;
        entries.extend(f.entries);
        index += 1;
        if last {
            return Ok((h, entries));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHEAP: KdfCost = KdfCost {
        m_kib: 256,
        t: 1,
        p: 1,
    };

    fn secret(name: &str, v: &[u8]) -> Entry {
        Entry::Secret {
            name: name.into(),
            value: Zeroizing::new(v.to_vec()),
        }
    }

    fn sample() -> (Vec<u8>, Vec<Entry>) {
        let (mut s, mut out) =
            Sealer::new("a long passphrase", "did:web:pds.example", "kid", 5, CHEAP).unwrap();
        let a = vec![
            secret("jwt_secret", b"j"),
            Entry::PlcKey {
                did_key: "did:key:zA".into(),
                role: "current".into(),
            },
        ];
        let b = vec![Entry::SigningKey {
            did: "did:plc:x".into(),
            key_type: "k256".into(),
            value: Zeroizing::new(vec![1, 2, 3]),
        }];
        out.extend(s.frame(a.clone(), false).unwrap());
        out.extend(s.frame(b.clone(), false).unwrap());
        out.extend(s.frame(vec![], true).unwrap());
        assert!(
            s.frame(vec![], true).is_err(),
            "nothing after the last frame"
        );
        (out, [a, b].concat())
    }

    #[test]
    fn round_trips_and_refuses_tampering() {
        let (bytes, entries) = sample();
        let (h, got) = open(&bytes, "a long passphrase").unwrap();
        assert_eq!(
            (h.service_did.as_str(), h.created_at),
            ("did:web:pds.example", 5)
        );
        assert_eq!(got, entries);
        assert_eq!(header(&bytes).unwrap().master_key_id, "kid");

        let err = |b: &[u8], p: &str| open(b, p).unwrap_err().to_string();
        assert!(err(&bytes, "the wrong passphrase").contains("passphrase"));
        // Cut after a frame: the last flag is missing.
        let first_len = {
            let end = bytes.iter().position(|b| *b == b'\n').unwrap() + 1;
            end + 4 + u32::from_be_bytes(bytes[end..end + 4].try_into().unwrap()) as usize
        };
        assert!(open(&bytes[..first_len], "a long passphrase").is_err());
        // A flipped byte anywhere in a frame.
        let mut flipped = bytes.clone();
        let i = flipped.len() - 3;
        flipped[i] ^= 1;
        assert!(open(&flipped, "a long passphrase").is_err());
        // A changed header (e.g. another install's DID).
        let text = String::from_utf8_lossy(&bytes).replace("pds.example", "pds.exampla");
        assert!(open(text.as_bytes(), "a long passphrase").is_err());
        // Frames from another backup.
        let (other, _) = sample();
        let end = bytes.iter().position(|b| *b == b'\n').unwrap() + 1;
        let other_end = other.iter().position(|b| *b == b'\n').unwrap() + 1;
        let mut mixed = bytes[..end].to_vec();
        mixed.extend_from_slice(&other[other_end..]);
        assert!(open(&mixed, "a long passphrase").is_err());
        assert!(Sealer::new("short", "d", "k", 0, CHEAP).is_err());
        assert!(open(b"{}\n", "a long passphrase").is_err());
    }
}
