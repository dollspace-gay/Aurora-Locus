//! `backup restore` (a `backup create` backup into an empty SQLite
//! install) and `backup post-restore` (after a Postgres point-in-time
//! recovery). Both run offline. Every refusal comes before anything is
//! written; then both finish the same way: a seq floor above what relays
//! have seen, the accounts whose live DID documents don't list their
//! restored signing key listed, every account re-announced (`#identity`,
//! `#account`), and with `--emit-sync` a fresh commit and `#sync` each.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use locus_blob::{BlobStore, DiskStore};
use locus_core::bootstrap::{Bootstrap, DatabaseUrl, MasterKeySource};
use locus_core::keys::MasterKey;
use locus_core::lease::DEFAULT_LEASE_TTL;
use locus_core::settings::Snapshot;
use locus_repo::{RepoEngine, TidClock};
use locus_store::Store;
use locus_store::env::CLOCK_SLOT_FAMILY;
use locus_store::runtime::Settings;
use proto_blue_crypto::{K256Keypair, Keypair as _};
use serde_json::Value;

/// Which blob backend a restore writes to, or a post-restore makes active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum BackendKind {
    Disk,
    S3,
}

impl BackendKind {
    fn name(self) -> &'static str {
        match self {
            Self::Disk => "disk",
            Self::S3 => "s3",
        }
    }
}

/// The steps both commands end with.
#[derive(Debug, Clone, Default)]
pub struct Finish {
    pub seq_floor: Option<i64>,
    pub emit_sync: bool,
}

/// What a run reports, line by line.
#[derive(Debug, Default)]
pub struct Report(pub Vec<String>);

impl Report {
    fn say(&mut self, line: impl Into<String>) {
        self.0.push(line.into());
    }
}

fn e(x: impl std::fmt::Display) -> String {
    x.to_string()
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap_or_default()
}

/// The seq floor: the database's last seq, a given floor, and each
/// crawler's report of this host plus the margin; the highest wins.
struct Floor {
    value: i64,
    source: String,
    answered: bool,
}

async fn resolve_floor(
    store: &Store,
    snap: &Snapshot,
    hostname: &str,
    given: Option<i64>,
    report: &mut Report,
) -> Result<Floor, String> {
    let last = store.seq_state().await.map_err(e)?.last_seq;
    let margin = snap.int("sync.restore_seq_margin");
    let mut floor = Floor {
        value: last,
        source: "the restored database's last seq".into(),
        answered: false,
    };
    if let Some(g) = given.filter(|g| *g > floor.value) {
        floor.value = g;
        floor.source = "--seq-floor".into();
    }
    for crawler in snap.list("sync.crawlers") {
        let url = format!(
            "{}/xrpc/com.atproto.sync.getHostStatus?hostname={hostname}",
            crawler.trim_end_matches('/')
        );
        let seq = match http().get(&url).send().await {
            Ok(r) if r.status().is_success() => {
                r.json::<Value>().await.ok().and_then(|v| v["seq"].as_i64())
            }
            _ => None,
        };
        match seq {
            Some(seq) => {
                floor.answered = true;
                let candidate = seq.saturating_add(margin);
                report.say(format!("{crawler} reports seq {seq}"));
                if candidate > floor.value {
                    floor.value = candidate;
                    floor.source = format!("{crawler}'s seq plus the {margin} margin");
                }
            }
            None => report.say(format!("{crawler} didn't answer getHostStatus")),
        }
    }
    Ok(floor)
}

/// The atproto signing key an account's live DID document lists.
async fn live_signing_key(snap: &Snapshot, did: &str) -> Result<Option<String>, String> {
    if did.starts_with("did:plc:") {
        let url = format!(
            "{}/{did}/data",
            snap.text("identity.plc_url").trim_end_matches('/')
        );
        let r = http().get(url).send().await.map_err(e)?;
        if !r.status().is_success() {
            return Err(format!("the PLC directory answered {}", r.status()));
        }
        let v: Value = r.json().await.map_err(e)?;
        Ok(v["verificationMethods"]["atproto"]
            .as_str()
            .map(str::to_owned))
    } else {
        let host = did.trim_start_matches("did:web:").replace("%3A", ":");
        let r = http()
            .get(format!("https://{host}/.well-known/did.json"))
            .send()
            .await
            .map_err(e)?;
        if !r.status().is_success() {
            return Err(format!("{host} answered {}", r.status()));
        }
        let v: Value = r.json().await.map_err(e)?;
        Ok(v["verificationMethod"]
            .as_array()
            .and_then(|m| {
                m.iter()
                    .find(|m| m["id"].as_str().is_some_and(|id| id.ends_with("#atproto")))
            })
            .and_then(|m| m["publicKeyMultibase"].as_str())
            .map(|mb| format!("did:key:{mb}")))
    }
}

async fn signing_did(store: &Store, key: &MasterKey, did: &str) -> Option<String> {
    let (_, sk) = store.get_signing_key(did, key).await.ok()??;
    K256Keypair::from_private_key(&sk).ok().map(|k| k.did())
}

/// The finishing steps, against a restored database.
async fn finish(
    store: &Store,
    key: &MasterKey,
    snap: &Snapshot,
    floor: &Floor,
    opts: &Finish,
    report: &mut Report,
) -> Result<(), String> {
    // The seq floor: the next event's seq is above it.
    let mut tx = store.begin().await.map_err(e)?;
    let last = tx.raise_seq_floor(floor.value).await.map_err(e)?;
    tx.commit().await.map_err(e)?;
    report.say(format!(
        "The next event's seq is {} (from {}).",
        last + 1,
        floor.source
    ));
    if !floor.answered && opts.seq_floor.is_none() {
        report.say(
            "No relay reported a seq: relays that saw later seqs may answer FutureCursor; \
             run `aurora-locus crawl request` once a node is up.",
        );
    }

    // Live documents, re-announcement and fresh commits, account by account.
    let mut mismatched: HashSet<String> = HashSet::new();
    let mut accounts = Vec::new();
    let mut after = String::new();
    loop {
        let page = store.accounts_after(&after, 500).await.map_err(e)?;
        let Some(last) = page.last().map(|a| a.did.clone()) else {
            break;
        };
        accounts.extend(page);
        after = last;
    }
    for a in &accounts {
        if a.status == "deleted" {
            continue;
        }
        let hosted_web = a.did_method == "web" && store.hosts_did_web(&a.did).await.map_err(e)?;
        if hosted_web {
            // Served from the restored data; the #identity below tells
            // relays to re-resolve it.
            continue;
        }
        let restored = signing_did(store, key, &a.did).await;
        let live = live_signing_key(snap, &a.did).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        match live {
            Ok(Some(k)) if Some(&k) == restored.as_ref() => {}
            Ok(other) => {
                mismatched.insert(a.did.clone());
                report.say(format!(
                    "{}: its DID document lists {} for signing, not the restored key; \
                     `aurora-locus keys import` a later key backup, then `identity publish` or `repair identity`",
                    a.did,
                    other.unwrap_or_else(|| "no key".into())
                ));
            }
            Err(err) => {
                mismatched.insert(a.did.clone());
                report.say(format!(
                    "{}: its DID document couldn't be fetched ({err})",
                    a.did
                ));
            }
        }
    }
    let mut announced = 0;
    for a in &accounts {
        let mut tx = store.begin().await.map_err(e)?;
        let identity = locus_repo::event::identity_body(&a.did, Some(&a.handle)).map_err(e)?;
        tx.insert_outbox(&a.did, "identity", &identity)
            .await
            .map_err(e)?;
        let active = a.status == "active";
        let account =
            locus_repo::event::account_body(&a.did, active, (!active).then_some(a.status.as_str()))
                .map_err(e)?;
        tx.insert_outbox(&a.did, "account", &account)
            .await
            .map_err(e)?;
        tx.commit().await.map_err(e)?;
        announced += 1;
    }
    report.say(format!(
        "Re-announced {announced} accounts (#identity, #account)."
    ));

    if opts.emit_sync {
        let node_id = format!("restore-{}", locus_core::bootstrap::random_node_id());
        let (slot, lease) = store
            .claim_slot(
                CLOCK_SLOT_FAMILY,
                crate::node::CLOCK_SLOTS,
                &node_id,
                DEFAULT_LEASE_TTL,
            )
            .await
            .map_err(e)?
            .ok_or("no clock id is free")?;
        let clock = u16::try_from(slot).map_err(e)?;
        let engine = RepoEngine::new(store.clone(), std::sync::Arc::new(TidClock::new(clock)));
        let mut synced = 0;
        for a in accounts
            .iter()
            .filter(|a| a.status == "active" && !mismatched.contains(&a.did))
        {
            let Some((_, sk)) = store.get_signing_key(&a.did, key).await.map_err(e)? else {
                continue;
            };
            let signer = K256Keypair::from_private_key(&sk).map_err(e)?;
            match engine.resign_with_sync(&a.did, &signer).await {
                Ok(_) => synced += 1,
                Err(err) => report.say(format!("{}: no fresh commit ({err})", a.did)),
            }
        }
        let _ = store.release_lease(&lease).await;
        report.say(format!(
            "Wrote a fresh commit and #sync for {synced} active accounts."
        ));
    }
    Ok(())
}

/// The master key a restore checks the snapshot with: the configured one.
async fn configured_key(bootstrap: &Bootstrap, store: &Store) -> Result<MasterKey, String> {
    let key = match &bootstrap.master_key {
        MasterKeySource::Kms { key_id } => crate::node::kms_master_key(store, key_id, false)
            .await
            .map_err(e)?,
        _ => locus_core::keys::load_master_key(bootstrap, false).map_err(e)?,
    };
    store
        .check_master_key(&key)
        .await
        .map_err(|err| format!("the configured master key doesn't open this backup: {err}"))?;
    Ok(key)
}

/// The blob store a backend name and settings describe.
async fn backend_store(
    kind: BackendKind,
    disk_root: Option<&Path>,
    data_dir: &Path,
    store: &Store,
    key: &MasterKey,
    snap: &Snapshot,
) -> Result<BlobStore, String> {
    match kind {
        BackendKind::Disk => Ok(BlobStore::Disk(DiskStore::new(
            disk_root.map_or_else(|| data_dir.to_path_buf(), Path::to_path_buf),
        ))),
        BackendKind::S3 => {
            let env: HashMap<String, String> = std::env::vars().collect();
            let config = locus_blob::s3_config(store, key, snap, &env)
                .await
                .map_err(e)?;
            let config = locus_blob::s3_env_overrides(config, &env)
                .ok_or("--backend s3 needs S3 settings: none were restored, and none are in env")?;
            Ok(BlobStore::S3(locus_blob::S3Store::new(&config).map_err(e)?))
        }
    }
}

fn hostname_of(bootstrap: &Bootstrap, setup: Option<String>) -> String {
    setup
        .or_else(|| bootstrap.hostname.clone())
        .unwrap_or_else(|| "localhost".into())
}

/// The objects a backup holds: their keys, by walking `objects/`.
fn backup_objects(root: &Path) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|err| format!("{}: {err}", dir.display()))? {
            let entry = entry.map_err(e)?;
            let path = entry.path();
            if entry.file_type().map_err(e)?.is_dir() {
                stack.push(path);
            } else if path.extension().is_none_or(|x| x != "part") {
                let rel = path.strip_prefix(root).map_err(e)?;
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// `backup restore` options.
#[derive(Debug, Clone)]
pub struct RestoreOpts {
    pub dir: PathBuf,
    pub backend: BackendKind,
    pub disk_root: Option<PathBuf>,
    pub accept_failed: bool,
    pub finish: Finish,
}

/// `backup restore`: into an empty SQLite install only.
pub async fn restore(bootstrap: &Bootstrap, opts: RestoreOpts) -> Result<Report, String> {
    let DatabaseUrl::Sqlite(target) = &bootstrap.database else {
        return Err(
            "restore into a SQLite install (then `aurora-locus migrate storage --to postgres`)"
                .into(),
        );
    };
    if target.exists() {
        return Err(format!(
            "a database exists at {}: restore runs only into an empty install, so move it aside first",
            target.display()
        ));
    }
    let manifest: locus_http::backups::Manifest = serde_json::from_slice(
        &std::fs::read(opts.dir.join("manifest.json"))
            .map_err(|err| format!("this isn't a whole backup (no manifest.json): {err}"))?,
    )
    .map_err(|err| format!("manifest.json: {err}"))?;
    if manifest.format != locus_http::backups::FORMAT || manifest.version != 1 {
        return Err("this backup's format isn't one this version reads".into());
    }
    let mut report = Report::default();
    if manifest.status != "done" {
        if !opts.accept_failed {
            return Err(format!(
                "this backup failed ({}); --accept-failed restores it anyway, with objects possibly missing",
                manifest.error.as_deref().unwrap_or("unknown")
            ));
        }
        report.say("Restoring a failed backup: objects it names may be missing.");
    }
    let tmp = target.with_extension("restoring");
    for p in [
        tmp.clone(),
        tmp.with_extension("restoring-wal"),
        tmp.with_extension("restoring-shm"),
    ] {
        let _ = std::fs::remove_file(p);
    }
    if let Some(parent) = tmp.parent() {
        std::fs::create_dir_all(parent).map_err(e)?;
    }
    std::fs::copy(opts.dir.join("db.sqlite"), &tmp).map_err(|err| format!("db.sqlite: {err}"))?;
    let result = restore_into(bootstrap, &opts, &manifest, &tmp, &mut report).await;
    match result {
        Ok(()) => {
            std::fs::rename(&tmp, target).map_err(e)?;
            // Any write-ahead log goes with it.
            for suffix in ["-wal", "-shm"] {
                let mut from = tmp.clone().into_os_string();
                from.push(suffix);
                let mut to = target.clone().into_os_string();
                to.push(suffix);
                if Path::new(&from).exists() {
                    std::fs::rename(&from, &to).map_err(e)?;
                }
            }
            Ok(report)
        }
        Err(err) => {
            let _ = std::fs::remove_file(&tmp);
            Err(err)
        }
    }
}

async fn restore_into(
    bootstrap: &Bootstrap,
    opts: &RestoreOpts,
    manifest: &locus_http::backups::Manifest,
    tmp: &Path,
    report: &mut Report,
) -> Result<(), String> {
    let store = Store::connect(&DatabaseUrl::Sqlite(tmp.to_path_buf()))
        .await
        .map_err(e)?;
    let key = configured_key(bootstrap, &store).await?;
    store.migrate().await.map_err(e)?;
    let snap = Settings::load(store.clone()).await.map_err(e)?.current();
    let host = hostname_of(
        bootstrap,
        store
            .setup_state()
            .await
            .map_err(e)?
            .and_then(|r| r.hostname),
    );
    let floor = resolve_floor(&store, &snap, &host, opts.finish.seq_floor, report).await?;
    if opts.finish.emit_sync && opts.finish.seq_floor.is_none() && !floor.answered {
        return Err(
            "--emit-sync needs a seq floor: give --seq-floor, or a crawler that answers getHostStatus"
                .into(),
        );
    }
    let blobs = backend_store(
        opts.backend,
        opts.disk_root.as_deref(),
        &bootstrap.data_dir,
        &store,
        &key,
        &snap,
    )
    .await?;
    blobs.check().await.map_err(e)?;

    // The objects, to the named backend.
    let objects_dir = opts.dir.join("objects");
    let held = backup_objects(&objects_dir)?;
    let source = BlobStore::Disk(DiskStore::new(&objects_dir));
    for k in &held {
        let r = source
            .read(k, None)
            .await
            .map_err(e)?
            .ok_or_else(|| format!("{k} vanished from the backup"))?;
        blobs.write_stream(k, r.body, u64::MAX).await.map_err(e)?;
    }
    report.say(format!(
        "Wrote {} objects to the {} store.",
        held.len(),
        opts.backend.name()
    ));
    let held: HashSet<String> = held.into_iter().collect();
    let mut after = String::new();
    let mut missing = 0;
    loop {
        let keys = store.tracked_keys_after(&after, 1000).await.map_err(e)?;
        let Some(last) = keys.last().cloned() else {
            break;
        };
        for k in keys.iter().filter(|k| !held.contains(*k)) {
            missing += 1;
            report.say(format!("Missing from the backup: {k}"));
        }
        after = last;
    }
    if missing > 0 {
        report.say(format!(
            "{missing} objects the database names aren't in the backup; reads of them miss."
        ));
    }
    let _ = manifest;

    // The blob rows, pointed at that backend; moves restart.
    let mut tx = store.begin().await.map_err(e)?;
    let root = opts.disk_root.as_ref().map(|p| p.display().to_string());
    let epoch = tx
        .settle_restored_blobs(
            opts.backend.name(),
            if opts.backend == BackendKind::Disk {
                root.as_deref()
            } else {
                None
            },
            false,
        )
        .await
        .map_err(e)?;
    let moves = tx.restart_pending_moves(epoch).await.map_err(e)?;
    tx.commit().await.map_err(e)?;
    if moves > 0 {
        report.say(format!("Restarted {moves} blob moves."));
    }

    finish(&store, &key, &snap, &floor, &opts.finish, report).await?;
    report.say(
        "Keys changed after this backup come back with `aurora-locus keys import` of a later key backup.",
    );
    store.close().await;
    Ok(())
}

/// `backup post-restore` options.
#[derive(Debug, Clone, Default)]
pub struct PostRestoreOpts {
    pub backend: Option<BackendKind>,
    pub disk_root: Option<PathBuf>,
    pub accept_missing_majority: bool,
    pub finish: Finish,
}

/// `backup post-restore`: after a Postgres point-in-time recovery, before
/// any node starts.
pub async fn post_restore(bootstrap: &Bootstrap, opts: PostRestoreOpts) -> Result<Report, String> {
    // 1. Checks, writing nothing. (open_store checks the master key.)
    let (store, key) = crate::node::open_store(bootstrap).await.map_err(e)?;
    if store.key_store_empty().await.map_err(e)? {
        return Err("this install is empty: that's `backup restore`".into());
    }
    // The recovered leases are the restore point's, so they can look live
    // though no node runs; post-restore runs before any node starts, as
    // the operator ensures, and releases them all.
    let mut report = Report::default();
    let snap = Settings::load(store.clone()).await.map_err(e)?.current();
    let host = hostname_of(
        bootstrap,
        store
            .setup_state()
            .await
            .map_err(e)?
            .and_then(|r| r.hostname),
    );
    let floor = resolve_floor(&store, &snap, &host, opts.finish.seq_floor, &mut report).await?;
    if opts.finish.emit_sync && opts.finish.seq_floor.is_none() && !floor.answered {
        return Err(
            "--emit-sync needs a seq floor: give --seq-floor, or a crawler that answers getHostStatus"
                .into(),
        );
    }
    let restored = store.blob_backend().await.map_err(e)?;
    let restored_kind = if restored.active == "s3" {
        BackendKind::S3
    } else {
        BackendKind::Disk
    };
    let restored_root = restored.disk_root.as_ref().map(PathBuf::from);
    let source = backend_store(
        restored_kind,
        restored_root.as_deref(),
        &bootstrap.data_dir,
        &store,
        &key,
        &snap,
    )
    .await;
    let (target_kind, target) = match opts.backend {
        Some(kind) => (
            kind,
            backend_store(
                kind,
                opts.disk_root.as_deref(),
                &bootstrap.data_dir,
                &store,
                &key,
                &snap,
            )
            .await?,
        ),
        None => (restored_kind, source.clone()?),
    };
    target
        .check()
        .await
        .map_err(|err| format!("the blob store doesn't answer: {err}"))?;

    // 2. The blob check.
    let mut missing = Vec::new();
    let mut checked = 0u64;
    let mut after = String::new();
    loop {
        let keys = store.tracked_keys_after(&after, 1000).await.map_err(e)?;
        let Some(last) = keys.last().cloned() else {
            break;
        };
        for k in &keys {
            checked += 1;
            if target
                .size(k)
                .await
                .map_err(|err| format!("checking {k}: {err}"))?
                .is_none()
            {
                missing.push(k.clone());
            }
        }
        after = last;
    }
    if opts.backend.is_none()
        && !opts.accept_missing_majority
        && checked > 0
        && missing.len() as u64 * 2 > checked
    {
        return Err(format!(
            "{} of {checked} objects are missing from the restored backend; if another backend \
             holds them, rerun with --backend naming it (or --accept-missing-majority)",
            missing.len()
        ));
    }
    let mut unrecoverable = 0;
    for k in &missing {
        if target.restore_newest_version(k).await.unwrap_or(false) {
            report.say(format!("Restored an earlier version of {k}."));
            continue;
        }
        let copied = match (&opts.backend, &source) {
            (Some(_), Ok(src)) => match src.read(k, None).await {
                Ok(Some(r)) => target.write_stream(k, r.body, u64::MAX).await.is_ok(),
                _ => false,
            },
            _ => false,
        };
        if copied {
            report.say(format!("Copied {k} from the restored backend."));
        } else {
            unrecoverable += 1;
            report.say(format!("Unrecoverable: {k}"));
        }
    }
    report.say(format!(
        "Checked {checked} objects: {} were missing, {unrecoverable} couldn't be recovered.",
        missing.len()
    ));

    // 3. The blob backend: any migration ended, --backend recorded.
    let mut tx = store.begin().await.map_err(e)?;
    let root = match (opts.backend, &opts.disk_root, &restored.disk_root) {
        (Some(BackendKind::Disk), Some(p), _) => Some(p.display().to_string()),
        (Some(_), _, _) => None,
        (None, _, r) => r.clone(),
    };
    let ended_job = restored.paused;
    let epoch = tx
        .settle_restored_blobs(target_kind.name(), root.as_deref(), opts.backend.is_some())
        .await
        .map_err(e)?;
    let moves = tx.restart_pending_moves(epoch).await.map_err(e)?;
    tx.commit().await.map_err(e)?;
    if ended_job {
        report.say(
            "A blob migration that was running was ended; run `aurora-locus migrate blobs` again.",
        );
    }
    if moves > 0 {
        report.say(format!("Restarted {moves} blob moves."));
    }

    // 4. The finishing steps.
    finish(&store, &key, &snap, &floor, &opts.finish, &mut report).await?;
    store.close().await;
    Ok(report)
}
