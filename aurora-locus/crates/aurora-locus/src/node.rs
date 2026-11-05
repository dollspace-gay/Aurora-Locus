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
    cancel: CancellationToken,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Node {
    /// Opens the database, claims a clock id, applies env values and loads
    /// live settings. Background tasks stop when `cancel` fires.
    pub async fn start(
        bootstrap: Bootstrap,
        log: LogControl,
        cancel: CancellationToken,
    ) -> CoreResult<Self> {
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

        // The master key is checked before anything is written, migrations
        // included: a node with another install's key must not touch it.
        let store = Store::connect(&bootstrap.database).await?;
        let empty = store.key_store_empty().await?;
        let generated_path = match &bootstrap.master_key {
            locus_core::bootstrap::MasterKeySource::File { path, .. } if !path.exists() => {
                Some(path.clone())
            }
            _ => None,
        };
        let master_key = Arc::new(locus_core::keys::load_master_key(&bootstrap, empty)?);
        if let Some(path) = generated_path {
            tracing::warn!(
                path = %path.display(),
                "generated a new master key; back it up: losing it loses every stored secret"
            );
        }
        store.check_master_key(&master_key).await?;
        tracing::info!(key_id = %master_key.key_id(), source = master_key.source().as_str(), "master key checked");
        store.migrate().await?;

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

        let mut node = Self {
            bootstrap,
            node_id,
            store,
            settings,
            master_key,
            clock_id,
            clock_lease,
            cancel,
            tasks: Vec::new(),
        };
        node.spawn_clock_renewal();
        node.tasks
            .push(node.settings.spawn_watcher(node.cancel.clone()));
        node.spawn_log_level(log);
        node.spawn_env_recheck(env_values);
        node.spawn_block_gc();
        node.spawn_http().await?;
        Ok(node)
    }

    /// Loads the server secrets and starts serving HTTP. The port is bound
    /// here so a busy port fails the boot rather than a background task.
    async fn spawn_http(&mut self) -> CoreResult<()> {
        let hostname = self
            .bootstrap
            .hostname
            .clone()
            .unwrap_or_else(|| "localhost".to_owned())
            .to_ascii_lowercase();
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
        });
        self.spawn_identity_repair(Arc::clone(&state));
        self.spawn_account_jobs(Arc::clone(&state));
        self.spawn_blob_jobs(Arc::clone(&state));
        self.spawn_sync_jobs(Arc::clone(&state));
        let addr = self.bootstrap.listen;
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| CoreError::Config(format!("can't listen on {addr}: {e}")))?;
        tracing::info!(%addr, public_url = %state.public_url, "listening");
        let cancel = self.cancel.clone();
        self.tasks.push(tokio::spawn(async move {
            if let Err(e) = locus_http::serve(state, listener, cancel.clone()).await {
                tracing::error!("http server failed; stopping the node: {e}");
                cancel.cancel();
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
        for name in ["blob-moves", "blob-deletes", "blob-gc"] {
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

    /// Waits until shutdown is requested, then stops background tasks and
    /// gives up this node's leases so others can take over at once.
    pub async fn run_until_stopped(self) {
        tracing::info!(
            node_id = %self.node_id,
            clock_id = self.clock_id,
            role = ?self.bootstrap.role,
            key_id = %self.master_key.key_id(),
            settings_version = self.settings.current().version(),
            "running"
        );
        self.cancel.cancelled().await;
        tracing::info!("stopping");
        for t in self.tasks {
            let _ = t.await;
        }
        if let Err(e) = self.store.release_lease(&self.clock_lease).await {
            tracing::warn!("couldn't release the clock id: {e}");
        }
        self.store.close().await;
        tracing::info!("stopped");
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
