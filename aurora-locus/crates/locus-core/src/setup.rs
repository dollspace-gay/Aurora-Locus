//! First-run setup's plan: everything setup decides, in one value that the
//! web wizard fills step by step, `aurora-locus setup --config setup.toml`
//! reads from a file and headless setup builds from env. All three are
//! checked by the same rules here and applied by the same transaction, so
//! they end in the same state.

use serde::{Deserialize, Serialize};

use crate::bootstrap::is_local_hostname;

/// Bluesky's AppView, the distro installer's default.
pub const DEFAULT_APPVIEW_URL: &str = "https://api.bsky.app";
pub const DEFAULT_APPVIEW_DID: &str = "did:web:api.bsky.app";
/// Bluesky's relay.
pub const DEFAULT_CRAWLER: &str = "https://bsky.network";
/// Bluesky's moderation service, where reports go by default.
pub const DEFAULT_REPORT_URL: &str = "https://mod.bsky.app";
pub const DEFAULT_REPORT_DID: &str = "did:plc:ar7c4by46qjdydhdevvrndac";

/// Cloudflare's request body cap on its Free and Pro plans.
pub const CLOUDFLARE_BODY_CAP: i64 = 100 * 1024 * 1024;

/// How requests reach locus.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    /// The bundled Caddy (the default).
    #[default]
    Caddy,
    /// Cloudflare's proxy (orange cloud) in front of Caddy.
    Cloudflare,
    /// Another reverse proxy.
    Other,
    /// Locus terminates TLS itself.
    Direct,
}

impl ProxyMode {
    pub const ALL: [Self; 4] = [Self::Caddy, Self::Cloudflare, Self::Other, Self::Direct];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Caddy => "caddy",
            Self::Cloudflare => "cloudflare",
            Self::Other => "other",
            Self::Direct => "direct",
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Caddy => "Caddy, as installed (the default)",
            Self::Cloudflare => {
                "Behind Cloudflare's proxy (orange cloud), with Caddy at the origin"
            }
            Self::Other => "Another reverse proxy",
            Self::Direct => "No proxy: locus handles TLS itself",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.as_str() == s)
    }
}

/// The first operator: the console's owner.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OperatorPlan {
    pub username: String,
    /// The password as typed (a config file, the CLI). Never kept: the
    /// wizard's draft holds only `password_hash`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
}

/// Email: an SMTP URL and the address mail comes from, or none.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmailPlan {
    /// The SMTP URL, carrying credentials. The wizard stores it in the key
    /// store as soon as it's entered, so its draft never holds it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smtp_url: Option<String>,
    pub from_address: String,
    /// Chosen to go without email, knowing what won't work.
    pub skipped: bool,
}

/// The network services the PDS talks to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServicesPlan {
    pub appview_url: String,
    pub appview_did: String,
    pub crawlers: Vec<String>,
    pub moderation_url: String,
    pub moderation_did: String,
    pub report_url: String,
    pub report_did: String,
}

impl Default for ServicesPlan {
    fn default() -> Self {
        Self {
            appview_url: DEFAULT_APPVIEW_URL.into(),
            appview_did: DEFAULT_APPVIEW_DID.into(),
            crawlers: vec![DEFAULT_CRAWLER.into()],
            moderation_url: String::new(),
            moderation_did: String::new(),
            report_url: DEFAULT_REPORT_URL.into(),
            report_did: DEFAULT_REPORT_DID.into(),
        }
    }
}

/// Everything setup decides.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SetupPlan {
    /// The PDS hostname, no scheme or port.
    pub hostname: String,
    pub service_name: String,
    pub contact_email: String,
    pub proxy: ProxyMode,
    /// CIDRs whose forwarded headers are trusted; empty keeps the default
    /// (loopback only).
    pub trusted_proxies: Vec<String>,
    /// Domains handles go under, as `.example.com`.
    pub handle_domains: Vec<String>,
    /// The handle domain's subdomains already in use (www, mail, …), kept
    /// out of handles.
    pub existing_subdomains: Vec<String>,
    pub operator: OperatorPlan,
    /// The reference's admin password (Basic auth), for an install without
    /// an operator or alongside one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub admin_password: Option<String>,
    pub email: EmailPlan,
    pub services: ServicesPlan,
    pub private_tier: bool,
    /// An operator-held recovery key (a did:key) added to new accounts.
    pub recovery_did_key: String,
    /// The operator has a copy of the master key (or a key backup).
    pub keys_backup_acknowledged: bool,
    /// Warnings from the final checks the operator accepted, by id.
    pub acknowledged: Vec<String>,
}

/// One thing wrong with a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The field it's about (as the config file names it).
    pub field: &'static str,
    pub message: String,
}

fn problem(field: &'static str, message: impl Into<String>) -> Problem {
    Problem {
        field,
        message: message.into(),
    }
}

/// Lowercase, trimmed, no trailing dot.
#[must_use]
pub fn normalize_host(s: &str) -> String {
    s.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// A DNS hostname of at least two labels, each 1–63 letters, digits or
/// hyphens, not starting or ending with a hyphen; or a local name.
#[must_use]
pub fn valid_hostname(host: &str) -> bool {
    if is_local_hostname(host) {
        return true;
    }
    let labels: Vec<&str> = host.split('.').collect();
    host.len() <= 253
        && labels.len() >= 2
        && labels.iter().all(|l| {
            (1..=63).contains(&l.len())
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                && !l.starts_with('-')
                && !l.ends_with('-')
        })
        && labels
            .last()
            .is_some_and(|tld| !tld.bytes().all(|b| b.is_ascii_digit()))
}

/// A handle domain as `.example.com`, from `example.com`, `.example.com`
/// or `*.example.com`.
#[must_use]
pub fn normalize_handle_domain(s: &str) -> String {
    let d = normalize_host(s);
    let d = d.trim_start_matches("*.").trim_start_matches('.');
    format!(".{d}")
}

/// The handle domains the wizard suggests for a hostname, best first: the
/// parent's wildcard for a subdomain (`pds.example.com` → `.example.com`:
/// shorter handles, and one wildcard level is what Cloudflare's free
/// certificate covers), then the hostname's own.
#[must_use]
pub fn suggested_handle_domains(hostname: &str) -> Vec<String> {
    let host = normalize_host(hostname);
    let own = format!(".{host}");
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() >= 3 && !is_local_hostname(&host) {
        vec![format!(".{}", labels[1..].join(".")), own]
    } else {
        vec![own]
    }
}

/// Usernames are 1–64 letters, digits, `-`, `_` or `.`.
#[must_use]
pub fn valid_username(u: &str) -> bool {
    (1..=64).contains(&u.len())
        && u.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// The shortest operator password.
pub const MIN_OPERATOR_PASSWORD: usize = 12;

impl SetupPlan {
    /// The plan read from a `setup.toml`.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let mut plan: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        plan.normalize();
        Ok(plan)
    }

    /// Lowercases names, writes handle domains as `.domain`, and fills a
    /// subdomain's suggested handle domain when none is given.
    pub fn normalize(&mut self) {
        self.hostname = normalize_host(&self.hostname);
        self.handle_domains = self
            .handle_domains
            .iter()
            .filter(|d| !d.trim().is_empty())
            .map(|d| normalize_handle_domain(d))
            .collect();
        self.handle_domains.dedup();
        if self.handle_domains.is_empty() && !self.hostname.is_empty() {
            self.handle_domains = suggested_handle_domains(&self.hostname)
                .into_iter()
                .take(1)
                .collect();
        }
        self.existing_subdomains = self
            .existing_subdomains
            .iter()
            .map(|s| normalize_host(s))
            .filter(|s| !s.is_empty())
            .collect();
        self.existing_subdomains.sort();
        self.existing_subdomains.dedup();
        self.operator.username = self.operator.username.trim().to_owned();
    }

    /// Whether an operator account is part of this plan.
    #[must_use]
    pub fn has_operator(&self) -> bool {
        !self.operator.username.is_empty()
    }

    /// Handles that must not be registered: the PDS hostname's own label
    /// when the hostname sits directly under a handle domain, and the
    /// existing subdomains.
    #[must_use]
    pub fn reserved_handles(&self) -> Vec<String> {
        let mut out = Vec::new();
        for d in &self.handle_domains {
            if let Some(front) = self.hostname.strip_suffix(d.as_str())
                && !front.is_empty()
                && !front.contains('.')
            {
                out.push(front.to_owned());
            }
            for s in &self.existing_subdomains {
                let label = s.strip_suffix(d.as_str()).unwrap_or(s);
                if !label.is_empty() && !label.contains('.') {
                    out.push(label.to_owned());
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Everything that keeps this plan from being applied. `dev` allows a
    /// local hostname.
    #[must_use]
    pub fn problems(&self, dev: bool) -> Vec<Problem> {
        let mut out = Vec::new();
        out.extend(self.hostname_problems(dev));
        out.extend(self.handle_problems());
        out.extend(self.operator_problems());
        out.extend(self.email_problems());
        out.extend(self.services_problems());
        if !self.recovery_did_key.is_empty() && !self.recovery_did_key.starts_with("did:key:") {
            out.push(problem("recovery_did_key", "A recovery key is a did:key."));
        }
        if !self.keys_backup_acknowledged {
            out.push(problem(
                "keys_backup_acknowledged",
                "Confirm you have a copy of the master key: without it, every stored key is lost.",
            ));
        }
        out
    }

    pub fn hostname_problems(&self, dev: bool) -> Vec<Problem> {
        let h = &self.hostname;
        if h.is_empty() {
            return vec![problem("hostname", "Enter the PDS hostname.")];
        }
        if h.contains("://") || h.contains('/') || h.contains(':') {
            return vec![problem(
                "hostname",
                "Enter just the hostname, like pds.example.com: no https://, port or path.",
            )];
        }
        if !valid_hostname(h) {
            return vec![problem("hostname", format!("{h} isn't a valid hostname."))];
        }
        if is_local_hostname(h) && !dev {
            return vec![problem(
                "hostname",
                "A local hostname works only in dev mode (PDS_DEV_MODE).",
            )];
        }
        let mut out = Vec::new();
        for p in &self.trusted_proxies {
            if !valid_cidr(p) {
                out.push(problem(
                    "trusted_proxies",
                    format!("{p} isn't an address range like 10.0.0.0/8."),
                ));
            }
        }
        out
    }

    pub fn handle_problems(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        if self.handle_domains.is_empty() {
            out.push(problem(
                "handle_domains",
                "Choose at least one handle domain.",
            ));
        }
        for d in &self.handle_domains {
            let bare = d.trim_start_matches('.');
            // One label is enough (`.test`, as the reference allows): a
            // handle adds its own.
            if !d.starts_with('.')
                || !(valid_hostname(bare) || valid_hostname(&format!("x.{bare}")))
            {
                out.push(problem(
                    "handle_domains",
                    format!("{d} isn't a valid domain."),
                ));
            }
        }
        for s in &self.existing_subdomains {
            let bare = s.trim_start_matches('.');
            if bare
                .split('.')
                .any(|l| l.is_empty() || !l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
            {
                out.push(problem(
                    "existing_subdomains",
                    format!("{s} isn't a subdomain name."),
                ));
            }
        }
        out
    }

    pub fn operator_problems(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        if !self.has_operator() {
            if self.admin_password.as_deref().is_none_or(str::is_empty) {
                out.push(problem(
                    "operator",
                    "Create the first operator (or, for headless installs, give an admin password).",
                ));
            }
            return out;
        }
        if !valid_username(&self.operator.username) {
            out.push(problem(
                "operator.username",
                "Usernames are 1–64 letters, digits, '-', '_' or '.'.",
            ));
        }
        match (&self.operator.password, &self.operator.password_hash) {
            (Some(p), _) if p.chars().count() < MIN_OPERATOR_PASSWORD => out.push(problem(
                "operator.password",
                format!("Operator passwords are at least {MIN_OPERATOR_PASSWORD} characters."),
            )),
            (Some(p), _) if p.len() > 256 => {
                out.push(problem("operator.password", "That password is too long."));
            }
            (None, None) => out.push(problem(
                "operator.password",
                "Choose the operator's password.",
            )),
            _ => {}
        }
        out
    }

    pub fn email_problems(&self) -> Vec<Problem> {
        let e = &self.email;
        if e.skipped {
            return Vec::new();
        }
        let mut out = Vec::new();
        if let Some(url) = &e.smtp_url
            && !(url.starts_with("smtp://") || url.starts_with("smtps://"))
        {
            out.push(problem(
                "email.smtp_url",
                "An SMTP URL starts with smtp:// or smtps://.",
            ));
        }
        if e.from_address.is_empty() {
            out.push(problem(
                "email.from_address",
                "Enter the address email comes from.",
            ));
        } else if !looks_like_email(&e.from_address) {
            out.push(problem(
                "email.from_address",
                format!("{} isn't an email address.", e.from_address),
            ));
        }
        out
    }

    pub fn services_problems(&self) -> Vec<Problem> {
        let s = &self.services;
        let mut out = Vec::new();
        let url = |field, v: &str, out: &mut Vec<Problem>| {
            if !v.is_empty() && !v.starts_with("https://") && !v.starts_with("http://") {
                out.push(problem(field, format!("{v} isn't a URL.")));
            }
        };
        let did = |field, v: &str, out: &mut Vec<Problem>| {
            if !v.is_empty() && !v.starts_with("did:") {
                out.push(problem(field, format!("{v} isn't a DID.")));
            }
        };
        url("services.appview_url", &s.appview_url, &mut out);
        did("services.appview_did", &s.appview_did, &mut out);
        if s.appview_url.is_empty() != s.appview_did.is_empty() {
            out.push(problem(
                "services.appview_did",
                "An AppView needs both its URL and its DID.",
            ));
        }
        for c in &s.crawlers {
            url("services.crawlers", c, &mut out);
        }
        url("services.moderation_url", &s.moderation_url, &mut out);
        did("services.moderation_did", &s.moderation_did, &mut out);
        if s.moderation_url.is_empty() != s.moderation_did.is_empty() {
            out.push(problem(
                "services.moderation_did",
                "A moderation service needs both its URL and its DID.",
            ));
        }
        url("services.report_url", &s.report_url, &mut out);
        did("services.report_did", &s.report_did, &mut out);
        if s.report_url.is_empty() != s.report_did.is_empty() {
            out.push(problem(
                "services.report_did",
                "Report routing needs both a URL and a DID.",
            ));
        }
        out
    }

    /// The DNS records the hostname and handles need, as (name, what it
    /// points at, a note).
    #[must_use]
    pub fn dns_records(&self) -> Vec<(String, String, String)> {
        let target = if self.proxy == ProxyMode::Cloudflare {
            "this server, proxied (orange cloud)"
        } else {
            "this server's address (A and AAAA)"
        };
        let mut out = vec![(
            self.hostname.clone(),
            target.to_owned(),
            "The PDS itself.".to_owned(),
        )];
        for d in &self.handle_domains {
            out.push((
                format!("*{d}"),
                target.to_owned(),
                "Every handle under it.".to_owned(),
            ));
        }
        for s in &self.existing_subdomains {
            let name = if s.contains('.') {
                s.clone()
            } else {
                let parent = self.handle_domains.first().map_or("", String::as_str);
                format!("{s}{parent}")
            };
            out.push((
                name,
                "leave as it is".to_owned(),
                "A specific record wins over the wildcard, so this keeps working.".to_owned(),
            ));
        }
        out
    }
}

fn looks_like_email(s: &str) -> bool {
    let addr = s
        .rsplit_once('<')
        .map_or(s, |(_, a)| a.trim_end_matches('>'));
    addr.split_once('@')
        .is_some_and(|(l, d)| !l.is_empty() && d.contains('.') && !d.starts_with('.'))
}

fn valid_cidr(s: &str) -> bool {
    let (addr, bits) = s.split_once('/').unwrap_or((s, ""));
    match addr.parse::<std::net::IpAddr>() {
        Ok(ip) => {
            bits.is_empty()
                || bits
                    .parse::<u8>()
                    .is_ok_and(|b| b <= if ip.is_ipv4() { 32 } else { 128 })
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete() -> SetupPlan {
        let mut p = SetupPlan {
            hostname: "PDS.Example.com.".into(),
            operator: OperatorPlan {
                username: "alice".into(),
                password: Some("correct horse battery".into()),
                password_hash: None,
            },
            email: EmailPlan {
                skipped: true,
                ..EmailPlan::default()
            },
            existing_subdomains: vec!["www".into(), "Mail.example.com".into()],
            keys_backup_acknowledged: true,
            ..SetupPlan::default()
        };
        p.normalize();
        p
    }

    #[test]
    fn a_subdomain_gets_the_parent_wildcard_and_its_own_label_reserved() {
        let p = complete();
        assert_eq!(p.hostname, "pds.example.com");
        assert_eq!(p.handle_domains, [".example.com"]);
        assert_eq!(
            suggested_handle_domains("pds.example.com"),
            [".example.com", ".pds.example.com"]
        );
        assert_eq!(suggested_handle_domains("example.com"), [".example.com"]);
        assert_eq!(p.reserved_handles(), ["mail", "pds", "www"]);
        assert!(p.problems(false).is_empty(), "{:?}", p.problems(false));
        let names: Vec<_> = p.dns_records().into_iter().map(|r| r.0).collect();
        assert_eq!(
            names,
            [
                "pds.example.com",
                "*.example.com",
                "mail.example.com",
                "www.example.com"
            ]
        );
    }

    #[test]
    fn handles_under_the_hostname_reserve_nothing_extra() {
        let mut p = complete();
        p.handle_domains = vec!["*.pds.example.com".into()];
        p.existing_subdomains.clear();
        p.normalize();
        assert_eq!(p.handle_domains, [".pds.example.com"]);
        assert!(p.reserved_handles().is_empty());
    }

    #[test]
    fn problems_name_their_fields() {
        let fields = |p: &SetupPlan, dev| {
            p.problems(dev)
                .into_iter()
                .map(|p| p.field)
                .collect::<Vec<_>>()
        };
        let mut p = complete();
        p.hostname = "https://pds.example.com".into();
        assert_eq!(fields(&p, false), ["hostname"]);
        p.hostname = "localhost".into();
        p.handle_domains = vec![".localhost".into()];
        assert_eq!(fields(&p, false), ["hostname"]);
        assert!(fields(&p, true).is_empty());
        // One label is a handle domain (`alice.test`); bad labels aren't.
        let mut p = complete();
        p.handle_domains = vec![".test".into()];
        assert!(fields(&p, false).is_empty());
        for bad in [".123", ".-x", "..", "test"] {
            p.handle_domains = vec![bad.into()];
            assert_eq!(fields(&p, false), ["handle_domains"], "{bad}");
        }

        let mut p = complete();
        p.operator.password = Some("short".into());
        p.keys_backup_acknowledged = false;
        p.email = EmailPlan {
            smtp_url: Some("mail.example.com".into()),
            from_address: "nobody".into(),
            skipped: false,
        };
        p.services.appview_did.clear();
        p.trusted_proxies = vec!["10.0.0.0/8".into(), "10.0.0.0/99".into()];
        assert_eq!(
            fields(&p, false),
            [
                "trusted_proxies",
                "operator.password",
                "email.smtp_url",
                "email.from_address",
                "services.appview_did",
                "keys_backup_acknowledged"
            ]
        );

        // No operator: an admin password will do (headless installs).
        let mut p = complete();
        p.operator = OperatorPlan::default();
        assert_eq!(fields(&p, false), ["operator"]);
        p.admin_password = Some("from-pds-env".into());
        assert!(fields(&p, false).is_empty());
    }

    #[test]
    fn a_config_file_reads_and_unknown_keys_are_refused() {
        let p = SetupPlan::from_toml(
            r#"
hostname = "pds.example.com"
proxy = "cloudflare"
keys_backup_acknowledged = true
[operator]
username = "alice"
password = "correct horse battery"
[email]
smtp_url = "smtps://u:p@mail.example.com"
from_address = "PDS <pds@example.com>"
[services]
moderation_url = "https://mod.example.com"
moderation_did = "did:web:mod.example.com"
"#,
        )
        .unwrap();
        assert_eq!(p.proxy, ProxyMode::Cloudflare);
        assert_eq!(p.handle_domains, [".example.com"]);
        assert_eq!(p.services.appview_url, DEFAULT_APPVIEW_URL, "defaults stay");
        assert!(p.problems(false).is_empty(), "{:?}", p.problems(false));
        assert!(
            SetupPlan::from_toml("hostnme = \"x\"")
                .unwrap_err()
                .contains("hostnme")
        );
    }

    #[test]
    fn the_draft_never_serializes_a_password_it_doesnt_have() {
        let mut p = complete();
        p.operator.password = None;
        p.operator.password_hash = Some("$argon2id$…".into());
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("\"password\"") && !json.contains("smtp_url"));
        let back: SetupPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
    }
}
