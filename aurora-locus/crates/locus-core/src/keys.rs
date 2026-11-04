//! The master key that encrypts every stored secret (account signing keys,
//! the PDS rotation key, session secrets, credentials).
//!
//! One key per install: its id is derived from the key material, and the
//! database holds the id with a MAC under the key, so a node holding a
//! different key refuses to start or to write a secret. Secrets are sealed
//! with XChaCha20-Poly1305; the associated data names the slot a secret
//! belongs to, so a ciphertext can't be moved to another slot.

use std::io::Write;
use std::path::Path;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::Zeroizing;

use crate::bootstrap::{Bootstrap, DatabaseUrl, MasterKeySource};
use crate::error::{CoreError, CoreResult};

const KEY_ID_LABEL: &[u8] = b"aurora-locus master key id v1";
const CHECK_LABEL: &[u8] = b"aurora-locus master key check v1";
const SEAL_VERSION: u8 = 1;
const NONCE_LEN: usize = 24;

/// Which kind of source the key came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    File,
    Env,
    Kms,
}

impl SourceKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Env => "env",
            Self::Kms => "kms",
        }
    }
}

/// The install's master key. Zeroed when dropped.
pub struct MasterKey {
    key: Zeroizing<[u8; 32]>,
    source: SourceKind,
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MasterKey")
            .field("key_id", &self.key_id())
            .field("source", &self.source)
            .finish()
    }
}

fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC takes any key length");
    for p in parts {
        mac.update(p);
    }
    mac.finalize().into_bytes().into()
}

impl MasterKey {
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32], source: SourceKind) -> Self {
        Self {
            key: Zeroizing::new(bytes),
            source,
        }
    }

    /// Parses 64 hex characters (surrounding whitespace ignored).
    pub fn from_hex(text: &str, source: SourceKind) -> CoreResult<Self> {
        let raw =
            Zeroizing::new(hex::decode(text.trim()).map_err(|_| {
                CoreError::Config("the master key must be 64 hex characters".into())
            })?);
        let bytes: [u8; 32] = raw.as_slice().try_into().map_err(|_| {
            CoreError::Config("the master key must be 32 bytes (64 hex characters)".into())
        })?;
        Ok(Self::from_bytes(bytes, source))
    }

    #[must_use]
    pub fn generate(source: SourceKind) -> Self {
        Self::from_bytes(rand::random(), source)
    }

    #[must_use]
    pub fn source(&self) -> SourceKind {
        self.source
    }

    /// The key's id: derived from the key material, so the same key always
    /// has the same id and different keys never share one.
    #[must_use]
    pub fn key_id(&self) -> String {
        hex::encode(&hmac(self.key.as_slice(), &[KEY_ID_LABEL])[..16])
    }

    /// The MAC stored alongside the key id.
    #[must_use]
    pub fn check_mac(&self) -> Vec<u8> {
        hmac(
            self.key.as_slice(),
            &[CHECK_LABEL, self.key_id().as_bytes()],
        )
        .to_vec()
    }

    /// Whether a stored `(key_id, mac)` was made with this key.
    #[must_use]
    pub fn verifies(&self, key_id: &str, mac: &[u8]) -> bool {
        use subtle_eq::ct_eq;
        key_id == self.key_id() && ct_eq(mac, &self.check_mac())
    }

    /// Encrypts `plaintext` for the slot named by `aad`.
    pub fn seal(&self, aad: &[u8], plaintext: &[u8]) -> CoreResult<Vec<u8>> {
        let cipher = XChaCha20Poly1305::new(self.key.as_slice().into());
        let nonce: [u8; NONCE_LEN] = rand::random();
        let ct = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .map_err(|_| CoreError::Storage("encryption failed".into()))?;
        let mut out = Vec::with_capacity(1 + NONCE_LEN + ct.len());
        out.push(SEAL_VERSION);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Decrypts a sealed secret for the slot named by `aad`. A failure means
    /// the wrong key, the wrong slot, or corrupt data.
    pub fn open(&self, aad: &[u8], sealed: &[u8]) -> CoreResult<Zeroizing<Vec<u8>>> {
        if sealed.len() < 1 + NONCE_LEN || sealed[0] != SEAL_VERSION {
            return Err(CoreError::Storage(
                "sealed secret has an unknown format".into(),
            ));
        }
        let cipher = XChaCha20Poly1305::new(self.key.as_slice().into());
        let (nonce, ct) = sealed[1..].split_at(NONCE_LEN);
        cipher
            .decrypt(XNonce::from_slice(nonce), Payload { msg: ct, aad })
            .map(Zeroizing::new)
            .map_err(|_| CoreError::Storage("a stored secret failed to decrypt".into()))
    }
}

/// Constant-time comparison without another dependency.
mod subtle_eq {
    pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }
}

/// Loads the master key from the configured source.
///
/// A file key is generated only when the key store is empty, the data dir is
/// persistent and the backend is SQLite (a single node); anywhere else locus
/// never generates one, so a key can't silently differ per node or per boot.
pub fn load_master_key(bootstrap: &Bootstrap, key_store_empty: bool) -> CoreResult<MasterKey> {
    match &bootstrap.master_key {
        MasterKeySource::Env => {
            let text = std::env::var("LOCUS_MASTER_KEY").map_err(|_| {
                CoreError::Config("master key source is env but LOCUS_MASTER_KEY is unset".into())
            })?;
            MasterKey::from_hex(&text, SourceKind::Env)
        }
        MasterKeySource::Kms { .. } => Err(CoreError::Config(
            "KMS master keys aren't available in this build yet; use a file or env key".into(),
        )),
        MasterKeySource::File { path, shared } => match std::fs::read_to_string(path) {
            Ok(text) => MasterKey::from_hex(&text, SourceKind::File),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let sqlite = matches!(bootstrap.database, DatabaseUrl::Sqlite(_));
                if !key_store_empty {
                    return Err(CoreError::Config(format!(
                        "the master key file {} is missing, but the database already holds secrets \
                         encrypted with a master key; restore the file",
                        path.display()
                    )));
                }
                if *shared || !sqlite || !bootstrap.data_persistent {
                    return Err(CoreError::Config(format!(
                        "no master key at {}: with {}, supply a key (a shared file, LOCUS_MASTER_KEY, or KMS) \
                         rather than letting each node generate its own",
                        path.display(),
                        if *shared {
                            "a declared shared key file"
                        } else if !sqlite {
                            "Postgres"
                        } else {
                            "a non-persistent data dir"
                        }
                    )));
                }
                let key = MasterKey::generate(SourceKind::File);
                write_key_file(path, &key)?;
                Ok(key)
            }
            Err(e) => Err(CoreError::Config(format!(
                "can't read the master key {}: {e}",
                path.display()
            ))),
        },
    }
}

fn write_key_file(path: &Path, key: &MasterKey) -> CoreResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Config(format!("can't create {}: {e}", parent.display())))?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path).map_err(|e| {
        CoreError::Config(format!(
            "can't create the master key {}: {e}",
            path.display()
        ))
    })?;
    let text = Zeroizing::new(hex::encode(key.key.as_slice()));
    f.write_all(text.as_bytes())
        .and_then(|()| f.write_all(b"\n"))
        .and_then(|()| f.sync_all())
        .map_err(|e| {
            CoreError::Config(format!(
                "can't write the master key {}: {e}",
                path.display()
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn seal_open_round_trip_and_slot_binding() {
        let k = MasterKey::generate(SourceKind::Env);
        let sealed = k.seal(b"signing_key:did:plc:a", b"secret bytes").unwrap();
        assert_eq!(
            k.open(b"signing_key:did:plc:a", &sealed)
                .unwrap()
                .as_slice(),
            b"secret bytes"
        );
        // Moved to another slot, or opened with another key: refused.
        assert!(k.open(b"signing_key:did:plc:b", &sealed).is_err());
        assert!(
            MasterKey::generate(SourceKind::Env)
                .open(b"signing_key:did:plc:a", &sealed)
                .is_err()
        );
        // Tampering: refused.
        let mut bad = sealed.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(k.open(b"signing_key:did:plc:a", &bad).is_err());
    }

    #[test]
    fn key_id_is_derived_and_check_verifies_only_the_same_key() {
        let k = MasterKey::from_hex(&"ab".repeat(32), SourceKind::File).unwrap();
        let same = MasterKey::from_hex(&"ab".repeat(32), SourceKind::Env).unwrap();
        let other = MasterKey::from_hex(&"cd".repeat(32), SourceKind::File).unwrap();
        assert_eq!(k.key_id(), same.key_id());
        assert_ne!(k.key_id(), other.key_id());
        assert!(same.verifies(&k.key_id(), &k.check_mac()));
        assert!(!other.verifies(&k.key_id(), &k.check_mac()));
        assert!(MasterKey::from_hex("abcd", SourceKind::Env).is_err());
    }

    fn boot(dir: &Path, extra: &[(&str, &str)]) -> Bootstrap {
        let mut env: HashMap<String, String> = HashMap::new();
        env.insert("LOCUS_DATA_DIR".into(), dir.display().to_string());
        for (k, v) in extra {
            env.insert((*k).into(), (*v).into());
        }
        Bootstrap::load(None, &env).unwrap()
    }

    #[test]
    fn file_keys_are_generated_only_where_safe() {
        let dir = tempfile::tempdir().unwrap();
        let b = boot(dir.path(), &[]);
        // Empty store, SQLite, persistent: generated once, then reused.
        let k1 = load_master_key(&b, true).unwrap();
        let k2 = load_master_key(&b, false).unwrap();
        assert_eq!(k1.key_id(), k2.key_id());

        // Postgres never generates.
        let dir = tempfile::tempdir().unwrap();
        let b = boot(dir.path(), &[("LOCUS_DATABASE_URL", "postgres://x/y")]);
        assert!(load_master_key(&b, true).is_err());

        // Non-persistent data dir never generates.
        let dir = tempfile::tempdir().unwrap();
        let b = boot(dir.path(), &[("LOCUS_DATA_PERSISTENT", "false")]);
        assert!(load_master_key(&b, true).is_err());

        // A missing file with secrets already stored is an error, not a new key.
        let dir = tempfile::tempdir().unwrap();
        let b = boot(dir.path(), &[]);
        assert!(load_master_key(&b, false).is_err());
    }
}
