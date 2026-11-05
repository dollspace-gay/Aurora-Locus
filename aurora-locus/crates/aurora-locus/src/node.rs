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
    let master_key = Arc::new(locus_core::keys::load_master_key(bootstrap, empty)?);
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
            blob_store: format!(
                "On disk, in the data directory ({})",
                self.bootstrap.data_dir.display()
            ),
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
        let secrets = ServerSecrets::load_or_create(&self.store, &self.master_key).await?;
        let sessions =
            LegacySessions::new(self.store.clone(), secrets.jwt.clone(), service_did.clone());
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
        let state = Arc::new(AppState {
            repo: RepoEngine::new(self.store.clone(), Arc::new(TidClock::new(clock_id))),
            store: self.store.clone(),
            settings: self.settings.clone(),
            master_key: self.master_key.clone(),
            secrets,
            sessions,
            mailer,
            blobs: locus_blob::DiskStore::new(self.bootstrap.data_dir.clone()),
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
        });
        self.setup_done = state.setup.completed.clone();
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
            self.spawn_account_jobs(Arc::clone(&state));
            self.spawn_blob_jobs(Arc::clone(&state));
            self.spawn_sync_jobs(Arc::clone(&state));
        }
        let addr = self.bootstrap.listen;
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| CoreError::Config(format!("can't listen on {addr}: {e}")))?;
        tracing::info!(%addr, public_url = %state.public_url, setup_mode = self.setup_mode, "listening");
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
                            // Console sign-ins past their end.
                            let result = match result {
                                Ok(t) => state.store.prune_operator_sessions().await.map(|n| t + n),
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
    /// failure left, deleting queued keys after their grace period, and GC
    /// of never-referenced temp blobs and long-unreferenced ones.
    fn spawn_blob_jobs(&mut self, state: Arc<AppState>) {
        const EVERY: Duration = Duration::from_secs(60);
        for name in ["blob-moves", "blob-deletes", "blob-gc", "blob-rescan"] {
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
