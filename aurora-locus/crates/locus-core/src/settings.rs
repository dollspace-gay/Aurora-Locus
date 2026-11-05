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
        key: "sync.deleted_history_preservation_days",
        kind: Kind::Int { min: 0, max: 90 },
        default: "0",
        env: None,
        description: "Days to keep a deleted account's firehose history before removing it. 0 removes it at once.",
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
