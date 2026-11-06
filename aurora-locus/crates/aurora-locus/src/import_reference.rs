//! `aurora-locus import reference-pds <data dir> --env <pds.env>`: turns a
//! stopped reference PDS (0.5.x) into a locus install with the same
//! hostname, DIDs, handles, repos, blobs, sessions and firehose position.
//! Offline, into an empty SQLite install (`migrate storage` moves it to
//! Postgres afterwards).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use locus_core::bootstrap::{Bootstrap, DatabaseUrl};
use locus_core::keys::MasterKey;
use locus_core::setup::normalize_host;
use locus_repo::import::{TreeWalk, decode_record, import_blob_cids, split_tree_key};
use locus_repo::records::backlinks;
use locus_store::Store;
use locus_store::blob::Place;
use locus_store::reference::{AccountImport, ImportedBlob, Report, Source};
use locus_store::repo::RepoRoot;
use proto_blue_lex_data::LexValue;
use proto_blue_repo::commit::SignedCommit;

use crate::node;

/// Who the import writes as (settings, audit).
const ACTOR: &str = "import";

/// The ranges the reference trusts for client IPs in every role (loopback,
/// link-local, unique-local and RFC 1918), so client-IP handling doesn't
/// change with the import.
const REFERENCE_PROXIES: &[&str] = &[
    "127.0.0.0/8",
    "::1/128",
    "169.254.0.0/16",
    "fe80::/10",
    "fc00::/7",
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
];

/// Blob key prefixes locus uses itself: an in-place prefix may not be one.
const LOCUS_PREFIXES: &[&str] = &["blobs", "gated", "quarantine", "branding", "tmp"];

pub struct Opts {
    pub dir: PathBuf,
    pub env_file: PathBuf,
}

fn e(x: impl std::fmt::Display) -> String {
    x.to_string()
}

/// `KEY=VALUE` lines (comments, blank lines and `export ` skipped; one
/// level of quotes removed).
pub fn parse_env_file(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim();
        let v = v
            .strip_prefix('"')
            .and_then(|x| x.strip_suffix('"'))
            .or_else(|| v.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')))
            .unwrap_or(v);
        out.insert(k.trim().to_owned(), v.to_owned());
    }
    out
}

/// Where the reference's files are. Paths in `pds.env` are the reference's
/// own (often inside its container): one under `PDS_DATA_DIRECTORY` is
/// taken relative to the data dir given here.
struct Layout {
    dir: PathBuf,
    container_dir: Option<PathBuf>,
}

impl Layout {
    fn path(&self, env: &HashMap<String, String>, var: &str, default: &str) -> PathBuf {
        match env.get(var).map(|v| v.trim()).filter(|v| !v.is_empty()) {
            Some(v) => {
                let p = PathBuf::from(v);
                match self
                    .container_dir
                    .as_ref()
                    .and_then(|c| p.strip_prefix(c).ok())
                {
                    Some(rel) => self.dir.join(rel),
                    None if p.is_relative() => self.dir.join(p),
                    None => p,
                }
            }
            None => self.dir.join(default),
        }
    }
}

/// Where the imported blobs are.
enum Blobs {
    /// In S3, under the reference's keys.
    S3,
    /// On disk: in place under `root` (the parent of the blob dir) with
    /// keys `<prefix>/<did>/<cid>`, or copied into locus's layout when the
    /// blob dir's name is one of locus's prefixes.
    Disk {
        location: PathBuf,
        root: PathBuf,
        prefix: Option<String>,
    },
}

/// Runs the import; the report's lines.
pub async fn run(bootstrap: &Bootstrap, opts: Opts) -> Result<Vec<String>, String> {
    let _lock = crate::node_lock::acquire(bootstrap, crate::node_lock::Holder::Offline)
        .await
        .map_err(|e| e.to_string())?;
    if matches!(bootstrap.database, DatabaseUrl::Postgres(_)) {
        return Err(
            "import into a SQLite install, then move it with `aurora-locus migrate storage --to postgres`"
                .into(),
        );
    }
    let text = std::fs::read_to_string(&opts.env_file)
        .map_err(|x| format!("can't read {}: {x}", opts.env_file.display()))?;
    let env = parse_env_file(&text);
    let hostname = env
        .get("PDS_HOSTNAME")
        .map(|h| normalize_host(h))
        .filter(|h| !h.is_empty())
        .ok_or("the reference's pds.env has no PDS_HOSTNAME")?;
    let layout = Layout {
        dir: opts.dir.clone(),
        container_dir: env
            .get("PDS_DATA_DIRECTORY")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from),
    };
    let account_db = layout.path(&env, "PDS_ACCOUNT_DB_LOCATION", "account.sqlite");
    let sequencer_db = layout.path(&env, "PDS_SEQUENCER_DB_LOCATION", "sequencer.sqlite");
    let actors_dir = layout.path(&env, "PDS_ACTOR_STORE_DIRECTORY", "actors");
    let blobs = if env
        .get("PDS_BLOBSTORE_S3_BUCKET")
        .is_some_and(|b| !b.is_empty())
    {
        Blobs::S3
    } else {
        let location = layout.path(&env, "PDS_BLOBSTORE_DISK_LOCATION", "blocks");
        let root = location
            .parent()
            .map_or_else(|| opts.dir.clone(), Path::to_path_buf);
        let name = location
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let prefix = (!name.is_empty() && !LOCUS_PREFIXES.contains(&name.as_str())).then_some(name);
        Blobs::Disk {
            location,
            root,
            prefix,
        }
    };

    // The reference must be stopped: its databases are checked first.
    let accounts = Source::open(&account_db).await.map_err(e)?;
    let sequencer = Source::open(&sequencer_db).await.map_err(e)?;

    let (store, key) = node::open_store(bootstrap).await.map_err(e)?;
    if store.setup_completed().await.map_err(e)? || store.has_accounts().await.map_err(e)? {
        return Err("the locus install isn't empty; import into a new one".into());
    }
    let mut report = Report::default();
    let now = locus_core::time::now_ms();

    // Keys and secrets from pds.env (the JWT secret, so the reference's
    // tokens stay valid; the DPoP secret; the PLC rotation key as current;
    // the admin password).
    let secrets = locus_http::ServerSecrets::load_or_create_from(&store, &key, &env)
        .await
        .map_err(e)?;

    // Settings through the env mapping, applied as a node would.
    let (values, errors) = locus_store::env::collect_env(&env);
    for x in errors {
        report.skip(format!("env value: {x}"));
    }
    {
        let mut tx = store.begin().await.map_err(e)?;
        for v in &values {
            if tx.cas_env_applied(v.key, None, &v.hash).await.map_err(e)? {
                tx.set_setting(v.key, v.value.clone(), ACTOR)
                    .await
                    .map_err(e)?;
            }
        }
        tx.commit().await.map_err(e)?;
    }
    report.add("settings from pds.env", values.len() as u64);

    // account.sqlite
    let pepper = secrets.email_token_pepper.clone();
    let hash = move |t: &str| locus_http::email::token_hash_with(&pepper, t);
    let mut tx = store.begin().await.map_err(e)?;
    report.merge(
        tx.import_reference_accounts(
            &accounts,
            &AccountImport {
                now_ms: now,
                token_hash: &hash,
            },
        )
        .await
        .map_err(e)?,
    );
    tx.commit().await.map_err(e)?;
    accounts.close().await;

    // Actor stores.
    let mut imported = HashSet::new();
    let mut prefixes = Vec::new();
    for (did, dir) in actor_dirs(&actors_dir)? {
        if store.account_by_did(&did).await.map_err(e)?.is_none() {
            report.skip(format!("{did}: an actor store with no account"));
            continue;
        }
        let r = import_actor(&store, &key, &did, &dir, &blobs, &bootstrap.data_dir, now)
            .await
            .map_err(|x| format!("{did}: {x}"))?;
        report.merge(r);
        imported.insert(did);
    }
    report.add("repos", imported.len() as u64);
    match &blobs {
        Blobs::S3 => prefixes.push("blocks".to_owned()),
        Blobs::Disk {
            prefix: Some(p), ..
        } => prefixes.push(p.clone()),
        Blobs::Disk { prefix: None, .. } => report.skip(
            "the blob directory's name is one of locus's prefixes; blobs were copied".to_owned(),
        ),
    }

    // The event log, within locus's retention, and its position.
    let snap = store.load_snapshot().await.map_err(e)?;
    let retention_ms = snap.int("sync.retention_hours").max(0) * 3_600_000;
    let mut tx = store.begin().await.map_err(e)?;
    let (events, last_seq) = tx
        .import_reference_seq(&sequencer, now - retention_ms)
        .await
        .map_err(e)?;
    let (active, root) = match &blobs {
        Blobs::S3 => ("s3", None),
        Blobs::Disk {
            root,
            prefix: Some(_),
            ..
        } => ("disk", Some(root.display().to_string())),
        Blobs::Disk { prefix: None, .. } => ("disk", None),
    };
    tx.set_imported_blob_backend(active, root.as_deref(), &prefixes)
        .await
        .map_err(e)?;
    tx.commit().await.map_err(e)?;
    sequencer.close().await;
    report.add("events", events);

    // Setup, marked complete with what pds.env gives.
    let mailer = locus_mail::Mailer::new(store.clone(), key.clone());
    mailer.import_env_and_check(&env).await.map_err(e)?;
    let snap = store.load_snapshot().await.map_err(e)?;
    let mut plan = locus_http::setup::seed_plan(&snap, Some(&hostname));
    plan.admin_password = Some(String::from_utf8_lossy(&secrets.admin_password).into_owned());
    plan.trusted_proxies = REFERENCE_PROXIES.iter().map(|s| (*s).to_owned()).collect();
    plan.keys_backup_acknowledged = true;
    plan.email.skipped = true;
    plan.normalize();
    locus_http::setup::apply(&store, &key, &plan, bootstrap.dev_mode_honored())
        .await
        .map_err(|x| format!("setup: {x}"))?;

    let mut tx = store.begin().await.map_err(e)?;
    tx.record_reference_import(now, &report.to_json())
        .await
        .map_err(e)?;
    tx.commit().await.map_err(e)?;

    let mut lines = vec![format!(
        "Imported {hostname} (last seq {last_seq}). Start it with `aurora-locus serve`."
    )];
    for (k, v) in &report.counts {
        lines.push(format!("  {k}: {v}"));
    }
    for s in &report.skipped {
        lines.push(format!("  skipped: {s}"));
    }
    lines.push(format!(
        "PLC rotation key: {} (current). Keep a copy of the master key off this server.",
        secrets.plc_keys.current_did().unwrap_or_default()
    ));
    Ok(lines)
}

/// Every `actors/<2 hex>/<did>/` directory.
fn actor_dirs(root: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut out = Vec::new();
    let Ok(shards) = std::fs::read_dir(root) else {
        return Ok(out);
    };
    for shard in shards.flatten() {
        let Ok(entries) = std::fs::read_dir(shard.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("did:") && entry.path().is_dir() {
                out.push((name, entry.path()));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// One actor: signing key, repo (blocks the current tree reaches, with
/// their reference counts), record index, blobs and preferences.
async fn import_actor(
    store: &Store,
    key: &MasterKey,
    did: &str,
    dir: &Path,
    blobs: &Blobs,
    data_dir: &Path,
    now: i64,
) -> Result<Report, String> {
    let mut report = Report::default();
    let signing = std::fs::read(dir.join("key")).map_err(|x| format!("its key: {x}"))?;
    let src = Source::open(&dir.join("store.sqlite")).await.map_err(e)?;
    let mut tx = store.begin().await.map_err(e)?;
    tx.put_signing_key(did, "k256", &signing, key)
        .await
        .map_err(e)?;

    if let Some((commit_cid, rev)) = src.repo_root().await.map_err(e)? {
        let (commit_bytes, commit_rev) = src
            .block(&commit_cid)
            .await
            .map_err(e)?
            .ok_or_else(|| format!("its commit block {commit_cid} is missing"))?;
        let value: LexValue =
            proto_blue_lex_cbor::decode(&commit_bytes).map_err(|x| format!("its commit: {x}"))?;
        let commit =
            SignedCommit::from_lex_value(&value).map_err(|x| format!("its commit: {x}"))?;
        if commit.did != did {
            return Err(format!("its commit is for {}", commit.did));
        }
        tx.insert_imported_block(did, &commit_cid, &commit_rev, &commit_bytes, 1)
            .await
            .map_err(e)?;
        let mut kept = 1u64;

        // The tree: nodes now, record blocks after (one per CID, counted
        // once per leaf).
        let index = src.records().await.map_err(e)?;
        let mut leaves: Vec<(String, String)> = Vec::new();
        let mut walk = TreeWalk::new(commit.data.clone());
        while let Some(cid) = walk.next_node().map_err(e)? {
            let k = cid.to_string_base32();
            let (bytes, brev) = src
                .block(&k)
                .await
                .map_err(e)?
                .ok_or_else(|| format!("tree node {k} is missing"))?;
            for (path, value) in walk.visit(&bytes).map_err(e)? {
                leaves.push((path, value.to_string_base32()));
            }
            tx.insert_imported_block(did, &k, &brev, &bytes, 1)
                .await
                .map_err(e)?;
            kept += 1;
        }
        let mut counts: HashMap<&str, i64> = HashMap::new();
        for (_, c) in &leaves {
            *counts.entry(c.as_str()).or_default() += 1;
        }
        for (c, n) in &counts {
            let (bytes, brev) = src
                .block(c)
                .await
                .map_err(e)?
                .ok_or_else(|| format!("record block {c} is missing"))?;
            tx.insert_imported_block(did, c, &brev, &bytes, *n)
                .await
                .map_err(e)?;
            kept += 1;
        }
        for (path, c) in &leaves {
            let (collection, rkey) = split_tree_key(path).map_err(e)?;
            let r = index.get(path).filter(|r| &r.cid == c);
            tx.insert_imported_record(
                did,
                &collection,
                &rkey,
                c,
                r.map_or(rev.as_str(), |r| r.rev.as_str()),
                r.map_or(now, |r| r.indexed_at),
                r.and_then(|r| r.takedown_ref.as_deref()),
            )
            .await
            .map_err(e)?;
            let (bytes, _) = src
                .block(c)
                .await
                .map_err(e)?
                .ok_or_else(|| format!("record block {c} is missing"))?;
            let value = decode_record(&collection, &rkey, &bytes).map_err(e)?;
            tx.replace_record_blobs(did, &collection, &rkey, &import_blob_cids(&value))
                .await
                .map_err(e)?;
            tx.replace_backlinks(did, &collection, &rkey, &backlinks(&collection, &value))
                .await
                .map_err(e)?;
        }
        tx.set_repo_root(
            did,
            &RepoRoot {
                commit_cid: commit_cid.clone(),
                rev,
                data_cid: commit.data.to_string_base32(),
            },
        )
        .await
        .map_err(e)?;
        report.add("records", leaves.len() as u64);
        report.add("blocks", kept);
        let total = u64::try_from(src.block_count().await.map_err(e)?).unwrap_or(0);
        report.add("blocks dropped (unreachable)", total.saturating_sub(kept));
    } else {
        report.skip(format!("{did}: no repo"));
    }

    // Blobs.
    for b in src.blobs().await.map_err(e)? {
        if b.temp_key.is_some() {
            report.add("unused uploads dropped", 1);
            continue;
        }
        let quarantined = b.takedown_ref.is_some();
        let storage_key = match blobs {
            Blobs::S3 if quarantined => format!("quarantine/{did}/{}", b.cid),
            Blobs::S3 => format!("blocks/{did}/{}", b.cid),
            Blobs::Disk {
                prefix: Some(p),
                location,
                ..
            } if !quarantined => {
                if !location.join(did).join(&b.cid).exists() {
                    report.skip(format!("{did}: blob {} has no bytes", b.cid));
                }
                format!("{p}/{did}/{}", b.cid)
            }
            Blobs::Disk {
                location,
                root,
                prefix,
            } => {
                let from = if quarantined {
                    location.join("quarantine").join(did).join(&b.cid)
                } else {
                    location.join(did).join(&b.cid)
                };
                let place = if quarantined {
                    Place::Quarantine
                } else {
                    Place::Normal
                };
                let to_key = place.new_key(did, &b.cid);
                // In place, the disk root is the blob dir's parent; when
                // copying, it's locus's data dir.
                let base = if prefix.is_some() {
                    root.clone()
                } else {
                    data_dir.to_path_buf()
                };
                let to = base.join(&to_key);
                if let Some(parent) = to.parent() {
                    std::fs::create_dir_all(parent).map_err(e)?;
                }
                if let Err(x) = std::fs::copy(&from, &to) {
                    report.skip(format!("{did}: blob {}: {x}", b.cid));
                }
                report.add("blobs copied", 1);
                to_key
            }
        };
        let referenced = tx.blob_has_records(did, &b.cid).await.map_err(e)?;
        tx.insert_imported_blob(
            did,
            &ImportedBlob {
                cid: b.cid.clone(),
                mime_type: b.mime_type,
                size: b.size,
                storage_key,
                state: if quarantined {
                    "quarantined"
                } else {
                    "permanent"
                }
                .into(),
                created_at: b.created_at,
                takedown_ref: b.takedown_ref,
                unreferenced_since: (!referenced).then_some(now),
            },
        )
        .await
        .map_err(e)?;
        report.add("blobs", 1);
    }

    // Preferences.
    for (name, json) in src.prefs().await.map_err(e)? {
        let namespace = if locus_store::prefs::in_namespace("app.bsky", &name) {
            "app.bsky".to_owned()
        } else {
            name.split('.').take(2).collect::<Vec<_>>().join(".")
        };
        tx.insert_imported_pref(did, &namespace, &name, &json)
            .await
            .map_err(e)?;
        report.add("preferences", 1);
    }
    tx.commit().await.map_err(e)?;
    src.close().await;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_file_lines() {
        let env = parse_env_file(
            "# comment\nPDS_HOSTNAME=pds.example.com\nexport PDS_JWT_SECRET=\"abc\"\n\nPDS_X='y z'\nbad line\n",
        );
        assert_eq!(env["PDS_HOSTNAME"], "pds.example.com");
        assert_eq!(env["PDS_JWT_SECRET"], "abc");
        assert_eq!(env["PDS_X"], "y z");
        assert_eq!(env.len(), 3);
    }

    #[test]
    fn container_paths_are_rebased() {
        let l = Layout {
            dir: PathBuf::from("/srv/pds"),
            container_dir: Some(PathBuf::from("/pds")),
        };
        let mut env = HashMap::new();
        assert_eq!(
            l.path(&env, "PDS_BLOBSTORE_DISK_LOCATION", "blocks"),
            PathBuf::from("/srv/pds/blocks")
        );
        env.insert(
            "PDS_BLOBSTORE_DISK_LOCATION".to_owned(),
            "/pds/media".to_owned(),
        );
        assert_eq!(
            l.path(&env, "PDS_BLOBSTORE_DISK_LOCATION", "blocks"),
            PathBuf::from("/srv/pds/media")
        );
        env.insert(
            "PDS_BLOBSTORE_DISK_LOCATION".to_owned(),
            "/mnt/elsewhere".to_owned(),
        );
        assert_eq!(
            l.path(&env, "PDS_BLOBSTORE_DISK_LOCATION", "blocks"),
            PathBuf::from("/mnt/elsewhere")
        );
    }
}
