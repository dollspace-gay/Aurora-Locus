//! `aurora-locus migrate storage --to postgres --url <url>` (offline):
//! copies a SQLite install into an empty Postgres database. The SQLite
//! file is left as it was.

use locus_core::bootstrap::{Bootstrap, DatabaseUrl, MasterKeySource};
use locus_store::Store;
use locus_store::env::CLOCK_SLOT_FAMILY;
use locus_store::secrets::KeyCheck;

fn e(x: impl std::fmt::Display) -> String {
    x.to_string()
}

pub async fn run(bootstrap: &Bootstrap, url: &str) -> Result<Vec<String>, String> {
    let _lock = crate::node_lock::acquire(bootstrap, crate::node_lock::Holder::Offline)
        .await
        .map_err(|e| e.to_string())?;
    if !matches!(bootstrap.database, DatabaseUrl::Sqlite(_)) {
        return Err("this install is already on Postgres".into());
    }
    if !url.starts_with("postgres://") && !url.starts_with("postgresql://") {
        return Err("--url is a postgres:// URL".into());
    }
    // A key file one node generated can't serve several: declare it shared
    // (the same file on every node), or move to an env or KMS key first.
    if let MasterKeySource::File { shared: false, .. } = &bootstrap.master_key {
        return Err(
            "the master key is this node's generated file; on Postgres every node needs the same \
             key: set LOCUS_MASTER_KEY_SHARED_FILE=true (and give every node that file), or \
             `aurora-locus keys rewrap --to env` or `--to kms` first"
                .into(),
        );
    }
    let (src, key) = crate::node::open_store(bootstrap).await.map_err(e)?;
    let live = src.live_slot_holders(CLOCK_SLOT_FAMILY).await.map_err(e)?;
    if !live.is_empty() {
        return Err(format!(
            "{} node(s) are running; stop every node first (migrate storage runs offline)",
            live.len()
        ));
    }
    if let Some(m) = src.active_blob_migration().await.map_err(e)? {
        return Err(format!(
            "the blob migration {} (to {}) is still running: let it finish, or \
             `aurora-locus migrate blobs --cancel`",
            m.id, m.target
        ));
    }
    if src.blob_backend().await.map_err(e)?.paused {
        return Err("blob storage is paused for a cutover: let it finish, or \
             `aurora-locus migrate blobs --cancel`"
            .into());
    }
    let dst = Store::connect(&DatabaseUrl::Postgres(url.to_owned()))
        .await
        .map_err(e)?;
    dst.migrate().await.map_err(e)?;
    let report = locus_store::copy::sqlite_to_postgres(&src, &dst)
        .await
        .map_err(e)?;
    if dst.check_master_key(&key).await.map_err(e)? != KeyCheck::Matches {
        return Err("the copied key check doesn't match this install's master key".into());
    }
    let mut lines: Vec<String> = report
        .tables
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(t, n)| format!("  {t}: {n}"))
        .collect();
    lines.insert(0, "Copied:".into());
    lines.push(format!(
        "The key sequence continues after {}.",
        report.key_seq
    ));
    lines.push(format!(
        "Set LOCUS_DATABASE_URL={url} on every node, then start them. The SQLite file is \
         unchanged; keep it until the new install checks out."
    ));
    src.close().await;
    dst.close().await;
    Ok(lines)
}
