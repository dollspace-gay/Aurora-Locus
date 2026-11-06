//! Importing a reference PDS installation (`import reference-pds`): its
//! SQLite files read, the rows the import writes, and the import mark
//! (imported tokens are accepted until they expire). Also the login-time
//! upgrade of imported password hashes.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use locus_core::time::parse_rfc3339_ms;
use locus_core::{CoreError, CoreResult};
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions, SqliteRow};

use crate::accounts::{AppPassword, RefreshToken, Session};
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

macro_rules! tx_exec {
    ($tx:expr, $q:expr, $($arg:expr),*) => {
        match $tx {
            Tx::Sqlite(t) => sqlx::query(&$q)$(.bind($arg))*.execute(&mut **t).await.map(|r| r.rows_affected()),
            Tx::Postgres(t) => sqlx::query(&$q)$(.bind($arg))*.execute(&mut **t).await.map(|r| r.rows_affected()),
        }
        .map_err(db_err)
    };
}

/// What an import brought over, by kind, and what it left out.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub counts: BTreeMap<String, u64>,
    pub skipped: Vec<String>,
}

impl Report {
    pub fn add(&mut self, kind: &str, n: u64) {
        *self.counts.entry(kind.to_owned()).or_default() += n;
    }

    pub fn skip(&mut self, why: impl Into<String>) {
        self.skipped.push(why.into());
    }

    pub fn merge(&mut self, other: Report) {
        for (k, v) in other.counts {
            self.add(&k, v);
        }
        self.skipped.extend(other.skipped);
    }

    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"counts": self.counts, "skipped": self.skipped})
    }
}

/// An ISO time from the reference as ms (0 if it doesn't parse).
fn ms(s: &str) -> i64 {
    parse_rfc3339_ms(s).unwrap_or(0)
}

fn opt_ms(s: Option<String>) -> Option<i64> {
    s.as_deref().and_then(parse_rfc3339_ms)
}

fn random_id() -> String {
    hex::encode(rand::random::<[u8; 16]>())
}

// ---------------------------------------------------------------- reading

/// One of the reference's SQLite files.
pub struct Source {
    pool: SqlitePool,
    path: PathBuf,
}

impl Source {
    /// Opens a reference SQLite file, refusing one another process has
    /// open (the reference is still running). Taking it out of WAL mode
    /// needs every other connection closed, so it's the test; it also folds
    /// a leftover `-wal` from a crash back in. The reference turns WAL on
    /// again when it opens the file.
    pub async fn open(path: &Path) -> CoreResult<Self> {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .busy_timeout(Duration::ZERO);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .map_err(|e| CoreError::Invalid(format!("can't open {}: {e}", path.display())))?;
        let in_use = || {
            CoreError::Conflict(format!(
                "{} is in use by another process; stop the reference PDS first",
                path.display()
            ))
        };
        let mode: String = match sqlx::query_scalar("PRAGMA journal_mode = DELETE")
            .fetch_one(&pool)
            .await
        {
            Ok(m) => m,
            Err(sqlx::Error::Database(e))
                if e.message().contains("locked") || e.message().contains("busy") =>
            {
                return Err(in_use());
            }
            Err(e) => return Err(db_err(e)),
        };
        if !mode.eq_ignore_ascii_case("delete") {
            return Err(in_use());
        }
        Ok(Self {
            pool,
            path: path.to_path_buf(),
        })
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    async fn has_table(&self, name: &str) -> CoreResult<bool> {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        )
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(n > 0)
    }

    async fn has_column(&self, table: &str, column: &str) -> CoreResult<bool> {
        let rows = sqlx::query(&format!("PRAGMA table_info(\"{table}\")"))
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(rows
            .iter()
            .any(|r| r.try_get::<String, _>("name").is_ok_and(|n| n == column)))
    }

    async fn rows(&self, q: &str) -> CoreResult<Vec<SqliteRow>> {
        sqlx::query(q).fetch_all(&self.pool).await.map_err(db_err)
    }

    // ------------------------------------------------------------ actor stores

    /// The repo's (commit CID, rev).
    pub async fn repo_root(&self) -> CoreResult<Option<(String, String)>> {
        sqlx::query_as("SELECT cid, rev FROM repo_root LIMIT 1")
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)
    }

    /// A block's bytes and the rev that introduced it.
    pub async fn block(&self, cid: &str) -> CoreResult<Option<(Vec<u8>, String)>> {
        sqlx::query_as("SELECT content, repoRev FROM repo_block WHERE cid = ?1")
            .bind(cid)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)
    }

    /// How many blocks the store holds.
    pub async fn block_count(&self) -> CoreResult<i64> {
        sqlx::query_scalar("SELECT COUNT(*) FROM repo_block")
            .fetch_one(&self.pool)
            .await
            .map_err(db_err)
    }

    /// The record index, by `collection/rkey`.
    pub async fn records(&self) -> CoreResult<HashMap<String, RefRecord>> {
        let rows: Vec<(String, String, String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT collection, rkey, cid, repoRev, indexedAt, takedownRef FROM record",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(rows
            .into_iter()
            .map(|(c, r, cid, rev, at, td)| {
                (
                    format!("{c}/{r}"),
                    RefRecord {
                        cid,
                        rev,
                        indexed_at: ms(&at),
                        takedown_ref: td,
                    },
                )
            })
            .collect())
    }

    /// Blob metadata.
    pub async fn blobs(&self) -> CoreResult<Vec<RefBlob>> {
        let rows: Vec<BlobTuple> = sqlx::query_as(
            "SELECT cid, mimeType, size, tempKey, createdAt, takedownRef FROM blob ORDER BY cid",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(rows
            .into_iter()
            .map(
                |(cid, mime_type, size, temp_key, at, takedown_ref)| RefBlob {
                    cid,
                    mime_type,
                    size,
                    temp_key,
                    created_at: ms(&at),
                    takedown_ref,
                },
            )
            .collect())
    }

    /// Preferences, in write order: (name, JSON).
    pub async fn prefs(&self) -> CoreResult<Vec<(String, String)>> {
        if !self.has_table("account_pref").await? {
            return Ok(Vec::new());
        }
        sqlx::query_as("SELECT name, valueJson FROM account_pref ORDER BY id")
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)
    }

    // ------------------------------------------------------------ sequencer

    /// The highest seq the reference assigned (0 if none).
    pub async fn last_seq(&self) -> CoreResult<i64> {
        let v: Option<i64> = sqlx::query_scalar("SELECT MAX(seq) FROM repo_seq")
            .fetch_one(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(v.unwrap_or(0))
    }
}

/// An actor store's blob row: (cid, mimeType, size, tempKey, createdAt,
/// takedownRef).
type BlobTuple = (String, String, i64, Option<String>, String, Option<String>);

/// A record row of an actor store.
#[derive(Debug, Clone)]
pub struct RefRecord {
    pub cid: String,
    pub rev: String,
    pub indexed_at: i64,
    pub takedown_ref: Option<String>,
}

/// A blob row of an actor store.
#[derive(Debug, Clone)]
pub struct RefBlob {
    pub cid: String,
    pub mime_type: String,
    pub size: i64,
    /// Set while the upload is unused (its bytes are in the temp dir).
    pub temp_key: Option<String>,
    pub created_at: i64,
    pub takedown_ref: Option<String>,
}

// ---------------------------------------------------------------- writing

/// How the account database's rows are turned into locus's.
pub struct AccountImport<'a> {
    pub now_ms: i64,
    /// The stored form of an emailed token.
    pub token_hash: &'a (dyn Fn(&str) -> String + Sync),
}

/// The scope of a legacy session made with an app password (or none).
fn session_scope(privileged: Option<bool>) -> &'static str {
    match privileged {
        Some(true) => "com.atproto.appPassPrivileged",
        Some(false) => "com.atproto.appPass",
        None => "com.atproto.access",
    }
}

fn email_purpose(p: &str) -> Option<&'static str> {
    Some(match p {
        "confirm_email" => "confirm_email",
        "update_email" => "update_email",
        "reset_password" => "reset_password",
        "delete_account" => "delete_account",
        "plc_operation" => "plc_operation",
        "auth_factor" => "auth_factor",
        _ => return None,
    })
}

fn did_method(did: &str) -> &'static str {
    if did.starts_with("did:web:") {
        "web"
    } else {
        "plc"
    }
}

fn s(r: &SqliteRow, c: &str) -> String {
    r.try_get::<Option<String>, _>(c)
        .ok()
        .flatten()
        .unwrap_or_default()
}

fn os(r: &SqliteRow, c: &str) -> Option<String> {
    r.try_get::<Option<String>, _>(c).ok().flatten()
}

fn i(r: &SqliteRow, c: &str) -> i64 {
    r.try_get::<Option<i64>, _>(c)
        .ok()
        .flatten()
        .unwrap_or_default()
}

impl Tx {
    /// Imports `account.sqlite`: accounts, app passwords, legacy sessions
    /// (one per refresh-token chain), emailed tokens, invites and the OAuth
    /// tables. Password hashes keep the reference's scrypt form behind the
    /// `scrypt:` marker; imported app passwords have no lookup hash yet.
    pub async fn import_reference_accounts(
        &mut self,
        src: &Source,
        opts: &AccountImport<'_>,
    ) -> CoreResult<Report> {
        let pg = matches!(self, Tx::Postgres(_));
        let mut report = Report::default();
        let factor = if src.has_table("account_email_auth_factor").await? {
            "(SELECT f.emailAuthFactorEnabledAt FROM account_email_auth_factor f WHERE f.did = a.did)"
        } else {
            "NULL"
        };
        let rows = src
            .rows(&format!(
                "SELECT a.did, a.handle, a.createdAt, a.takedownRef, a.deactivatedAt, a.deleteAfter, \
                 c.email, c.passwordScrypt, c.emailConfirmedAt, c.invitesDisabled, {factor} AS factorAt \
                 FROM actor a LEFT JOIN account c ON c.did = a.did ORDER BY a.createdAt, a.did"
            ))
            .await?;
        let ins = sql(
            "INSERT INTO account (did, handle, email, email_confirmed_at, password_hash, created_at, status, \
             status_ref, deactivated_at, delete_after, invites_disabled, did_method, email_auth_factor_at) \
             VALUES ({1}, {2}, {3}, {4}, {5}, {6}, {7}, {8}, {9}, {10}, {11}, {12}, {13})",
            pg,
        );
        let mut dids: Vec<String> = Vec::new();
        for r in &rows {
            let did = s(r, "did");
            let Some(handle) = os(r, "handle").map(|h| h.to_ascii_lowercase()) else {
                report.skip(format!("{did}: no handle"));
                continue;
            };
            let takedown = os(r, "takedownRef");
            let deactivated = opt_ms(os(r, "deactivatedAt"));
            let status = if takedown.is_some() {
                "takendown"
            } else if deactivated.is_some() {
                "deactivated"
            } else {
                "active"
            };
            let password = os(r, "passwordScrypt").map(|p| format!("scrypt:{p}"));
            tx_exec!(
                self,
                ins,
                &did,
                &handle,
                os(r, "email").map(|e| e.to_ascii_lowercase()),
                opt_ms(os(r, "emailConfirmedAt")),
                password,
                ms(&s(r, "createdAt")),
                status,
                takedown,
                deactivated,
                opt_ms(os(r, "deleteAfter")),
                i(r, "invitesDisabled") != 0,
                did_method(&did),
                opt_ms(os(r, "factorAt"))
            )?;
            dids.push(did);
        }
        report.add("accounts", dids.len() as u64);
        let known: std::collections::HashSet<&str> = dids.iter().map(String::as_str).collect();

        // App passwords: found the reference's way until first used.
        let privileged_col = src.has_column("app_password", "privileged").await?;
        let rows = src
            .rows(&format!(
                "SELECT did, name, passwordScrypt, createdAt, {} AS privileged FROM app_password",
                if privileged_col { "privileged" } else { "0" }
            ))
            .await?;
        let mut app_privileged: HashMap<(String, String), bool> = HashMap::new();
        let mut n = 0u64;
        for r in &rows {
            let did = s(r, "did");
            if !known.contains(did.as_str()) {
                continue;
            }
            let p = AppPassword {
                did: did.clone(),
                name: s(r, "name"),
                lookup_hash: String::new(),
                hash: format!("scrypt:{}", s(r, "passwordScrypt")),
                privileged: i(r, "privileged") != 0,
                created_at: ms(&s(r, "createdAt")),
            };
            app_privileged.insert((did, p.name.clone()), p.privileged);
            self.insert_app_password(&p).await?;
            n += 1;
        }
        report.add("app passwords", n);

        // Legacy sessions: a refresh token and the ones it rotated into are
        // one session.
        let rows = src
            .rows("SELECT id, did, expiresAt, nextId, appPasswordName FROM refresh_token")
            .await?;
        let next: HashMap<String, String> = rows
            .iter()
            .filter_map(|r| os(r, "nextId").map(|n| (s(r, "id"), n)))
            .collect();
        let ids: std::collections::HashSet<String> = rows.iter().map(|r| s(r, "id")).collect();
        let end_of = |id: &str| {
            let mut cur = id.to_owned();
            for _ in 0..10_000 {
                match next.get(&cur) {
                    Some(n) if ids.contains(n) => cur.clone_from(n),
                    _ => break,
                }
            }
            cur
        };
        let mut sessions: HashMap<String, (String, Option<String>, i64, Vec<RefreshToken>)> =
            HashMap::new();
        for r in &rows {
            let did = s(r, "did");
            let exp = ms(&s(r, "expiresAt"));
            if !known.contains(did.as_str()) || exp <= opts.now_ms {
                continue;
            }
            let entry = sessions
                .entry(end_of(&s(r, "id")))
                .or_insert_with(|| (did.clone(), os(r, "appPasswordName"), 0, Vec::new()));
            entry.2 = entry.2.max(exp);
            entry.3.push(RefreshToken {
                id: s(r, "id"),
                session_id: String::new(),
                expires_at: exp,
                next_id: os(r, "nextId"),
            });
        }
        let mut tokens = 0u64;
        for (_, (did, app_name, exp, chain)) in sessions.iter_mut() {
            let id = random_id();
            let privileged = app_name.as_ref().map(|n| {
                app_privileged
                    .get(&(did.clone(), n.clone()))
                    .copied()
                    .unwrap_or(false)
            });
            self.insert_session(&Session {
                id: id.clone(),
                did: did.clone(),
                kind: "legacy".into(),
                scope: session_scope(privileged).into(),
                app_password_name: app_name.clone(),
                created_at: opts.now_ms,
                expires_at: *exp,
                revoked_at: None,
            })
            .await?;
            for t in chain.iter_mut() {
                t.session_id.clone_from(&id);
                self.insert_refresh_token(t).await?;
                tokens += 1;
            }
        }
        report.add("sessions", sessions.len() as u64);
        report.add("refresh tokens", tokens);

        // Emailed tokens (they last 15 minutes; most will have expired).
        let ins = sql(
            "INSERT INTO email_token (did, purpose, token_hash, requested_at) VALUES ({1}, {2}, {3}, {4})",
            pg,
        );
        let mut n = 0u64;
        for r in &src
            .rows("SELECT purpose, did, token, requestedAt FROM email_token")
            .await?
        {
            let did = s(r, "did");
            let Some(purpose) = email_purpose(&s(r, "purpose")) else {
                continue;
            };
            if !known.contains(did.as_str()) {
                continue;
            }
            tx_exec!(
                self,
                ins,
                &did,
                purpose,
                (opts.token_hash)(&s(r, "token")),
                ms(&s(r, "requestedAt"))
            )?;
            n += 1;
        }
        report.add("email tokens", n);

        // Invites.
        let ins = sql(
            "INSERT INTO invite_code (code, available_uses, disabled, for_account, created_by, created_at) \
             VALUES ({1}, {2}, {3}, {4}, {5}, {6})",
            pg,
        );
        let mut n = 0u64;
        for r in &src
            .rows("SELECT code, availableUses, disabled, forAccount, createdBy, createdAt FROM invite_code")
            .await?
        {
            tx_exec!(
                self,
                ins,
                s(r, "code"),
                i(r, "availableUses"),
                i(r, "disabled") != 0,
                s(r, "forAccount"),
                s(r, "createdBy"),
                ms(&s(r, "createdAt"))
            )?;
            n += 1;
        }
        report.add("invite codes", n);
        let ins = sql(
            "INSERT INTO invite_code_use (code, used_by, used_at) VALUES ({1}, {2}, {3}) \
             ON CONFLICT DO NOTHING",
            pg,
        );
        for r in &src
            .rows("SELECT code, usedBy, usedAt FROM invite_code_use")
            .await?
        {
            tx_exec!(self, ins, s(r, "code"), s(r, "usedBy"), ms(&s(r, "usedAt")))?;
        }

        report.merge(self.import_reference_oauth(src, &known).await?);
        Ok(report)
    }

    /// The OAuth tables, which locus keeps as the reference does (times as
    /// ms). Pending authorization requests are left out: they last minutes.
    async fn import_reference_oauth(
        &mut self,
        src: &Source,
        known: &std::collections::HashSet<&str>,
    ) -> CoreResult<Report> {
        let pg = matches!(self, Tx::Postgres(_));
        let mut report = Report::default();
        if !src.has_table("token").await? {
            return Ok(report);
        }
        let ins = sql(
            "INSERT INTO oauth_device (id, session_id, user_agent, ip_address, last_seen_at) \
             VALUES ({1}, {2}, {3}, {4}, {5})",
            pg,
        );
        let mut n = 0u64;
        for r in &src
            .rows("SELECT id, sessionId, userAgent, ipAddress, lastSeenAt FROM device")
            .await?
        {
            tx_exec!(
                self,
                ins,
                s(r, "id"),
                s(r, "sessionId"),
                os(r, "userAgent"),
                s(r, "ipAddress"),
                ms(&s(r, "lastSeenAt"))
            )?;
            n += 1;
        }
        report.add("oauth devices", n);
        if src.has_table("account_device").await? {
            let ins = sql(
                "INSERT INTO oauth_account_device (did, device_id, created_at, updated_at) \
                 VALUES ({1}, {2}, {3}, {4})",
                pg,
            );
            for r in &src
                .rows("SELECT did, deviceId, createdAt, updatedAt FROM account_device")
                .await?
            {
                if known.contains(s(r, "did").as_str()) {
                    tx_exec!(
                        self,
                        ins,
                        s(r, "did"),
                        s(r, "deviceId"),
                        ms(&s(r, "createdAt")),
                        ms(&s(r, "updatedAt"))
                    )?;
                }
            }
        }
        if src.has_table("authorized_client").await? {
            let ins = sql(
                "INSERT INTO oauth_authorized_client (did, client_id, created_at, updated_at, data) \
                 VALUES ({1}, {2}, {3}, {4}, {5})",
                pg,
            );
            let mut n = 0u64;
            for r in &src
                .rows("SELECT did, clientId, createdAt, updatedAt, data FROM authorized_client")
                .await?
            {
                if known.contains(s(r, "did").as_str()) {
                    tx_exec!(
                        self,
                        ins,
                        s(r, "did"),
                        s(r, "clientId"),
                        ms(&s(r, "createdAt")),
                        ms(&s(r, "updatedAt")),
                        s(r, "data")
                    )?;
                    n += 1;
                }
            }
            report.add("oauth authorized clients", n);
        }
        // Tokens keep their ids, so spent refresh tokens still point at them.
        let scope = if src.has_column("token", "scope").await? {
            "scope"
        } else {
            "NULL AS scope"
        };
        let ins = sql(
            "INSERT INTO oauth_token (id, did, token_id, created_at, updated_at, expires_at, client_id, \
             client_auth, device_id, parameters, details, code, current_refresh_token, scope) \
             VALUES ({1}, {2}, {3}, {4}, {5}, {6}, {7}, {8}, {9}, {10}, {11}, {12}, {13}, {14})",
            pg,
        );
        let mut kept = std::collections::HashSet::new();
        for r in &src
            .rows(&format!(
                "SELECT id, did, tokenId, createdAt, updatedAt, expiresAt, clientId, clientAuth, deviceId, \
                 parameters, details, code, currentRefreshToken, {scope} FROM token"
            ))
            .await?
        {
            let did = s(r, "did");
            if !known.contains(did.as_str()) {
                continue;
            }
            let id = i(r, "id");
            tx_exec!(
                self,
                ins,
                id,
                &did,
                s(r, "tokenId"),
                ms(&s(r, "createdAt")),
                ms(&s(r, "updatedAt")),
                ms(&s(r, "expiresAt")),
                s(r, "clientId"),
                s(r, "clientAuth"),
                os(r, "deviceId"),
                s(r, "parameters"),
                os(r, "details"),
                os(r, "code"),
                os(r, "currentRefreshToken"),
                os(r, "scope")
            )?;
            kept.insert(id);
        }
        report.add("oauth tokens", kept.len() as u64);
        let ins = sql(
            "INSERT INTO oauth_used_refresh (refresh_token, token_id) VALUES ({1}, {2})",
            pg,
        );
        for r in &src
            .rows("SELECT refreshToken, tokenId FROM used_refresh_token")
            .await?
        {
            if kept.contains(&i(r, "tokenId")) {
                tx_exec!(self, ins, s(r, "refreshToken"), i(r, "tokenId"))?;
            }
        }
        if src.has_table("lexicon").await? {
            let ins = sql(
                "INSERT INTO oauth_lexicon (nsid, created_at, updated_at, last_succeeded_at, uri, lexicon) \
                 VALUES ({1}, {2}, {3}, {4}, {5}, {6})",
                pg,
            );
            for r in &src
                .rows(
                    "SELECT nsid, createdAt, updatedAt, lastSucceededAt, uri, lexicon FROM lexicon",
                )
                .await?
            {
                tx_exec!(
                    self,
                    ins,
                    s(r, "nsid"),
                    ms(&s(r, "createdAt")),
                    ms(&s(r, "updatedAt")),
                    opt_ms(os(r, "lastSucceededAt")),
                    os(r, "uri"),
                    os(r, "lexicon")
                )?;
            }
        }
        Ok(report)
    }

    /// Imports `repo_seq` rows sequenced at or after `since_ms` (invalidated
    /// ones skipped; event types locus doesn't serve too), and continues
    /// `seq_state` from the reference's last seq so relay cursors stay
    /// valid. Returns (events imported, last seq).
    pub async fn import_reference_seq(
        &mut self,
        src: &Source,
        since_ms: i64,
    ) -> CoreResult<(u64, i64)> {
        let pg = matches!(self, Tx::Postgres(_));
        let last = src.last_seq().await?;
        let ins = sql(
            "INSERT INTO seq_event (seq, did, kind, payload, sequenced_at) VALUES ({1}, {2}, {3}, {4}, {5})",
            pg,
        );
        let mut n = 0u64;
        let mut first: Option<i64> = None;
        let mut after = 0i64;
        loop {
            let rows: Vec<(i64, String, String, Vec<u8>, i64, String)> = sqlx::query_as(
                "SELECT seq, did, eventType, event, invalidated, sequencedAt FROM repo_seq \
                 WHERE seq > ?1 ORDER BY seq LIMIT 1000",
            )
            .bind(after)
            .fetch_all(&src.pool)
            .await
            .map_err(db_err)?;
            let Some(l) = rows.last() else { break };
            after = l.0;
            for (seq, did, kind, event, invalidated, at) in rows {
                let at = ms(&at);
                let kind = match kind.as_str() {
                    "append" => "commit",
                    "sync" | "identity" | "account" => kind.as_str(),
                    _ => continue,
                };
                if invalidated != 0 || at < since_ms {
                    continue;
                }
                tx_exec!(self, ins, seq, &did, kind, &event, at)?;
                first.get_or_insert(seq);
                n += 1;
            }
        }
        // Everything below the first kept event counts as pruned, so an
        // older cursor gets OutdatedCursor rather than a gap.
        let pruned = first.map_or(last, |f| f - 1);
        let q = sql(
            "UPDATE seq_state SET last_seq = {1}, pruned_through = {2} WHERE id = 1",
            pg,
        );
        tx_exec!(self, q, last, pruned)?;
        Ok((n, last))
    }

    /// One repo block with its count of references from the current tree
    /// (added to, if the block is already there).
    pub async fn insert_imported_block(
        &mut self,
        did: &str,
        cid: &str,
        rev: &str,
        content: &[u8],
        refs: i64,
    ) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO repo_block (did, cid, rev_introduced, size, content, ref_count) \
             VALUES ({1}, {2}, {3}, {4}, {5}, {6}) \
             ON CONFLICT (did, cid) DO UPDATE SET ref_count = repo_block.ref_count + excluded.ref_count",
            matches!(self, Tx::Postgres(_)),
        );
        tx_exec!(
            self,
            q,
            did,
            cid,
            rev,
            i64::try_from(content.len()).unwrap_or(i64::MAX),
            content,
            refs
        )?;
        Ok(())
    }

    /// One row of the record index.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_imported_record(
        &mut self,
        did: &str,
        collection: &str,
        rkey: &str,
        cid: &str,
        rev: &str,
        indexed_at: i64,
        takedown_ref: Option<&str>,
    ) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO record (did, collection, rkey, cid, rev, indexed_at, takedown_ref) \
             VALUES ({1}, {2}, {3}, {4}, {5}, {6}, {7})",
            matches!(self, Tx::Postgres(_)),
        );
        tx_exec!(
            self,
            q,
            did,
            collection,
            rkey,
            cid,
            rev,
            indexed_at,
            takedown_ref
        )?;
        Ok(())
    }

    /// One blob row. `state` is `permanent` or `quarantined`; an unused
    /// blob is marked unreferenced, so GC treats it as any other.
    pub async fn insert_imported_blob(&mut self, did: &str, b: &ImportedBlob) -> CoreResult<()> {
        let key_seq = self.next_key_seq().await?;
        let q = sql(
            "INSERT INTO blob (did, cid, mime_type, size, storage_key, state, created_at, takedown_ref, \
             unreferenced_since, key_seq) VALUES ({1}, {2}, {3}, {4}, {5}, {6}, {7}, {8}, {9}, {10})",
            matches!(self, Tx::Postgres(_)),
        );
        tx_exec!(
            self,
            q,
            did,
            &b.cid,
            &b.mime_type,
            b.size,
            &b.storage_key,
            &b.state,
            b.created_at,
            &b.takedown_ref,
            b.unreferenced_since,
            key_seq
        )?;
        Ok(())
    }

    /// One preference.
    pub async fn insert_imported_pref(
        &mut self,
        did: &str,
        namespace: &str,
        name: &str,
        json: &str,
    ) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO account_pref (did, namespace, name, value_json) VALUES ({1}, {2}, {3}, {4})",
            matches!(self, Tx::Postgres(_)),
        );
        tx_exec!(self, q, did, namespace, name, json)?;
        Ok(())
    }

    /// Sets up `blob_backend` for the imported blobs: the backend they're
    /// on, epoch 0, not paused, no migration job, and the in-place prefixes.
    pub async fn set_imported_blob_backend(
        &mut self,
        active: &str,
        disk_root: Option<&str>,
        import_prefixes: &[String],
    ) -> CoreResult<()> {
        let q = sql(
            "UPDATE blob_backend SET active = {1}, disk_root = {2}, epoch = 0, paused = {3}, \
             migration_job = NULL, import_prefixes = {4} WHERE id = 1",
            matches!(self, Tx::Postgres(_)),
        );
        let prefixes = serde_json::to_string(import_prefixes).unwrap_or_else(|_| "[]".into());
        tx_exec!(self, q, active, disk_root, false, prefixes)?;
        Ok(())
    }

    /// Records the import (its time and report).
    pub async fn record_reference_import(
        &mut self,
        imported_at: i64,
        report: &serde_json::Value,
    ) -> CoreResult<()> {
        let q = sql(
            "INSERT INTO reference_import (id, imported_at, report) VALUES (1, {1}, {2})",
            matches!(self, Tx::Postgres(_)),
        );
        tx_exec!(self, q, imported_at, report.to_string())?;
        Ok(())
    }
}

/// A blob row the import writes.
#[derive(Debug, Clone)]
pub struct ImportedBlob {
    pub cid: String,
    pub mime_type: String,
    pub size: i64,
    pub storage_key: String,
    pub state: String,
    pub created_at: i64,
    pub takedown_ref: Option<String>,
    pub unreferenced_since: Option<i64>,
}

// ---------------------------------------------------------------- after import

/// The import mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportMark {
    pub imported_at: i64,
    pub report: serde_json::Value,
}

impl Store {
    /// Whether the install has any account (an import needs none).
    pub async fn has_accounts(&self) -> CoreResult<bool> {
        let q = "SELECT 1 FROM account LIMIT 1";
        let r: Option<(i32,)> = run!(self, q, fetch_optional,)?;
        Ok(r.is_some())
    }

    /// When the install was imported from a reference PDS, if it was.
    pub async fn reference_import(&self) -> CoreResult<Option<ImportMark>> {
        let q = sql(
            "SELECT imported_at, report FROM reference_import WHERE id = 1",
            self.is_postgres(),
        );
        let row: Option<(i64, String)> = run!(self, q, fetch_optional,)?;
        Ok(row.map(|(imported_at, report)| ImportMark {
            imported_at,
            report: serde_json::from_str(&report).unwrap_or_default(),
        }))
    }

    /// Replaces an imported password hash after a successful login (only
    /// if it's still the one verified).
    pub async fn upgrade_password_hash(&self, did: &str, old: &str, new: &str) -> CoreResult<bool> {
        let q = sql(
            "UPDATE account SET password_hash = {1} WHERE did = {2} AND password_hash = {3}",
            self.is_postgres(),
        );
        Ok(exec!(self, q, new, did, old)? == 1)
    }

    /// Whether the account has app passwords imported from a reference PDS
    /// and not used since (they have no lookup hash yet).
    pub async fn has_imported_app_passwords(&self, did: &str) -> CoreResult<bool> {
        let q = sql(
            "SELECT 1 FROM app_password WHERE did = {1} AND lookup_hash = '' LIMIT 1",
            self.is_postgres(),
        );
        let r: Option<(i32,)> = run!(self, q, fetch_optional, did)?;
        Ok(r.is_some())
    }

    /// The imported app password with this stored hash (found the
    /// reference's way: by equality).
    pub async fn imported_app_password(
        &self,
        did: &str,
        hash: &str,
    ) -> CoreResult<Option<AppPassword>> {
        let q = sql(
            "SELECT did, name, lookup_hash, hash, privileged, created_at FROM app_password \
             WHERE did = {1} AND lookup_hash = '' AND hash = {2}",
            self.is_postgres(),
        );
        run!(self, q, fetch_optional, did, hash)
    }

    /// Gives an imported app password its lookup hash and an Argon2id hash
    /// after its first successful use.
    pub async fn upgrade_app_password(
        &self,
        did: &str,
        name: &str,
        old_hash: &str,
        lookup_hash: &str,
        new_hash: &str,
    ) -> CoreResult<bool> {
        let q = sql(
            "UPDATE app_password SET lookup_hash = {1}, hash = {2} \
             WHERE did = {3} AND name = {4} AND hash = {5}",
            self.is_postgres(),
        );
        Ok(exec!(self, q, lookup_hash, new_hash, did, name, old_hash)? == 1)
    }
}
