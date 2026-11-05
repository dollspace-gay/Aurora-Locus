//! DID and handle resolution for the network: proto-blue's `DidResolver`
//! over the SSRF-safe fetcher and a database-backed cache shared by every
//! node, and handle lookups by DNS (`_atproto` TXT, over one shared
//! resolver) raced against proto-blue's HTTPS method, with a handle cache.

use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use futures::future::{self, Either};
use hickory_resolver::TokioResolver;
use hickory_resolver::config::{NameServerConfig, ResolverConfig as DnsConfig};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::proto::rr::RData;
use locus_store::Store;
use proto_blue_identity::IdentityError;
use proto_blue_identity::cache::DidCache;
use proto_blue_identity::did::DidResolver;
use proto_blue_identity::handle::HandleResolver;
use proto_blue_identity::types::{CacheResult, DidDocument};
use serde_json::{Value, json};

use crate::fetch::{FetchPolicy, SafeFetcher};
use proto_blue_common::fetch::{FetchError, FetchHandler, HttpRequest, HttpResponse};

/// Sends requests under the operator-configured PLC directory through the
/// operator client, and everything else through the user-influenced one.
struct RoutedFetcher {
    prefix: String,
    operator: SafeFetcher,
    user: Arc<SafeFetcher>,
}

#[async_trait::async_trait]
impl FetchHandler for RoutedFetcher {
    async fn fetch(&self, req: HttpRequest) -> Result<HttpResponse, FetchError> {
        if req.url.starts_with(&self.prefix) {
            self.operator.fetch(req).await
        } else {
            self.user.fetch(req).await
        }
    }
}

/// How long a resolved handle is trusted.
const HANDLE_POSITIVE_MS: i64 = 10 * 60 * 1000;
/// How long a handle that didn't resolve stays unresolved.
const HANDLE_NEGATIVE_MS: i64 = 60 * 1000;

/// What the resolvers are built from; rebuilt when any of it changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolverConfig {
    pub plc_url: String,
    pub timeout_ms: u64,
    pub did_stale_ms: i64,
    pub did_max_ms: i64,
    pub backup_nameservers: Vec<IpAddr>,
    pub ssrf_protection: bool,
}

pub struct Resolvers {
    config: ResolverConfig,
    did: DidResolver,
    https: HandleResolver,
    dns: Option<TokioResolver>,
    backups: Vec<TokioResolver>,
    timeout: Duration,
    store: Store,
}

impl Resolvers {
    pub fn new(store: Store, config: ResolverConfig) -> Result<Self, IdentityError> {
        let fetcher = Arc::new(
            SafeFetcher::new(FetchPolicy {
                ssrf_protection: config.ssrf_protection,
                strict: true,
            })
            .map_err(IdentityError::Fetch)?,
        );
        // did:plc documents come from the directory the operator configured,
        // which may be a local one on plain HTTP; everything else (did:web
        // hosts) is user-influenced.
        let did_fetcher: Arc<dyn FetchHandler> = Arc::new(RoutedFetcher {
            prefix: format!("{}/", config.plc_url.trim_end_matches('/')),
            operator: SafeFetcher::new(FetchPolicy {
                ssrf_protection: false,
                strict: false,
            })
            .map_err(IdentityError::Fetch)?,
            user: Arc::clone(&fetcher),
        });
        let cache: Arc<dyn DidCache> = Arc::new(DbDidCache {
            store: store.clone(),
            stale_ms: config.did_stale_ms,
            max_ms: config.did_max_ms,
        });
        let did = DidResolver::with_fetch_handler(
            Some(&config.plc_url),
            config.timeout_ms,
            Some(cache),
            did_fetcher,
        );
        // Without proto-blue's `dns` feature this resolver is HTTPS only; the
        // DNS method is ours, over one shared resolver.
        let https = HandleResolver::with_fetch_handler(config.timeout_ms, fetcher);
        let dns = match TokioResolver::builder_tokio().and_then(|b| b.build()) {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!("no system DNS resolver, handle DNS lookups use backups only: {e}");
                None
            }
        };
        let backups = config
            .backup_nameservers
            .iter()
            .filter_map(|ip| {
                let mut ns = NameServerConfig::udp_and_tcp(*ip);
                ns.trust_negative_responses = true;
                TokioResolver::builder_with_config(
                    DnsConfig::from_parts(None, vec![], vec![ns]),
                    TokioRuntimeProvider::default(),
                )
                .build()
                .map_err(|e| tracing::warn!(%ip, "backup nameserver ignored: {e}"))
                .ok()
            })
            .collect();
        Ok(Self {
            timeout: Duration::from_millis(config.timeout_ms),
            config,
            did,
            https,
            dns,
            backups,
            store,
        })
    }

    #[must_use]
    pub fn config(&self) -> &ResolverConfig {
        &self.config
    }

    /// A DID's document, from the cache when fresh enough.
    pub async fn resolve_did(
        &self,
        did: &str,
        force_refresh: bool,
    ) -> Result<Option<DidDocument>, IdentityError> {
        self.did.resolve(did, force_refresh).await
    }

    /// The DID a handle resolves to over the network (DNS or HTTPS), or None.
    pub async fn resolve_handle(&self, handle: &str) -> Option<String> {
        match self.store.cached_handle(handle).await {
            Ok(Some(c)) => {
                let ttl = if c.value.is_some() {
                    HANDLE_POSITIVE_MS
                } else {
                    HANDLE_NEGATIVE_MS
                };
                if c.age_ms < ttl {
                    return c.value;
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("handle cache read failed: {e}"),
        }
        let did = self.resolve_handle_uncached(handle).await;
        if let Err(e) = self.store.cache_handle(handle, did.as_deref()).await {
            tracing::warn!("handle cache write failed: {e}");
        }
        did
    }

    /// The network lookup, skipping the cache (handle changes use this, so a
    /// just-published record counts).
    pub async fn resolve_handle_uncached(&self, handle: &str) -> Option<String> {
        let dns = self.resolve_dns(handle);
        let https = async { self.https.resolve(handle).await.ok().flatten() };
        race_first_some(dns, https)
            .await
            .filter(|did| proto_blue_syntax::Did::new(did).is_ok())
    }

    async fn resolve_dns(&self, handle: &str) -> Option<String> {
        let name = format!("_atproto.{handle}.");
        if let Some(r) = &self.dns
            && let Some(did) = self.txt_did(r, &name).await
        {
            return Some(did);
        }
        for r in &self.backups {
            if let Some(did) = self.txt_did(r, &name).await {
                return Some(did);
            }
        }
        None
    }

    /// Exactly one `did=` TXT record, as the spec requires.
    async fn txt_did(&self, resolver: &TokioResolver, name: &str) -> Option<String> {
        let lookup = tokio::time::timeout(self.timeout, resolver.txt_lookup(name))
            .await
            .ok()?
            .ok()?;
        let mut found: Vec<String> = lookup
            .answers()
            .iter()
            .filter_map(|record| match &record.data {
                RData::TXT(txt) => txt.to_string().strip_prefix("did=").map(str::to_owned),
                _ => None,
            })
            .collect();
        (found.len() == 1).then(|| found.remove(0))
    }
}

async fn race_first_some<T>(
    a: impl Future<Output = Option<T>>,
    b: impl Future<Output = Option<T>>,
) -> Option<T> {
    let (a, b) = (Box::pin(a), Box::pin(b));
    match future::select(a, b).await {
        Either::Left((Some(v), _)) | Either::Right((Some(v), _)) => Some(v),
        Either::Left((None, other)) => other.await,
        Either::Right((None, other)) => other.await,
    }
}

/// DID documents cached in the database, shared by every node.
struct DbDidCache {
    store: Store,
    stale_ms: i64,
    max_ms: i64,
}

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

impl DidCache for DbDidCache {
    fn cache_did(&self, did: &str, doc: DidDocument) -> BoxFut<'_, ()> {
        let did = did.to_owned();
        Box::pin(async move {
            if let Err(e) = self
                .store
                .cache_did_doc(&did, &doc_to_json(&doc).to_string())
                .await
            {
                tracing::warn!("DID cache write failed: {e}");
            }
        })
    }

    fn check_cache(&self, did: &str) -> BoxFut<'_, Option<CacheResult>> {
        let did = did.to_owned();
        Box::pin(async move {
            let cached = match self.store.cached_did_doc(&did).await {
                Ok(c) => c?,
                Err(e) => {
                    tracing::warn!("DID cache read failed: {e}");
                    return None;
                }
            };
            let doc: DidDocument = serde_json::from_str(&cached.value).ok()?;
            Some(CacheResult {
                did,
                doc,
                updated_at: 0,
                stale: cached.age_ms > self.stale_ms,
                expired: cached.age_ms > self.max_ms,
            })
        })
    }

    fn clear_entry(&self, did: &str) -> BoxFut<'_, ()> {
        let did = did.to_owned();
        Box::pin(async move {
            if let Err(e) = self.store.clear_did_doc(&did).await {
                tracing::warn!("DID cache delete failed: {e}");
            }
        })
    }

    fn clear(&self) -> BoxFut<'_, ()> {
        // Never used by the resolver; entries age out.
        Box::pin(async {})
    }
}

/// A resolved DID document as JSON (proto-blue's type only deserializes).
#[must_use]
pub fn doc_to_json(doc: &DidDocument) -> Value {
    json!({
        "@context": [
            "https://www.w3.org/ns/did/v1",
            "https://w3id.org/security/multikey/v1",
            "https://w3id.org/security/suites/secp256k1-2019/v1"
        ],
        "id": doc.id,
        "alsoKnownAs": doc.also_known_as,
        "verificationMethod": doc.verification_method.iter().map(|vm| {
            let mut v = json!({"id": vm.id, "type": vm.method_type, "controller": vm.controller});
            if let Some(k) = &vm.public_key_multibase {
                v["publicKeyMultibase"] = json!(k);
            }
            v
        }).collect::<Vec<_>>(),
        "service": doc.service.iter().map(|s| json!({
            "id": s.id, "type": s.service_type, "serviceEndpoint": s.service_endpoint
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn race_prefers_the_first_answer_and_waits_on_a_miss() {
        let slow = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Some(2)
        };
        assert_eq!(race_first_some(async { Some(1) }, slow).await, Some(1));
        let slow = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            Some(2)
        };
        assert_eq!(race_first_some(async { None }, slow).await, Some(2));
        assert_eq!(
            race_first_some(async { None::<u8> }, async { None }).await,
            None
        );
    }

    #[test]
    fn documents_round_trip_through_the_cache_form() {
        let raw = json!({
            "id": "did:plc:abc",
            "alsoKnownAs": ["at://alice.test"],
            "verificationMethod": [{"id": "did:plc:abc#atproto", "type": "Multikey",
                "controller": "did:plc:abc", "publicKeyMultibase": "zQ3sh"}],
            "service": [{"id": "#atproto_pds", "type": "AtprotoPersonalDataServer",
                "serviceEndpoint": "https://pds.test"}]
        });
        let doc: DidDocument = serde_json::from_value(raw.clone()).unwrap();
        let back = doc_to_json(&doc);
        for k in ["id", "alsoKnownAs", "verificationMethod", "service"] {
            assert_eq!(back[k], raw[k], "{k}");
        }
    }
}
