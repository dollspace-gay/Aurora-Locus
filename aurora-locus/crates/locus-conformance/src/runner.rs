//! Runs steps against both servers and compares what they answer.
//!
//! A step goes to the server under test (`Own`), to the other server
//! (`Peer`: the source of a migration, its answers not compared), to the
//! shared mock PLC (`Plc`), or reads the newest email a server sent
//! (`Mail`). Captures (`{{var}}`, `{{peer.var}}` for the other server's)
//! are taken from the raw response before normalizing; a string that is
//! exactly `{{var}}` takes the captured JSON value as it is.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use locus_auth::oauth::jose::Es256Key;
use serde_json::{Map, Value, json};

use crate::normalize::{Diff, Normalizer, diff};

/// Who a step authenticates as.
#[derive(Debug, Clone, Copy)]
pub enum Auth {
    None,
    /// The access token captured as `<name>.access`.
    User(&'static str),
    /// The refresh token captured as `<name>.refresh`.
    Refresh(&'static str),
    /// A token no server issued.
    Bogus,
    /// Basic auth with the admin password.
    Admin,
    /// A bearer token captured as this variable (a service JWT).
    Var(&'static str),
}

#[derive(Debug, Clone)]
pub enum Body {
    None,
    Json(Value),
    Bytes(&'static [u8], &'static str),
    /// Bytes captured with `keep_bytes`, and their content type.
    Var(&'static str, &'static str),
    /// `application/x-www-form-urlencoded`.
    Form(Vec<(&'static str, String)>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Own,
    Peer,
    Plc,
    Mail,
}

/// How a step signs a DPoP proof with the server's client key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dpop {
    None,
    /// A proof without a nonce.
    Fresh,
    /// A proof with the nonce the server last sent.
    Nonce,
}

/// One request, made the same way to both servers.
#[derive(Debug, Clone)]
pub struct Step {
    pub name: String,
    pub get: bool,
    pub nsid: &'static str,
    /// A non-XRPC path (`/oauth/par`), in place of `/xrpc/<nsid>`.
    pub path: Option<&'static str>,
    pub query: Vec<(&'static str, String)>,
    pub auth: Auth,
    pub body: Body,
    pub headers: Vec<(&'static str, String)>,
    /// Variables to capture from the response: (name, JSON pointer).
    pub capture: Vec<(String, String)>,
    /// A variable to capture the raw response body into.
    pub capture_bytes: Option<String>,
    /// Response headers whose presence is compared (as `/$headers/<name>`).
    pub show_headers: Vec<&'static str>,
    pub target: Target,
    pub dpop: Dpop,
    /// A firehose subscription from this cursor (its frames as the body).
    pub subscribe: Option<String>,
    /// Arrays (JSON pointers) whose order isn't specified: sorted before
    /// comparing.
    pub unordered: Vec<&'static str>,
    pub wait_ms: u64,
}

impl Step {
    pub fn get(name: &str, nsid: &'static str) -> Self {
        Self {
            name: name.to_owned(),
            get: true,
            nsid,
            path: None,
            query: Vec::new(),
            auth: Auth::None,
            body: Body::None,
            headers: Vec::new(),
            capture: Vec::new(),
            capture_bytes: None,
            show_headers: Vec::new(),
            target: Target::Own,
            dpop: Dpop::None,
            subscribe: None,
            unordered: Vec::new(),
            wait_ms: 0,
        }
    }
    /// A procedure with no input (no body at all).
    pub fn call(name: &str, nsid: &'static str) -> Self {
        Self {
            get: false,
            ..Self::get(name, nsid)
        }
    }
    pub fn post(name: &str, nsid: &'static str, body: Value) -> Self {
        Self {
            get: false,
            body: Body::Json(body),
            ..Self::get(name, nsid)
        }
    }
    /// A GET of a non-XRPC path.
    pub fn page(name: &str, path: &'static str) -> Self {
        Self {
            path: Some(path),
            ..Self::get(name, "")
        }
    }
    /// A form POST to a non-XRPC path (the OAuth endpoints).
    pub fn form(name: &str, path: &'static str, form: &[(&'static str, &str)]) -> Self {
        Self {
            get: false,
            path: Some(path),
            body: Body::Form(form.iter().map(|(k, v)| (*k, (*v).to_owned())).collect()),
            ..Self::get(name, "")
        }
    }
    /// A firehose subscription from `cursor` (empty: none).
    pub fn subscribe(name: &str, cursor: &str) -> Self {
        Self {
            subscribe: Some(cursor.to_owned()),
            ..Self::get(name, "com.atproto.sync.subscribeRepos")
        }
    }
    /// An operation posted to the mock PLC for `did`.
    pub fn plc(name: &str, did: &'static str, op: Value) -> Self {
        Self {
            target: Target::Plc,
            ..Self::post(name, did, op)
        }
    }
    /// A pause, so events already written reach the firehose before what
    /// follows (the reference sequences from its database asynchronously).
    pub fn wait(name: &str, ms: u64) -> Self {
        Self {
            wait_ms: ms,
            ..Self::get(name, "")
        }
    }
    /// The token in the newest email this server sent to `to`.
    pub fn mail(name: &str, to: &'static str) -> Self {
        Self {
            target: Target::Mail,
            ..Self::get(name, to)
        }
    }
    #[must_use]
    pub fn q(mut self, k: &'static str, v: &str) -> Self {
        self.query.push((k, v.to_owned()));
        self
    }
    #[must_use]
    pub fn auth(mut self, a: Auth) -> Self {
        self.auth = a;
        self
    }
    #[must_use]
    pub fn bytes(mut self, b: &'static [u8], mime: &'static str) -> Self {
        self.get = false;
        self.body = Body::Bytes(b, mime);
        self
    }
    /// Sends bytes captured earlier.
    #[must_use]
    pub fn bytes_var(mut self, var: &'static str, mime: &'static str) -> Self {
        self.get = false;
        self.body = Body::Var(var, mime);
        self
    }
    #[must_use]
    pub fn header(mut self, k: &'static str, v: &str) -> Self {
        self.headers.push((k, v.to_owned()));
        self
    }
    #[must_use]
    pub fn keep(mut self, var: &str, pointer: &str) -> Self {
        self.capture.push((var.to_owned(), pointer.to_owned()));
        self
    }
    #[must_use]
    pub fn keep_bytes(mut self, var: &str) -> Self {
        self.capture_bytes = Some(var.to_owned());
        self
    }
    #[must_use]
    pub fn show(mut self, header: &'static str) -> Self {
        self.show_headers.push(header);
        self
    }
    /// Sent to the other server, not compared.
    #[must_use]
    pub fn on_peer(mut self) -> Self {
        self.target = Target::Peer;
        self
    }
    #[must_use]
    pub fn unordered(mut self, pointer: &'static str) -> Self {
        self.unordered.push(pointer);
        self
    }
    #[must_use]
    pub fn dpop(mut self, d: Dpop) -> Self {
        self.dpop = d;
        self
    }
}

/// One server under test.
pub struct Server {
    pub base: String,
    pub admin_password: String,
    pub norm: Normalizer,
    pub vars: HashMap<String, Value>,
    pub blobs: HashMap<String, Vec<u8>>,
    /// The shared mock PLC.
    pub plc: String,
    /// The mail catcher's API and the address this server sends from.
    pub mail: Option<(String, String)>,
    /// The OAuth client's DPoP key and the nonce this server last sent.
    dpop_key: Es256Key,
    dpop_nonce: Option<String>,
    client: reqwest::Client,
}

/// What a server answered, normalized: status, error name, body.
#[derive(Debug, Clone)]
pub struct Answer {
    pub status: u16,
    pub body: Value,
    /// The error message, shown with a difference but never compared.
    pub message: Option<String>,
}

fn b64url(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl Server {
    pub fn new(base: &str, admin_password: &str, plc: &str) -> Self {
        let host = base
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .trim_end_matches('/')
            .to_owned();
        Self {
            base: base.trim_end_matches('/').to_owned(),
            admin_password: admin_password.to_owned(),
            norm: Normalizer::new(&host),
            vars: HashMap::new(),
            blobs: HashMap::new(),
            plc: plc.trim_end_matches('/').to_owned(),
            mail: None,
            dpop_key: Es256Key::generate(),
            dpop_nonce: None,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_default(),
        }
    }

    fn var<'a>(&'a self, peer: &'a HashMap<String, Value>, k: &str) -> Option<&'a Value> {
        match k.strip_prefix("peer.") {
            Some(p) => peer.get(p),
            None => self.vars.get(k),
        }
    }

    /// `{{var}}` replaced in every string.
    fn fill(&self, peer: &HashMap<String, Value>, v: &Value) -> Value {
        match v {
            Value::String(s) => {
                if let Some(k) = s.strip_prefix("{{").and_then(|r| r.strip_suffix("}}"))
                    && !k.contains('}')
                    && let Some(x) = self.var(peer, k)
                {
                    return x.clone();
                }
                Value::String(self.fill_str(peer, s))
            }
            Value::Array(a) => Value::Array(a.iter().map(|x| self.fill(peer, x)).collect()),
            Value::Object(m) => Value::Object(
                m.iter()
                    .map(|(k, x)| (k.clone(), self.fill(peer, x)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    fn fill_str(&self, peer: &HashMap<String, Value>, s: &str) -> String {
        let mut out = s.to_owned();
        while let Some(i) = out.find("{{") {
            let Some(j) = out[i..].find("}}") else { break };
            let k = &out[i + 2..i + j];
            let v = self.var(peer, k).map_or_else(String::new, |v| {
                v.as_str().map_or_else(|| v.to_string(), str::to_owned)
            });
            out.replace_range(i..i + j + 2, &v);
        }
        out
    }

    fn token(&self, peer: &HashMap<String, Value>, k: &str) -> String {
        self.var(peer, k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    fn proof(&self, method: &str, url: &str, with_nonce: bool) -> String {
        let mut header = Map::new();
        header.insert("typ".into(), json!("dpop+jwt"));
        let mut jwk = self.dpop_key.jwk();
        if let Some(m) = jwk.as_object_mut() {
            for k in ["kid", "use", "alg"] {
                m.remove(k);
            }
        }
        header.insert("jwk".into(), jwk);
        let mut claims = json!({
            "jti": b64url(&rand::random::<[u8; 16]>()),
            "htm": method,
            "htu": url.split('?').next().unwrap_or(url),
            "iat": now(),
        });
        if with_nonce && let Some(n) = &self.dpop_nonce {
            claims["nonce"] = json!(n);
        }
        self.dpop_key.sign(header, &claims)
    }

    pub async fn run(&mut self, step: &Step, peer: &Server) -> Answer {
        let pv = &peer.vars;
        if step.wait_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(step.wait_ms)).await;
            return Answer {
                status: 0,
                body: Value::Null,
                message: None,
            };
        }
        if let Some(cursor) = &step.subscribe {
            let cursor = self.fill_str(pv, cursor);
            let (frames, last) = crate::firehose::backfill(&self.base, &cursor).await;
            if let Some(last) = last {
                for (var, _) in &step.capture {
                    self.vars.insert(var.clone(), json!(last));
                }
            }
            let body = self.norm.value(&Value::Array(frames));
            return Answer {
                status: 101,
                body,
                message: None,
            };
        }
        if step.target == Target::Mail {
            return self.mail_token(step).await;
        }
        let base = match step.target {
            Target::Peer => peer.base.clone(),
            Target::Plc => self.plc.clone(),
            _ => self.base.clone(),
        };
        let url = match (step.target, step.path) {
            (Target::Plc, _) => format!("{base}/{}", self.fill_str(pv, step.nsid)),
            (_, Some(p)) => format!("{base}{p}"),
            _ => format!("{base}/xrpc/{}", step.nsid),
        };
        let query: Vec<(&str, String)> = step
            .query
            .iter()
            .map(|(k, v)| (*k, self.fill_str(pv, v)))
            .collect();
        let method = if step.get { "GET" } else { "POST" };
        let mut req = if step.get {
            self.client.get(&url)
        } else {
            self.client.post(&url)
        }
        .query(&query);
        // The other server's tokens, for a step sent there.
        let tokens = if step.target == Target::Peer {
            pv
        } else {
            &self.vars
        };
        let tok = |k: String| {
            tokens
                .get(&k)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        req = match step.auth {
            Auth::None => req,
            Auth::User(u) => req.bearer_auth(tok(format!("{u}.access"))),
            Auth::Refresh(u) => req.bearer_auth(tok(format!("{u}.refresh"))),
            Auth::Bogus => {
                req.bearer_auth("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0.bm90LWEtc2lnbmF0dXJl")
            }
            Auth::Admin => req.basic_auth(
                "admin",
                Some(if step.target == Target::Peer {
                    &peer.admin_password
                } else {
                    &self.admin_password
                }),
            ),
            Auth::Var(v) => req.bearer_auth(self.token(pv, v)),
        };
        for (k, v) in &step.headers {
            req = req.header(*k, self.fill_str(pv, v));
        }
        if step.dpop != Dpop::None {
            req = req.header("dpop", self.proof(method, &url, step.dpop == Dpop::Nonce));
        }
        req = match &step.body {
            Body::None => req,
            Body::Json(v) => req.json(&self.fill(pv, v)),
            Body::Bytes(b, mime) => req.header("content-type", *mime).body(b.to_vec()),
            Body::Var(v, mime) => req
                .header("content-type", *mime)
                .body(self.blobs.get(*v).cloned().unwrap_or_default()),
            Body::Form(f) => {
                let f: Vec<(&str, String)> =
                    f.iter().map(|(k, v)| (*k, self.fill_str(pv, v))).collect();
                req.form(&f)
            }
        };
        let (status, raw, headers) = match req.send().await {
            Ok(r) => {
                let status = r.status().as_u16();
                let headers = r.headers().clone();
                let ct = headers
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_owned();
                let bytes = r.bytes().await.unwrap_or_default();
                if let Some(var) = &step.capture_bytes {
                    self.blobs.insert(var.clone(), bytes.to_vec());
                }
                let body = if ct.starts_with("application/json") {
                    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
                } else if bytes.is_empty() {
                    Value::Null
                } else {
                    json!({"$bytes": ct.split(';').next().unwrap_or_default(), "len": "<n>"})
                };
                (status, body, headers)
            }
            Err(e) => (
                0,
                json!({"transport": e.to_string()}),
                reqwest::header::HeaderMap::new(),
            ),
        };
        if let Some(n) = headers.get("dpop-nonce").and_then(|v| v.to_str().ok()) {
            self.dpop_nonce = Some(n.to_owned());
        }
        for (var, pointer) in &step.capture {
            if let Some(v) = raw.pointer(pointer) {
                self.vars.insert(var.clone(), v.clone());
            }
        }
        // Error messages are free text; only the name is compared.
        let message = raw["message"]
            .as_str()
            .or_else(|| raw["error_description"].as_str())
            .map(str::to_owned);
        let mut body = self.norm.value(&raw);
        if status >= 400
            && let Some(m) = body.as_object_mut()
            && m.contains_key("error")
        {
            m.remove("message");
            m.remove("error_description");
        }
        for pointer in &step.unordered {
            if let Some(Value::Array(a)) = body.pointer_mut(pointer) {
                a.sort_by_key(Value::to_string);
            }
        }
        if !step.show_headers.is_empty() {
            let shown: Map<String, Value> = step
                .show_headers
                .iter()
                .map(|h| ((*h).to_owned(), json!(headers.contains_key(*h))))
                .collect();
            if !body.is_object() {
                body = json!({"$body": body});
            }
            body["$headers"] = Value::Object(shown);
        }
        Answer {
            status,
            body,
            message,
        }
    }

    /// The `XXXXX-XXXXX` token in the newest email from this server to the
    /// step's address (waits up to 5 s for it to arrive).
    async fn mail_token(&mut self, step: &Step) -> Answer {
        let Some((api, from)) = self.mail.clone() else {
            return Answer {
                status: 0,
                body: json!({"mail": "not configured"}),
                message: None,
            };
        };
        let q = format!("to:\"{}\" from:\"{from}\"", step.nsid);
        for _ in 0..25 {
            let found = async {
                let list: Value = self
                    .client
                    .get(format!("{api}/api/v1/search"))
                    .query(&[("query", q.as_str()), ("limit", "1")])
                    .send()
                    .await
                    .ok()?
                    .json()
                    .await
                    .ok()?;
                let id = list["messages"][0]["ID"].as_str()?.to_owned();
                let msg: Value = self
                    .client
                    .get(format!("{api}/api/v1/message/{id}"))
                    .send()
                    .await
                    .ok()?
                    .json()
                    .await
                    .ok()?;
                let text = msg["Text"].as_str()?.to_owned();
                Some((id, find_token(&text)?))
            }
            .await;
            if let Some((id, token)) = found
                && self.vars.get("mail.last").and_then(Value::as_str) != Some(id.as_str())
            {
                self.vars.insert("mail.last".into(), json!(id));
                for (var, _) in &step.capture {
                    self.vars.insert(var.clone(), json!(token));
                }
                return Answer {
                    status: 200,
                    body: json!({"token": "<token>"}),
                    message: None,
                };
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        Answer {
            status: 404,
            body: json!({"token": null}),
            message: None,
        }
    }
}

/// A `XXXXX-XXXXX` token (base32 letters and digits).
fn find_token(text: &str) -> Option<String> {
    let b = text.as_bytes();
    (0..b.len().saturating_sub(10)).find_map(|i| {
        let w = &b[i..i + 11];
        let ok = |c: &u8| c.is_ascii_uppercase() || c.is_ascii_digit() || c.is_ascii_lowercase();
        let edge = |j: usize| b.get(j).is_none_or(|c| !c.is_ascii_alphanumeric());
        (w[5] == b'-'
            && w[..5].iter().all(ok)
            && w[6..].iter().all(ok)
            && (i == 0 || edge(i - 1))
            && edge(i + 11))
        .then(|| String::from_utf8_lossy(w).into_owned())
    })
}

/// One allowed difference, with the design section that justifies it.
#[derive(Debug, Clone)]
pub struct Allowed {
    pub step: String,
    pub path: String,
    pub section: String,
    pub why: String,
    /// Only in this run mode (`mail` or `no-mail`), if set.
    pub when: Option<String>,
}

pub fn load_allowlist(v: &Value, mode: &str) -> Vec<Allowed> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|e| Allowed {
            step: e["step"].as_str().unwrap_or_default().to_owned(),
            path: e["path"].as_str().unwrap_or_default().to_owned(),
            section: e["section"].as_str().unwrap_or_default().to_owned(),
            why: e["why"].as_str().unwrap_or_default().to_owned(),
            when: e["when"].as_str().map(str::to_owned),
        })
        .filter(|a| a.when.as_deref().is_none_or(|w| w == mode))
        .collect()
}

/// A step's outcome: differences, split into allowed and not.
pub struct Outcome {
    pub step: String,
    pub unexpected: Vec<Diff>,
    pub allowed: Vec<(Diff, Allowed)>,
}

fn matches(a: &Allowed, step: &str, path: &str) -> bool {
    let step_ok =
        a.step == step || (a.step.ends_with('*') && step.starts_with(a.step.trim_end_matches('*')));
    let path_ok = path == a.path
        || path.starts_with(&format!("{}/", a.path.trim_end_matches('/')))
        || a.path == "*";
    step_ok && path_ok
}

#[must_use]
pub fn compare(step: &str, locus: &Answer, reference: &Answer, allow: &[Allowed]) -> Outcome {
    let mut diffs = Vec::new();
    if locus.status != reference.status {
        diffs.push(Diff {
            path: "status".into(),
            locus: json!(locus.status),
            reference: json!(reference.status),
        });
    }
    diffs.extend(diff(&locus.body, &reference.body));
    let mut out = Outcome {
        step: step.to_owned(),
        unexpected: Vec::new(),
        allowed: Vec::new(),
    };
    for d in diffs {
        match allow.iter().find(|a| matches(a, step, &d.path)) {
            Some(a) => out.allowed.push((d, a.clone())),
            None => out.unexpected.push(d),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_entries_match_by_step_and_path_prefix() {
        let a = Allowed {
            step: "server.*".into(),
            path: "/links".into(),
            section: "§1".into(),
            why: "x".into(),
            when: None,
        };
        assert!(matches(&a, "server.describe", "/links"));
        assert!(matches(&a, "server.describe", "/links/privacyPolicy"));
        assert!(!matches(&a, "server.describe", "/linksX"));
        assert!(!matches(&a, "repo.create", "/links"));
    }

    #[test]
    fn allowlist_entries_apply_in_their_mode() {
        let v = json!([{"step": "a", "path": "/", "when": "no-mail"}, {"step": "b", "path": "/"}]);
        assert_eq!(load_allowlist(&v, "no-mail").len(), 2);
        assert_eq!(load_allowlist(&v, "mail").len(), 1);
    }

    #[test]
    fn tokens_are_found_in_email_text() {
        assert_eq!(
            find_token("Your code: ABCDE-23456\nthanks").as_deref(),
            Some("ABCDE-23456")
        );
        assert_eq!(find_token("no code here, x-y"), None);
    }

    #[test]
    fn variables_fill_strings_and_whole_values() {
        let mut s = Server::new("http://localhost:1", "pw", "http://localhost:2");
        s.vars.insert("a".into(), json!("x"));
        s.vars.insert("keys".into(), json!(["k1", "k2"]));
        let peer = HashMap::from([("a".to_owned(), json!("p"))]);
        let v = s.fill(&peer, &json!({"s": "{{a}}/{{peer.a}}", "k": "{{keys}}"}));
        assert_eq!(v, json!({"s": "x/p", "k": ["k1", "k2"]}));
    }
}
