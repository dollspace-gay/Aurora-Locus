//! Branding storage: theme versions (one draft, published ones kept), the
//! version in use, and uploads with the versions using them.

use locus_core::{CoreError, CoreResult};

use crate::blob::sql;
use crate::{Store, Tx, db_err};

macro_rules! run {
    ($store:expr, $q:expr, $method:ident, $($arg:expr),*) => {
        match $store {
            Store::Sqlite(p) => sqlx::query_as(&$q)$(.bind($arg))*.$method(p).await,
            Store::Postgres(p) => sqlx::query_as(&$q)$(.bind($arg))*.$method(p).await,
        }
        .map_err(db_err)
    };
}

macro_rules! exec {
    ($store:expr, $q:expr, $($arg:expr),*) => {
        match $store {
            Store::Sqlite(p) => sqlx::query(&$q)$(.bind($arg))*.execute(p).await.map(|r| r.rows_affected()),
            Store::Postgres(p) => sqlx::query(&$q)$(.bind($arg))*.execute(p).await.map(|r| r.rows_affected()),
        }
        .map_err(db_err)
    };
}

macro_rules! tx_run {
    ($tx:expr, $q:expr, $method:ident, $($arg:expr),*) => {
        match $tx {
            Tx::Sqlite(t) => sqlx::query_as(&$q)$(.bind($arg))*.$method(&mut **t).await,
            Tx::Postgres(t) => sqlx::query_as(&$q)$(.bind($arg))*.$method(&mut **t).await,
        }
        .map_err(db_err)
    };
}

macro_rules! tx_exec {
    ($tx:expr, $q:expr, $($arg:expr),*) => {
        match $tx {
            Tx::Sqlite(t) => sqlx::query(&$q)$(.bind($arg))*.execute(&mut **t).await.map(|r| r.rows_affected()),
            Tx::Postgres(t) => sqlx::query(&$q)$(.bind($arg))*.execute(&mut **t).await.map(|r| r.rows_affected()),
        }
        .map_err(db_err)
    };
}

/// A theme version.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct VersionRow {
    pub version: i64,
    pub state: String,
    pub theme_json: String,
    pub created_at: i64,
    pub created_by: String,
    pub published_at: Option<i64>,
    pub published_by: Option<String>,
}

/// An upload.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AssetRow {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub mime: String,
    pub size: i64,
    pub storage_key: String,
    pub created_at: i64,
    pub unused_since: Option<i64>,
}

const VERSION_COLS: &str =
    "version, state, theme_json, created_at, created_by, published_at, published_by";
const ASSET_COLS: &str = "id, kind, name, mime, size, storage_key, created_at, unused_since";

impl Store {
    /// The version in use, if any.
    pub async fn branding_active(&self) -> CoreResult<Option<VersionRow>> {
        let q = format!(
            "SELECT {VERSION_COLS} FROM branding_version WHERE version = (SELECT active FROM branding_state WHERE id = 1)"
        );
        run!(self, q, fetch_optional,)
    }

    pub async fn branding_draft(&self) -> CoreResult<Option<VersionRow>> {
        let q = format!(
            "SELECT {VERSION_COLS} FROM branding_version WHERE state = 'draft' ORDER BY version DESC LIMIT 1"
        );
        run!(self, q, fetch_optional,)
    }

    pub async fn branding_versions(&self, limit: i64) -> CoreResult<Vec<VersionRow>> {
        let q = sql(
            &format!(
                "SELECT {VERSION_COLS} FROM branding_version WHERE state = 'published' ORDER BY version DESC LIMIT {{1}}"
            ),
            self.is_postgres(),
        );
        run!(self, q, fetch_all, limit)
    }

    pub async fn branding_version(&self, version: i64) -> CoreResult<Option<VersionRow>> {
        let q = sql(
            &format!("SELECT {VERSION_COLS} FROM branding_version WHERE version = {{1}}"),
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, version)
    }

    /// Saves the draft (making it if there's none), recording the uploads
    /// it uses. Returns its version.
    pub async fn save_branding_draft(
        &self,
        theme_json: &str,
        assets: &[String],
        by: &str,
    ) -> CoreResult<i64> {
        let mut tx = self.begin().await?;
        let pg = matches!(tx, Tx::Postgres(_));
        let existing: Option<(i64,)> = tx_run!(
            &mut tx,
            "SELECT version FROM branding_version WHERE state = 'draft' ORDER BY version DESC LIMIT 1",
            fetch_optional,
        )?;
        let version = if let Some((v,)) = existing {
            let q = sql(
                "UPDATE branding_version SET theme_json = {2}, created_by = {3} WHERE version = {1}",
                pg,
            );
            tx_exec!(&mut tx, q, v, theme_json, by)?;
            v
        } else {
            let q = sql(
                "INSERT INTO branding_version (state, theme_json, created_at, created_by) VALUES ('draft', {1}, {now}, {2}) RETURNING version",
                pg,
            );
            let (v,): (i64,) = tx_run!(&mut tx, q, fetch_one, theme_json, by)?;
            v
        };
        tx.set_branding_uses(version, assets).await?;
        tx.commit().await?;
        Ok(version)
    }

    /// Publishes the draft as a new version and puts it in use; the draft
    /// starts over from it. None when there's no draft.
    pub async fn publish_branding_draft(&self, by: &str) -> CoreResult<Option<i64>> {
        let mut tx = self.begin().await?;
        let pg = matches!(tx, Tx::Postgres(_));
        let draft: Option<(i64,)> = tx_run!(
            &mut tx,
            "SELECT version FROM branding_version WHERE state = 'draft' ORDER BY version DESC LIMIT 1",
            fetch_optional,
        )?;
        let Some((v,)) = draft else {
            tx.rollback().await?;
            return Ok(None);
        };
        let q = sql(
            "UPDATE branding_version SET state = 'published', published_at = {now}, published_by = {2} WHERE version = {1}",
            pg,
        );
        tx_exec!(&mut tx, q, v, by)?;
        let q = sql("UPDATE branding_state SET active = {1} WHERE id = 1", pg);
        tx_exec!(&mut tx, q, v)?;
        tx.commit().await?;
        Ok(Some(v))
    }

    /// Puts a published version back in use (rollback). False if there's
    /// no such published version.
    pub async fn activate_branding(&self, version: i64) -> CoreResult<bool> {
        let q = sql(
            "UPDATE branding_state SET active = {1} WHERE id = 1 AND EXISTS \
             (SELECT 1 FROM branding_version WHERE version = {1} AND state = 'published')",
            self.is_postgres(),
        );
        Ok(exec!(self, q, version)? == 1)
    }

    /// Stops using branding: the built-in look.
    pub async fn deactivate_branding(&self) -> CoreResult<()> {
        exec!(self, "UPDATE branding_state SET active = NULL WHERE id = 1",)?;
        Ok(())
    }

    /// Records an upload whose bytes are at `a.storage_key`, drawing its
    /// key sequence value after the backend read, as blob rows do.
    ///
    /// `epoch` is the backend epoch the bytes were written under: while a
    /// cutover pauses key changes, or after the backend changed, the row
    /// isn't written and the bytes are queued for deletion (an error asks
    /// the caller to try again).
    pub async fn put_branding_asset(&self, a: &AssetRow, epoch: i64) -> CoreResult<()> {
        let mut tx = self.begin().await?;
        let backend = tx.blob_backend().await?;
        if backend.paused || backend.epoch != epoch {
            tx.queue_blob_delete(&a.storage_key, epoch).await?;
            tx.commit().await?;
            return Err(CoreError::Conflict(
                "blob storage is being moved; try again shortly".into(),
            ));
        }
        let seq = tx.next_key_seq().await?;
        let q = sql(
            &format!(
                "INSERT INTO branding_asset ({ASSET_COLS}, key_seq) \
                 VALUES ({{1}}, {{2}}, {{3}}, {{4}}, {{5}}, {{6}}, {{now}}, {{now}}, {{7}})"
            ),
            self.is_postgres(),
        );
        tx_exec!(
            &mut tx,
            q,
            &a.id,
            &a.kind,
            &a.name,
            &a.mime,
            a.size,
            &a.storage_key,
            seq
        )?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn branding_asset(&self, id: &str) -> CoreResult<Option<AssetRow>> {
        let q = sql(
            &format!("SELECT {ASSET_COLS} FROM branding_asset WHERE id = {{1}}"),
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, id)
    }

    pub async fn branding_assets(&self) -> CoreResult<Vec<AssetRow>> {
        let q = format!("SELECT {ASSET_COLS} FROM branding_asset ORDER BY created_at DESC");
        run!(self, q, fetch_all,)
    }

    /// Removes uploads unused for longer than `grace_ms`, queueing their
    /// bytes for deletion in the same transaction. Returns how many.
    pub async fn collect_branding_assets(&self, grace_ms: i64) -> CoreResult<u64> {
        let mut tx = self.begin().await?;
        let pg = matches!(tx, Tx::Postgres(_));
        // The backend first, before any row is read, as every transaction
        // that names a storage key.
        let epoch = tx.blob_backend().await?.epoch;
        let q = sql(
            "SELECT id, storage_key FROM branding_asset WHERE unused_since IS NOT NULL AND unused_since < {now} - {1}",
            pg,
        );
        let gone: Vec<(String, String)> = tx_run!(&mut tx, q, fetch_all, grace_ms)?;
        for (id, key) in &gone {
            let q = sql("DELETE FROM branding_asset WHERE id = {1}", pg);
            tx_exec!(&mut tx, q, id)?;
            tx.queue_blob_delete(key, epoch).await?;
        }
        tx.commit().await?;
        Ok(gone.len() as u64)
    }
}

impl Tx {
    /// Records which uploads `version` uses, and keeps each upload's
    /// `unused_since` in step: cleared while any version uses it, set when
    /// none does.
    pub async fn set_branding_uses(&mut self, version: i64, assets: &[String]) -> CoreResult<()> {
        let pg = matches!(self, Tx::Postgres(_));
        let q = sql("DELETE FROM branding_asset_use WHERE version = {1}", pg);
        tx_exec!(self, q, version)?;
        for a in assets {
            let q = sql(
                "INSERT INTO branding_asset_use (asset_id, version) VALUES ({1}, {2}) ON CONFLICT DO NOTHING",
                pg,
            );
            tx_exec!(self, q, a, version)?;
        }
        let q = sql(
            "UPDATE branding_asset SET unused_since = NULL WHERE unused_since IS NOT NULL \
             AND id IN (SELECT asset_id FROM branding_asset_use)",
            pg,
        );
        tx_exec!(self, q,)?;
        let q = sql(
            "UPDATE branding_asset SET unused_since = {now} WHERE unused_since IS NULL \
             AND id NOT IN (SELECT asset_id FROM branding_asset_use)",
            pg,
        );
        tx_exec!(self, q,)?;
        Ok(())
    }
}
