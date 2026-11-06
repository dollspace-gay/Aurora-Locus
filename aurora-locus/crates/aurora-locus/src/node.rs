//! Starting and running a node.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use locus_auth::session::LegacySessions;
use locus_core::bootstrap::{self, Bootstrap};
use locus_core::lease::{DEFAULT_LEASE_TTL, LeaseToken};
use locus_core::{CoreError, CoreResult};
use locus_http::{AppState, ResolverCache, ServerSecrets};
use locus_repo::{RepoEngine, TidClock};
use locus_store::Store;
use locus_store::env::{CLOCK_SLOT_FAMILY, collect_env};
use locus_store::runtime::Settings;
use tokio_util::sync::CancellationToken;

use crate::logging::LogControl;

/// TID clock ids are 10 bits, so at most 1024 nodes can mint TIDs at once.
pub const CLOCK_SLOTS: u32 = 1024;

/// How often each node re-checks env values (a node's lease lapsing makes no event).
const ENV_RECHECK: Duration = Duration::from_secs(60);

/// A running node's shared state.
pub struct Node {
    pub bootstrap: Bootstrap,
    pub node_id: String,
    pub store: Store,
    pub settings: Arc<Settings>,
    /// Encrypts every stored secret. The same key on every node.
    pub master_key: Arc<locus_core::keys::MasterKey>,
    /// This node's TID clock id. Holding its slot is what makes the node live.
    pub clock_id: u32,
    clock_lease: LeaseToken,
    /// Shutdown (signals).
    outer: CancellationToken,
    /// This node's own tasks: a child of `outer`, also fired to restart.
    cancel: CancellationToken,
    /// Fires when setup completes while this node is in setup mode.
    setup_done: CancellationToken,
    /// Cancelled when the install's keys changed: the service restarts in
    /// place to reload them.
    reload: CancellationToken,
    /// In setup mode until setup completes.
    pub setup_mode: bool,
    /// This node's recent log lines, for the console.
    logs: Arc<locus_core::logs::LogRing>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

/// How a node's run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// Shut down.
    Stopped,
    /// Setup completed: start again, now in service.
    Restart,
}

/// Opens the database and checks the master key before anything is
/// written (migrations included: a node with another install's key must
/// not touch it), then migrates.
pub async fn open_store(
    bootstrap: &Bootstrap,
) -> CoreResult<(Store, Arc<locus_core::keys::MasterKey>)> {
    let store = Store::connect(&bootstrap.database).await?;
    let empty = store.key_store_empty().await?;
    let generated_path = match &bootstrap.master_key {
        locus_core::bootstrap::MasterKeySource::File { path, .. } if !path.exists() => {
            Some(path.clone())
        }
        _ => None,
    };
    let master_key = Arc::new(match &bootstrap.master_key {
        locus_core::bootstrap::MasterKeySource::Kms { key_id } => {
            kms_master_key(&store, key_id, empty).await?
        }
        _ => locus_core::keys::load_master_key(bootstrap, empty)?,
    });
    if let Some(path) = generated_path {
        tracing::warn!(
            path = %path.display(),
            "generated a new master key; back it up: losing it loses every stored secret"
        );
    }
    store.check_master_key(&master_key).await?;
    tracing::info!(key_id = %master_key.key_id(), source = master_key.source().as_str(), "master key checked");
    store.migrate().await?;
    Ok((store, master_key))
}

/// The master key from KMS: the install's data key, kept wrapped by KMS in
/// the database so every node unwraps the same one. A new install's first
/// node makes it (a node racing it uses whichever was recorded first).
pub async fn kms_master_key(
    store: &Store,
    key_id: &str,
    key_store_empty: bool,
) -> CoreResult<locus_core::keys::MasterKey> {
    use locus_core::keys::{MasterKey, SourceKind};
    let env: HashMap<String, String> = std::env::vars().collect();
    let kms = locus_identity::kms::Kms::from_env(&env)
        .map_err(|e| CoreError::Config(format!("the KMS master key: {e}")))?;
    let unwrap = |e: locus_identity::kms::KmsError| {
        CoreError::Config(format!("couldn't unwrap the master key with KMS: {e}"))
    };
    if let Some((recorded, wrapped)) = store.master_key_wrap().await? {
        if recorded != key_id {
            tracing::warn!(
                configured = key_id,
                recorded,
                "the master key is wrapped by another KMS key than LOCUS_MASTER_KEY_KMS_ID names; using the recorded one"
            );
        }
        let key = kms.decrypt(&recorded, &wrapped).await.map_err(unwrap)?;
        return Ok(MasterKey::from_bytes(*key, SourceKind::Kms));
    }
    if !key_store_empty {
        return Err(CoreError::Config(
            "the database's secrets are under a master key that isn't from KMS; \
             switch with `aurora-locus keys rewrap --to kms` first"
                .into(),
        ));
    }
    // A new install: the table has to exist before the key is recorded.
    store.migrate().await?;
    let (plain, wrapped) = kms.generate_data_key(key_id).await.map_err(unwrap)?;
    let mut tx = store.begin().await?;
    let (recorded, kept) = tx.put_master_key_wrap(key_id, &wrapped).await?;
    tx.commit().await?;
    let key = if kept == wrapped {
        plain
    } else {
        kms.decrypt(&recorded, &kept).await.map_err(unwrap)?
    };
    Ok(MasterKey::from_bytes(*key, SourceKind::Kms))
}

/// Where this node writes the setup token: `<data>/setup-token`, when the
/// data directory persists.
fn token_path(bootstrap: &Bootstrap) -> Option<std::path::PathBuf> {
    bootstrap
        .data_persistent
        .then(|| bootstrap.data_dir.join("setup-token"))
}

/// Writes a file only its owner can read.
pub fn write_private(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)?.write_all(contents.as_bytes())
}

/// Makes the setup token if no node has, printing it and writing it to the
/// data directory; otherwise says where it is.
async fn announce_setup_token(store: &Store, bootstrap: &Bootstrap) -> CoreResult<()> {
    let path = token_path(bootstrap);
    let token = locus_http::setup::new_token();
    if store
        .create_setup_token(&locus_http::setup::token_hash(&token))
        .await?
    {
        tracing::warn!(
            setup_token = %token,
            "setup mode: open /setup on this server and enter this one-time setup token"
        );
        if let Some(p) = &path {
            match write_private(p, &format!("{token}\n")) {
                Ok(()) => {
                    tracing::warn!(path = %p.display(), "the setup token is also in this file")
                }
                Err(e) => {
                    tracing::warn!(path = %p.display(), "couldn't write the setup token file: {e}")
                }
            }
        }
    } else {
        match path.filter(|p| p.exists()) {
            Some(p) => {
                tracing::warn!(path = %p.display(), "setup mode: the setup token is in this file")
            }
            None => tracing::warn!(
                "setup mode: the setup token is in the log of the first node that started; \
                 `aurora-locus setup --new-token` makes a new one"
            ),
        }
    }
    Ok(())
}

impl Node {
    /// Opens the database, claims a clock id, applies env values and loads
    /// live settings. Background tasks stop when `cancel` fires.
    pub async fn start(
        bootstrap: Bootstrap,
        log: LogControl,
        outer: CancellationToken,
    ) -> CoreResult<Self> {
        let cancel = outer.child_token();
        let node_id = match (&bootstrap.node_id, bootstrap.node_id_path()) {
            (Some(id), _) => id.clone(),
            (None, Some(path)) => bootstrap::load_or_create_node_id(&path)?,
            // No persistent data dir: a fresh id each boot. Env handling only
            // counts live nodes, so it doesn't need ids stable across restarts.
            (None, None) => bootstrap::random_node_id(),
        };
        tracing::info!(node_id, role = ?bootstrap.role, "starting");
        if bootstrap.dev_mode && !bootstrap.dev_mode_honored() {
            tracing::warn!("PDS_DEV_MODE is ignored: the hostname isn't local");
        }
        if bootstrap.ssrf_protection_disabled() {
            tracing::warn!("SSRF protection is OFF for user-influenced requests on this node");
        }

        let (store, master_key) = open_store(&bootstrap).await?;

        let (clock_id, clock_lease) = store
            .claim_slot(CLOCK_SLOT_FAMILY, CLOCK_SLOTS, &node_id, DEFAULT_LEASE_TTL)
            .await?
            .ok_or_else(|| CoreError::Config(format!("all {CLOCK_SLOTS} clock ids are in use")))?;
        tracing::info!(clock_id, "clock id claimed");

        let env: HashMap<String, String> = std::env::vars().collect();
        let (env_values, env_errors) = collect_env(&env);
        for e in &env_errors {
            tracing::warn!("env value ignored: {e}");
        }
        for key in store.report_and_apply_env(&node_id, &env_values).await? {
            tracing::info!(key, "applied env value");
        }

        let settings = Settings::load(store.clone()).await?;
        log.apply(settings.current().text("log.level"));

        // Setup mode until setup completes; after it, the hostname setup
        // chose is the one served.
        let setup = store.setup_state().await?;
        let setup_mode = setup.as_ref().is_none_or(|r| r.completed_at.is_none());
        let bootstrap_hostname = bootstrap
            .hostname
            .as_deref()
            .map(locus_core::setup::normalize_host);
        let hostname = match setup.as_ref().and_then(|r| r.hostname.clone()) {
            Some(h) if !setup_mode => {
                if bootstrap_hostname.as_deref().is_some_and(|b| b != h) {
                    tracing::warn!(
                        configured = bootstrap_hostname.as_deref().unwrap_or(""),
                        serving = %h,
                        "PDS_HOSTNAME differs from the hostname setup chose; it's ignored (the hostname changes only by migration)"
                    );
                }
                h
            }
            _ => bootstrap_hostname
                .clone()
                .unwrap_or_else(|| "localhost".to_owned()),
        };
        if setup_mode {
            announce_setup_token(&store, &bootstrap).await?;
        } else if let Some(p) = token_path(&bootstrap).filter(|p| p.exists()) {
            // The token stopped working when setup completed.
            if let Err(e) = std::fs::remove_file(&p) {
                tracing::warn!(path = %p.display(), "couldn't remove the old setup token file: {e}");
            }
        }

        let mut node = Self {
            bootstrap,
            node_id,
            store,
            settings,
            master_key,
            clock_id,
            clock_lease,
            outer,
            cancel,
            setup_done: CancellationToken::new(),
            reload: CancellationToken::new(),
            logs: log.ring(),
            setup_mode,
            tasks: Vec::new(),
        };
        node.spawn_clock_renewal();
        node.tasks
            .push(node.settings.spawn_watcher(node.cancel.clone()));
        node.spawn_log_level(log);
        node.spawn_env_recheck(env_values);
        if !setup_mode {
            node.spawn_block_gc();
        }
        node.spawn_http(hostname, bootstrap_hostname).await?;
        Ok(node)
    }

    /// What setup mode tells the wizard about this node.
    fn setup_info(&self) -> locus_http::setup::SetupInfo {
        locus_http::setup::SetupInfo {
            database: if self.store.is_postgres() {
                "Postgres".into()
            } else {
                "SQLite".into()
            },
            blob_store: match self.settings.current().text("blobs.s3_bucket").trim() {
                "" => format!(
                    "On disk, in the data directory ({})",
                    self.bootstrap.data_dir.display()
                ),
                bucket => format!("In the S3 bucket {bucket}"),
            },
            data_dir: self.bootstrap.data_dir.clone(),
            token_path: token_path(&self.bootstrap)
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            master_key_path: match &self.bootstrap.master_key {
                locus_core::bootstrap::MasterKeySource::File { path, .. } => {
                    path.display().to_string()
                }
                _ => String::new(),
            },
        }
    }

    /// Loads the server secrets and starts serving HTTP. The port is bound
    /// here so a busy port fails the boot rather than a background task.
    /// In setup mode only setup is served and no background job runs.
    async fn spawn_http(
        &mut self,
        hostname: String,
        bootstrap_hostname: Option<String>,
    ) -> CoreResult<()> {
        let public_url = if bootstrap::is_local_hostname(&hostname) {
            format!("http://{hostname}:{}", self.bootstrap.listen.port())
        } else {
            format!("https://{hostname}")
        };
        let service_did = format!("did:web:{hostname}");
        // Read before the keys, so a change made while they load is seen.
        let key_generation = self.store.key_state().await?.generation;
        let secrets = ServerSecrets::load_or_create(&self.store, &self.master_key).await?;
        let hosts = locus_http::hostnames::Hosts::load(&self.store, &hostname).await?;
        let sessions =
            LegacySessions::new(self.store.clone(), secrets.jwt.clone(), service_did.clone())
                .accepting(
                    hosts
                        .old_full
                        .iter()
                        .map(|h| format!("did:web:{h}"))
                        .collect(),
                )
                .accepting_imported(self.store.reference_import().await?.map(|m| m.imported_at));
        let clock_id = u16::try_from(self.clock_id)
            .map_err(|_| CoreError::Config(format!("clock id {} out of range", self.clock_id)))?;
        let mailer = locus_mail::Mailer::new(self.store.clone(), Arc::clone(&self.master_key));
        let env: HashMap<String, String> = std::env::vars().collect();
        mailer.import_env_and_check(&env).await?;
        let buffer =
            usize::try_from(self.settings.current().int("sync.subscriber_buffer")).unwrap_or(500);
        let firehose = locus_sync::Firehose::new(self.store.clone(), buffer);
        self.tasks.push(tokio::spawn(
            Arc::clone(&firehose).run_tail(self.cancel.clone(), Duration::from_millis(250)),
        ));
        let (kr, audit_rx) = locus_http::kryphocron::Kr::new(secrets.correlation_key);
        self.tasks
            .push(tokio::spawn(locus_http::kryphocron::run_audit_writer(
                self.store.clone(),
                audit_rx,
                self.cancel.clone(),
            )));
        let blobs = locus_blob::open_blob_store(
            &self.store,
            &self.master_key,
            &self.settings.current(),
            &env,
            &self.bootstrap.data_dir,
        )
        .await?;
        tracing::info!(backend = blobs.name(), "blob store opened");
        let gate_settings = self.settings.clone();
        let state = Arc::new(AppState {
            repo: RepoEngine::new(self.store.clone(), Arc::new(TidClock::new(clock_id)))
                .with_media_gate(move || {
                    locus_http::media_gate::gate_on_in(&gate_settings.current())
                }),
            store: self.store.clone(),
            settings: self.settings.clone(),
            master_key: self.master_key.clone(),
            secrets,
            sessions,
            mailer,
            blobs,
            scratch: locus_blob::DiskStore::new(self.bootstrap.data_dir.clone()),
            firehose,
            hostname,
            public_url,
            service_did,
            node_id: self.node_id.clone(),
            ssrf_protection: !self.bootstrap.ssrf_protection_disabled(),
            resolvers: ResolverCache::default(),
            limits: locus_http::ratelimit::Limiter::default(),
            oauth_clients: locus_http::oauth::ClientCache::default(),
            kr,
            setup: Arc::new(if self.setup_mode {
                locus_http::setup::Gate::setup_mode(
                    bootstrap_hostname,
                    self.bootstrap.dev_mode_honored(),
                    self.setup_info(),
                )
            } else {
                locus_http::setup::Gate::finished()
            }),
            logs: Arc::clone(&self.logs),
            branding: locus_http::branding::Cache::default(),
            socket_secret: locus_http::socket::new_secret(),
            reload: self.reload.clone(),
            hosts: arc_swap::ArcSwap::from_pointee(hosts),
            role: self.bootstrap.role,
            dev_mode: self.bootstrap.dev_mode_honored(),
        });
        self.setup_done = state.setup.completed.clone();
        self.spawn_key_watch(key_generation);
        if self.setup_mode {
            self.tasks.push(tokio::spawn(locus_http::setup::watch(
                Arc::clone(&state),
                self.cancel.clone(),
            )));
        } else {
            // Encodes need the current generation: read it before serving.
            locus_http::kryphocron::refresh_oracle(&state).await?;
            self.spawn_oracle_refresh(Arc::clone(&state));
            self.spawn_kryphocron_jobs(Arc::clone(&state));
            self.spawn_audience_jobs(Arc::clone(&state));
            self.spawn_identity_repair(Arc::clone(&state));
            self.spawn_plc_key_scan(Arc::clone(&state));
            self.spawn_backup_schedule(Arc::clone(&state));
            self.spawn_hostname_migration(Arc::clone(&state));
            self.spawn_blob_migration(Arc::clone(&state));
            self.spawn_account_jobs(Arc::clone(&state));
            self.spawn_blob_jobs(Arc::clone(&state));
            self.spawn_sync_jobs(Arc::clone(&state));
        }
        let addr = self.bootstrap.listen;
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| CoreError::Config(format!("can't listen on {addr}: {e}")))?;
        tracing::info!(%addr, public_url = %state.public_url, setup_mode = self.setup_mode, "listening");
        // Direct TLS: HTTPS with ACME certificates, and port 80 for the
        // challenges and redirects.
        if let Some(tls) = self.bootstrap.tls.clone() {
            let acme = locus_http::acme::Acme::new(Arc::clone(&state), tls.clone());
            self.tasks.push(tokio::spawn(
                Arc::clone(&acme).maintain(self.cancel.clone()),
            ));
            let http = tokio::net::TcpListener::bind(tls.http_listen)
                .await
                .map_err(|e| {
                    CoreError::Config(format!("can't listen on {} for ACME: {e}", tls.http_listen))
                })?;
            let (a, c) = (Arc::clone(&acme), self.cancel.clone());
            self.tasks.push(tokio::spawn(async move {
                if let Err(e) = locus_http::acme::serve_http(a, http, c).await {
                    tracing::error!("the HTTP-01 listener failed: {e}");
                }
            }));
            let (st, c, outer) = (Arc::clone(&state), self.cancel.clone(), self.outer.clone());
            self.tasks.push(tokio::spawn(async move {
                if let Err(e) = locus_http::acme::serve(st, acme, tls.listen, c).await {
                    tracing::error!("the TLS listener failed; stopping the node: {e}");
                    outer.cancel();
                }
            }));
            tracing::info!(https = %tls.listen, http = %tls.http_listen, directory = %tls.directory, "direct TLS on");
        }
        // Cloudflare's IP ranges, daily, while in Cloudflare mode.
        {
            let st = Arc::clone(&state);
            self.tasks
                .push(tokio::spawn(locus_http::cloudflare::refresh_loop(
                    move || st.snapshot().text("deploy.proxy_mode") == "cloudflare",
                    self.cancel.clone(),
                )));
        }
        // The admin socket, for the CLI on this machine.
        #[cfg(unix)]
        if let Some(path) = self.bootstrap.admin_socket.clone() {
            match locus_http::socket::bind(&path) {
                Ok(l) => {
                    tracing::info!(path = %path.display(), "admin socket listening");
                    let (st, cancel) = (Arc::clone(&state), self.cancel.clone());
                    self.tasks.push(tokio::spawn(async move {
                        if let Err(e) = locus_http::socket::serve(st, l, path, cancel).await {
                            tracing::warn!("admin socket stopped: {e}");
                        }
                    }));
                }
                Err(e) => tracing::warn!(
                    path = %path.display(),
                    "admin socket unavailable ({e}); the CLI can use HTTP with an API token instead"
                ),
            }
        }
        // Headless setup runs once the listener is up, so its checks can
        // reach this node through the public URL.
        if self.setup_mode
            && self.bootstrap.setup_auto.as_deref().is_some_and(|v| {
                matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes")
            })
        {
            let st = Arc::clone(&state);
            self.tasks.push(tokio::spawn(async move {
                let env: HashMap<String, String> = std::env::vars().collect();
                match locus_http::setup::headless(&st, &env).await {
                    Ok(()) => tracing::info!("headless setup completed"),
                    Err(e) => {
                        tracing::error!("headless setup didn't complete; still in setup mode: {e}")
                    }
                }
            }));
        }
        let (outer, cancel) = (self.outer.clone(), self.cancel.clone());
        self.tasks.push(tokio::spawn(async move {
            if let Err(e) = locus_http::serve(state, listener, cancel.clone()).await {
                tracing::error!("http server failed; stopping the node: {e}");
                outer.cancel();
            }
        }));
        Ok(())
    }

    /// The sequencer (outbox → event log, polled every 20 ms and woken by
    /// itself while there's a backlog), log retention and deleted
    /// accounts' history purges, and crawler requests; each on one node at
    /// a time.
    fn spawn_sync_jobs(&mut self, state: Arc<AppState>) {
        const BATCH: i64 = 500;
        let st = Arc::clone(&state);
        self.tasks
            .push(tokio::spawn(locus_store::runtime::run_leased(
                self.store.clone(),
                "sequencer".into(),
                self.node_id.clone(),
                DEFAULT_LEASE_TTL,
                self.cancel.clone(),
                move |lease, cancel| {
                    let state = Arc::clone(&st);
                    async move {
                        loop {
                            let days = state
                                .snapshot()
                                .int("sync.deleted_history_preservation_days");
                            let n = locus_sync::sequence_once(
                                &state.store,
                                &lease,
                                BATCH,
                                days * 86_400_000,
                            )
                            .await?;
                            if n > 0 {
                                state.firehose.wake();
                                if n as i64 == BATCH {
                                    continue;
                                }
                            }
                            tokio::select! {
                                () = cancel.cancelled() => return Ok(()),
                                () = tokio::time::sleep(Duration::from_millis(20)) => {}
                            }
                        }
                    }
                },
            )));

        // Expired rate-limit counters (database backend).
        let st = Arc::clone(&state);
        self.tasks
            .push(tokio::spawn(locus_store::runtime::run_leased(
                self.store.clone(),
                "rate-limit-prune".into(),
                self.node_id.clone(),
                DEFAULT_LEASE_TTL,
                self.cancel.clone(),
                move |_lease, cancel| {
                    let state = Arc::clone(&st);
                    async move {
                        loop {
                            let mut total = 0;
                            let result = loop {
                                match state.store.prune_rate_limits(5_000).await {
                                    Ok(0) => break Ok(total),
                                    Ok(n) => total += n,
                                    Err(e) => break Err(e),
                                }
                            };
                            // OAuth replay windows and expired requests too.
                            let result = match result {
                                Ok(t) => loop {
                                    match state.store.prune_oauth(5_000).await {
                                        Ok(0) => break Ok(t),
                                        Ok(n) => total += n,
                                        Err(e) => break Err(e),
                                    }
                                },
                                e => e,
                            };
                            // Console and portal sign-ins past their end.
                            let result = match result {
                                Ok(t) => state.store.prune_operator_sessions().await.map(|n| t + n),
                                e => e,
                            };
                            let result = match result {
                                Ok(t) => state.store.prune_portal_sessions().await.map(|n| t + n),
                                e => e,
                            };
                            // Branding uploads no version has used for a week.
                            let result = match result {
                                Ok(t) => locus_http::branding::collect(&state).await.map(|n| t + n),
                                e => e,
                            };
                            if let Err(e) = &result {
                                tracing::warn!("rate limit pruning failed: {e}");
                            }
                            let err = result.as_ref().err().map(ToString::to_string);
                            let _ = state
                                .store
                                .record_job_run("rate-limit-prune", err.as_deref(), None)
                                .await;
                            tokio::select! {
                                () = cancel.cancelled() => return Ok(()),
                                () = tokio::time::sleep(Duration::from_secs(60)) => {}
                            }
                        }
                    }
                },
            )));

        let st = Arc::clone(&state);
        self.tasks
            .push(tokio::spawn(locus_store::runtime::run_leased(
                self.store.clone(),
                "seq-retention".into(),
                self.node_id.clone(),
                DEFAULT_LEASE_TTL,
                self.cancel.clone(),
                move |_lease, cancel| {
                    let state = Arc::clone(&st);
                    async move {
                        loop {
                            let hours = state.snapshot().int("sync.retention_hours");
                            let result = async {
                                let pruned =
                                    state.store.prune_seq_events(hours * 3_600_000).await?;
                                let purged = state.store.run_seq_purges().await?;
                                Ok::<_, CoreError>(pruned + purged)
                            }
                            .await;
                            match &result {
                                Ok(n) if *n > 0 => tracing::info!(events = n, "event log pruned"),
                                Ok(_) => {}
                                Err(e) => tracing::warn!("event log pruning failed: {e}"),
                            }
                            let err = result.as_ref().err().map(ToString::to_string);
                            let _ = state
                                .store
                                .record_job_run("seq-retention", err.as_deref(), None)
                                .await;
                            tokio::select! {
                                () = cancel.cancelled() => return Ok(()),
                                () = tokio::time::sleep(Duration::from_secs(300)) => {}
                            }
                        }
                    }
                },
            )));

        let st = Arc::clone(&state);
        self.tasks
            .push(tokio::spawn(locus_store::runtime::run_leased(
                self.store.clone(),
                "crawlers".into(),
                self.node_id.clone(),
                DEFAULT_LEASE_TTL,
                self.cancel.clone(),
                move |_lease, cancel| {
                    let state = Arc::clone(&st);
                    async move {
                        let http = reqwest::Client::builder()
                            .timeout(Duration::from_secs(10))
                            .build()
                            .map_err(|e| CoreError::Config(e.to_string()))?;
                        let mut schedule =
                            locus_sync::CrawlSchedule::new(Duration::from_secs(20 * 60));
                        let mut seen = state.store.seq_state().await?.last_seq;
                        loop {
                            let crawlers: Vec<String> = state
                                .snapshot()
                                .list("sync.crawlers")
                                .into_iter()
                                .map(str::to_owned)
                                .collect();
                            let now = std::time::Instant::now();
                            if let Ok(st) = state.store.seq_state().await
                                && st.last_seq > seen
                            {
                                seen = st.last_seq;
                                schedule.on_events(&crawlers, now);
                            }
                            for c in schedule.due(now) {
                                match locus_sync::request_crawl(&http, &c, &state.hostname).await {
                                    Ok(()) => tracing::info!(crawler = c, "crawl requested"),
                                    Err(e) => {
                                        tracing::warn!(crawler = c, "crawl request failed: {e}")
                                    }
                                }
                            }
                            tokio::select! {
                                () = cancel.cancelled() => return Ok(()),
                                () = tokio::time::sleep(Duration::from_secs(10)) => {}
                            }
                        }
                    }
                },
            )));
    }

    /// Blob upkeep, each on one node at a time: finishing moves a crash or
    /// failure left, moving blobs into or out of the gated store and
    /// quarantine (the gate switched, or a move that never started),
    /// deleting queued keys after their grace period, and GC of
    /// never-referenced temp blobs and long-unreferenced ones.
    fn spawn_blob_jobs(&mut self, state: Arc<AppState>) {
        const EVERY: Duration = Duration::from_secs(60);
        for name in [
            "blob-moves",
            "blob-placement",
            "blob-deletes",
            "blob-gc",
            "blob-rescan",
        ] {
            let state = Arc::clone(&state);
            self.tasks
                .push(tokio::spawn(locus_store::runtime::run_leased(
                    self.store.clone(),
                    name.into(),
                    self.node_id.clone(),
                    DEFAULT_LEASE_TTL,
                    self.cancel.clone(),
                    move |lease, cancel| {
                        let state = Arc::clone(&state);
                        async move {
                            loop {
                                let snap = state.snapshot();
                                let result = match name {
                                    "blob-moves" => locus_blob::run_pending_moves(
                                        &state.store,
                                        &state.blobs,
                                        500,
                                    )
                                    .await
                                    .map(|n| n as u64),
                                    "blob-placement" => locus_blob::run_placement(
                                        &state.store,
                                        &state.blobs,
                                        locus_http::media_gate::gate_on_in(&snap),
                                        500,
                                    )
                                    .await
                                    .map(|n| n as u64),
                                    "blob-deletes" => locus_blob::run_deletes(
                                        &state.store,
                                        &state.blobs,
                                        snap.int("blobs.delete_grace_ms"),
                                        1000,
                                    )
                                    .await
                                    .map(|n| n as u64),
                                    // Uploads stored unscanned while the
                                    // scanner was down.
                                    "blob-rescan" => locus_http::scanner::rescan_pass(&state).await,
                                    _ => {
                                        locus_blob::run_gc(
                                            &state.store,
                                            &lease,
                                            snap.int("blobs.temp_ttl_ms"),
                                            snap.int("blobs.unreferenced_grace_ms"),
                                            500,
                                        )
                                        .await
                                    }
                                };
                                match &result {
                                    Ok(n) if *n > 0 => {
                                        tracing::info!(job = name, blobs = n, "done")
                                    }
                                    Ok(_) => {}
                                    Err(e) => tracing::warn!(job = name, "failed: {e}"),
                                }
                                let err = result.as_ref().err().map(ToString::to_string);
                                let _ =
                                    state.store.record_job_run(name, err.as_deref(), None).await;
                                tokio::select! {
                                    () = cancel.cancelled() => return Ok(()),
                                    () = tokio::time::sleep(EVERY) => {}
                                }
                            }
                        }
                    },
                )));
        }
    }

    /// The bulk-job resumer (finishes deletions a stopped node started) and
    /// the `deleteAfter` job, each on one node at a time.
    fn spawn_account_jobs(&mut self, state: Arc<AppState>) {
        const EVERY: Duration = Duration::from_secs(30);
        for name in ["bulk-resumer", "delete-after"] {
            let state = Arc::clone(&state);
            self.tasks
                .push(tokio::spawn(locus_store::runtime::run_leased(
                    self.store.clone(),
                    name.into(),
                    self.node_id.clone(),
                    DEFAULT_LEASE_TTL,
                    self.cancel.clone(),
                    move |_lease, cancel| {
                        let state = Arc::clone(&state);
                        async move {
                            loop {
                                let result = match name {
                                    "bulk-resumer" => {
                                        locus_http::account::resume_bulk_jobs(&state).await
                                    }
                                    _ => locus_http::account::delete_due_accounts(&state).await,
                                };
                                match &result {
                                    Ok(n) if *n > 0 => {
                                        tracing::info!(job = name, accounts = n, "done")
                                    }
                                    Ok(_) => {}
                                    Err(e) => tracing::warn!(job = name, "failed: {e}"),
                                }
                                let err = result.as_ref().err().map(ToString::to_string);
                                let _ =
                                    state.store.record_job_run(name, err.as_deref(), None).await;
                                tokio::select! {
                                    () = cancel.cancelled() => return Ok(()),
                                    () = tokio::time::sleep(EVERY) => {}
                                }
                            }
                        }
                    },
                )));
        }
    }

    /// Every node re-reads the current rotation generation every 30 s; a
    /// node that can't for 5 minutes refuses private-post encodes (they
    /// fail closed) until it can again.
    fn spawn_oracle_refresh(&mut self, state: Arc<AppState>) {
        let cancel = self.cancel.clone();
        self.tasks.push(tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = cancel.cancelled() => return,
                    () = tokio::time::sleep(locus_kryphocron::rotation::REFRESH_INTERVAL) => {}
                }
                if let Err(e) = locus_http::kryphocron::refresh_oracle(&state).await {
                    tracing::warn!("rotation generation refresh failed: {e}");
                }
            }
        }));
    }

    /// The audience sync (polling, queued edits, local audiences created in
    /// prism) and the re-point job, each on one node at a time.
    fn spawn_audience_jobs(&mut self, state: Arc<AppState>) {
        let st = Arc::clone(&state);
        self.tasks
            .push(tokio::spawn(locus_store::runtime::run_leased(
                self.store.clone(),
                "kr-audience-sync".into(),
                self.node_id.clone(),
                DEFAULT_LEASE_TTL,
                self.cancel.clone(),
                move |_lease, cancel| locus_http::audience_sync::run(Arc::clone(&st), cancel),
            )));
        let st = Arc::clone(&state);
        self.tasks
            .push(tokio::spawn(locus_store::runtime::run_leased(
                self.store.clone(),
                "kr-repoint".into(),
                self.node_id.clone(),
                DEFAULT_LEASE_TTL,
                self.cancel.clone(),
                move |_lease, cancel| {
                    let state = Arc::clone(&st);
                    async move {
                        loop {
                            let result =
                                locus_http::audience_sync::repoint_pass(&state, &cancel).await;
                            if let Err(e) = &result {
                                tracing::warn!("re-point pass failed: {e}");
                            }
                            let err = result.as_ref().err().map(ToString::to_string);
                            let _ = state
                                .store
                                .record_job_run("kr-repoint", err.as_deref(), None)
                                .await;
                            tokio::select! {
                                () = cancel.cancelled() => return Ok(()),
                                () = tokio::time::sleep(Duration::from_secs(60)) => {}
                            }
                        }
                    }
                },
            )));
    }

    /// Kryphocron upkeep, each on one node at a time: new rotation
    /// generations and the rewrite-on-rotate passes, and audit retention.
    fn spawn_kryphocron_jobs(&mut self, state: Arc<AppState>) {
        const EVERY: Duration = Duration::from_secs(60);
        for name in ["kr-rotation", "kr-audit-retention"] {
            let state = Arc::clone(&state);
            self.tasks
                .push(tokio::spawn(locus_store::runtime::run_leased(
                    self.store.clone(),
                    name.into(),
                    self.node_id.clone(),
                    DEFAULT_LEASE_TTL,
                    self.cancel.clone(),
                    move |lease, cancel| {
                        let state = Arc::clone(&state);
                        async move {
                            loop {
                                let result = if name == "kr-rotation" {
                                    locus_http::kryphocron::rotation_pass(&state, &lease, &cancel)
                                        .await
                                        .map(|r| r.rewritten)
                                } else {
                                    let days =
                                        state.snapshot().int("kryphocron.audit_retention_days");
                                    let before = locus_core::time::now_ms() - days * 86_400_000;
                                    let mut total = 0;
                                    loop {
                                        match state.store.prune_kr_audit(before, 5_000).await {
                                            Ok(0) => break Ok(total),
                                            Ok(n) => total += n,
                                            Err(e) => break Err(e),
                                        }
                                    }
                                };
                                match &result {
                                    Ok(n) if *n > 0 => {
                                        tracing::info!(job = name, records = n, "done")
                                    }
                                    Ok(_) => {}
                                    Err(e) => tracing::warn!(job = name, "failed: {e}"),
                                }
                                let err = result.as_ref().err().map(ToString::to_string);
                                let _ =
                                    state.store.record_job_run(name, err.as_deref(), None).await;
                                tokio::select! {
                                    () = cancel.cancelled() => return Ok(()),
                                    () = tokio::time::sleep(EVERY) => {}
                                }
                            }
                        }
                    },
                )));
        }
    }

    /// Retries identity changes that didn't finish (handle changes after an
    /// unclear PLC answer, tombstones of DIDs from failed sign-ups), on one
    /// node at a time.
    /// Watches the install's key generation; a change (another node, or
    /// this one, changed a key nodes hold in memory) restarts the service.
    fn spawn_key_watch(&mut self, generation: i64) {
        let store = self.store.clone();
        let reload = self.reload.clone();
        let cancel = self.cancel.clone();
        self.tasks.push(tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = cancel.cancelled() => return,
                    () = tokio::time::sleep(Duration::from_secs(3)) => {}
                }
                match store.key_state().await {
                    Ok(k) if k.generation != generation => {
                        reload.cancel();
                        return;
                    }
                    Ok(_) => {}
                    Err(e) => tracing::debug!("key generation check failed: {e}"),
                }
            }
        }));
    }

    /// `migrate blobs`: one node at a time runs the copy passes and the
    /// cutover, fenced on the job's lease (a cancel drops the lease).
    fn spawn_blob_migration(&mut self, state: Arc<AppState>) {
        self.tasks
            .push(tokio::spawn(locus_store::runtime::run_leased(
                self.store.clone(),
                locus_store::migrate_blobs::MIGRATE_LEASE.into(),
                self.node_id.clone(),
                DEFAULT_LEASE_TTL,
                self.cancel.clone(),
                move |lease, cancel| {
                    let state = Arc::clone(&state);
                    async move {
                        loop {
                            let result = locus_http::blob_migration::step(&state, &lease).await;
                            let busy = match &result {
                                Ok(locus_http::blob_migration::Step::Copied(n)) => {
                                    tracing::info!(copied = n, "blob migration pass");
                                    true
                                }
                                Ok(locus_http::blob_migration::Step::Flipped(_)) => true,
                                Ok(
                                    locus_http::blob_migration::Step::Waiting(why)
                                    | locus_http::blob_migration::Step::CutoverRetried(why),
                                ) => {
                                    tracing::warn!("blob migration: {why}");
                                    false
                                }
                                Ok(_) => false,
                                Err(locus_core::CoreError::LeaseLost(m)) => {
                                    return Err(locus_core::CoreError::LeaseLost(m.clone()));
                                }
                                Err(e) => {
                                    tracing::warn!("blob migration step failed: {e}");
                                    false
                                }
                            };
                            let wait = if busy { 1 } else { 10 };
                            tokio::select! {
                                () = cancel.cancelled() => return Ok(()),
                                () = tokio::time::sleep(Duration::from_secs(wait)) => {}
                            }
                        }
                    }
                },
            )));
    }

    /// The hostname migration job: moves accounts' PDS endpoints, then
    /// rechecks stragglers daily; one node at a time.
    fn spawn_hostname_migration(&mut self, state: Arc<AppState>) {
        const NAME: &str = "hostname-migration";
        self.tasks
            .push(tokio::spawn(locus_store::runtime::run_leased(
                self.store.clone(),
                NAME.into(),
                self.node_id.clone(),
                DEFAULT_LEASE_TTL,
                self.cancel.clone(),
                move |_lease, cancel| {
                    let state = Arc::clone(&state);
                    async move {
                        loop {
                            let result = locus_http::hostnames::step(&state).await;
                            let busy = matches!(result, Ok(true));
                            if let Err(e) = &result {
                                tracing::warn!("hostname migration step failed: {e}");
                            }
                            let err = result.as_ref().err().map(ToString::to_string);
                            let _ = state.store.record_job_run(NAME, err.as_deref(), None).await;
                            let wait = if busy { 1 } else { 60 };
                            tokio::select! {
                                () = cancel.cancelled() => return Ok(()),
                                () = tokio::time::sleep(Duration::from_secs(wait)) => {}
                            }
                        }
                    }
                },
            )));
    }

    /// Scheduled backups (`backups.every_hours`), checked every ten
    /// minutes on one node at a time; the backup itself takes its own
    /// lease.
    fn spawn_backup_schedule(&mut self, state: Arc<AppState>) {
        const NAME: &str = "backup-schedule";
        self.tasks
            .push(tokio::spawn(locus_store::runtime::run_leased(
                self.store.clone(),
                NAME.into(),
                self.node_id.clone(),
                DEFAULT_LEASE_TTL,
                self.cancel.clone(),
                move |_lease, cancel| {
                    let state = Arc::clone(&state);
                    async move {
                        loop {
                            let result =
                                locus_http::backups::schedule_step(Arc::clone(&state)).await;
                            let err = result.as_ref().err().map(ToString::to_string);
                            let _ = state.store.record_job_run(NAME, err.as_deref(), None).await;
                            tokio::select! {
                                () = cancel.cancelled() => return Ok(()),
                                () = tokio::time::sleep(Duration::from_secs(600)) => {}
                            }
                        }
                    }
                },
            )));
    }

    /// The PLC key scan (which held keys each account's document lists),
    /// on one node at a time.
    fn spawn_plc_key_scan(&mut self, state: Arc<AppState>) {
        const NAME: &str = "plc-key-scan";
        self.tasks
            .push(tokio::spawn(locus_store::runtime::run_leased(
                self.store.clone(),
                NAME.into(),
                self.node_id.clone(),
                DEFAULT_LEASE_TTL,
                self.cancel.clone(),
                move |_lease, cancel| {
                    let state = Arc::clone(&state);
                    async move {
                        loop {
                            let result = locus_http::keys::plc_scan_step(&state, 100).await;
                            let idle = match &result {
                                Ok(0) => true,
                                Ok(n) => {
                                    tracing::debug!(checked = n, "PLC key scan");
                                    false
                                }
                                Err(e) => {
                                    tracing::warn!("PLC key scan failed: {e}");
                                    true
                                }
                            };
                            let err = result.as_ref().err().map(ToString::to_string);
                            let _ = state.store.record_job_run(NAME, err.as_deref(), None).await;
                            let wait = if idle { 60 } else { 1 };
                            tokio::select! {
                                () = cancel.cancelled() => return Ok(()),
                                () = tokio::time::sleep(Duration::from_secs(wait)) => {}
                            }
                        }
                    }
                },
            )));
    }

    fn spawn_identity_repair(&mut self, state: Arc<AppState>) {
        const NAME: &str = "identity-repair";
        const EVERY: Duration = Duration::from_secs(30);
        self.tasks
            .push(tokio::spawn(locus_store::runtime::run_leased(
                self.store.clone(),
                NAME.into(),
                self.node_id.clone(),
                DEFAULT_LEASE_TTL,
                self.cancel.clone(),
                move |_lease, cancel| {
                    let state = Arc::clone(&state);
                    async move {
                        loop {
                            let result = locus_http::identity::run_identity_repairs(&state).await;
                            match &result {
                                Ok(n) if *n > 0 => tracing::info!(repaired = n, "identity repair"),
                                Ok(_) => {}
                                Err(e) => tracing::warn!("identity repair failed: {e}"),
                            }
                            let err = result.as_ref().err().map(ToString::to_string);
                            let _ = state.store.record_job_run(NAME, err.as_deref(), None).await;
                            tokio::select! {
                                () = cancel.cancelled() => return Ok(()),
                                () = tokio::time::sleep(EVERY) => {}
                            }
                        }
                    }
                },
            )));
    }

    /// Deletes repo blocks whose reference count has been 0 for longer than
    /// the grace period, on one node at a time.
    fn spawn_block_gc(&mut self) {
        const NAME: &str = "gc-blocks";
        const GRACE_MS: i64 = 10 * 60 * 1000;
        const EVERY: Duration = Duration::from_secs(60);
        const BATCH: i64 = 5_000;
        let store = self.store.clone();
        self.tasks
            .push(tokio::spawn(locus_store::runtime::run_leased(
                self.store.clone(),
                NAME.into(),
                self.node_id.clone(),
                DEFAULT_LEASE_TTL,
                self.cancel.clone(),
                move |_lease, cancel| {
                    let store = store.clone();
                    async move {
                        loop {
                            let mut total = 0;
                            let result = loop {
                                match store.gc_zero_blocks(GRACE_MS, BATCH).await {
                                    Ok(n) => {
                                        total += n;
                                        if n < BATCH as u64 || cancel.is_cancelled() {
                                            break Ok(());
                                        }
                                    }
                                    Err(e) => break Err(e),
                                }
                            };
                            if total > 0 {
                                tracing::info!(blocks = total, "block gc");
                            }
                            let err = result.as_ref().err().map(ToString::to_string);
                            let _ = store.record_job_run(NAME, err.as_deref(), None).await;
                            tokio::select! {
                                () = cancel.cancelled() => return Ok(()),
                                () = tokio::time::sleep(EVERY) => {}
                            }
                        }
                    }
                },
            )));
    }

    /// Renews the clock-id slot. Losing it means another node may mint TIDs
    /// with the same clock id, so the node stops.
    fn spawn_clock_renewal(&mut self) {
        let (store, token, cancel) = (
            self.store.clone(),
            self.clock_lease.clone(),
            self.cancel.clone(),
        );
        self.tasks.push(tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = cancel.cancelled() => return,
                    () = tokio::time::sleep(token.renew_interval()) => {}
                }
                match store.renew_lease(&token).await {
                    Ok(true) => {}
                    Ok(false) => {
                        tracing::error!("clock id lease lost; stopping the node");
                        cancel.cancel();
                        return;
                    }
                    Err(e) => tracing::warn!("clock id renewal failed, will retry: {e}"),
                }
            }
        }));
    }

    fn spawn_log_level(&mut self, log: LogControl) {
        let mut rx = self.settings.subscribe();
        let cancel = self.cancel.clone();
        self.tasks.push(tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = cancel.cancelled() => return,
                    res = rx.changed() => if res.is_err() { return },
                }
                let level = rx.borrow_and_update().text("log.level").to_owned();
                log.apply(&level);
            }
        }));
    }

    fn spawn_env_recheck(&mut self, values: Vec<locus_store::env::EnvValue>) {
        let (store, cancel) = (self.store.clone(), self.cancel.clone());
        self.tasks.push(tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = cancel.cancelled() => return,
                    () = tokio::time::sleep(ENV_RECHECK) => {}
                }
                match store.apply_env(&values).await {
                    Ok(applied) => {
                        for key in applied {
                            tracing::info!(key, "applied env value");
                        }
                        let _ = store.record_job_run("env-recheck", None, None).await;
                    }
                    Err(e) => {
                        tracing::warn!("env re-check failed: {e}");
                        let _ = store
                            .record_job_run("env-recheck", Some(&e.to_string()), None)
                            .await;
                    }
                }
            }
        }));
    }

    /// Waits until shutdown is requested (or, in setup mode, setup
    /// completes), then stops background tasks and gives up this node's
    /// leases so others can take over at once.
    pub async fn run_until_stopped(self) -> Ended {
        tracing::info!(
            node_id = %self.node_id,
            clock_id = self.clock_id,
            role = ?self.bootstrap.role,
            key_id = %self.master_key.key_id(),
            settings_version = self.settings.current().version(),
            "running"
        );
        let ended = tokio::select! {
            () = self.cancel.cancelled() => Ended::Stopped,
            () = self.setup_done.cancelled() => Ended::Restart,
            () = self.reload.cancelled() => {
                tracing::info!("the install's keys changed; reloading");
                Ended::Restart
            }
        };
        // A shutdown during the restart wins.
        let ended = if self.outer.is_cancelled() {
            Ended::Stopped
        } else {
            ended
        };
        self.cancel.cancel();
        tracing::info!(?ended, "stopping");
        for t in self.tasks {
            let _ = t.await;
        }
        if let Err(e) = self.store.release_lease(&self.clock_lease).await {
            tracing::warn!("couldn't release the clock id: {e}");
        }
        self.store.close().await;
        tracing::info!("stopped");
        ended
    }
}

/// Cancels `cancel` on SIGTERM or Ctrl-C.
pub fn cancel_on_signal(cancel: CancellationToken) {
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            let mut term =
                match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!("can't watch SIGTERM: {e}");
                        let _ = tokio::signal::ctrl_c().await;
                        cancel.cancel();
                        return;
                    }
                };
            tokio::select! {
                _ = term.recv() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        cancel.cancel();
    });
}
