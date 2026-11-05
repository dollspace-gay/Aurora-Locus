//! Bootstrap configuration: the few settings a node needs before it can reach
//! the database, or that can't change in a running process. Everything else is
//! a runtime setting stored in the database and changed live.
//!
//! Sources, lowest precedence first: built-in defaults, `locus.toml`, the
//! reference PDS's `PDS_*` variables (so an existing `pds.env` boots), then
//! `LOCUS_*` variables.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{CoreError, CoreResult};

/// The node's role in a deployment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// A standalone PDS.
    Pds,
    /// A PDS behind an entryway.
    Shard,
    /// The front door owning accounts, auth and handles for many shards.
    Entryway,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Pretty,
    Json,
}

/// Where the master key that encrypts stored secrets comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MasterKeySource {
    /// A key file. `shared` marks an operator-supplied file shared by every node.
    File { path: PathBuf, shared: bool },
    /// Hex in `LOCUS_MASTER_KEY`.
    Env,
    /// A KMS key id.
    Kms { key_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatabaseUrl {
    Sqlite(PathBuf),
    Postgres(String),
}

impl DatabaseUrl {
    pub fn parse(url: &str) -> CoreResult<Self> {
        if let Some(rest) = url.strip_prefix("sqlite://") {
            Ok(Self::Sqlite(PathBuf::from(rest)))
        } else if let Some(rest) = url.strip_prefix("sqlite:") {
            Ok(Self::Sqlite(PathBuf::from(rest)))
        } else if url.starts_with("postgres://") || url.starts_with("postgresql://") {
            Ok(Self::Postgres(url.to_owned()))
        } else {
            Err(CoreError::Config(format!(
                "database url must start with sqlite:// or postgres://, got {url:?}"
            )))
        }
    }
}

#[derive(Debug, Clone)]
pub struct Bootstrap {
    pub database: DatabaseUrl,
    pub listen: SocketAddr,
    pub admin_socket: Option<PathBuf>,
    pub data_dir: PathBuf,
    /// Whether the data dir survives restarts. A non-persistent data dir can't
    /// hold a generated master key or node id.
    pub data_persistent: bool,
    pub master_key: MasterKeySource,
    pub node_id: Option<String>,
    pub log_format: LogFormat,
    pub redis_url: Option<String>,
    pub role: Role,
    /// Initial public hostname. After setup the hostname lives in the database.
    pub hostname: Option<String>,
    /// `PDS_DEV_MODE` as configured. See [`Bootstrap::dev_mode_honored`].
    pub dev_mode: bool,
    /// `PDS_DISABLE_SSRF_PROTECTION` as configured (unset, true or false).
    pub disable_ssrf_protection: Option<bool>,
    /// Read once in setup mode to complete headless setup.
    pub setup_auto: Option<String>,
    /// Direct TLS: locus terminates TLS itself, with ACME certificates.
    pub tls: Option<DirectTls>,
}

/// Direct TLS (`LOCUS_TLS=true`): no proxy in front.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectTls {
    /// HTTPS (`LOCUS_TLS_LISTEN`, default `0.0.0.0:443`).
    pub listen: SocketAddr,
    /// Plain HTTP for ACME's HTTP-01 challenges and redirects
    /// (`LOCUS_TLS_HTTP_LISTEN`, default `0.0.0.0:80`).
    pub http_listen: SocketAddr,
    /// The ACME directory (`LOCUS_ACME_DIRECTORY`, default Let's Encrypt).
    pub directory: String,
    /// The ACME account contact (`LOCUS_ACME_EMAIL`).
    pub email: String,
    /// Where certificates and the ACME account are kept
    /// (`LOCUS_TLS_CACHE`, default `<data>/tls`).
    pub cache: PathBuf,
    /// A CA (PEM) the ACME directory's certificate is checked against
    /// instead of the system's (`LOCUS_ACME_CA`; a private or test ACME).
    pub directory_ca: Option<PathBuf>,
}

/// Let's Encrypt's production directory.
pub const LETS_ENCRYPT: &str = "https://acme-v02.api.letsencrypt.org/directory";

/// The optional `locus.toml` file. Every field is optional; env overrides it.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    database_url: Option<String>,
    listen: Option<String>,
    admin_socket: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    data_persistent: Option<bool>,
    master_key_file: Option<PathBuf>,
    master_key_shared_file: Option<bool>,
    master_key_source: Option<String>,
    master_key_kms_id: Option<String>,
    node_id: Option<String>,
    log_format: Option<LogFormat>,
    redis_url: Option<String>,
    role: Option<Role>,
    hostname: Option<String>,
    dev_mode: Option<bool>,
    disable_ssrf_protection: Option<bool>,
    tls: Option<bool>,
    tls_listen: Option<String>,
    tls_http_listen: Option<String>,
    acme_directory: Option<String>,
    acme_email: Option<String>,
    tls_cache: Option<PathBuf>,
}

const DEFAULT_PORT: u16 = 2583;

impl Bootstrap {
    /// Loads bootstrap config from the process environment and, if present,
    /// the file named by `LOCUS_CONFIG` (default `./locus.toml`).
    pub fn from_env() -> CoreResult<Self> {
        let env: HashMap<String, String> = std::env::vars().collect();
        let file = env
            .get("LOCUS_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("locus.toml"));
        let text = match std::fs::read_to_string(&file) {
            Ok(t) => Some(t),
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    && !env.contains_key("LOCUS_CONFIG") =>
            {
                None
            }
            Err(e) => {
                return Err(CoreError::Config(format!(
                    "can't read {}: {e}",
                    file.display()
                )));
            }
        };
        Self::load(text.as_deref(), &env)
    }

    /// Builds bootstrap config from an optional TOML document and an env map.
    pub fn load(file: Option<&str>, env: &HashMap<String, String>) -> CoreResult<Self> {
        let file: FileConfig = match file {
            Some(text) => {
                toml::from_str(text).map_err(|e| CoreError::Config(format!("locus.toml: {e}")))?
            }
            None => FileConfig::default(),
        };
        let get = |locus: &str, pds: Option<&str>| -> Option<String> {
            env.get(locus)
                .or_else(|| pds.and_then(|p| env.get(p)))
                .filter(|v| !v.is_empty())
                .cloned()
        };

        let data_dir = get("LOCUS_DATA_DIR", Some("PDS_DATA_DIRECTORY"))
            .map(PathBuf::from)
            .or(file.data_dir)
            .unwrap_or_else(|| PathBuf::from("./data"));

        let database = match get("LOCUS_DATABASE_URL", None).or(file.database_url) {
            Some(url) => DatabaseUrl::parse(&url)?,
            None => DatabaseUrl::Sqlite(data_dir.join("locus.sqlite")),
        };

        let listen = match get("LOCUS_LISTEN", None).or(file.listen) {
            Some(addr) => addr
                .parse()
                .map_err(|e| CoreError::Config(format!("listen address {addr:?}: {e}")))?,
            None => {
                let port = match get("LOCUS_PORT", Some("PDS_PORT")) {
                    Some(p) => p
                        .parse()
                        .map_err(|e| CoreError::Config(format!("port {p:?}: {e}")))?,
                    None => DEFAULT_PORT,
                };
                SocketAddr::from(([0, 0, 0, 0], port))
            }
        };

        let tls_on = match get("LOCUS_TLS", None) {
            Some(v) => parse_bool("LOCUS_TLS", &v)?,
            None => file.tls.unwrap_or(false),
        };
        let addr = |name: &str, v: Option<String>, default: u16| -> CoreResult<SocketAddr> {
            match v {
                Some(a) => a
                    .parse()
                    .map_err(|e| CoreError::Config(format!("{name} {a:?}: {e}"))),
                None => Ok(SocketAddr::from(([0, 0, 0, 0], default))),
            }
        };
        let tls = if tls_on {
            let email = get("LOCUS_ACME_EMAIL", None)
                .or(file.acme_email)
                .unwrap_or_default();
            Some(DirectTls {
                listen: addr(
                    "LOCUS_TLS_LISTEN",
                    get("LOCUS_TLS_LISTEN", None).or(file.tls_listen),
                    443,
                )?,
                http_listen: addr(
                    "LOCUS_TLS_HTTP_LISTEN",
                    get("LOCUS_TLS_HTTP_LISTEN", None).or(file.tls_http_listen),
                    80,
                )?,
                directory: get("LOCUS_ACME_DIRECTORY", None)
                    .or(file.acme_directory)
                    .unwrap_or_else(|| LETS_ENCRYPT.to_owned()),
                email,
                cache: get("LOCUS_TLS_CACHE", None)
                    .map(PathBuf::from)
                    .or(file.tls_cache)
                    .unwrap_or_else(|| data_dir.join("tls")),
                directory_ca: get("LOCUS_ACME_CA", None).map(PathBuf::from),
            })
        } else {
            None
        };

        let data_persistent = match get("LOCUS_DATA_PERSISTENT", None) {
            Some(v) => parse_bool("LOCUS_DATA_PERSISTENT", &v)?,
            None => file.data_persistent.unwrap_or(true),
        };

        let master_key = match get("LOCUS_MASTER_KEY_SOURCE", None)
            .or(file.master_key_source)
            .as_deref()
        {
            None | Some("file") => MasterKeySource::File {
                path: get("LOCUS_MASTER_KEY_FILE", None)
                    .map(PathBuf::from)
                    .or(file.master_key_file)
                    .unwrap_or_else(|| data_dir.join("master.key")),
                shared: match get("LOCUS_MASTER_KEY_SHARED_FILE", None) {
                    Some(v) => parse_bool("LOCUS_MASTER_KEY_SHARED_FILE", &v)?,
                    None => file.master_key_shared_file.unwrap_or(false),
                },
            },
            Some("env") => MasterKeySource::Env,
            Some("kms") => MasterKeySource::Kms {
                key_id: get("LOCUS_MASTER_KEY_KMS_ID", None)
                    .or(file.master_key_kms_id)
                    .ok_or_else(|| {
                        CoreError::Config(
                            "master key source kms needs LOCUS_MASTER_KEY_KMS_ID".into(),
                        )
                    })?,
            },
            Some(other) => {
                return Err(CoreError::Config(format!(
                    "master key source must be file, env or kms, got {other:?}"
                )));
            }
        };

        let role = match get("LOCUS_ROLE", None) {
            Some(r) => match r.as_str() {
                "pds" => Role::Pds,
                "shard" => Role::Shard,
                "entryway" => Role::Entryway,
                other => {
                    return Err(CoreError::Config(format!(
                        "role must be pds, shard or entryway, got {other:?}"
                    )));
                }
            },
            None => file.role.unwrap_or(Role::Pds),
        };

        let log_format = match get("LOCUS_LOG_FORMAT", None).as_deref() {
            Some("json") => LogFormat::Json,
            Some("pretty") => LogFormat::Pretty,
            Some(other) => {
                return Err(CoreError::Config(format!(
                    "log format must be json or pretty, got {other:?}"
                )));
            }
            None => file.log_format.unwrap_or(LogFormat::Pretty),
        };

        let dev_mode = match get("LOCUS_DEV_MODE", Some("PDS_DEV_MODE")) {
            Some(v) => parse_bool("PDS_DEV_MODE", &v)?,
            None => file.dev_mode.unwrap_or(false),
        };
        let disable_ssrf_protection = match get(
            "LOCUS_DISABLE_SSRF_PROTECTION",
            Some("PDS_DISABLE_SSRF_PROTECTION"),
        ) {
            Some(v) => Some(parse_bool("PDS_DISABLE_SSRF_PROTECTION", &v)?),
            None => file.disable_ssrf_protection,
        };

        Ok(Self {
            database,
            listen,
            admin_socket: get("LOCUS_ADMIN_SOCKET", None)
                .map(PathBuf::from)
                .or(file.admin_socket)
                .or_else(|| Some(data_dir.join("admin.sock"))),
            data_persistent,
            master_key,
            node_id: get("LOCUS_NODE_ID", None).or(file.node_id),
            log_format,
            redis_url: get("LOCUS_REDIS_URL", Some("PDS_REDIS_SCRATCH_ADDRESS")).or(file.redis_url),
            role,
            hostname: get("LOCUS_HOSTNAME", Some("PDS_HOSTNAME"))
                .or(file.hostname)
                .map(|h| h.to_ascii_lowercase()),
            dev_mode,
            disable_ssrf_protection,
            setup_auto: get("LOCUS_SETUP_AUTO", None),
            tls,
            data_dir,
        })
    }

    /// Dev mode counts only on a local hostname (`localhost`, `*.localhost`
    /// or a loopback IP). On a public hostname it's ignored with a warning,
    /// because it turns off checks users rely on.
    pub fn dev_mode_honored(&self) -> bool {
        self.dev_mode && self.hostname.as_deref().is_none_or(is_local_hostname)
    }

    /// Whether the user-influenced HTTP client drops its SSRF rules. Same rule
    /// as the reference PDS: the explicit flag if set, else dev mode (as
    /// honored), else off.
    pub fn ssrf_protection_disabled(&self) -> bool {
        self.disable_ssrf_protection
            .unwrap_or_else(|| self.dev_mode_honored())
    }

    /// Path of the persisted node id, when the data dir is persistent.
    pub fn node_id_path(&self) -> Option<PathBuf> {
        self.data_persistent.then(|| self.data_dir.join("node-id"))
    }
}

fn parse_bool(name: &str, value: &str) -> CoreResult<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(CoreError::Config(format!(
            "{name} must be true or false, got {value:?}"
        ))),
    }
}

/// `localhost`, a `*.localhost` name, or a loopback IP literal.
pub fn is_local_hostname(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    bare.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// Reads or creates the persisted node id.
pub fn load_or_create_node_id(path: &Path) -> CoreResult<String> {
    match std::fs::read_to_string(path) {
        Ok(id) if !id.trim().is_empty() => Ok(id.trim().to_owned()),
        Ok(_) | Err(_) => {
            let id = random_node_id();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    CoreError::Config(format!("can't create {}: {e}", parent.display()))
                })?;
            }
            std::fs::write(path, &id)
                .map_err(|e| CoreError::Config(format!("can't write {}: {e}", path.display())))?;
            Ok(id)
        }
    }
}

/// A random node id: `node-` and 16 hex characters from the OS RNG.
pub fn random_node_id() -> String {
    let bytes: [u8; 8] = rand::random();
    format!("node-{}", hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect()
    }

    #[test]
    fn defaults_to_sqlite_in_data_dir() {
        let b = Bootstrap::load(None, &env(&[("LOCUS_DATA_DIR", "/tmp/x")])).unwrap();
        assert_eq!(
            b.database,
            DatabaseUrl::Sqlite(PathBuf::from("/tmp/x/locus.sqlite"))
        );
        assert_eq!(b.listen.port(), DEFAULT_PORT);
        assert_eq!(b.role, Role::Pds);
    }

    #[test]
    fn reference_env_names_are_accepted() {
        let b = Bootstrap::load(
            None,
            &env(&[
                ("PDS_PORT", "3000"),
                ("PDS_HOSTNAME", "PDS.Example.com"),
                ("PDS_DATA_DIRECTORY", "/pds"),
            ]),
        )
        .unwrap();
        assert_eq!(b.listen.port(), 3000);
        assert_eq!(b.hostname.as_deref(), Some("pds.example.com"));
        assert_eq!(b.data_dir, PathBuf::from("/pds"));
    }

    #[test]
    fn locus_env_beats_reference_env_beats_file() {
        let file = "hostname = \"file.example\"\n";
        let b = Bootstrap::load(Some(file), &env(&[("PDS_HOSTNAME", "pds.example")])).unwrap();
        assert_eq!(b.hostname.as_deref(), Some("pds.example"));
        let b = Bootstrap::load(
            Some(file),
            &env(&[
                ("PDS_HOSTNAME", "pds.example"),
                ("LOCUS_HOSTNAME", "locus.example"),
            ]),
        )
        .unwrap();
        assert_eq!(b.hostname.as_deref(), Some("locus.example"));
    }

    #[test]
    fn ssrf_rule_matches_reference() {
        let local = |pairs: &[(&str, &str)]| {
            let mut p = vec![("PDS_HOSTNAME", "localhost")];
            p.extend_from_slice(pairs);
            Bootstrap::load(None, &env(&p)).unwrap()
        };
        assert!(!local(&[]).ssrf_protection_disabled());
        assert!(local(&[("PDS_DEV_MODE", "true")]).ssrf_protection_disabled());
        assert!(
            !local(&[
                ("PDS_DEV_MODE", "true"),
                ("PDS_DISABLE_SSRF_PROTECTION", "false")
            ])
            .ssrf_protection_disabled()
        );
        // Dev mode on a public hostname is ignored, but the explicit flag isn't.
        let public = Bootstrap::load(
            None,
            &env(&[
                ("PDS_HOSTNAME", "pds.example.com"),
                ("PDS_DEV_MODE", "true"),
            ]),
        )
        .unwrap();
        assert!(!public.dev_mode_honored());
        assert!(!public.ssrf_protection_disabled());
        let public = Bootstrap::load(
            None,
            &env(&[
                ("PDS_HOSTNAME", "pds.example.com"),
                ("PDS_DISABLE_SSRF_PROTECTION", "true"),
            ]),
        )
        .unwrap();
        assert!(public.ssrf_protection_disabled());
    }

    #[test]
    fn local_hostnames() {
        assert!(is_local_hostname("localhost"));
        assert!(is_local_hostname("pds.localhost"));
        assert!(is_local_hostname("127.0.0.1"));
        assert!(is_local_hostname("[::1]"));
        assert!(!is_local_hostname("example.com"));
        assert!(!is_local_hostname("10.0.0.1"));
    }

    #[test]
    fn node_id_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node-id");
        let a = load_or_create_node_id(&path).unwrap();
        let b = load_or_create_node_id(&path).unwrap();
        assert_eq!(a, b);
        assert!(a.starts_with("node-"));
    }
}
