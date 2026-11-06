//! `aurora-locus keys rewrap`: changes the master key's source. Every
//! stored secret is decrypted with the current key and re-encrypted under
//! the new one, and the install's key check replaced, in one transaction,
//! so a failure before it commits leaves everything as it was. Offline:
//! no node may be running.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use locus_core::bootstrap::{Bootstrap, MasterKeySource};
use locus_core::keys::{MasterKey, SourceKind};
use locus_store::env::CLOCK_SLOT_FAMILY;

/// Where the new master key comes from.
#[derive(Debug, Clone)]
pub enum Target {
    /// A key file at a new path (an existing file there is used).
    File(PathBuf),
    /// An env key: generated and written to `out` for the operator to put
    /// in `LOCUS_MASTER_KEY`.
    Env { out: PathBuf },
    /// A KMS data key under this KMS key.
    Kms(String),
}

fn write_new(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(text.as_bytes())
        .and_then(|()| f.write_all(b"\n"))
        .and_then(|()| f.sync_all())
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Runs the rewrap; returns what to do next.
pub async fn run(bootstrap: &Bootstrap, to: Target) -> Result<String, String> {
    let _lock = crate::node_lock::acquire(bootstrap, crate::node_lock::Holder::Offline)
        .await
        .map_err(|e| e.to_string())?;
    let (store, old) = crate::node::open_store(bootstrap)
        .await
        .map_err(|e| e.to_string())?;
    let live = store
        .live_slot_holders(CLOCK_SLOT_FAMILY)
        .await
        .map_err(|e| e.to_string())?;
    if !live.is_empty() {
        return Err(format!(
            "{} node(s) are running; stop every node first (keys rewrap runs offline)",
            live.len()
        ));
    }
    let mut wrap: Option<(String, Vec<u8>)> = None;
    let (new, next) = match &to {
        Target::File(path) => {
            if let MasterKeySource::File { path: current, .. } = &bootstrap.master_key
                && current == path
            {
                return Err("the new key file must be at a new path, not the current one".into());
            }
            let key = match std::fs::read_to_string(path) {
                Ok(text) => {
                    MasterKey::from_hex(&text, SourceKind::File).map_err(|e| e.to_string())?
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    let key = MasterKey::generate(SourceKind::File);
                    write_new(path, &key.to_hex())?;
                    key
                }
                Err(e) => return Err(format!("{}: {e}", path.display())),
            };
            (
                key,
                format!(
                    "Set LOCUS_MASTER_KEY_SOURCE=file and LOCUS_MASTER_KEY_FILE={} on every node, then start them.",
                    path.display()
                ),
            )
        }
        Target::Env { out } => {
            if out.exists() {
                return Err(format!("{} exists; choose a new file", out.display()));
            }
            let key = MasterKey::generate(SourceKind::Env);
            // Written before anything changes, so the key can't be lost.
            write_new(out, &key.to_hex())?;
            (
                key,
                format!(
                    "Put the key in {} into LOCUS_MASTER_KEY, set LOCUS_MASTER_KEY_SOURCE=env on every node, \
                     start them, then delete the file.",
                    out.display()
                ),
            )
        }
        Target::Kms(key_id) => {
            let env = std::env::vars().collect();
            let kms = locus_identity::kms::Kms::from_env(&env).map_err(|e| e.to_string())?;
            let (plain, wrapped) = kms
                .generate_data_key(key_id)
                .await
                .map_err(|e| e.to_string())?;
            wrap = Some((key_id.clone(), wrapped));
            (
                MasterKey::from_bytes(*plain, SourceKind::Kms),
                format!(
                    "Set LOCUS_MASTER_KEY_SOURCE=kms and LOCUS_MASTER_KEY_KMS_ID={key_id} on every node, then start them."
                ),
            )
        }
    };
    if new.key_id() == old.key_id() {
        return Err("that is the key in use already".into());
    }
    let mut tx = store.begin().await.map_err(|e| e.to_string())?;
    let (secrets, signing) = tx.rewrap_all(&old, &new).await.map_err(|e| e.to_string())?;
    tx.set_master_key_wrap(wrap.as_ref().map(|(id, w)| (id.as_str(), w.as_slice())))
        .await
        .map_err(|e| e.to_string())?;
    tx.commit().await.map_err(|e| e.to_string())?;
    store.close().await;
    Ok(format!(
        "Rewrapped {secrets} secrets and {signing} account keys under master key {}.\n{next}\n\
         Key backups made before this still need the old key: make a new one (aurora-locus keys backup) once a node runs.",
        new.key_id()
    ))
}
