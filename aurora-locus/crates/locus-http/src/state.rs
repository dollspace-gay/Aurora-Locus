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
    /// otherwise generated.
    pub async fn load_or_create(store: &Store, key: &MasterKey) -> CoreResult<Self> {
        if store.plc_rotation_keys().await?.is_empty() {
            let candidate = match std::env::var("PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX") {
                Ok(h) if !h.trim().is_empty() => {
                    let bytes = Zeroizing::new(hex::decode(h.trim()).map_err(|e| {
                        CoreError::Config(format!("PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX: {e}"))
                    })?);
                    K256Keypair::from_private_key(&bytes).map_err(|e| {
                        CoreError::Config(format!("PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX: {e}"))
                    })?
                }
                _ => K256Keypair::generate(),
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
                Ok(()) => tx.commit().await?,
                // Another node created the current key first; use theirs.
                Err(CoreError::Conflict(_)) => tx.rollback().await?,
                Err(e) => return Err(e),
            }
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

    /// The held keys a document lists, in document order.
    pub fn listed(&self, rotation_keys: &[String]) -> Vec<String> {
        rotation_keys
            .iter()
            .filter(|k| self.held.iter().any(|(id, _, _)| id == *k))
            .cloned()
            .collect()
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
    /// Blob bytes.
    pub blobs: locus_blob::DiskStore,
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
