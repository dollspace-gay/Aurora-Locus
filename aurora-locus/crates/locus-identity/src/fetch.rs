//! The SSRF-safe HTTP client for URLs a user or remote party can influence
//! (DID documents, did:web hosts, handle HTTPS lookups, and later OAuth
//! client metadata and proxy targets).
//!
//! It follows the reference's `safeFetchWrap`:
//! - with SSRF protection on: HTTPS only, and every address a name resolves
//!   to must be public unicast (checked at connect time, so a DNS answer
//!   can't be swapped in after the check), as must an IP-literal host;
//! - strict fetches (documents and handle lookups) also refuse IP-literal
//!   hosts and the reference's forbidden domains in every mode, and a
//!   non-default port while protection is on;
//! - never follows redirects, caps responses at 512 KB, and times out.
//!
//! With protection off (the reference's `PDS_DISABLE_SSRF_PROTECTION` or a
//! local dev mode) plain HTTP, other ports and private addresses work.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use proto_blue_common::fetch::{
    FetchError, FetchHandler, HttpHeaders, HttpMethod, HttpRequest, HttpResponse,
};
use reqwest::Url;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// Largest response read, as the reference.
pub const MAX_RESPONSE_BYTES: usize = 512 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);

/// The reference's forbidden domains: never atproto services.
const FORBIDDEN_DOMAINS: &[&str] = &[
    "example.com",
    "example.org",
    "example.net",
    "googleusercontent.com",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchPolicy {
    /// The reference's `ssrfProtection`.
    pub ssrf_protection: bool,
    /// Documents and handle lookups: no IP-literal hosts, no forbidden
    /// domains, and (with protection on) no non-default ports.
    pub strict: bool,
}

/// The user-influenced client.
#[derive(Clone)]
pub struct SafeFetcher {
    http: reqwest::Client,
    policy: FetchPolicy,
}

impl SafeFetcher {
    pub fn new(policy: FetchPolicy) -> Result<Self, FetchError> {
        Self::build(policy, UnicastResolver { lookup: None })
    }

    fn build(policy: FetchPolicy, resolver: UnicastResolver) -> Result<Self, FetchError> {
        let mut builder = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .connect_timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("aurora-locus/", env!("CARGO_PKG_VERSION")));
        if policy.ssrf_protection {
            // A proxy would resolve names itself, skipping the resolver
            // below, so the environment's proxy settings are not used.
            builder = builder.no_proxy().dns_resolver(Arc::new(resolver));
        }
        let http = builder
            .build()
            .map_err(|e| FetchError::Other(e.to_string()))?;
        Ok(Self { http, policy })
    }

    #[must_use]
    pub fn policy(&self) -> FetchPolicy {
        self.policy
    }

    /// Checks a URL against the policy before any connection.
    pub fn check_url(&self, url: &Url) -> Result<(), String> {
        check_url(url, self.policy)
    }
}

/// A client for streamed requests to targets a user can influence (proxy
/// targets): no redirects, no overall timeout (the caller bounds the wait
/// for headers and for each body chunk), and with SSRF protection on, the
/// unicast-only resolver. `http2` allows HTTP/2 by ALPN; otherwise HTTP/1.1
/// only. Check each URL with [`check_url`] (non-strict) first.
pub fn streaming_client(ssrf_protection: bool, http2: bool) -> Result<reqwest::Client, FetchError> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("aurora-locus/", env!("CARGO_PKG_VERSION")));
    if ssrf_protection {
        // As in `SafeFetcher::build`: no environment proxy, which would
        // bypass the resolver.
        builder = builder
            .no_proxy()
            .dns_resolver(Arc::new(UnicastResolver { lookup: None }));
    }
    if !http2 {
        builder = builder.http1_only();
    }
    builder
        .build()
        .map_err(|e| FetchError::Other(e.to_string()))
}

/// The URL policy (applied before connecting; the resolver covers names).
pub fn check_url(url: &Url, policy: FetchPolicy) -> Result<(), String> {
    match url.scheme() {
        "https" => {}
        "http" if !policy.ssrf_protection => {}
        other => return Err(format!("{other}: URLs are not allowed")),
    }
    let host = url.host().ok_or("URL has no host")?;
    let ip = match host {
        url::Host::Ipv4(a) => Some(IpAddr::V4(a)),
        url::Host::Ipv6(a) => Some(IpAddr::V6(a)),
        url::Host::Domain(_) => None,
    };
    if policy.strict {
        if ip.is_some() {
            return Err("IP address hosts are not allowed".into());
        }
        if policy.ssrf_protection && url.port().is_some() {
            return Err("custom ports are not allowed".into());
        }
        if let Some(name) = url.host_str() {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            if FORBIDDEN_DOMAINS
                .iter()
                .any(|d| name == *d || name.ends_with(&format!(".{d}")))
            {
                return Err("forbidden domain name".into());
            }
        }
    }
    if policy.ssrf_protection
        && let Some(ip) = ip
        && !is_public_unicast(ip)
    {
        return Err("Hostname is a non-unicast address".into());
    }
    Ok(())
}

/// Whether an address is public unicast (the reference's ipaddr.js
/// `range() === 'unicast'`, with IPv4-mapped addresses checked as IPv4).
#[must_use]
pub fn is_public_unicast(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => is_public_v4(a),
        IpAddr::V6(a) => match a.to_ipv4_mapped() {
            Some(v4) => is_public_v4(v4),
            None => is_public_v6(a),
        },
    }
}

fn is_public_v4(a: Ipv4Addr) -> bool {
    let [o0, o1, o2, _] = a.octets();
    let special = match (o0, o1, o2) {
        (0 | 10 | 127, _, _) => true, // "this network", private, loopback
        (100, 64..=127, _) => true,   // carrier-grade NAT
        (169, 254, _) => true,        // link-local
        (172, 16..=31, _) => true,    // private
        (192, 0, 0 | 2) => true,      // IETF protocol assignments, documentation
        (192, 31, 196) | (192, 175, 48) => true, // AS112
        (192, 52, 193) => true,       // AMT
        (192, 88, 99) => true,        // 6to4 relay
        (192, 168, _) => true,        // private
        (198, 18 | 19, _) => true,    // benchmarking
        (198, 51, 100) | (203, 0, 113) => true, // documentation
        (224..=255, _, _) => true,    // multicast, reserved, broadcast
        _ => false,
    };
    !special
}

fn is_public_v6(a: Ipv6Addr) -> bool {
    let s = a.segments();
    // Only global unicast (2000::/3) can be public.
    if s[0] & 0xe000 != 0x2000 {
        return false;
    }
    let special = match (s[0], s[1], s[2]) {
        (0x2001, 0..=0x01ff, _) => true, // IETF protocol assignments (Teredo, benchmarking, ORCHID)
        (0x2001, 0x0db8, _) => true,     // documentation
        (0x2002, _, _) => true,          // 6to4
        (0x2620, 0x004f, 0x8000) => true, // AS112
        (0x3fff, _, _) => true,          // documentation (RFC 9637)
        _ => false,
    };
    !special
}

/// A name lookup standing in for the system resolver (tests).
type Lookup = dyn Fn(&str) -> futures::future::BoxFuture<'static, std::io::Result<Vec<SocketAddr>>>
    + Send
    + Sync;

/// Resolves (with the system resolver unless `lookup` is set) and refuses
/// any name that resolves to a non-public address, or that is a
/// local-network name. Every connection resolves anew, so a name that
/// answered publicly once and privately later is refused then.
struct UnicastResolver {
    lookup: Option<Arc<Lookup>>,
}

impl Resolve for UnicastResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().trim_end_matches('.').to_ascii_lowercase();
        let lookup = self.lookup.clone();
        Box::pin(async move {
            if is_local_name(&host) {
                return Err("Hostname is not a public domain".into());
            }
            let addrs: Vec<SocketAddr> = match lookup {
                Some(f) => f(&host).await?,
                None => tokio::net::lookup_host((host.as_str(), 0)).await?.collect(),
            };
            if addrs.is_empty() {
                return Err(format!("{host} did not resolve").into());
            }
            if addrs.iter().any(|a| !is_public_unicast(a.ip())) {
                return Err("Hostname resolved to non-unicast address".into());
            }
            let iter: Addrs = Box::new(addrs.into_iter());
            Ok(iter)
        })
    }
}

/// Names used on local networks (the reference's heuristic).
fn is_local_name(host: &str) -> bool {
    let mut parts = host.rsplit('.');
    let tld = parts.next().unwrap_or("");
    parts.next().is_none() || matches!(tld, "test" | "local" | "localhost" | "invalid" | "example")
}

#[async_trait]
impl FetchHandler for SafeFetcher {
    async fn fetch(&self, req: HttpRequest) -> Result<HttpResponse, FetchError> {
        let url = Url::parse(&req.url).map_err(|e| FetchError::InvalidUrl(e.to_string()))?;
        self.check_url(&url).map_err(FetchError::InvalidUrl)?;
        let method = match req.method {
            HttpMethod::Get => reqwest::Method::GET,
            HttpMethod::Post => reqwest::Method::POST,
            HttpMethod::Put => reqwest::Method::PUT,
            HttpMethod::Delete => reqwest::Method::DELETE,
            HttpMethod::Patch => reqwest::Method::PATCH,
            HttpMethod::Head => reqwest::Method::HEAD,
            HttpMethod::Options => reqwest::Method::OPTIONS,
        };
        let mut builder = self.http.request(method, url);
        for (k, v) in &req.headers {
            builder = builder.header(k, v);
        }
        if let Some(body) = req.body {
            builder = builder.body(body);
        }
        let mut res = builder.send().await.map_err(|e| {
            if e.is_timeout() {
                FetchError::Timeout
            } else {
                FetchError::Network(e.to_string())
            }
        })?;
        let status = res.status().as_u16();
        let mut headers = HttpHeaders::new();
        for (k, v) in res.headers() {
            if let Ok(v) = v.to_str() {
                headers.insert(k.as_str().to_ascii_lowercase(), v.to_owned());
            }
        }
        if res
            .content_length()
            .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
        {
            return Err(FetchError::Body("response too large".into()));
        }
        let mut body = Vec::new();
        while let Some(chunk) = res
            .chunk()
            .await
            .map_err(|e| FetchError::Body(e.to_string()))?
        {
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(FetchError::Body("response too large".into()));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON: FetchPolicy = FetchPolicy {
        ssrf_protection: true,
        strict: true,
    };
    const ON_LOOSE: FetchPolicy = FetchPolicy {
        ssrf_protection: true,
        strict: false,
    };
    const OFF: FetchPolicy = FetchPolicy {
        ssrf_protection: false,
        strict: true,
    };
    const OFF_LOOSE: FetchPolicy = FetchPolicy {
        ssrf_protection: false,
        strict: false,
    };

    fn ok(url: &str, p: FetchPolicy) -> bool {
        check_url(&Url::parse(url).unwrap(), p).is_ok()
    }

    #[test]
    fn url_policy() {
        assert!(ok("https://pds.social/.well-known/did.json", ON));
        assert!(
            !ok("http://pds.social/", ON),
            "no plain http with protection on"
        );
        assert!(ok("http://pds.social/", OFF));
        assert!(!ok("file:///etc/passwd", OFF));
        assert!(
            !ok("https://pds.social:8443/", ON),
            "no custom port on strict fetches"
        );
        assert!(ok("https://pds.social:8443/", ON_LOOSE));
        assert!(ok("http://localhost:2583/", OFF));
        // IP hosts: never on strict fetches; public ones on loose fetches.
        assert!(!ok("https://1.1.1.1/", ON));
        assert!(
            !ok("http://127.0.0.1/", OFF),
            "strict refuses IP hosts in every mode"
        );
        assert!(ok("https://1.1.1.1/", ON_LOOSE));
        assert!(!ok("https://10.0.0.1/", ON_LOOSE));
        assert!(
            !ok("https://[::ffff:127.0.0.1]/", ON_LOOSE),
            "mapped addresses checked as IPv4"
        );
        assert!(ok("http://10.0.0.1/", OFF_LOOSE));
        // Forbidden domains in every mode.
        for p in [ON, OFF] {
            assert!(!ok("https://example.com/", p));
            assert!(!ok("https://alice.example.org/", p));
            assert!(!ok("https://x.googleusercontent.com/", p));
        }
        assert!(ok("https://notexample.com/", ON));
    }

    #[test]
    fn unicast_ranges() {
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "2606:4700::1111",
            "2001:4860:4860::8888",
        ] {
            assert!(is_public_unicast(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "0.0.0.0",
            "10.1.2.3",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.2.1",
            "192.168.1.1",
            "198.18.0.1",
            "203.0.113.5",
            "224.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "fe80::1",
            "fc00::1",
            "fd12::1",
            "ff02::1",
            "2001:db8::1",
            "2001::1",
            "2002::1",
            "::ffff:10.0.0.1",
            "64:ff9b::1.2.3.4",
        ] {
            assert!(!is_public_unicast(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn local_names() {
        assert!(is_local_name("localhost"));
        assert!(is_local_name("locus"));
        assert!(is_local_name("alice.test"));
        assert!(is_local_name("printer.local"));
        assert!(!is_local_name("pds.social"));
    }

    #[tokio::test]
    async fn the_resolver_refuses_names_that_resolve_privately() {
        let r = UnicastResolver { lookup: None };
        let err = r.resolve("localhost".parse().unwrap()).await.err().unwrap();
        assert!(err.to_string().contains("not a public domain"));
    }

    #[tokio::test]
    async fn redirects_are_not_followed_and_bodies_are_capped() {
        use test_server::serve;
        let (base, _guard) = serve().await;
        let f = SafeFetcher::new(OFF_LOOSE).unwrap();
        let res = f
            .fetch(HttpRequest::get(format!("{base}/redirect")))
            .await
            .unwrap();
        assert_eq!(res.status, 302, "the redirect comes back as-is");
        let err = f
            .fetch(HttpRequest::get(format!("{base}/big")))
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::Body(_)), "{err}");
        let res = f
            .fetch(HttpRequest::get(format!("{base}/small")))
            .await
            .unwrap();
        assert_eq!(res.body, b"did:plc:abc");
    }

    /// A listener that only counts the connections it gets.
    async fn counting_listener() -> (u16, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = Arc::clone(&hits);
        tokio::spawn(async move {
            while let Ok((sock, _)) = listener.accept().await {
                h.fetch_add(1, Ordering::SeqCst);
                drop(sock);
            }
        });
        (port, hits)
    }

    #[tokio::test]
    async fn a_name_that_rebinds_to_a_private_address_is_refused_at_connect_time() {
        use std::sync::Mutex;
        use std::sync::atomic::Ordering;
        let (port, hits) = counting_listener().await;
        // What the name answers, in turn: public first, then the loopback
        // address of the listener, then mapped, CGNAT, unique-local and a
        // mix of public and private.
        let answers: Arc<Mutex<Vec<Vec<IpAddr>>>> = Arc::new(Mutex::new(
            [
                vec!["1.1.1.1"],
                vec!["127.0.0.1"],
                vec!["::ffff:127.0.0.1"],
                vec!["100.64.0.1"],
                vec!["fd00::1"],
                vec!["1.1.1.1", "127.0.0.1"],
            ]
            .into_iter()
            .rev()
            .map(|a| a.iter().map(|ip| ip.parse().unwrap()).collect())
            .collect(),
        ));
        let lookups = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (a, n) = (Arc::clone(&answers), Arc::clone(&lookups));
        let lookup: Arc<Lookup> = Arc::new(move |_host: &str| {
            n.fetch_add(1, Ordering::SeqCst);
            let ips = a.lock().unwrap().pop().unwrap_or_default();
            Box::pin(async move { Ok(ips.into_iter().map(|ip| SocketAddr::new(ip, 0)).collect()) })
        });
        let resolver = UnicastResolver {
            lookup: Some(Arc::clone(&lookup)),
        };
        // The first answer is public: the check passes.
        assert!(
            resolver
                .resolve("rebind.pds.social".parse().unwrap())
                .await
                .is_ok()
        );
        // The client resolves again when it connects, and refuses the
        // private answer without connecting; so it does for every other
        // non-public answer.
        let f = SafeFetcher::build(
            ON_LOOSE,
            UnicastResolver {
                lookup: Some(lookup),
            },
        )
        .unwrap();
        for case in ["loopback", "mapped", "cgnat", "ula", "mixed"] {
            let err = f
                .fetch(HttpRequest::get(format!(
                    "https://rebind.pds.social:{port}/"
                )))
                .await
                .unwrap_err();
            assert!(matches!(err, FetchError::Network(_)), "{case}: {err}");
        }
        assert_eq!(
            lookups.load(Ordering::SeqCst),
            6,
            "every connection resolved anew"
        );
        assert_eq!(hits.load(Ordering::SeqCst), 0, "nothing connected");
    }

    /// The environment's proxy settings don't reach the safe clients: a
    /// proxy resolves names itself, which would skip the unicast-only
    /// resolver. The rebinding test runs again in a child process with the
    /// proxy variables set (setting them here would need `unsafe`).
    #[test]
    fn proxy_environment_does_not_bypass_the_resolver() {
        let name = format!(
            "{}::a_name_that_rebinds_to_a_private_address_is_refused_at_connect_time",
            module_path!().split_once("::").unwrap().1
        );
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([name.as_str(), "--exact"])
            .env("HTTPS_PROXY", "http://127.0.0.1:9")
            .env("https_proxy", "http://127.0.0.1:9")
            .env("HTTP_PROXY", "http://127.0.0.1:9")
            .env("http_proxy", "http://127.0.0.1:9")
            .env("ALL_PROXY", "http://127.0.0.1:9")
            .env("all_proxy", "http://127.0.0.1:9")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.contains("1 passed"),
            "{stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A tiny HTTP/1.1 server on a random port, enough for the tests above.
    mod test_server {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        pub async fn serve() -> (String, tokio::task::JoinHandle<()>) {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let task = tokio::spawn(async move {
                loop {
                    let Ok((mut sock, _)) = listener.accept().await else {
                        return;
                    };
                    tokio::spawn(async move {
                        let mut buf = vec![0u8; 4096];
                        let n = sock.read(&mut buf).await.unwrap_or(0);
                        let req = String::from_utf8_lossy(&buf[..n]);
                        let path = req.split_whitespace().nth(1).unwrap_or("/");
                        let resp = match path {
                            "/redirect" => "HTTP/1.1 302 Found\r\nlocation: http://127.0.0.1:1/\r\ncontent-length: 0\r\n\r\n".to_owned(),
                            "/big" => {
                                let body = "x".repeat(super::MAX_RESPONSE_BYTES + 1);
                                format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{body}", body.len())
                            }
                            _ => "HTTP/1.1 200 OK\r\ncontent-length: 11\r\n\r\ndid:plc:abc".to_owned(),
                        };
                        let _ = sock.write_all(resp.as_bytes()).await;
                    });
                }
            });
            (format!("http://{addr}"), task)
        }
    }
}
