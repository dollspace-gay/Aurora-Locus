//! Shared state for every handler, and the server secrets created on first
//! boot.

use std::sync::Arc;

use locus_auth::session::LegacySessions;
use locus_core::keys::MasterKey;
use locus_core::settings::Snapshot;
use locus_core::{CoreError, CoreResult};
use locus_identity::plc_client::PlcClient;
use locus_identity::resolve::{ResolverConfig, Resolvers};
use locus_repo::RepoEngine;
use locus_store::Store;
use locus_store::identity::{PlcKeyRole, plc_key_secret_name};
use locus_store::runtime::Settings;
use proto_blue_crypto::{ExportableKeypair, K256Keypair, Keypair};
use zeroize::Zeroizing;

/// Names of the server's own secrets in the key store.
pub mod secret_names {
    pub const JWT: &str = "jwt_secret";
    pub const APP_PASSWORD_PEPPER: &str = "app_password_pepper";
    pub const EMAIL_TOKEN_PEPPER: &str = "email_token_pepper";
    pub const SERVICE_KEY: &str = "service_signing_key";
    pub const OAUTH_KEY: &str = "oauth_signing_key";
    pub const DPOP_SECRET: &str = "dpop_secret";
    pub const CORRELATION_KEY: &str = "kryphocron_correlation_key";
    pub const AUDIENCE_KEY: &str = "kryphocron_audience_key";
    pub const ADMIN_PASSWORD: &str = "admin_password";
}

/// The server's secrets, decrypted for this process.
pub struct ServerSecrets {
    pub jwt: Zeroizing<Vec<u8>>,
    pub app_password_pepper: Zeroizing<Vec<u8>>,
    /// Keys the hashes emailed tokens are stored as.
    pub email_token_pepper: Zeroizing<Vec<u8>>,
    pub plc_keys: PlcKeys,
    pub service_key: K256Keypair,
    /// Signs OAuth access tokens (ES256).
    pub oauth_key: locus_auth::oauth::jose::Es256Key,
    /// DPoP nonces, from a secret every node shares.
    pub dpop_nonces: locus_auth::oauth::dpop::Nonces,
    /// Keys correlation ids in kryphocron audit rows.
    pub correlation_key: [u8; 32],
    /// Signs membership assertions (`#kryphocron_audience`), never service
    /// auth.
    pub audience_key: K256Keypair,
    /// The reference's single admin credential (`admin:<password>` Basic
    /// auth): `PDS_ADMIN_PASSWORD` when set, otherwise generated and kept.
    pub admin_password: Zeroizing<Vec<u8>>,
}

/// The PDS's held PLC rotation keys: the current one, plus any pending or
/// secondary ones it can still sign with.
pub struct PlcKeys {
    /// Index of the current key in `held`.
    current: usize,
    held: Vec<(String, PlcKeyRole, K256Keypair)>,
}

impl PlcKeys {
    /// Loads the held keys. On first boot the current key is created:
    /// imported from `PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX` if set,
    /// otherwise generated. Later, a changed env value is stored as the
    /// pending key (it becomes current after a key backup), as
    /// `keys set plc-rotation-key` does.
    pub async fn load_or_create(store: &Store, key: &MasterKey) -> CoreResult<Self> {
        let env_key = env_plc_key()?;
        if store.plc_rotation_keys().await?.is_empty() {
            let candidate = match &env_key {
                Some(k) => keypair_from(k)?,
                None => K256Keypair::generate(),
            };
            let did_key = candidate.did();
            let private = Zeroizing::new(candidate.export_private_key());
            let mut tx = store.begin().await?;
            tx.put_secret_if_absent(&plc_key_secret_name(&did_key), &private, key)
                .await?;
            match tx
                .insert_plc_rotation_key(&did_key, PlcKeyRole::Current)
                .await
            {
                Ok(()) => {
                    if env_key.is_some() {
                        tx.cas_env_applied(PLC_ENV_APPLIED, None, &env_hash(&did_key))
                            .await?;
                    }
                    tx.commit().await?;
                }
                // Another node created the current key first; use theirs.
                Err(CoreError::Conflict(_)) => tx.rollback().await?,
                Err(e) => return Err(e),
            }
        } else if let Some(k) = &env_key {
            apply_env_plc_key(store, key, &keypair_from(k)?).await?;
        }
        let mut held = Vec::new();
        for row in store.plc_rotation_keys().await? {
            let bytes = store
                .get_secret(&plc_key_secret_name(&row.did_key), key)
                .await?
                .ok_or_else(|| {
                    CoreError::Config(format!(
                        "PLC rotation key {} is missing from the key store",
                        row.did_key
                    ))
                })?;
            let pair = K256Keypair::from_private_key(&bytes).map_err(|e| {
                CoreError::Config(format!(
                    "stored PLC rotation key {} is invalid: {e}",
                    row.did_key
                ))
            })?;
            if pair.did() != row.did_key {
                return Err(CoreError::Config(format!(
                    "stored PLC rotation key doesn't match its id {}",
                    row.did_key
                )));
            }
            held.push((row.did_key, row.role, pair));
        }
        let current = held
            .iter()
            .position(|(_, role, _)| *role == PlcKeyRole::Current)
            .ok_or_else(|| CoreError::Config("no current PLC rotation key".into()))?;
        Ok(Self { current, held })
    }

    /// The key new genesis operations and recommendations name.
    pub fn current(&self) -> &K256Keypair {
        &self.held[self.current].2
    }

    /// The held key to sign an account's next op with: the first one its
    /// latest document lists, in document order.
    pub fn signer_for<'a>(&'a self, rotation_keys: &[String]) -> Option<&'a K256Keypair> {
        rotation_keys.iter().find_map(|k| {
            self.held
                .iter()
                .find(|(id, _, _)| id == k)
                .map(|(_, _, p)| p)
        })
    }

    /// Every held key's did:key and role.
    pub fn held_keys(&self) -> Vec<(String, PlcKeyRole)> {
        self.held
            .iter()
            .map(|(id, role, _)| (id.clone(), *role))
            .collect()
    }

    /// The held keys a document lists, in document order.
    pub fn listed(&self, rotation_keys: &[String]) -> Vec<String> {
        rotation_keys
            .iter()
            .filter(|k| self.held.iter().any(|(id, _, _)| id == *k))
            .cloned()
            .collect()
    }
}

/// The `env_applied` name the PLC rotation key's env value is tracked
/// under (its hash is of the public did:key).
pub const PLC_ENV_APPLIED: &str = "env:PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX";

fn env_plc_key() -> CoreResult<Option<Zeroizing<Vec<u8>>>> {
    match std::env::var("PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX") {
        Ok(h) if !h.trim().is_empty() => Ok(Some(Zeroizing::new(hex::decode(h.trim()).map_err(
            |e| CoreError::Config(format!("PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX: {e}")),
        )?))),
        _ => Ok(None),
    }
}

fn keypair_from(bytes: &[u8]) -> CoreResult<K256Keypair> {
    K256Keypair::from_private_key(bytes)
        .map_err(|e| CoreError::Config(format!("PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX: {e}")))
}

fn env_hash(did_key: &str) -> String {
    hex::encode(<sha2::Sha256 as sha2::Digest>::digest(did_key.as_bytes()))
}

/// Applies a changed `PDS_PLC_ROTATION_KEY_*` value: a new key is stored
/// pending. A value set back to the current key, or changed while another
/// key is pending, needs the listing guard, so it's left (and warned
/// about at every boot) for `keys set plc-rotation-key`.
pub(crate) async fn apply_env_plc_key(
    store: &Store,
    key: &MasterKey,
    env: &K256Keypair,
) -> CoreResult<()> {
    let did_key = env.did();
    let hash = env_hash(&did_key);
    let previous = store.env_applied_hashes().await?.remove(PLC_ENV_APPLIED);
    if previous.as_deref() == Some(hash.as_str()) {
        return Ok(());
    }
    let rows = store.plc_rotation_keys().await?;
    let role_of = |r: PlcKeyRole| rows.iter().find(|k| k.role == r).map(|k| k.did_key.clone());
    let (current, pending) = (role_of(PlcKeyRole::Current), role_of(PlcKeyRole::Pending));
    let mut tx = store.begin().await?;
    if current.as_deref() == Some(did_key.as_str()) || pending.as_deref() == Some(did_key.as_str())
    {
        if let Some(p) = pending.filter(|p| *p != did_key) {
            tracing::warn!(
                pending = p,
                "PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX is the current key again while another key is pending; \
                 remove the pending one with `aurora-locus keys set plc-rotation-key --discard-pending`"
            );
            tx.rollback().await?;
            return Ok(());
        }
    } else {
        if let Some(p) = pending {
            tracing::warn!(
                pending = p,
                "PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX changed while another key is pending; \
                 replace it with `aurora-locus keys set plc-rotation-key`"
            );
            tx.rollback().await?;
            return Ok(());
        }
        let private = Zeroizing::new(env.export_private_key());
        tx.store_pending_plc_key(&did_key, &private, key).await?;
        tracing::warn!(
            did_key,
            "the PLC rotation key from PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX is pending: it becomes \
             current after a key backup (`aurora-locus keys backup`), or with \
             `aurora-locus keys set plc-rotation-key --acknowledge-no-backup`"
        );
    }
    if tx
        .cas_env_applied(PLC_ENV_APPLIED, previous.as_deref(), &hash)
        .await?
    {
        tx.commit().await
    } else {
        // Another node applied it first.
        tx.rollback().await
    }
}

impl ServerSecrets {
    /// Loads every server secret, creating any that are missing. On first
    /// boot, `PDS_JWT_SECRET` and `PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX`
    /// are imported if set, so an existing `pds.env` keeps its keys.
    pub async fn load_or_create(store: &Store, key: &MasterKey) -> CoreResult<Self> {
        let jwt = load_or(store, key, secret_names::JWT, || {
            std::env::var("PDS_JWT_SECRET")
                .ok()
                .filter(|s| !s.is_empty())
                .map(String::into_bytes)
                .unwrap_or_else(|| rand::random::<[u8; 32]>().to_vec())
        })
        .await?;
        let app_password_pepper = load_or(store, key, secret_names::APP_PASSWORD_PEPPER, || {
            rand::random::<[u8; 32]>().to_vec()
        })
        .await?;
        let email_token_pepper = load_or(store, key, secret_names::EMAIL_TOKEN_PEPPER, || {
            rand::random::<[u8; 32]>().to_vec()
        })
        .await?;
        let plc_keys = PlcKeys::load_or_create(store, key).await?;
        let service = load_or(store, key, secret_names::SERVICE_KEY, || {
            K256Keypair::generate().export_private_key()
        })
        .await?;
        let oauth = load_or(store, key, secret_names::OAUTH_KEY, || {
            locus_auth::oauth::jose::Es256Key::generate().to_bytes()
        })
        .await?;
        let oauth_key = locus_auth::oauth::jose::Es256Key::from_bytes(&oauth)
            .map_err(|e| CoreError::Config(format!("stored OAuth signing key: {e}")))?;
        let dpop = load_or(store, key, secret_names::DPOP_SECRET, || {
            std::env::var("PDS_DPOP_SECRET")
                .ok()
                .and_then(|h| hex::decode(h.trim()).ok())
                .filter(|b| b.len() == 32)
                .unwrap_or_else(|| rand::random::<[u8; 32]>().to_vec())
        })
        .await?;
        let correlation = load_or(store, key, secret_names::CORRELATION_KEY, || {
            rand::random::<[u8; 32]>().to_vec()
        })
        .await?;
        let correlation_key: [u8; 32] = correlation.as_slice().try_into().map_err(|_| {
            CoreError::Config("stored kryphocron correlation key isn't 32 bytes".into())
        })?;
        let audience = load_or(store, key, secret_names::AUDIENCE_KEY, || {
            K256Keypair::generate().export_private_key()
        })
        .await?;
        let stored_admin = load_or(store, key, secret_names::ADMIN_PASSWORD, || {
            hex::encode(rand::random::<[u8; 16]>()).into_bytes()
        })
        .await?;
        let admin_password = match std::env::var("PDS_ADMIN_PASSWORD") {
            Ok(p) if !p.is_empty() => Zeroizing::new(p.into_bytes()),
            _ => stored_admin,
        };
        let keypair = |bytes: &[u8], what: &str| {
            K256Keypair::from_private_key(bytes).map_err(|e| {
                CoreError::Config(format!("stored {what} is not a valid secp256k1 key: {e}"))
            })
        };
        Ok(Self {
            plc_keys,
            service_key: keypair(&service, "service key")?,
            oauth_key,
            dpop_nonces: locus_auth::oauth::dpop::Nonces::new(&dpop),
            correlation_key,
            audience_key: keypair(&audience, "membership assertion key")?,
            admin_password,
            jwt,
            app_password_pepper,
            email_token_pepper,
        })
    }
}

async fn load_or(
    store: &Store,
    key: &MasterKey,
    name: &str,
    make: impl FnOnce() -> Vec<u8>,
) -> CoreResult<Zeroizing<Vec<u8>>> {
    if let Some(v) = store.get_secret(name, key).await? {
        return Ok(v);
    }
    let value = Zeroizing::new(make());
    let mut tx = store.begin().await?;
    tx.put_secret_if_absent(name, &value, key).await?;
    tx.commit().await?;
    // Another node may have written it first; read back what won.
    store
        .get_secret(name, key)
        .await?
        .ok_or_else(|| CoreError::Storage(format!("secret {name} vanished after it was written")))
}

/// Everything a handler needs.
pub struct AppState {
    pub store: Store,
    pub settings: Arc<Settings>,
    pub master_key: Arc<MasterKey>,
    pub secrets: ServerSecrets,
    pub sessions: LegacySessions,
    pub mailer: locus_mail::Mailer,
    /// Blob bytes: the install's active backend.
    pub blobs: locus_blob::BlobStore,
    /// This node's local files that aren't blobs (repository imports in
    /// progress), under the data directory.
    pub scratch: locus_blob::DiskStore,
    /// This node's view of the event log.
    pub firehose: Arc<locus_sync::Firehose>,
    pub repo: RepoEngine,
    /// The PDS hostname (no scheme or port).
    pub hostname: String,
    /// `https://<hostname>`, or `http://localhost:<port>` on localhost.
    pub public_url: String,
    /// `did:web:<hostname>`.
    pub service_did: String,
    /// This node's id (holder of the bulk jobs it runs).
    pub node_id: String,
    /// The reference's `ssrfProtection`, fixed at boot.
    pub ssrf_protection: bool,
    pub resolvers: ResolverCache,
    /// Rate-limit counters (in-process backend) and firehose subscriber counts.
    pub limits: crate::ratelimit::Limiter,
    /// Fetched OAuth client metadata and keys.
    pub oauth_clients: crate::oauth::ClientCache,
    /// The kryphocron codec, rotation oracle and audit sinks.
    pub kr: Arc<crate::kryphocron::Kr>,
    /// Whether this node is in setup mode.
    pub setup: Arc<crate::setup::Gate>,
    /// This node's recent log lines.
    pub logs: Arc<locus_core::logs::LogRing>,
    /// The published branding theme, re-read every few seconds.
    pub branding: crate::branding::Cache,
    /// Marks requests that came over the admin socket (never sent anywhere).
    pub socket_secret: String,
    /// Cancelled when the keys this node holds in memory changed: the node
    /// then restarts its service in place to reload them.
    pub reload: tokio_util::sync::CancellationToken,
}

/// The network resolvers, rebuilt when the settings they're made from change.
#[derive(Default)]
pub struct ResolverCache(std::sync::Mutex<Option<Arc<Resolvers>>>);

impl AppState {
    /// DID and handle resolution over the network.
    pub fn resolvers(&self) -> Result<Arc<Resolvers>, crate::error::XrpcError> {
        let snap = self.snapshot();
        let backup_nameservers = snap
            .list("identity.handle_backup_nameservers")
            .into_iter()
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        let config = ResolverConfig {
            plc_url: snap
                .text("identity.plc_url")
                .trim_end_matches('/')
                .to_owned(),
            timeout_ms: u64::try_from(snap.int("identity.resolver_timeout_ms")).unwrap_or(3000),
            did_stale_ms: snap.int("identity.did_cache_stale_ms"),
            did_max_ms: snap.int("identity.did_cache_max_ms"),
            backup_nameservers,
            ssrf_protection: self.ssrf_protection,
        };
        let mut slot = self
            .resolvers
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(r) = slot.as_ref()
            && *r.config() == config
        {
            return Ok(Arc::clone(r));
        }
        let r = Arc::new(
            Resolvers::new(self.store.clone(), config)
                .map_err(|e| crate::error::XrpcError::internal(e.to_string()))?,
        );
        *slot = Some(Arc::clone(&r));
        Ok(r)
    }

    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.settings.current()
    }

    /// Handle domains, each starting with a dot; `.<hostname>` when none are set.
    pub fn handle_domains(&self) -> Vec<String> {
        let snap = self.snapshot();
        let set: Vec<String> = snap
            .list("identity.handle_domains")
            .into_iter()
            .map(|d| {
                let d = d.to_ascii_lowercase();
                if d.starts_with('.') {
                    d
                } else {
                    format!(".{d}")
                }
            })
            .collect();
        if set.is_empty() {
            vec![format!(".{}", self.hostname)]
        } else {
            set
        }
    }

    pub fn plc(&self) -> Result<PlcClient, crate::error::XrpcError> {
        PlcClient::new(self.snapshot().text("identity.plc_url"))
            .map_err(|e| crate::error::XrpcError::internal(e.to_string()))
    }

    /// A DID's document: one locus hosts is read from the database, any
    /// other is resolved (from the shared cache unless `force`).
    pub async fn resolve_did_doc(
        &self,
        did: &str,
        force: bool,
    ) -> Result<Option<proto_blue_common::DidDocument>, crate::error::XrpcError> {
        if let Some(hosted) = self.store.hosted_did_web(did).await? {
            return Ok(hosted.doc.and_then(|d| serde_json::from_str(&d).ok()));
        }
        self.resolvers()?
            .resolve_did(did, force)
            .await
            .map_err(|e| {
                crate::error::XrpcError::auth_required(format!("could not resolve iss did: {e}"))
            })
    }

    /// An account's repo signing key.
    pub async fn signing_key(&self, did: &str) -> Result<K256Keypair, crate::error::XrpcError> {
        let (_, sk) = self
            .store
            .get_signing_key(did, &self.master_key)
            .await?
            .ok_or_else(|| crate::error::XrpcError::internal("account has no signing key"))?;
        K256Keypair::from_private_key(&sk)
            .map_err(|e| crate::error::XrpcError::internal(e.to_string()))
    }

    pub fn service_did_key(&self) -> String {
        self.secrets.service_key.did()
    }
}
