//! Runtime settings: the registry of every setting (type, default, validation,
//! the reference PDS env var it maps from) and the immutable snapshot a request
//! reads from start to end.
//!
//! Settings are stored as JSON in the database. A change bumps a global
//! version; nodes reload and swap in a new [`Snapshot`], so a request never
//! sees a change halfway through.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::error::{CoreError, CoreResult};

/// The shape of a setting's value.
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Bool,
    /// An integer in an inclusive range.
    Int {
        min: i64,
        max: i64,
    },
    /// Free text, with a maximum length in bytes.
    Text {
        max_len: usize,
    },
    /// One of a fixed set of strings.
    Choice(&'static [&'static str]),
    /// An absolute http(s) URL.
    Url,
    /// A list of strings, each up to `max_len` bytes.
    TextList {
        max_len: usize,
    },
}

/// One registered runtime setting.
#[derive(Debug)]
pub struct Def {
    /// Dotted key, e.g. `log.level`.
    pub key: &'static str,
    pub kind: Kind,
    /// Default as JSON text (parsed once and validated in tests).
    pub default: &'static str,
    /// The reference PDS env var this maps from, if any. Its value seeds the
    /// setting at first boot and applies again when it changes install-wide.
    pub env: Option<&'static str>,
    /// One line for the console and CLI.
    pub description: &'static str,
}

impl Def {
    pub fn default_value(&self) -> Value {
        serde_json::from_str(self.default).expect("setting defaults are valid JSON")
    }

    /// Validates `value` for this setting and returns it in normal form.
    pub fn validate(&self, value: Value) -> CoreResult<Value> {
        let bad = |why: &str| CoreError::Invalid(format!("{}: {why}", self.key));
        match self.kind {
            Kind::Bool => match value {
                Value::Bool(_) => Ok(value),
                _ => Err(bad("expected true or false")),
            },
            Kind::Int { min, max } => match value.as_i64() {
                Some(n) if (min..=max).contains(&n) => Ok(Value::from(n)),
                Some(n) => Err(bad(&format!("{n} is outside {min}..={max}"))),
                None => Err(bad("expected an integer")),
            },
            Kind::Text { max_len } => match value {
                Value::String(ref s) if s.len() <= max_len => Ok(value),
                Value::String(_) => Err(bad(&format!("longer than {max_len} bytes"))),
                _ => Err(bad("expected text")),
            },
            Kind::Choice(options) => match value {
                Value::String(ref s) if options.contains(&s.as_str()) => Ok(value),
                _ => Err(bad(&format!("expected one of {}", options.join(", ")))),
            },
            Kind::Url => match value {
                Value::String(ref s) if s.is_empty() => Ok(value),
                Value::String(ref s) if is_http_url(s) => Ok(value),
                _ => Err(bad("expected an http(s) URL")),
            },
            Kind::TextList { max_len } => match value {
                Value::Array(ref items) => {
                    for item in items {
                        match item {
                            Value::String(s) if s.len() <= max_len => {}
                            Value::String(_) => {
                                return Err(bad(&format!(
                                    "an item is longer than {max_len} bytes"
                                )));
                            }
                            _ => return Err(bad("expected a list of text")),
                        }
                    }
                    Ok(value)
                }
                _ => Err(bad("expected a list of text")),
            },
        }
    }

    /// Parses an env/file string into this setting's JSON form.
    pub fn parse_env(&self, raw: &str) -> CoreResult<Value> {
        let value =
            match self.kind {
                Kind::Bool => match raw.to_ascii_lowercase().as_str() {
                    "true" | "1" | "yes" | "on" => Value::Bool(true),
                    "false" | "0" | "no" | "off" => Value::Bool(false),
                    _ => {
                        return Err(CoreError::Invalid(format!(
                            "{}: expected true or false",
                            self.key
                        )));
                    }
                },
                Kind::Int { .. } => Value::from(raw.trim().parse::<i64>().map_err(|_| {
                    CoreError::Invalid(format!("{}: expected an integer", self.key))
                })?),
                Kind::TextList { .. } => Value::Array(
                    raw.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(|s| Value::String(s.to_owned()))
                        .collect(),
                ),
                Kind::Text { .. } | Kind::Choice(_) | Kind::Url => Value::String(raw.to_owned()),
            };
        self.validate(value)
    }
}

fn is_http_url(s: &str) -> bool {
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"));
    matches!(rest, Some(r) if !r.is_empty() && !r.starts_with('/'))
}

/// Every runtime setting. Later work adds to this list; keys never change
/// meaning once released.
pub static REGISTRY: &[Def] = &[
    Def {
        key: "log.level",
        kind: Kind::Choice(&["error", "warn", "info", "debug", "trace"]),
        default: r#""info""#,
        env: Some("LOG_LEVEL"),
        description: "Log level, applied live.",
    },
    Def {
        key: "server.service_name",
        kind: Kind::Text { max_len: 128 },
        default: r#""""#,
        env: Some("PDS_SERVICE_NAME"),
        description: "Service name shown on the landing page, OAuth pages and emails. Empty means \"<hostname> PDS\".",
    },
    Def {
        key: "server.contact_email",
        kind: Kind::Text { max_len: 320 },
        default: r#""""#,
        env: Some("PDS_CONTACT_EMAIL_ADDRESS"),
        description: "Contact address published in describeServer.",
    },
    Def {
        key: "server.landing_page",
        kind: Kind::Bool,
        default: "false",
        env: None,
        description: "Serve a public landing page at /. Off: / returns the plain banner.",
    },
    Def {
        key: "server.privacy_policy_url",
        kind: Kind::Url,
        default: r#""""#,
        env: Some("PDS_PRIVACY_POLICY_URL"),
        description: "Privacy policy link published in describeServer.",
    },
    Def {
        key: "server.terms_of_service_url",
        kind: Kind::Url,
        default: r#""""#,
        env: Some("PDS_TERMS_OF_SERVICE_URL"),
        description: "Terms of service link published in describeServer.",
    },
    Def {
        key: "http.internal_hosts",
        kind: Kind::TextList { max_len: 253 },
        default: "[]",
        env: None,
        description: "Extra names (besides localhost, IP addresses and single-label names) that get the full route set, for local tools and probes. Not a trust boundary.",
    },
    Def {
        key: "identity.handle_domains",
        kind: Kind::TextList { max_len: 253 },
        default: "[]",
        env: Some("PDS_SERVICE_HANDLE_DOMAINS"),
        description: "Domains new handles can be on, each starting with a dot. Empty means \".<hostname>\".",
    },
    Def {
        key: "identity.plc_url",
        kind: Kind::Url,
        default: r#""https://plc.directory""#,
        env: Some("PDS_DID_PLC_URL"),
        description: "The PLC directory.",
    },
    Def {
        key: "identity.recovery_did_key",
        kind: Kind::Text { max_len: 256 },
        default: r#""""#,
        env: Some("PDS_RECOVERY_DID_KEY"),
        description: "Operator recovery key (a did:key) added to new did:plc accounts' rotation keys.",
    },
    Def {
        key: "identity.resolver_timeout_ms",
        kind: Kind::Int {
            min: 100,
            max: 60_000,
        },
        default: "3000",
        env: Some("PDS_ID_RESOLVER_TIMEOUT"),
        description: "Timeout for each DID document or handle lookup, in milliseconds.",
    },
    Def {
        key: "identity.did_cache_stale_ms",
        kind: Kind::Int {
            min: 0,
            max: 7 * 24 * 3_600_000,
        },
        default: "3600000",
        env: Some("PDS_DID_CACHE_STALE_TTL"),
        description: "Age after which a cached DID document is refreshed in the background, in milliseconds.",
    },
    Def {
        key: "identity.did_cache_max_ms",
        kind: Kind::Int {
            min: 0,
            max: 30 * 24 * 3_600_000,
        },
        default: "86400000",
        env: Some("PDS_DID_CACHE_MAX_TTL"),
        description: "Age after which a cached DID document is no longer used, in milliseconds.",
    },
    Def {
        key: "identity.handle_backup_nameservers",
        kind: Kind::TextList { max_len: 64 },
        default: "[]",
        env: Some("PDS_HANDLE_BACKUP_NAMESERVERS"),
        description: "DNS servers (IP addresses) tried for _atproto lookups when the system resolver finds nothing.",
    },
    Def {
        key: "accounts.default_did_method",
        kind: Kind::Choice(&["plc", "web"]),
        default: r#""plc""#,
        env: None,
        description: "DID method for new accounts that don't ask for one.",
    },
    Def {
        key: "accounts.invite_required",
        kind: Kind::Bool,
        default: "true",
        env: Some("PDS_INVITE_REQUIRED"),
        description: "Require an invite code to create an account.",
    },
    Def {
        key: "accounts.invite_interval_ms",
        kind: Kind::Int {
            min: 0,
            max: 365 * 24 * 3_600_000,
        },
        default: "0",
        env: Some("PDS_INVITE_INTERVAL"),
        description: "Accounts earn an invite code every this many milliseconds (at most 5 unused). 0 turns it off.",
    },
    Def {
        key: "accounts.invite_epoch_ms",
        kind: Kind::Int {
            min: 0,
            max: i64::MAX,
        },
        default: "0",
        env: Some("PDS_INVITE_EPOCH"),
        description: "Earned invite codes count from this time (ms since the epoch).",
    },
    Def {
        key: "accounts.disposable_email_domains",
        kind: Kind::TextList { max_len: 253 },
        default: "[]",
        env: None,
        description: "Email domains refused at sign-up and email change, on top of the built-in disposable-address list.",
    },
    Def {
        key: "accounts.did_doc_with_session",
        kind: Kind::Bool,
        default: "false",
        env: Some("PDS_ENABLE_DID_DOC_WITH_SESSION"),
        description: "Include the DID document in createAccount, createSession and getSession responses.",
    },
    Def {
        key: "email.from_address",
        kind: Kind::Text { max_len: 320 },
        default: r#""""#,
        env: Some("PDS_EMAIL_FROM_ADDRESS"),
        description: "From address of account emails, e.g. \"PDS <noreply@example.com>\".",
    },
    Def {
        key: "email.moderation_from_address",
        kind: Kind::Text { max_len: 320 },
        default: r#""""#,
        env: Some("PDS_MODERATION_EMAIL_ADDRESS"),
        description: "From address of moderation emails. Empty uses the account email address.",
    },
    Def {
        key: "limits.repo_write_body",
        kind: Kind::Int {
            min: 1024,
            max: 64 * 1024 * 1024,
        },
        default: "2000000",
        env: None,
        description: "Largest request body for record writes, in bytes.",
    },
    Def {
        key: "limits.json_body",
        kind: Kind::Int {
            min: 1024,
            max: 64 * 1024 * 1024,
        },
        default: "153600",
        env: None,
        description: "Largest JSON request body for other endpoints, in bytes.",
    },
    Def {
        key: "limits.record_size",
        kind: Kind::Int {
            min: 1024,
            max: 2_000_000,
        },
        default: "1100000",
        env: None,
        description: "Largest single record (DAG-CBOR), in bytes.",
    },
    Def {
        key: "appview.url",
        kind: Kind::Url,
        default: r#""""#,
        env: Some("PDS_BSKY_APP_VIEW_URL"),
        description: "The AppView requests are proxied to by default. Empty: none.",
    },
    Def {
        key: "appview.did",
        kind: Kind::Text { max_len: 2048 },
        default: r#""""#,
        env: Some("PDS_BSKY_APP_VIEW_DID"),
        description: "The AppView's DID (the audience of its service tokens).",
    },
    Def {
        key: "appview.cdn_url_pattern",
        kind: Kind::Text { max_len: 2048 },
        default: r#""""#,
        env: Some("PDS_BSKY_APP_VIEW_CDN_URL_PATTERN"),
        description: "Image URL pattern for local records in read-after-write responses (%s: preset, did, cid).",
    },
    Def {
        key: "moderation.service_url",
        kind: Kind::Url,
        default: r#""""#,
        env: Some("PDS_MOD_SERVICE_URL"),
        description: "The moderation service tools.ozone.* requests go to. Empty: none.",
    },
    Def {
        key: "moderation.service_did",
        kind: Kind::Text { max_len: 2048 },
        default: r#""""#,
        env: Some("PDS_MOD_SERVICE_DID"),
        description: "The moderation service's DID.",
    },
    Def {
        key: "moderation.report_service_url",
        kind: Kind::Url,
        default: r#""""#,
        env: Some("PDS_REPORT_SERVICE_URL"),
        description: "Where reports go. Empty: the moderation service.",
    },
    Def {
        key: "moderation.report_service_did",
        kind: Kind::Text { max_len: 2048 },
        default: r#""""#,
        env: Some("PDS_REPORT_SERVICE_DID"),
        description: "The report service's DID.",
    },
    Def {
        key: "proxy.allow_http2",
        kind: Kind::Bool,
        default: "false",
        env: Some("PDS_PROXY_ALLOW_HTTP2"),
        description: "Allow HTTP/2 to proxied services.",
    },
    Def {
        key: "proxy.headers_timeout_ms",
        kind: Kind::Int {
            min: 100,
            max: 600_000,
        },
        default: "10000",
        env: Some("PDS_PROXY_HEADERS_TIMEOUT"),
        description: "How long a proxied request waits for the response headers, in ms.",
    },
    Def {
        key: "proxy.body_timeout_ms",
        kind: Kind::Int {
            min: 100,
            max: 600_000,
        },
        default: "30000",
        env: Some("PDS_PROXY_BODY_TIMEOUT"),
        description: "Longest wait between chunks of a proxied response body, in ms.",
    },
    Def {
        key: "proxy.max_response_size",
        kind: Kind::Int {
            min: 1024,
            max: 1 << 34,
        },
        default: "10485760",
        env: Some("PDS_PROXY_MAX_RESPONSE_SIZE"),
        description: "Largest proxied response, in bytes.",
    },
    Def {
        key: "proxy.max_retries",
        kind: Kind::Int { min: 0, max: 10 },
        default: "0",
        env: Some("PDS_PROXY_MAX_RETRIES"),
        description: "Retries of a proxied GET or HEAD after a connection error.",
    },
    Def {
        key: "proxy.prefer_compressed",
        kind: Kind::Bool,
        default: "false",
        env: Some("PDS_PROXY_PREFER_COMPRESSED"),
        description: "Ask proxied services for compressed responses when the PDS reads them itself.",
    },
    Def {
        key: "oauth.lexicon_authority_did",
        kind: Kind::Text { max_len: 2048 },
        default: r#""""#,
        env: Some("PDS_LEXICON_AUTHORITY_DID"),
        description: "The DID permission sets for include: scopes are resolved from, instead of DNS. Empty: DNS.",
    },
    Def {
        key: "oauth.trusted_clients",
        kind: Kind::TextList { max_len: 2048 },
        default: "[]",
        env: Some("PDS_OAUTH_TRUSTED_CLIENTS"),
        description: "OAuth client ids treated as trusted (longer sessions, as first-party).",
    },
    Def {
        key: "http.request_deadline_ms",
        kind: Kind::Int {
            min: 1_000,
            max: 3_600_000,
        },
        default: "60000",
        env: None,
        description: "Longest a request may run before it gets a retryable error, in ms (streams and uploads have their own caps).",
    },
    Def {
        key: "http.trusted_proxies",
        kind: Kind::TextList { max_len: 64 },
        default: r#"["127.0.0.0/8", "::1/128"]"#,
        env: Some("LOCUS_TRUSTED_PROXIES"),
        description: "Proxies (CIDRs) whose X-Forwarded-For / Forwarded headers are trusted for the client IP.",
    },
    Def {
        key: "rate_limits.enabled",
        kind: Kind::Bool,
        default: "true",
        env: Some("PDS_RATE_LIMITS_ENABLED"),
        description: "Apply rate limits.",
    },
    Def {
        key: "rate_limits.bypass_key",
        kind: Kind::Text { max_len: 512 },
        default: r#""""#,
        env: Some("PDS_RATE_LIMIT_BYPASS_KEY"),
        description: "Requests with this x-ratelimit-bypass header skip rate limits. Empty: none.",
    },
    Def {
        key: "rate_limits.bypass_ips",
        kind: Kind::TextList { max_len: 64 },
        default: "[]",
        env: Some("PDS_RATE_LIMIT_BYPASS_IPS"),
        description: "Client IPs or CIDRs that skip rate limits.",
    },
    Def {
        key: "rate_limits.backend",
        kind: Kind::Choice(&["auto", "memory", "database"]),
        default: r#""auto""#,
        env: None,
        description: "Where counters live: in each node, or in the database (shared). Auto: the database on Postgres.",
    },
    Def {
        key: "rate_limits.global_points",
        kind: Kind::Int {
            min: 1,
            max: 100_000_000,
        },
        default: "3000",
        env: None,
        description: "XRPC requests per client IP per 5 minutes.",
    },
    Def {
        key: "rate_limits.repo_write_hour_points",
        kind: Kind::Int {
            min: 1,
            max: 100_000_000,
        },
        default: "5000",
        env: None,
        description: "Repo write points per account per hour (create 3, update 2, delete 1).",
    },
    Def {
        key: "rate_limits.repo_write_day_points",
        kind: Kind::Int {
            min: 1,
            max: 100_000_000,
        },
        default: "35000",
        env: None,
        description: "Repo write points per account per day.",
    },
    Def {
        key: "sync.max_subscribers",
        kind: Kind::Int {
            min: 0,
            max: 1_000_000,
        },
        default: "1000",
        env: None,
        description: "Firehose connections per node. 0: no limit.",
    },
    Def {
        key: "sync.max_subscribers_per_ip",
        kind: Kind::Int {
            min: 0,
            max: 1_000_000,
        },
        default: "20",
        env: None,
        description: "Firehose connections per client IP per node. 0: no limit.",
    },
    Def {
        key: "limits.import_size",
        kind: Kind::Int {
            min: 1024,
            max: 1 << 40,
        },
        default: "1073741824",
        env: None,
        description: "Largest repository import (CAR upload), in bytes.",
    },
    Def {
        key: "accounts.accept_imports",
        kind: Kind::Bool,
        default: "true",
        env: Some("PDS_ACCEPTING_REPO_IMPORTS"),
        description: "Accept repository imports (com.atproto.repo.importRepo).",
    },
    Def {
        key: "blobs.upload_limit",
        kind: Kind::Int {
            min: 1,
            max: 1 << 40,
        },
        default: "314572800",
        env: Some("PDS_BLOB_UPLOAD_LIMIT"),
        description: "Largest blob upload, in bytes.",
    },
    Def {
        key: "blobs.temp_ttl_ms",
        kind: Kind::Int {
            min: 60_000,
            max: 30 * 24 * 3_600_000,
        },
        default: "21600000",
        env: None,
        description: "How long an uploaded blob no record references is kept, in milliseconds.",
    },
    Def {
        key: "blobs.unreferenced_grace_ms",
        kind: Kind::Int {
            min: 0,
            max: 30 * 24 * 3_600_000,
        },
        default: "3600000",
        env: None,
        description: "How long a blob whose last reference went is kept, in milliseconds.",
    },
    Def {
        key: "blobs.delete_grace_ms",
        kind: Kind::Int {
            min: 0,
            max: 24 * 3_600_000,
        },
        default: "300000",
        env: None,
        description: "How long a replaced or collected copy stays on storage before deletion, so reads in flight finish.",
    },
    Def {
        key: "sync.retention_hours",
        kind: Kind::Int {
            min: 1,
            max: 24 * 365,
        },
        default: "72",
        env: None,
        description: "How long the firehose keeps events for backfilling consumers, in hours.",
    },
    Def {
        key: "sync.subscriber_buffer",
        kind: Kind::Int {
            min: 10,
            max: 100_000,
        },
        default: "500",
        env: None,
        description: "How many events a firehose subscriber may fall behind before it's dropped as too slow (applies on restart).",
    },
    Def {
        key: "sync.ws_ping_interval_s",
        kind: Kind::Int { min: 5, max: 600 },
        default: "30",
        env: None,
        description: "Seconds between WebSocket pings to firehose subscribers.",
    },
    Def {
        key: "sync.crawlers",
        kind: Kind::TextList { max_len: 512 },
        default: "[]",
        env: Some("PDS_CRAWLERS"),
        description: "Relays asked to crawl this PDS when there are new events (at most every 20 minutes each).",
    },
    Def {
        key: "sync.deleted_history_preservation_days",
        kind: Kind::Int { min: 0, max: 90 },
        default: "0",
        env: None,
        description: "Days to keep a deleted account's firehose history before removing it. 0 removes it at once.",
    },
    Def {
        key: "kryphocron.private_tier_enabled",
        kind: Kind::Bool,
        default: "true",
        env: None,
        description: "Private-tier writes (private posts, blocks, mutes, audiences). Effective only with an AppView configured.",
    },
    Def {
        key: "kryphocron.rotation_period_days",
        kind: Kind::Int { min: 30, max: 90 },
        default: "60",
        env: None,
        description: "Days between rotation generations; private posts are re-encoded under each new one. Applies from the next generation.",
    },
    Def {
        key: "kryphocron.rotation_max_commits_per_minute",
        kind: Kind::Int {
            min: 1,
            max: 10_000,
        },
        default: "120",
        env: None,
        description: "Most rotation commits per minute, install-wide.",
    },
    Def {
        key: "kryphocron.audit_retention_days",
        kind: Kind::Int { min: 1, max: 3650 },
        default: "365",
        env: None,
        description: "Days to keep kryphocron audit events.",
    },
    Def {
        key: "kryphocron.prism_request_timeout_ms",
        kind: Kind::Int {
            min: 1000,
            max: 60_000,
        },
        default: "10000",
        env: None,
        description: "The bound on every call to the AppView's private-tier methods, in milliseconds.",
    },
    Def {
        key: "kryphocron.audience_check_ttl_s",
        kind: Kind::Int { min: 5, max: 3600 },
        default: "60",
        env: None,
        description: "How stale an owner's audience copy may be when a membership check uses it, in seconds; also the limit on refreshes other accounts' checks cause.",
    },
    Def {
        key: "kryphocron.audience_listing_deadline_s",
        kind: Kind::Int { min: 10, max: 3600 },
        default: "300",
        env: None,
        description: "The hard deadline of one audience listing from the AppView, in seconds.",
    },
    Def {
        key: "kryphocron.audience_poll_interval_s",
        kind: Kind::Int {
            min: 60,
            max: 86_400,
        },
        default: "900",
        env: None,
        description: "How often each account's audience copy is refreshed in the background, in seconds (stretched to stay within the call budget).",
    },
    Def {
        key: "kryphocron.prism_call_budget",
        kind: Kind::Int { min: 1, max: 100 },
        default: "5",
        env: None,
        description: "Most background calls per second this node makes to the AppView's private-tier methods.",
    },
    Def {
        key: "kryphocron.audience_max_members",
        kind: Kind::Int { min: 1, max: 1000 },
        default: "1000",
        env: None,
        description: "Most members in one audience (never above the AppView's 1,000).",
    },
    Def {
        key: "kryphocron.audience_max_per_account",
        kind: Kind::Int { min: 1, max: 200 },
        default: "200",
        env: None,
        description: "Most audiences per account (never above the AppView's 200).",
    },
    Def {
        key: "kryphocron.repoint_max_commits_per_minute",
        kind: Kind::Int {
            min: 1,
            max: 10_000,
        },
        default: "120",
        env: None,
        description: "Most commits per minute the re-point job makes (posts moved from a local audience URI to the AppView's).",
    },
    Def {
        key: "kryphocron.remote_block_scan_max",
        kind: Kind::Int {
            min: 100,
            max: 1_000_000,
        },
        default: "50000",
        env: None,
        description: "Most kryphocron block records read from another PDS to check a reply; past it the reply is refused.",
    },
];

/// Looks up a setting definition.
pub fn def(key: &str) -> CoreResult<&'static Def> {
    REGISTRY
        .iter()
        .find(|d| d.key == key)
        .ok_or_else(|| CoreError::NotFound(format!("no setting named {key}")))
}

/// An immutable view of every setting at one version.
#[derive(Debug, Clone)]
pub struct Snapshot {
    version: i64,
    values: BTreeMap<&'static str, Value>,
}

impl Snapshot {
    /// Builds a snapshot from stored values, filling defaults. Stored values
    /// for unknown keys (from a newer node) are ignored; stored values that no
    /// longer validate fall back to the default and are reported.
    pub fn build(version: i64, stored: &BTreeMap<String, Value>) -> (Self, Vec<String>) {
        let mut values = BTreeMap::new();
        let mut problems = Vec::new();
        for d in REGISTRY {
            let value = match stored.get(d.key) {
                Some(v) => match d.validate(v.clone()) {
                    Ok(v) => v,
                    Err(e) => {
                        problems.push(e.to_string());
                        d.default_value()
                    }
                },
                None => d.default_value(),
            };
            values.insert(d.key, value);
        }
        (Self { version, values }, problems)
    }

    /// A snapshot with every default, for tests and setup mode.
    pub fn defaults() -> Self {
        Self::build(0, &BTreeMap::new()).0
    }

    pub fn version(&self) -> i64 {
        self.version
    }

    pub fn get(&self, key: &str) -> &Value {
        self.values
            .get(key)
            .unwrap_or_else(|| panic!("unregistered setting {key}"))
    }

    pub fn bool(&self, key: &str) -> bool {
        self.get(key)
            .as_bool()
            .unwrap_or_else(|| panic!("{key} is not a bool"))
    }

    pub fn int(&self, key: &str) -> i64 {
        self.get(key)
            .as_i64()
            .unwrap_or_else(|| panic!("{key} is not an integer"))
    }

    pub fn text(&self, key: &str) -> &str {
        self.get(key)
            .as_str()
            .unwrap_or_else(|| panic!("{key} is not text"))
    }

    pub fn list(&self, key: &str) -> Vec<&str> {
        self.get(key)
            .as_array()
            .unwrap_or_else(|| panic!("{key} is not a list"))
            .iter()
            .filter_map(Value::as_str)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_is_valid_and_keys_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for d in REGISTRY {
            assert!(seen.insert(d.key), "duplicate key {}", d.key);
            d.validate(d.default_value())
                .unwrap_or_else(|e| panic!("default for {} invalid: {e}", d.key));
        }
    }

    #[test]
    fn validation_rejects_wrong_values() {
        let d = def("sync.deleted_history_preservation_days").unwrap();
        assert!(d.validate(Value::from(90)).is_ok());
        assert!(d.validate(Value::from(91)).is_err());
        assert!(d.validate(Value::from(-1)).is_err());
        assert!(d.validate(Value::from("10")).is_err());

        let d = def("log.level").unwrap();
        assert!(d.validate(Value::from("debug")).is_ok());
        assert!(d.validate(Value::from("verbose")).is_err());
    }

    #[test]
    fn env_values_parse() {
        assert_eq!(
            def("server.landing_page")
                .unwrap()
                .parse_env("TRUE")
                .unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            def("sync.deleted_history_preservation_days")
                .unwrap()
                .parse_env(" 30 ")
                .unwrap(),
            Value::from(30)
        );
        assert!(def("log.level").unwrap().parse_env("loud").is_err());
    }

    #[test]
    fn snapshot_fills_defaults_and_drops_bad_stored_values() {
        let mut stored = BTreeMap::new();
        stored.insert("log.level".to_owned(), Value::from("debug"));
        stored.insert(
            "sync.deleted_history_preservation_days".to_owned(),
            Value::from(500),
        );
        stored.insert("from.a.newer.node".to_owned(), Value::from(1));
        let (snap, problems) = Snapshot::build(7, &stored);
        assert_eq!(snap.version(), 7);
        assert_eq!(snap.text("log.level"), "debug");
        assert_eq!(snap.int("sync.deleted_history_preservation_days"), 0);
        assert_eq!(problems.len(), 1);
        assert!(!snap.bool("server.landing_page"));
    }

    #[test]
    fn urls() {
        assert!(is_http_url("https://api.bsky.app"));
        assert!(is_http_url("http://localhost:2584"));
        assert!(!is_http_url("ftp://x"));
        assert!(!is_http_url("https://"));
    }
}
