//! Shared state for every handler, and the server secrets created on first
//! boot.

use std::sync::Arc;

use locus_auth::session::LegacySessions;
use locus_core::keys::MasterKey;
use locus_core::settings::Snapshot;
use locus_core::{CoreError, CoreResult};
use locus_identity::plc_client::PlcClient;
use locus_repo::RepoEngine;
use locus_store::Store;
use locus_store::runtime::Settings;
use proto_blue_crypto::{ExportableKeypair, K256Keypair, Keypair};
use zeroize::Zeroizing;

/// Names of the server's own secrets in the key store.
pub mod secret_names {
    pub const JWT: &str = "jwt_secret";
    pub const APP_PASSWORD_PEPPER: &str = "app_password_pepper";
    pub const PLC_ROTATION_KEY: &str = "plc_rotation_key";
    pub const SERVICE_KEY: &str = "service_signing_key";
}

/// The server's secrets, decrypted for this process.
pub struct ServerSecrets {
    pub jwt: Zeroizing<Vec<u8>>,
    pub app_password_pepper: Zeroizing<Vec<u8>>,
    pub plc_rotation_key: K256Keypair,
    pub service_key: K256Keypair,
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
        let rotation = load_or(store, key, secret_names::PLC_ROTATION_KEY, || {
            std::env::var("PDS_PLC_ROTATION_KEY_K256_PRIVATE_KEY_HEX")
                .ok()
                .and_then(|h| hex::decode(h.trim()).ok())
                .unwrap_or_else(|| K256Keypair::generate().export_private_key())
        })
        .await?;
        let service = load_or(store, key, secret_names::SERVICE_KEY, || {
            K256Keypair::generate().export_private_key()
        })
        .await?;
        let keypair = |bytes: &[u8], what: &str| {
            K256Keypair::from_private_key(bytes).map_err(|e| {
                CoreError::Config(format!("stored {what} is not a valid secp256k1 key: {e}"))
            })
        };
        Ok(Self {
            plc_rotation_key: keypair(&rotation, "PLC rotation key")?,
            service_key: keypair(&service, "service key")?,
            jwt,
            app_password_pepper,
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
    pub repo: RepoEngine,
    /// The PDS hostname (no scheme or port).
    pub hostname: String,
    /// `https://<hostname>`, or `http://localhost:<port>` on localhost.
    pub public_url: String,
    /// `did:web:<hostname>`.
    pub service_did: String,
}

impl AppState {
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

    pub fn service_did_key(&self) -> String {
        self.secrets.service_key.did()
    }
}
