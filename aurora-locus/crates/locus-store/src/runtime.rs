//! Running pieces built on the store: the live settings handle and the
//! leased-job runner.

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use locus_core::lease::LeaseToken;
use locus_core::settings::Snapshot;
use locus_core::{CoreError, CoreResult};
use serde_json::Value;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::Store;
use crate::settings::SETTINGS_CHANNEL;

/// How often nodes poll `settings_version`, as a fallback to notifications.
pub const SETTINGS_POLL: Duration = Duration::from_secs(5);

/// Live runtime settings. Readers take one [`Snapshot`] per request and use
/// it to the end, so a change never applies halfway through. Components that
/// hold derived state (clients, rate limiters) [`subscribe`](Self::subscribe)
/// and rebuild when a new snapshot arrives.
pub struct Settings {
    store: Store,
    sender: watch::Sender<Arc<Snapshot>>,
}

impl Settings {
    pub async fn load(store: Store) -> CoreResult<Arc<Self>> {
        let snap = store.load_snapshot().await?;
        let (sender, _) = watch::channel(Arc::new(snap));
        Ok(Arc::new(Self { store, sender }))
    }

    /// The current snapshot.
    pub fn current(&self) -> Arc<Snapshot> {
        self.sender.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Arc<Snapshot>> {
        self.sender.subscribe()
    }

    /// Validates, stores (audited as `actor`) and applies a setting on this
    /// node at once. Other nodes pick it up by notification or poll.
    pub async fn set(&self, key: &str, value: Value, actor: &str) -> CoreResult<i64> {
        let version = self.store.set_setting(key, value, actor).await?;
        self.refresh().await?;
        Ok(version)
    }

    /// Reloads if the stored version moved. Returns whether it did.
    pub async fn refresh(&self) -> CoreResult<bool> {
        let stored = self.store.settings_version().await?;
        if stored == self.current().version() {
            return Ok(false);
        }
        let snap = self.store.load_snapshot().await?;
        // Never go backwards if two refreshes race.
        let mut changed = false;
        self.sender.send_if_modified(|cur| {
            if snap.version() > cur.version() {
                *cur = Arc::new(snap);
                changed = true;
            }
            changed
        });
        Ok(changed)
    }

    /// Keeps this node's snapshot current until `cancel` fires: Postgres
    /// `LISTEN` for immediate changes, plus a poll every [`SETTINGS_POLL`]
    /// in case a notification is missed or the listener is down.
    pub fn spawn_watcher(
        self: &Arc<Self>,
        cancel: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let mut listener = this.connect_listener().await;
            // A change made before LISTEN started sent no notification we'd see.
            if let Err(e) = this.refresh().await {
                tracing::warn!("settings refresh failed: {e}");
            }
            loop {
                let notified = async {
                    match listener.as_mut() {
                        Some(l) => l.recv().await.map(|_| ()).map_err(|e| e.to_string()),
                        None => std::future::pending().await,
                    }
                };
                tokio::select! {
                    () = cancel.cancelled() => return,
                    () = tokio::time::sleep(SETTINGS_POLL) => {}
                    res = notified => {
                        if let Err(e) = res {
                            tracing::warn!("settings listener lost ({e}); polling until it reconnects");
                            listener = None;
                        }
                    }
                }
                // Reconnect first, so a change made while the listener was
                // down is picked up by the refresh that follows.
                if listener.is_none() {
                    listener = this.connect_listener().await;
                }
                if let Err(e) = this.refresh().await {
                    tracing::warn!("settings refresh failed: {e}");
                }
            }
        })
    }

    async fn connect_listener(&self) -> Option<sqlx::postgres::PgListener> {
        let Store::Postgres(pool) = &self.store else {
            return None;
        };
        match sqlx::postgres::PgListener::connect_with(pool).await {
            Ok(mut l) => match l.listen(SETTINGS_CHANNEL).await {
                Ok(()) => Some(l),
                Err(e) => {
                    tracing::warn!("settings LISTEN failed: {e}");
                    None
                }
            },
            Err(e) => {
                tracing::warn!("settings listener connect failed: {e}");
                None
            }
        }
    }
}

/// Runs `job` on this node only while it holds the lease `name`, until
/// `cancel` fires. While another node holds the lease, it waits and retries.
/// It renews every third of `ttl`; if a renewal fails, or hasn't answered
/// by a sixth of `ttl` before the lease would run out (counted from before
/// each request, so never later than the store's own expiry), the job's own
/// cancellation token fires and the runner waits for it to stop before trying
/// to reacquire. Work the job commits must still call [`crate::Tx::fence`]
/// with the token: a paused process can wake after its lease expired.
pub async fn run_leased<F, Fut>(
    store: Store,
    name: String,
    holder: String,
    ttl: Duration,
    cancel: CancellationToken,
    job: F,
) where
    F: Fn(LeaseToken, CancellationToken) -> Fut,
    Fut: Future<Output = CoreResult<()>> + Send + 'static,
{
    let interval = ttl / 3;
    let margin = ttl / 6;
    while !cancel.is_cancelled() {
        let asked = Instant::now();
        let token = match store.try_acquire_lease(&name, &holder, ttl).await {
            Ok(Some(t)) => t,
            Ok(None) => {
                wait(&cancel, interval).await;
                continue;
            }
            Err(e) => {
                tracing::warn!(lease = %name, "acquire failed: {e}");
                wait(&cancel, interval).await;
                continue;
            }
        };
        tracing::info!(lease = %name, epoch = token.epoch, "lease acquired");

        let job_cancel = cancel.child_token();
        let mut handle = tokio::spawn(job(token.clone(), job_cancel.clone()));
        let mut finished = None;
        // The lease is surely ours until here: the store set its expiry
        // after this instant.
        let mut held_until = asked + ttl;
        loop {
            tokio::select! {
                res = &mut handle => {
                    finished = Some(res);
                    break;
                }
                () = tokio::time::sleep(interval) => {
                    let sent = Instant::now();
                    let left = held_until.saturating_duration_since(sent).saturating_sub(margin);
                    match tokio::time::timeout(left, store.renew_lease(&token)).await {
                        Ok(Ok(true)) => held_until = sent + ttl,
                        Ok(Ok(false)) => {
                            tracing::warn!(lease = %name, epoch = token.epoch, "lease lost; stopping job");
                            break;
                        }
                        Ok(Err(e)) => {
                            tracing::warn!(lease = %name, "renew failed ({e}); stopping job");
                            break;
                        }
                        Err(_) => {
                            tracing::warn!(lease = %name, "renew didn't answer before the lease ran out; stopping job");
                            break;
                        }
                    }
                }
            }
        }
        job_cancel.cancel();
        let res = match finished {
            Some(r) => r,
            None => handle.await,
        };
        match res {
            Ok(Ok(())) => {}
            Ok(Err(CoreError::LeaseLost(m))) => tracing::warn!(lease = %name, "job stopped: {m}"),
            Ok(Err(e)) => tracing::error!(lease = %name, "job failed: {e}"),
            Err(e) => tracing::error!(lease = %name, "job panicked: {e}"),
        }
        if let Err(e) = store.release_lease(&token).await {
            tracing::warn!(lease = %name, "release failed: {e}");
        }
        if !cancel.is_cancelled() {
            wait(&cancel, interval).await;
        }
    }
}

async fn wait(cancel: &CancellationToken, d: Duration) {
    tokio::select! {
        () = cancel.cancelled() => {}
        () = tokio::time::sleep(d) => {}
    }
}
