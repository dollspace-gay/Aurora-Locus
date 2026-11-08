//! First-run setup's state: the one-time token, wizard sessions, the
//! wizard's draft, and completion.

use locus_core::CoreResult;

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

macro_rules! scalar {
    ($store:expr, $q:expr, $method:ident, $($arg:expr),*) => {
        match $store {
            Store::Sqlite(p) => sqlx::query_scalar(&$q)$(.bind($arg))*.$method(p).await,
            Store::Postgres(p) => sqlx::query_scalar(&$q)$(.bind($arg))*.$method(p).await,
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

macro_rules! tx_exec {
    ($tx:expr, $q:expr, $($arg:expr),*) => {
        match $tx {
            Tx::Sqlite(t) => sqlx::query(&$q)$(.bind($arg))*.execute(&mut **t).await.map(|r| r.rows_affected()),
            Tx::Postgres(t) => sqlx::query(&$q)$(.bind($arg))*.execute(&mut **t).await.map(|r| r.rows_affected()),
        }
        .map_err(db_err)
    };
}

/// The setup row.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SetupRow {
    pub token_hash: String,
    pub created_at: i64,
    pub completed_at: Option<i64>,
    pub completed_by: Option<String>,
    pub hostname: Option<String>,
    pub draft_json: String,
    pub probe_nonce: Option<String>,
    pub probe_seen: Option<String>,
}

impl Store {
    pub async fn setup_state(&self) -> CoreResult<Option<SetupRow>> {
        let q = "SELECT token_hash, created_at, completed_at, completed_by, hostname, draft_json, \
                 probe_nonce, probe_seen FROM setup_state WHERE id = 1";
        run!(self, q, fetch_optional,)
    }

    /// Whether setup has completed.
    pub async fn setup_completed(&self) -> CoreResult<bool> {
        Ok(self
            .setup_state()
            .await?
            .is_some_and(|r| r.completed_at.is_some()))
    }

    /// Makes the setup row with this token's hash, unless a node made it
    /// first. True when this call made it (so this node shows the token).
    pub async fn create_setup_token(&self, token_hash: &str) -> CoreResult<bool> {
        let q = sql(
            "INSERT INTO setup_state (id, token_hash, created_at) VALUES (1, {1}, {now}) \
             ON CONFLICT (id) DO NOTHING",
            self.is_postgres(),
        );
        Ok(exec!(self, q, token_hash)? == 1)
    }

    /// Replaces the token (`aurora-locus setup --new-token`), while setup
    /// hasn't completed; sessions made with the old one end.
    pub async fn replace_setup_token(&self, token_hash: &str) -> CoreResult<bool> {
        let q = sql(
            "UPDATE setup_state SET token_hash = {1} WHERE id = 1 AND completed_at IS NULL",
            self.is_postgres(),
        );
        let replaced = exec!(self, q, token_hash)? == 1;
        if replaced {
            exec!(self, "DELETE FROM setup_session",)?;
        }
        Ok(replaced)
    }

    /// Whether `token_hash` is the token of a setup that hasn't completed.
    pub async fn setup_token_matches(&self, token_hash: &str) -> CoreResult<bool> {
        let q = sql(
            "SELECT COUNT(*) FROM setup_state WHERE id = 1 AND completed_at IS NULL AND token_hash = {1}",
            self.is_postgres(),
        );
        let n: i64 = scalar!(self, q, fetch_one, token_hash)?;
        Ok(n == 1)
    }

    pub async fn add_setup_session(&self, id_hash: &str, ttl_ms: i64) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO setup_session (id_hash, expires_at) VALUES ({1}, {now} + {2})",
            self.is_postgres(),
        );
        exec!(self, q, id_hash, ttl_ms)?;
        Ok(())
    }

    /// Whether a session is live: unexpired, and setup not yet completed.
    pub async fn setup_session_valid(&self, id_hash: &str) -> CoreResult<bool> {
        let q = sql(
            "SELECT COUNT(*) FROM setup_session s, setup_state st WHERE s.id_hash = {1} \
             AND s.expires_at > {now} AND st.id = 1 AND st.completed_at IS NULL",
            self.is_postgres(),
        );
        let n: i64 = scalar!(self, q, fetch_one, id_hash)?;
        Ok(n == 1)
    }

    /// Saves the wizard's answers, while setup hasn't completed.
    pub async fn save_setup_draft(&self, draft_json: &str) -> CoreResult<bool> {
        let q = sql(
            "UPDATE setup_state SET draft_json = {1} WHERE id = 1 AND completed_at IS NULL",
            self.is_postgres(),
        );
        Ok(exec!(self, q, draft_json)? == 1)
    }

    /// Starts a reachability probe: the nonce the public URL must answer
    /// with.
    pub async fn set_setup_probe(&self, nonce: &str) -> CoreResult<()> {
        let q = sql(
            "UPDATE setup_state SET probe_nonce = {1}, probe_seen = NULL WHERE id = 1",
            self.is_postgres(),
        );
        exec!(self, q, nonce)?;
        Ok(())
    }

    /// Records how a probe request arrived (its forwarded scheme).
    pub async fn record_setup_probe(&self, seen: &str) -> CoreResult<()> {
        let q = sql(
            "UPDATE setup_state SET probe_seen = {1} WHERE id = 1",
            self.is_postgres(),
        );
        exec!(self, q, seen)?;
        Ok(())
    }
}

impl Tx {
    /// Marks setup completed with its hostname, if no one has: false means
    /// another node (or run) completed it first, and this transaction must
    /// roll back.
    pub async fn complete_setup(&mut self, hostname: &str, by: &str) -> CoreResult<bool> {
        let q = sql(
            "UPDATE setup_state SET completed_at = {now}, completed_by = {2}, hostname = {1}, \
             draft_json = '{}' WHERE id = 1 AND completed_at IS NULL",
            matches!(self, Tx::Postgres(_)),
        );
        let done = tx_exec!(self, q, hostname, by)? == 1;
        if done {
            tx_exec!(self, "DELETE FROM setup_session",)?;
        }
        Ok(done)
    }

    /// Adds an operator inside a transaction; false if the name is taken.
    pub async fn create_operator(
        &mut self,
        username: &str,
        password_hash: &str,
        role: &str,
    ) -> CoreResult<bool> {
        let q = sql(
            "INSERT INTO operator (username, password_hash, role, created_at) VALUES ({1}, {2}, {3}, {now}) \
             ON CONFLICT (username) DO NOTHING",
            matches!(self, Tx::Postgres(_)),
        );
        Ok(tx_exec!(self, q, username, password_hash, role)? == 1)
    }
}
