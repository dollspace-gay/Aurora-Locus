//! Direct TLS: locus terminates TLS itself, for operators who run no proxy
//! (`LOCUS_TLS=true`). Certificates come from ACME (Let's Encrypt by
//! default) by HTTP-01, on demand: a TLS handshake for a name with no
//! certificate yet starts an order, if `/tls-check`'s rule approves the
//! name, and fails meanwhile (the client retries). The PDS hostname's is
//! ordered at start. Certificates and the ACME account live in the cache
//! directory; each is renewed 60 days after it was issued.
//!
//! Port 80 serves the HTTP-01 challenges and sends everything else to
//! HTTPS.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path as UrlPath, State};
use axum::http::{HeaderMap, Request, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, Identifier, NewAccount,
    NewOrder, OrderStatus, RetryPolicy,
};
use locus_core::bootstrap::DirectTls;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use tokio_util::sync::CancellationToken;

use crate::state::AppState;

/// A certificate is renewed this long after it was issued.
const RENEW_AFTER_MS: i64 = 60 * 86_400_000;
/// A name refused by the rule isn't asked about again for this long.
const REFUSED_FOR: Duration = Duration::from_secs(300);

/// The certificates and the ACME state.
pub struct Acme {
    state: Arc<AppState>,
    cfg: DirectTls,
    certs: RwLock<HashMap<String, (Arc<CertifiedKey>, i64)>>,
    pending: Mutex<HashSet<String>>,
    refused: Mutex<HashMap<String, Instant>>,
    /// HTTP-01: token → key authorization.
    challenges: RwLock<HashMap<String, String>>,
    account: tokio::sync::Mutex<Option<Account>>,
}

impl std::fmt::Debug for Acme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Acme")
            .field("cache", &self.cfg.cache)
            .finish_non_exhaustive()
    }
}

fn file_name(name: &str) -> Option<String> {
    let ok = !name.is_empty()
        && name.len() <= 253
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && !name.starts_with('.')
        && !name.contains("..");
    ok.then(|| name.to_owned())
}

/// A rustls key from PEM files' contents.
pub fn certified_key(chain_pem: &[u8], key_pem: &[u8]) -> Result<CertifiedKey, String> {
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(chain_pem)
        .collect::<Result<_, _>>()
        .map_err(|e| format!("certificate: {e}"))?;
    if chain.is_empty() {
        return Err("no certificate in the chain".into());
    }
    let key = PrivateKeyDer::from_pem_slice(key_pem).map_err(|e| format!("key: {e}"))?;
    let signing =
        rustls::crypto::ring::sign::any_supported_type(&key).map_err(|e| format!("key: {e}"))?;
    Ok(CertifiedKey::new(chain, signing))
}

impl Acme {
    /// Loads what the cache holds.
    pub fn new(state: Arc<AppState>, cfg: DirectTls) -> Arc<Self> {
        let me = Arc::new(Self {
            state,
            cfg,
            certs: RwLock::default(),
            pending: Mutex::default(),
            refused: Mutex::default(),
            challenges: RwLock::default(),
            account: tokio::sync::Mutex::new(None),
        });
        me.load_cache();
        me
    }

    fn cert_paths(&self, name: &str) -> Option<(PathBuf, PathBuf, PathBuf)> {
        let f = file_name(name)?;
        Some((
            self.cfg.cache.join(format!("{f}.pem")),
            self.cfg.cache.join(format!("{f}.key")),
            self.cfg.cache.join(format!("{f}.issued")),
        ))
    }

    fn load_cache(&self) {
        let Ok(dir) = std::fs::read_dir(&self.cfg.cache) else {
            return;
        };
        for entry in dir.flatten() {
            let path = entry.path();
            let Some(name) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".pem"))
            else {
                continue;
            };
            let Some((pem, key, issued)) = self.cert_paths(name) else {
                continue;
            };
            let (Ok(chain), Ok(k)) = (std::fs::read(&pem), std::fs::read(&key)) else {
                continue;
            };
            match certified_key(&chain, &k) {
                Ok(ck) => {
                    let at = std::fs::read_to_string(&issued)
                        .ok()
                        .and_then(|s| s.trim().parse().ok())
                        .unwrap_or(0);
                    self.certs
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(name.to_owned(), (Arc::new(ck), at));
                }
                Err(e) => tracing::warn!(name, "cached certificate unusable: {e}"),
            }
        }
    }

    /// Starts an order for `name` unless one is running or the name was
    /// just refused.
    pub fn request(self: &Arc<Self>, name: &str) {
        {
            let refused = self
                .refused
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if refused.get(name).is_some_and(|t| t.elapsed() < REFUSED_FOR) {
                return;
            }
        }
        if !self
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name.to_owned())
        {
            return;
        }
        let me = Arc::clone(self);
        let name = name.to_owned();
        tokio::spawn(async move {
            if !crate::health::approved(&me.state, &name).await {
                me.refused
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(name.clone(), Instant::now());
            } else {
                match me.issue(&name).await {
                    Ok(()) => tracing::info!(name, "certificate issued"),
                    Err(e) => {
                        tracing::warn!(name, "certificate order failed: {e}");
                        me.refused
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .insert(name.clone(), Instant::now());
                    }
                }
            }
            me.pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&name);
        });
    }

    fn builder(&self) -> Result<instant_acme::AccountBuilder, String> {
        match &self.cfg.directory_ca {
            Some(ca) => {
                Account::builder_with_root(ca).map_err(|e| format!("{}: {e}", ca.display()))
            }
            None => Account::builder().map_err(|e| e.to_string()),
        }
    }

    async fn account(&self) -> Result<Account, String> {
        let mut slot = self.account.lock().await;
        if let Some(a) = slot.as_ref() {
            return Ok(a.clone());
        }
        let creds_path = self.cfg.cache.join("acme-account.json");
        let account = if let Ok(text) = std::fs::read_to_string(&creds_path) {
            let creds: AccountCredentials = serde_json::from_str(&text)
                .map_err(|e| format!("{}: {e}", creds_path.display()))?;
            self.builder()?
                .from_credentials(creds)
                .await
                .map_err(|e| e.to_string())?
        } else {
            let contact = if self.cfg.email.is_empty() {
                Vec::new()
            } else {
                vec![format!("mailto:{}", self.cfg.email)]
            };
            let contact: Vec<&str> = contact.iter().map(String::as_str).collect();
            let (account, creds) = self
                .builder()?
                .create(
                    &NewAccount {
                        contact: &contact,
                        terms_of_service_agreed: true,
                        only_return_existing: false,
                    },
                    self.cfg.directory.clone(),
                    None,
                )
                .await
                .map_err(|e| e.to_string())?;
            let json = serde_json::to_string(&creds).map_err(|e| e.to_string())?;
            write_private(&creds_path, json.as_bytes()).map_err(|e| e.to_string())?;
            account
        };
        *slot = Some(account.clone());
        Ok(account)
    }

    /// Orders, proves and stores a certificate for `name`.
    async fn issue(&self, name: &str) -> Result<(), String> {
        let (pem_path, key_path, issued_path) = self.cert_paths(name).ok_or("not a hostname")?;
        let account = self.account().await?;
        let ids = [Identifier::Dns(name.to_owned())];
        let mut order = account
            .new_order(&NewOrder::new(&ids))
            .await
            .map_err(|e| e.to_string())?;
        let mut tokens = Vec::new();
        let mut authorizations = order.authorizations();
        while let Some(authz) = authorizations.next().await {
            let mut authz = authz.map_err(|e| e.to_string())?;
            if authz.status == AuthorizationStatus::Valid {
                continue;
            }
            let mut challenge = authz
                .challenge(ChallengeType::Http01)
                .ok_or("no http-01 challenge offered")?;
            let token = challenge.token.clone();
            let auth = challenge.key_authorization().as_str().to_owned();
            self.challenges
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(token.clone(), auth);
            tokens.push(token);
            challenge.set_ready().await.map_err(|e| e.to_string())?;
        }
        let result = async {
            let status = order
                .poll_ready(&RetryPolicy::default())
                .await
                .map_err(|e| e.to_string())?;
            if status != OrderStatus::Ready {
                return Err(format!("the order ended {status:?}"));
            }
            let key_pem = order.finalize().await.map_err(|e| e.to_string())?;
            let chain_pem = order
                .poll_certificate(&RetryPolicy::default())
                .await
                .map_err(|e| e.to_string())?;
            Ok((key_pem, chain_pem))
        }
        .await;
        {
            let mut c = self
                .challenges
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for t in &tokens {
                c.remove(t);
            }
        }
        let (key_pem, chain_pem) = result?;
        let ck = certified_key(chain_pem.as_bytes(), key_pem.as_bytes())?;
        let now = locus_core::time::now_ms();
        std::fs::create_dir_all(&self.cfg.cache).map_err(|e| e.to_string())?;
        write_private(&key_path, key_pem.as_bytes()).map_err(|e| e.to_string())?;
        std::fs::write(&pem_path, chain_pem.as_bytes()).map_err(|e| e.to_string())?;
        std::fs::write(&issued_path, now.to_string()).map_err(|e| e.to_string())?;
        self.certs
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name.to_owned(), (Arc::new(ck), now));
        Ok(())
    }

    /// The key authorization for an HTTP-01 token.
    #[must_use]
    pub fn challenge(&self, token: &str) -> Option<String> {
        self.challenges
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(token)
            .cloned()
    }

    /// Orders the PDS hostname's certificate if there's none, and renews
    /// any due, every 12 hours.
    pub async fn maintain(self: Arc<Self>, cancel: CancellationToken) {
        loop {
            let host = self.state.hostname.clone();
            if !self
                .certs
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(&host)
            {
                self.request(&host);
            }
            let due: Vec<String> = self
                .certs
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .filter(|(_, (_, at))| locus_core::time::now_ms() - at > RENEW_AFTER_MS)
                .map(|(n, _)| n.clone())
                .collect();
            for name in due {
                self.request(&name);
            }
            tokio::select! {
                () = cancel.cancelled() => return,
                () = tokio::time::sleep(Duration::from_secs(12 * 3600)) => {}
            }
        }
    }
}

/// rustls asks this for each handshake's certificate.
#[derive(Debug)]
pub struct Resolver(pub Arc<Acme>);

impl ResolvesServerCert for Resolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let name = hello.server_name()?.to_ascii_lowercase();
        let found = self
            .0
            .certs
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&name)
            .map(|(c, _)| Arc::clone(c));
        if found.is_none() {
            self.0.request(&name);
        }
        found
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)?.write_all(bytes)
}

// ---------------------------------------------------------------- port 80

async fn challenge(State(acme): State<Arc<Acme>>, UrlPath(token): UrlPath<String>) -> Response {
    match acme.challenge(&token) {
        Some(auth) => ([(header::CONTENT_TYPE, "text/plain")], auth).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn to_https(headers: HeaderMap, uri: Uri) -> Response {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|h| h.split(':').next().unwrap_or(h).to_owned())
        .unwrap_or_default();
    if host.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let path = uri.path_and_query().map_or("/", |p| p.as_str());
    let mut res = StatusCode::PERMANENT_REDIRECT.into_response();
    if let Ok(v) = format!("https://{host}{path}").parse() {
        res.headers_mut().insert(header::LOCATION, v);
    }
    res
}

/// Port 80: ACME challenges, and a redirect to HTTPS.
pub fn http_router(acme: Arc<Acme>) -> Router {
    Router::new()
        .route("/.well-known/acme-challenge/{token}", get(challenge))
        .fallback(to_https)
        .with_state(acme)
}

/// Serves port 80 until `cancel` fires.
pub async fn serve_http(
    acme: Arc<Acme>,
    listener: tokio::net::TcpListener,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    axum::serve(listener, http_router(acme))
        .with_graceful_shutdown(async move { cancel.cancelled().await })
        .await
}

// ---------------------------------------------------------------- port 443

/// A TLS listener: connections are accepted and their handshakes run
/// concurrently; finished ones are handed to the server.
pub struct TlsListener {
    rx: tokio::sync::mpsc::Receiver<(
        tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
        SocketAddr,
    )>,
    local: SocketAddr,
}

impl TlsListener {
    pub fn new(
        tcp: tokio::net::TcpListener,
        config: Arc<rustls::ServerConfig>,
        cancel: CancellationToken,
    ) -> std::io::Result<Self> {
        let local = tcp.local_addr()?;
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        let acceptor = tokio_rustls::TlsAcceptor::from(config);
        tokio::spawn(async move {
            loop {
                let (stream, addr) = tokio::select! {
                    () = cancel.cancelled() => return,
                    r = tcp.accept() => match r {
                        Ok(v) => v,
                        Err(e) => {
                            tracing::warn!("TLS accept failed: {e}");
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            continue;
                        }
                    },
                };
                let (acceptor, tx) = (acceptor.clone(), tx.clone());
                tokio::spawn(async move {
                    match tokio::time::timeout(Duration::from_secs(10), acceptor.accept(stream))
                        .await
                    {
                        Ok(Ok(tls)) => {
                            let _ = tx.send((tls, addr)).await;
                        }
                        Ok(Err(e)) => tracing::debug!(%addr, "TLS handshake failed: {e}"),
                        Err(_) => tracing::debug!(%addr, "TLS handshake timed out"),
                    }
                });
            }
        });
        Ok(Self { rx, local })
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<tokio::net::TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            if let Some(v) = self.rx.recv().await {
                return v;
            }
            // The accept loop ended (shutting down): wait to be dropped.
            std::future::pending::<()>().await;
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.local)
    }
}

/// The TLS listener's connect info: the peer.
#[derive(Clone, Copy, Debug)]
pub struct TlsPeer(pub SocketAddr);

impl axum::extract::connect_info::Connected<axum::serve::IncomingStream<'_, TlsListener>>
    for TlsPeer
{
    fn connect_info(stream: axum::serve::IncomingStream<'_, TlsListener>) -> Self {
        Self(*stream.remote_addr())
    }
}

/// Serves `router` over TLS on `listen` until `cancel` fires.
pub async fn serve(
    state: Arc<AppState>,
    acme: Arc<Acme>,
    listen: SocketAddr,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(std::io::Error::other)?
    .with_no_client_auth()
    .with_cert_resolver(Arc::new(Resolver(Arc::clone(&acme))));
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    let tcp = tokio::net::TcpListener::bind(listen).await?;
    let listener = TlsListener::new(tcp, Arc::new(config), cancel.clone())?;
    // The rate limiter reads the usual connect info.
    let app = crate::router(state)
        .layer(axum::middleware::map_request(crate::socket::strip))
        .layer(axum::middleware::map_request(
            |mut req: Request<Body>| async move {
                if let Some(ConnectInfo(TlsPeer(addr))) =
                    req.extensions().get::<ConnectInfo<TlsPeer>>().copied()
                {
                    req.extensions_mut().insert(ConnectInfo(addr));
                }
                req
            },
        ));
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<TlsPeer>(),
    )
    .with_graceful_shutdown(async move { cancel.cancelled().await })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_names_are_hostnames_only() {
        assert_eq!(
            file_name("pds.example.com").as_deref(),
            Some("pds.example.com")
        );
        for bad in ["", "../etc/passwd", "a/b", ".hidden", "a..b", "x y"] {
            assert!(file_name(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn a_self_signed_pair_loads() {
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = rcgen::CertificateParams::new(vec!["pds.example.com".into()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        assert!(certified_key(cert.pem().as_bytes(), key.serialize_pem().as_bytes()).is_ok());
        assert!(certified_key(b"nothing", key.serialize_pem().as_bytes()).is_err());
    }
}
