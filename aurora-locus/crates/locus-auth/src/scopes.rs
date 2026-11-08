//! atproto OAuth scopes, as the reference's `@atproto/oauth-scopes`:
//! `repo:`, `blob:`, `rpc:`, `account:`, `identity:` permissions,
//! `include:` permission sets, and the static `atproto` / `transition:*`
//! values. Scopes parse from strings (and from lexicon permission-set
//! entries), normalize to the reference's canonical string form, and match
//! requests. The canonical forms matter: they are what grants store and
//! what "Missing required scope" errors name.

use serde_json::Value;

/// The static scope values.
pub const STATIC_SCOPES: &[&str] = &[
    "atproto",
    "transition:email",
    "transition:generic",
    "transition:chat.bsky",
];

pub const REPO_ACTIONS: &[&str] = &["create", "update", "delete"];
pub const ACCOUNT_ATTRIBUTES: &[&str] = &["email", "repo", "status"];
pub const ACCOUNT_ACTIONS: &[&str] = &["read", "manage"];
pub const IDENTITY_ATTRIBUTES: &[&str] = &["handle", "*"];

// ---------------------------------------------------------------- syntax

/// A parameter value (lexicon permissions may hold numbers and booleans).
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    Str(String),
    Num(f64),
    Bool(bool),
}

impl Param {
    fn as_str(&self) -> Option<&str> {
        match self {
            Param::Str(s) => Some(s),
            _ => None,
        }
    }

    fn to_text(&self) -> String {
        match self {
            Param::Str(s) => s.clone(),
            Param::Num(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    format!("{}", *n as i64)
                } else {
                    n.to_string()
                }
            }
            Param::Bool(b) => b.to_string(),
        }
    }
}

/// A scope's prefix, positional value and named parameters, from a scope
/// string or a lexicon permission.
#[derive(Debug, Clone, Default)]
pub struct Syntax {
    pub prefix: String,
    pub positional: Option<String>,
    /// Named parameters in order; a key may repeat. A lexicon permission's
    /// non-array value is `Single`.
    params: Vec<(String, ParamEntry)>,
}

#[derive(Debug, Clone)]
enum ParamEntry {
    /// From a query string (every value is a string; keys may repeat).
    Query(String),
    /// From a lexicon permission: one value or an array.
    LexSingle(Param),
    LexArray(Vec<Param>),
}

/// The JS `encodeURIComponent`.
fn encode_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        let c = b as char;
        if c.is_ascii_alphanumeric() || "-_.!~*'()".contains(c) {
            out.push(c);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The `URLSearchParams` serialization (application/x-www-form-urlencoded).
fn encode_form(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        let c = b as char;
        if c.is_ascii_alphanumeric() || "*-._".contains(c) {
            out.push(c);
        } else if c == ' ' {
            out.push('+');
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The reference's `normalizeURIComponent`: these characters are left
/// unencoded in scope strings.
fn normalize_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            let seq = &s[i..i + 3];
            let c = match seq {
                "%3A" => Some(':'),
                "%2F" => Some('/'),
                "%2B" => Some('+'),
                "%2C" => Some(','),
                "%40" => Some('@'),
                "%25" => Some('%'),
                _ => None,
            };
            if let Some(c) = c {
                out.push(c);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// The JS `decodeURIComponent`; None on a malformed sequence.
fn decode_component(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

impl Syntax {
    /// Parses `prefix[:positional][?query]`.
    #[must_use]
    pub fn from_scope_str(scope: &str) -> Option<Self> {
        let q = scope.find('?');
        let c = scope.find(':');
        let end = match (q, c) {
            (None, None) => {
                return Some(Self {
                    prefix: scope.to_owned(),
                    ..Self::default()
                });
            }
            (Some(a), None) | (None, Some(a)) => a,
            (Some(a), Some(b)) => a.min(b),
        };
        let prefix = scope[..end].to_owned();
        let positional = match (c, q) {
            (Some(c), None) => Some(decode_component(&scope[c + 1..])?),
            (Some(c), Some(q)) if c < q => Some(decode_component(&scope[c + 1..q])?),
            _ => None,
        };
        let mut params = Vec::new();
        if let Some(q) = q
            && q < scope.len() - 1
        {
            for (k, v) in url::form_urlencoded::parse(&scope.as_bytes()[q + 1..]) {
                params.push((k.into_owned(), ParamEntry::Query(v.into_owned())));
            }
        }
        Some(Self {
            prefix,
            positional,
            params,
        })
    }

    /// A lexicon permission entry (`{type: "permission", resource, ...}`).
    #[must_use]
    pub fn from_lex(permission: &serde_json::Map<String, Value>) -> Option<Self> {
        let prefix = permission.get("resource")?.as_str()?.to_owned();
        let to_param = |v: &Value| match v {
            Value::String(s) => Some(Param::Str(s.clone())),
            Value::Bool(b) => Some(Param::Bool(*b)),
            Value::Number(n) => n.as_f64().map(Param::Num),
            _ => None,
        };
        let mut params = Vec::new();
        for (k, v) in permission {
            if k == "type" || k == "resource" || v.is_null() {
                continue;
            }
            let entry = match v {
                Value::Array(items) => {
                    ParamEntry::LexArray(items.iter().map(to_param).collect::<Option<Vec<_>>>()?)
                }
                other => ParamEntry::LexSingle(to_param(other)?),
            };
            params.push((k.clone(), entry));
        }
        Some(Self {
            prefix,
            positional: None,
            params,
        })
    }

    fn keys(&self) -> impl Iterator<Item = &str> {
        self.params.iter().map(|(k, _)| k.as_str())
    }

    /// One value: None when absent, Some(None) when it can't be single.
    fn single(&self, key: &str) -> Option<Option<Param>> {
        let found: Vec<&ParamEntry> = self
            .params
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v)
            .collect();
        match found.as_slice() {
            [] => None,
            [ParamEntry::Query(v)] => Some(Some(Param::Str(v.clone()))),
            [ParamEntry::LexSingle(p)] => Some(Some(p.clone())),
            _ => Some(None),
        }
    }

    /// Every value: None when absent, Some(None) when invalid.
    fn multi(&self, key: &str) -> Option<Option<Vec<Param>>> {
        let found: Vec<&ParamEntry> = self
            .params
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v)
            .collect();
        if found.is_empty() {
            return None;
        }
        let mut out = Vec::new();
        for f in found {
            match f {
                ParamEntry::Query(v) => out.push(Param::Str(v.clone())),
                ParamEntry::LexArray(a) => out.extend(a.iter().cloned()),
                ParamEntry::LexSingle(_) => return Some(None),
            }
        }
        Some(Some(out))
    }

    fn without(&self, key: &str) -> Self {
        let mut s = self.clone();
        s.params.retain(|(k, _)| k != key);
        s
    }

    fn with_single(&self, key: &str, value: &str) -> Self {
        let mut s = self.clone();
        s.params.push((
            key.to_owned(),
            ParamEntry::LexSingle(Param::Str(value.to_owned())),
        ));
        s
    }
}

/// `prefix`, `prefix:...` or `prefix?...`.
fn is_for(scope: &str, prefix: &str) -> bool {
    scope == prefix
        || (scope.len() > prefix.len()
            && scope.starts_with(prefix)
            && matches!(scope.as_bytes()[prefix.len()], b':' | b'?'))
}

// ---------------------------------------------------------------- the parser

#[derive(Clone, Copy)]
struct Field {
    key: &'static str,
    multiple: bool,
    required: bool,
    validate: fn(&Param) -> bool,
}

/// A parsed field: one value or several (the default when absent).
#[derive(Debug, Clone)]
enum Got {
    One(String),
    Many(Vec<String>),
    Absent,
}

/// The reference's `Parser.parse`: unknown keys refuse; a positional and the
/// same named parameter refuse; required fields must be present.
fn parse_fields(syntax: &Syntax, fields: &[Field], positional: &str) -> Option<Vec<Got>> {
    for k in syntax.keys() {
        if !fields.iter().any(|f| f.key == k) {
            return None;
        }
    }
    let mut out = Vec::with_capacity(fields.len());
    for f in fields {
        let got = if f.multiple {
            match syntax.multi(f.key) {
                Some(None) => return None,
                Some(Some(vals)) => {
                    if f.key == positional && syntax.positional.is_some() {
                        return None;
                    }
                    if vals.is_empty() || !vals.iter().all(f.validate) {
                        return None;
                    }
                    Got::Many(vals.iter().map(Param::to_text).collect())
                }
                None if f.key == positional && syntax.positional.is_some() => {
                    let p = Param::Str(syntax.positional.clone().unwrap_or_default());
                    if !(f.validate)(&p) {
                        return None;
                    }
                    Got::Many(vec![p.to_text()])
                }
                None if f.required => return None,
                None => Got::Absent,
            }
        } else {
            match syntax.single(f.key) {
                Some(None) => return None,
                Some(Some(v)) => {
                    if f.key == positional && syntax.positional.is_some() {
                        return None;
                    }
                    if !(f.validate)(&v) {
                        return None;
                    }
                    Got::One(v.to_text())
                }
                None if f.key == positional && syntax.positional.is_some() => {
                    let p = Param::Str(syntax.positional.clone().unwrap_or_default());
                    if !(f.validate)(&p) {
                        return None;
                    }
                    Got::One(p.to_text())
                }
                None if f.required => return None,
                None => Got::Absent,
            }
        };
        out.push(got);
    }
    Some(out)
}

/// The reference's `Parser.format`, given values already normalized and
/// with defaults already left out (None).
fn format(
    prefix: &str,
    positional_key: &str,
    values: &[(&str, Option<Vec<String>>, bool)],
) -> String {
    let mut positional = None;
    let mut query: Vec<(String, String)> = Vec::new();
    for (key, value, multiple) in values {
        let Some(v) = value else { continue };
        if *multiple {
            if *key == positional_key && v.len() == 1 {
                positional = Some(v[0].clone());
            } else {
                let mut seen: Vec<&String> = Vec::new();
                for item in v {
                    if !seen.contains(&item) {
                        seen.push(item);
                        query.push(((*key).to_owned(), item.clone()));
                    }
                }
            }
        } else if *key == positional_key {
            positional = v.first().cloned();
        } else if let Some(x) = v.first() {
            query.retain(|(k, _)| k != key);
            query.push(((*key).to_owned(), x.clone()));
        }
    }
    let mut out = prefix.to_owned();
    if let Some(p) = positional {
        out.push(':');
        out.push_str(&normalize_component(&encode_component(&p)));
    }
    if !query.is_empty() {
        let q = query
            .iter()
            .map(|(k, v)| format!("{}={}", encode_form(k), encode_form(v)))
            .collect::<Vec<_>>()
            .join("&");
        out.push('?');
        out.push_str(&normalize_component(&q));
    }
    out
}

fn same_set(a: &[String], b: &[&str]) -> bool {
    a.iter().all(|x| b.contains(&x.as_str())) && b.iter().all(|x| a.iter().any(|y| y == x))
}

fn is_nsid(p: &Param) -> bool {
    p.as_str()
        .is_some_and(|s| proto_blue_syntax::Nsid::new(s).is_ok())
}

fn is_collection(p: &Param) -> bool {
    p.as_str() == Some("*") || is_nsid(p)
}

fn one_of(values: &'static [&'static str]) -> impl Fn(&Param) -> bool {
    move |p| p.as_str().is_some_and(|s| values.contains(&s))
}

/// An atproto DID (plc or web) with a `#fragment`.
#[must_use]
pub fn is_did_ref_absolute(s: &str) -> bool {
    let Some((did, frag)) = s.split_once('#') else {
        return false;
    };
    !frag.is_empty()
        && (did.starts_with("did:plc:") || did.starts_with("did:web:"))
        && proto_blue_syntax::Did::new(did).is_ok()
}

fn is_aud(p: &Param) -> bool {
    p.as_str()
        .is_some_and(|s| s == "*" || is_did_ref_absolute(s))
}

// ---------------------------------------------------------------- mime

fn is_slash_pair(v: &str) -> bool {
    let Some(i) = v.find('/') else { return false };
    i > 0 && i < v.len() - 1 && !v[i + 1..].contains('/') && !v.contains(' ')
}

fn is_mime(v: &str) -> bool {
    is_slash_pair(v) && !v.contains('*')
}

fn is_accept(p: &Param) -> bool {
    let Some(v) = p.as_str() else { return false };
    v == "*/*" || (is_slash_pair(v) && (!v.contains('*') || v.ends_with("/*")))
}

fn accept_matches(accept: &str, mime: &str) -> bool {
    accept == "*/*"
        || accept
            .strip_suffix('*')
            .filter(|_| accept.ends_with("/*"))
            .map_or(accept == mime, |base| mime.starts_with(base))
}

// ---------------------------------------------------------------- permissions

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoPermission {
    pub collection: Vec<String>,
    pub action: Vec<String>,
}

impl RepoPermission {
    fn fields() -> [Field; 2] {
        [
            Field {
                key: "collection",
                multiple: true,
                required: true,
                validate: is_collection,
            },
            Field {
                key: "action",
                multiple: true,
                required: false,
                validate: |p| one_of(REPO_ACTIONS)(p),
            },
        ]
    }

    #[must_use]
    pub fn from_syntax(s: &Syntax) -> Option<Self> {
        if s.prefix != "repo" {
            return None;
        }
        let got = parse_fields(s, &Self::fields(), "collection")?;
        let collection = match &got[0] {
            Got::Many(v) => v.clone(),
            _ => return None,
        };
        let action = match &got[1] {
            Got::Many(v) => v.clone(),
            _ => REPO_ACTIONS.iter().map(|s| (*s).to_owned()).collect(),
        };
        Some(Self { collection, action })
    }

    #[must_use]
    pub fn parse(scope: &str) -> Option<Self> {
        if !is_for(scope, "repo") {
            return None;
        }
        Self::from_syntax(&Syntax::from_scope_str(scope)?)
    }

    #[must_use]
    pub fn matches(&self, collection: &str, action: &str) -> bool {
        self.action.iter().any(|a| a == action)
            && self.collection.iter().any(|c| c == "*" || c == collection)
    }

    fn canonical(collection: &[String], action: &[String]) -> String {
        let collection: Vec<String> = if collection.len() > 1 {
            if collection.iter().any(|c| c == "*") {
                vec!["*".into()]
            } else {
                let mut c = collection.to_vec();
                c.sort();
                c.dedup();
                c
            }
        } else {
            collection.to_vec()
        };
        let action: Vec<String> = REPO_ACTIONS
            .iter()
            .filter(|a| action.iter().any(|x| x == *a))
            .map(|a| (*a).to_owned())
            .collect();
        let action = (!same_set(&action, REPO_ACTIONS)).then_some(action);
        format(
            "repo",
            "collection",
            &[
                ("collection", Some(collection), true),
                ("action", action, true),
            ],
        )
    }

    /// The scope a request needs (for "Missing required scope").
    #[must_use]
    pub fn needed(collection: &str, action: &str) -> String {
        Self::canonical(&[collection.to_owned()], &[action.to_owned()])
    }
}

impl std::fmt::Display for RepoPermission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&Self::canonical(&self.collection, &self.action))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobPermission {
    pub accept: Vec<String>,
}

impl BlobPermission {
    #[must_use]
    pub fn parse(scope: &str) -> Option<Self> {
        if !is_for(scope, "blob") {
            return None;
        }
        let s = Syntax::from_scope_str(scope)?;
        let got = parse_fields(
            &s,
            &[Field {
                key: "accept",
                multiple: true,
                required: true,
                validate: is_accept,
            }],
            "accept",
        )?;
        match &got[0] {
            Got::Many(v) => Some(Self { accept: v.clone() }),
            _ => None,
        }
    }

    #[must_use]
    pub fn matches(&self, mime: &str) -> bool {
        is_mime(mime) && self.accept.iter().any(|a| accept_matches(a, mime))
    }

    fn canonical(accept: &[String]) -> String {
        let accept: Vec<String> = if accept.iter().any(|a| a == "*/*") {
            vec!["*/*".into()]
        } else {
            let lower: Vec<String> = accept.iter().map(|a| a.to_lowercase()).collect();
            let mut kept: Vec<String> = lower
                .iter()
                .filter(|v| {
                    if v.ends_with("/*") {
                        return true;
                    }
                    let base = v.split('/').next().unwrap_or("");
                    !lower.iter().any(|o| *o == format!("{base}/*"))
                })
                .cloned()
                .collect();
            kept.sort();
            kept
        };
        format("blob", "accept", &[("accept", Some(accept), true)])
    }

    #[must_use]
    pub fn needed(mime: &str) -> String {
        Self::canonical(&[mime.to_owned()])
    }
}

impl std::fmt::Display for BlobPermission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&Self::canonical(&self.accept))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcPermission {
    pub aud: String,
    pub lxm: Vec<String>,
}

impl RpcPermission {
    #[must_use]
    pub fn from_syntax(s: &Syntax) -> Option<Self> {
        if s.prefix != "rpc" {
            return None;
        }
        let got = parse_fields(
            s,
            &[
                Field {
                    key: "lxm",
                    multiple: true,
                    required: true,
                    validate: is_collection,
                },
                Field {
                    key: "aud",
                    multiple: false,
                    required: true,
                    validate: is_aud,
                },
            ],
            "lxm",
        )?;
        let (Got::Many(lxm), Got::One(aud)) = (&got[0], &got[1]) else {
            return None;
        };
        // rpc:*?aud=* is forbidden.
        if aud == "*" && lxm.iter().any(|l| l == "*") {
            return None;
        }
        Some(Self {
            aud: aud.clone(),
            lxm: lxm.clone(),
        })
    }

    #[must_use]
    pub fn parse(scope: &str) -> Option<Self> {
        if !is_for(scope, "rpc") {
            return None;
        }
        Self::from_syntax(&Syntax::from_scope_str(scope)?)
    }

    #[must_use]
    pub fn matches(&self, lxm: &str, aud: &str) -> bool {
        (self.aud == "*" || self.aud == aud) && self.lxm.iter().any(|l| l == "*" || l == lxm)
    }

    fn canonical(aud: &str, lxm: &[String]) -> String {
        let lxm: Vec<String> = if lxm.len() > 1 && lxm.iter().any(|l| l == "*") {
            vec!["*".into()]
        } else {
            let mut l = lxm.to_vec();
            l.sort();
            l.dedup();
            l
        };
        format(
            "rpc",
            "lxm",
            &[
                ("lxm", Some(lxm), true),
                ("aud", Some(vec![aud.to_owned()]), false),
            ],
        )
    }

    #[must_use]
    pub fn needed(lxm: &str, aud: &str) -> String {
        Self::canonical(aud, &[lxm.to_owned()])
    }
}

impl std::fmt::Display for RpcPermission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&Self::canonical(&self.aud, &self.lxm))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPermission {
    pub attr: String,
    pub action: Vec<String>,
}

impl AccountPermission {
    #[must_use]
    pub fn parse(scope: &str) -> Option<Self> {
        if !is_for(scope, "account") {
            return None;
        }
        let s = Syntax::from_scope_str(scope)?;
        let got = parse_fields(
            &s,
            &[
                Field {
                    key: "attr",
                    multiple: false,
                    required: true,
                    validate: |p| one_of(ACCOUNT_ATTRIBUTES)(p),
                },
                Field {
                    key: "action",
                    multiple: true,
                    required: false,
                    validate: |p| one_of(ACCOUNT_ACTIONS)(p),
                },
            ],
            "attr",
        )?;
        let Got::One(attr) = &got[0] else { return None };
        let action = match &got[1] {
            Got::Many(v) => v.clone(),
            _ => vec!["read".into()],
        };
        Some(Self {
            attr: attr.clone(),
            action,
        })
    }

    #[must_use]
    pub fn matches(&self, attr: &str, action: &str) -> bool {
        self.attr == attr && self.action.iter().any(|a| a == "manage" || a == action)
    }

    fn canonical(attr: &str, action: &[String]) -> String {
        let action = (!same_set(action, &["read"])).then(|| action.to_vec());
        format(
            "account",
            "attr",
            &[
                ("attr", Some(vec![attr.to_owned()]), false),
                ("action", action, true),
            ],
        )
    }

    #[must_use]
    pub fn needed(attr: &str, action: &str) -> String {
        Self::canonical(attr, &[action.to_owned()])
    }
}

impl std::fmt::Display for AccountPermission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&Self::canonical(&self.attr, &self.action))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityPermission {
    pub attr: String,
}

impl IdentityPermission {
    #[must_use]
    pub fn parse(scope: &str) -> Option<Self> {
        if !is_for(scope, "identity") {
            return None;
        }
        let s = Syntax::from_scope_str(scope)?;
        let got = parse_fields(
            &s,
            &[Field {
                key: "attr",
                multiple: false,
                required: true,
                validate: |p| one_of(IDENTITY_ATTRIBUTES)(p),
            }],
            "attr",
        )?;
        let Got::One(attr) = &got[0] else { return None };
        Some(Self { attr: attr.clone() })
    }

    #[must_use]
    pub fn matches(&self, attr: &str) -> bool {
        self.attr == "*" || self.attr == attr
    }

    #[must_use]
    pub fn needed(attr: &str) -> String {
        format(
            "identity",
            "attr",
            &[("attr", Some(vec![attr.to_owned()]), false)],
        )
    }
}

impl std::fmt::Display for IdentityPermission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&Self::needed(&self.attr))
    }
}

// ---------------------------------------------------------------- include

/// `include:<nsid>[?aud=<did#svc>]`: a lexicon permission set's `repo:` and
/// `rpc:` permissions, limited to the set's own namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncludeScope {
    pub nsid: String,
    pub aud: Option<String>,
}

impl IncludeScope {
    #[must_use]
    pub fn parse(scope: &str) -> Option<Self> {
        if !is_for(scope, "include") {
            return None;
        }
        let s = Syntax::from_scope_str(scope)?;
        let got = parse_fields(
            &s,
            &[
                Field {
                    key: "nsid",
                    multiple: false,
                    required: true,
                    validate: is_nsid,
                },
                Field {
                    key: "aud",
                    multiple: false,
                    required: false,
                    validate: |p| p.as_str().is_some_and(is_did_ref_absolute),
                },
            ],
            "nsid",
        )?;
        let Got::One(nsid) = &got[0] else { return None };
        let aud = match &got[1] {
            Got::One(a) => Some(a.clone()),
            _ => None,
        };
        Some(Self {
            nsid: nsid.clone(),
            aud,
        })
    }

    /// Whether `other` is under this set's namespace authority (everything
    /// up to the set's last `.`).
    #[must_use]
    pub fn is_parent_authority_of(&self, other: &str) -> bool {
        if other == "*" {
            return false;
        }
        let Some(group_end) = self.nsid.rfind('.') else {
            return false;
        };
        if group_end + 1 >= other.len() {
            return false;
        }
        other.as_bytes().get(..=group_end) == self.nsid.as_bytes().get(..=group_end)
    }

    /// The canonical `repo:` / `rpc:` scopes a permission set (lexicon
    /// `permission-set` JSON) grants through this include.
    #[must_use]
    pub fn expand(&self, permission_set: &Value) -> Vec<String> {
        let Some(perms) = permission_set.get("permissions").and_then(Value::as_array) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for p in perms {
            let Some(map) = p.as_object() else { continue };
            let Some(syntax) = Syntax::from_lex(map) else {
                continue;
            };
            match syntax.prefix.as_str() {
                "repo" => {
                    if let Some(r) = RepoPermission::from_syntax(&syntax)
                        && r.collection.iter().all(|c| self.is_parent_authority_of(c))
                    {
                        out.push(r.to_string());
                    }
                }
                "rpc" => {
                    let aud = map.get("aud");
                    if aud.is_some_and(|a| a != "*") {
                        continue; // a set can't name an audience
                    }
                    let syntax = if map.get("inheritAud") == Some(&Value::Bool(true))
                        && aud.is_none()
                        && let Some(a) = &self.aud
                    {
                        syntax.without("inheritAud").with_single("aud", a)
                    } else {
                        syntax
                    };
                    if let Some(r) = RpcPermission::from_syntax(&syntax)
                        && r.lxm.iter().all(|l| self.is_parent_authority_of(l))
                    {
                        out.push(r.to_string());
                    }
                }
                _ => {}
            }
        }
        out
    }
}

impl std::fmt::Display for IncludeScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let aud = self.aud.clone().map(|a| vec![a]);
        f.write_str(&format(
            "include",
            "nsid",
            &[
                ("nsid", Some(vec![self.nsid.clone()]), false),
                ("aud", aud, false),
            ],
        ))
    }
}

// ---------------------------------------------------------------- whole scopes

/// The canonical form of one scope value; None if locus can't interpret it.
#[must_use]
pub fn normalize_value(value: &str) -> Option<String> {
    if STATIC_SCOPES.contains(&value) {
        return Some(value.to_owned());
    }
    AccountPermission::parse(value)
        .map(|p| p.to_string())
        .or_else(|| BlobPermission::parse(value).map(|p| p.to_string()))
        .or_else(|| IdentityPermission::parse(value).map(|p| p.to_string()))
        .or_else(|| IncludeScope::parse(value).map(|p| p.to_string()))
        .or_else(|| RepoPermission::parse(value).map(|p| p.to_string()))
        .or_else(|| RpcPermission::parse(value).map(|p| p.to_string()))
}

/// Whether locus can interpret a scope value.
#[must_use]
pub fn is_atproto_scope(value: &str) -> bool {
    normalize_value(value).is_some()
}

/// A space-separated scope, each value normalized, unknown ones dropped,
/// sorted.
#[must_use]
pub fn normalize(scope: &str) -> String {
    let mut values: Vec<String> = scope.split(' ').filter_map(normalize_value).collect();
    values.sort();
    values.join(" ")
}

/// A scope with its permission checks, as the reference's
/// `ScopePermissionsTransition` (the `transition:*` values grant what they
/// did before granular scopes).
#[derive(Debug, Clone, Default)]
pub struct Permissions {
    values: Vec<String>,
}

/// A missing scope: the canonical scope the request needed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Missing required scope \"{0}\"")]
pub struct ScopeMissing(pub String);

impl Permissions {
    #[must_use]
    pub fn new(scope: &str) -> Self {
        Self {
            values: scope
                .split(' ')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
        }
    }

    #[must_use]
    pub fn has(&self, value: &str) -> bool {
        self.values.iter().any(|v| v == value)
    }

    fn generic(&self) -> bool {
        self.has("transition:generic")
    }

    #[must_use]
    pub fn allows_repo(&self, collection: &str, action: &str) -> bool {
        self.generic()
            || self
                .values
                .iter()
                .filter_map(|v| RepoPermission::parse(v))
                .any(|p| p.matches(collection, action))
    }

    #[must_use]
    pub fn allows_blob(&self, mime: &str) -> bool {
        self.generic()
            || self
                .values
                .iter()
                .filter_map(|v| BlobPermission::parse(v))
                .any(|p| p.matches(mime))
    }

    #[must_use]
    pub fn allows_rpc(&self, lxm: &str, aud: &str) -> bool {
        if self.generic() && (lxm == "*" || !lxm.starts_with("chat.bsky.")) {
            return true;
        }
        if self.has("transition:chat.bsky") && lxm.starts_with("chat.bsky.") {
            return true;
        }
        self.values
            .iter()
            .filter_map(|v| RpcPermission::parse(v))
            .any(|p| p.matches(lxm, aud))
    }

    #[must_use]
    pub fn allows_account(&self, attr: &str, action: &str) -> bool {
        if attr == "email" && action == "read" && self.has("transition:email") {
            return true;
        }
        self.values
            .iter()
            .filter_map(|v| AccountPermission::parse(v))
            .any(|p| p.matches(attr, action))
    }

    #[must_use]
    pub fn allows_identity(&self, attr: &str) -> bool {
        self.values
            .iter()
            .filter_map(|v| IdentityPermission::parse(v))
            .any(|p| p.matches(attr))
    }

    pub fn assert_repo(&self, collection: &str, action: &str) -> Result<(), ScopeMissing> {
        self.allows_repo(collection, action)
            .then_some(())
            .ok_or_else(|| ScopeMissing(RepoPermission::needed(collection, action)))
    }

    pub fn assert_blob(&self, mime: &str) -> Result<(), ScopeMissing> {
        self.allows_blob(mime)
            .then_some(())
            .ok_or_else(|| ScopeMissing(BlobPermission::needed(mime)))
    }

    pub fn assert_rpc(&self, lxm: &str, aud: &str) -> Result<(), ScopeMissing> {
        self.allows_rpc(lxm, aud)
            .then_some(())
            .ok_or_else(|| ScopeMissing(RpcPermission::needed(lxm, aud)))
    }

    pub fn assert_account(&self, attr: &str, action: &str) -> Result<(), ScopeMissing> {
        self.allows_account(attr, action)
            .then_some(())
            .ok_or_else(|| ScopeMissing(AccountPermission::needed(attr, action)))
    }

    pub fn assert_identity(&self, attr: &str) -> Result<(), ScopeMissing> {
        self.allows_identity(attr)
            .then_some(())
            .ok_or_else(|| ScopeMissing(IdentityPermission::needed(attr)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn repo_scopes_normalize_as_the_reference() {
        for (input, expected) in [
            ("repo:com.example.foo", "repo:com.example.foo"),
            (
                "repo:com.example.foo?action=create",
                "repo:com.example.foo?action=create",
            ),
            (
                "repo:com.example.foo?action=create&action=update",
                "repo:com.example.foo?action=create&action=update",
            ),
            ("repo:*?action=create&action=update&action=delete", "repo:*"),
            (
                "repo:com.example.foo?action=create&action=update&action=delete",
                "repo:com.example.foo",
            ),
            ("repo:*?action=create", "repo:*?action=create"),
            ("repo?collection=*&action=update", "repo:*?action=update"),
            (
                "repo?collection=*&collection=com.example.foo&action=update",
                "repo:*?action=update",
            ),
            ("repo?collection=*", "repo:*"),
            ("repo?collection=*&collection=com.example.foo", "repo:*"),
            (
                "repo?action=create&collection=com.example.foo",
                "repo:com.example.foo?action=create",
            ),
            (
                "repo?action=create&collection=com.example.foo&collection=com.example.bar",
                "repo?collection=com.example.bar&collection=com.example.foo&action=create",
            ),
        ] {
            assert_eq!(
                RepoPermission::parse(input)
                    .map(|p| p.to_string())
                    .as_deref(),
                Some(expected),
                "{input}"
            );
        }
        for bad in [
            "repo:foo bar",
            "repo:.foo",
            "repo:bar.",
            "invalid",
            "scope",
            "repo",
            "repo:com.example.foo?action=read",
            "repo:com.example.foo?other=1",
            "repo:com.example.foo?collection=com.example.bar",
        ] {
            assert!(RepoPermission::parse(bad).is_none(), "{bad}");
        }
        assert_eq!(
            RepoPermission::needed("app.bsky.feed.post", "create"),
            "repo:app.bsky.feed.post?action=create"
        );
        let p = RepoPermission::parse("repo:*?action=create").unwrap();
        assert!(p.matches("com.example.foo", "create"));
        assert!(!p.matches("com.example.foo", "delete"));
    }

    #[test]
    fn blob_rpc_account_identity_include() {
        let blob = |v: &[&str]| {
            BlobPermission::canonical(&v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())
        };
        assert_eq!(
            blob(&["image/png", "image/jpeg"]),
            "blob?accept=image/jpeg&accept=image/png"
        );
        assert_eq!(blob(&["*/*", "image/*"]), "blob:*/*");
        assert_eq!(blob(&["image/*", "image/png"]), "blob:image/*");
        assert_eq!(blob(&["image/png"]), "blob:image/png");
        let b = BlobPermission::parse("blob:image/*").unwrap();
        assert!(b.matches("image/png") && !b.matches("video/mp4") && !b.matches("image/*"));
        assert!(BlobPermission::parse("blob").is_none());

        let rpc =
            RpcPermission::parse("rpc:com.example.service?aud=did:web:example.com%23service_id")
                .unwrap();
        assert_eq!(rpc.aud, "did:web:example.com#service_id");
        assert_eq!(
            rpc.to_string(),
            "rpc:com.example.service?aud=did:web:example.com%23service_id"
        );
        assert_eq!(
            RpcPermission::parse("rpc?lxm=com.example.method1&aud=*")
                .unwrap()
                .to_string(),
            "rpc:com.example.method1?aud=*"
        );
        assert_eq!(
            RpcPermission::parse("rpc?aud=*&lxm=com.example.method2&lxm=com.example.method1")
                .unwrap()
                .to_string(),
            "rpc?lxm=com.example.method1&lxm=com.example.method2&aud=*"
        );
        assert!(RpcPermission::parse("rpc:*?aud=*").is_none(), "forbidden");
        assert!(
            RpcPermission::parse("rpc:com.example.m?aud=did:web:example.com").is_none(),
            "aud needs a fragment"
        );
        assert!(rpc.matches("com.example.service", "did:web:example.com#service_id"));
        assert!(!rpc.matches("com.example.OtherService", "did:web:example.com#service_id"));
        assert_eq!(
            RpcPermission::needed(
                "app.bsky.feed.getTimeline",
                "did:web:api.bsky.app#bsky_appview"
            ),
            "rpc:app.bsky.feed.getTimeline?aud=did:web:api.bsky.app%23bsky_appview"
        );

        for s in [
            "account:email",
            "account:email?action=manage",
            "account:repo",
            "account:repo?action=manage",
            "account:status",
            "account:status?action=manage",
        ] {
            assert_eq!(AccountPermission::parse(s).unwrap().to_string(), s);
        }
        assert_eq!(
            AccountPermission::parse("account:email?action=read")
                .unwrap()
                .to_string(),
            "account:email"
        );
        for bad in [
            "account:invalid",
            "invalid:email",
            "account",
            "",
            "account:",
        ] {
            assert!(AccountPermission::parse(bad).is_none(), "{bad}");
        }
        assert!(
            AccountPermission::parse("account:repo?action=manage")
                .unwrap()
                .matches("repo", "read")
        );
        assert_eq!(
            IdentityPermission::parse("identity:*").unwrap().to_string(),
            "identity:*"
        );
        assert!(IdentityPermission::parse("identity:invalid").is_none());

        assert_eq!(
            IncludeScope::parse("include:com.example.baz?aud=did:web:example.com#my_service")
                .unwrap(),
            IncludeScope {
                nsid: "com.example.baz".into(),
                aud: Some("did:web:example.com#my_service".into())
            }
        );
        assert_eq!(
            IncludeScope::parse("include?nsid=com.example.baz")
                .unwrap()
                .aud,
            None
        );
        for bad in [
            "",
            "repo:com.example.baz",
            "include",
            "include#",
            "include:",
            "include:com..example",
            "include:com",
            "include:com.example",
            "include:com.example.baz?aud=",
            "include:com.example.baz?aud=did:web:example.com",
        ] {
            assert!(IncludeScope::parse(bad).is_none(), "{bad:?}");
        }
        assert_eq!(
            IncludeScope {
                nsid: "com.example.foo".into(),
                aud: Some("did:web:example.com#my_service".into())
            }
            .to_string(),
            "include:com.example.foo?aud=did:web:example.com%23my_service"
        );
        let inc = IncludeScope::parse("include:com.example.foo.auth").unwrap();
        assert!(inc.is_parent_authority_of("com.example.foo.identifier"));
        assert!(inc.is_parent_authority_of("com.example.foo.bar.baz"));
        for no in [
            "com",
            "com.example",
            "com.example.bar",
            "com.atproto.foo.auth",
            "*",
        ] {
            assert!(!inc.is_parent_authority_of(no), "{no}");
        }
    }

    #[test]
    fn permission_sets_expand_within_their_namespace() {
        let inc =
            IncludeScope::parse("include:com.example.calendar.auth?aud=did:web:example.com#foo")
                .unwrap();
        let set = json!({"type": "permission-set", "permissions": [
            {"type": "permission", "resource": "blob", "accept": ["image/*"]},
            {"type": "permission", "resource": "rpc", "inheritAud": true, "lxm": ["com.example.calendar.listEvents"]},
            {"type": "permission", "resource": "rpc", "aud": "*", "lxm": ["com.example.calendar.getEvent"]},
            {"type": "permission", "resource": "rpc", "aud": "did:web:other.com#x", "lxm": ["com.example.calendar.x"]},
            {"type": "permission", "resource": "rpc", "aud": "*", "lxm": ["com.other.thing"]},
            {"type": "permission", "resource": "repo", "collection": ["com.example.calendar.event"], "action": ["create"]},
            {"type": "permission", "resource": "repo", "collection": ["*"]},
        ]});
        assert_eq!(
            inc.expand(&set),
            vec![
                "rpc:com.example.calendar.listEvents?aud=did:web:example.com%23foo".to_owned(),
                "rpc:com.example.calendar.getEvent?aud=*".to_owned(),
                "repo:com.example.calendar.event?action=create".to_owned(),
            ]
        );
        // inheritAud without an aud on the include stays unparseable.
        let bare = IncludeScope::parse("include:com.example.calendar.auth").unwrap();
        let only_inherit = json!({"permissions": [{"type": "permission", "resource": "rpc", "inheritAud": true, "lxm": ["com.example.calendar.listEvents"]}]});
        assert!(bare.expand(&only_inherit).is_empty());
    }

    #[test]
    fn transition_scopes_and_normalize() {
        let p = Permissions::new("atproto transition:generic");
        assert!(p.allows_repo("app.bsky.feed.post", "create"));
        assert!(p.allows_blob("image/png"));
        assert!(p.allows_rpc("app.bsky.feed.getTimeline", "did:web:x#y"));
        assert!(!p.allows_rpc("chat.bsky.convo.sendMessage", "did:web:x#y"));
        assert!(!p.allows_account("email", "read"));
        let chat = Permissions::new("atproto transition:chat.bsky transition:email");
        assert!(chat.allows_rpc("chat.bsky.convo.sendMessage", "did:web:x#y"));
        assert!(chat.allows_account("email", "read") && !chat.allows_account("email", "manage"));
        let granular = Permissions::new("atproto repo:app.bsky.feed.post?action=create");
        assert_eq!(
            granular
                .assert_repo("app.bsky.feed.post", "delete")
                .unwrap_err()
                .to_string(),
            "Missing required scope \"repo:app.bsky.feed.post?action=delete\""
        );
        assert_eq!(
            normalize(
                "repo:com.example.foo?action=create&action=update&action=delete nonsense atproto blob:image/png"
            ),
            "atproto blob:image/png repo:com.example.foo"
        );
    }
}
