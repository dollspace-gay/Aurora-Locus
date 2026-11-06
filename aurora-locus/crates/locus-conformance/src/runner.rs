//! Runs steps against both servers and compares what they answer.

use std::collections::HashMap;

use serde_json::{Value, json};

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
}

#[derive(Debug, Clone)]
pub enum Body {
    None,
    Json(Value),
    Bytes(&'static [u8], &'static str),
}

/// One request, made the same way to both servers.
#[derive(Debug, Clone)]
pub struct Step {
    pub name: String,
    pub get: bool,
    pub nsid: &'static str,
    pub query: Vec<(&'static str, String)>,
    pub auth: Auth,
    pub body: Body,
    /// Variables to capture from the response: (name, JSON pointer).
    pub capture: Vec<(String, String)>,
}

impl Step {
    pub fn get(name: &str, nsid: &'static str) -> Self {
        Self {
            name: name.to_owned(),
            get: true,
            nsid,
            query: Vec::new(),
            auth: Auth::None,
            body: Body::None,
            capture: Vec::new(),
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
    #[must_use]
    pub fn keep(mut self, var: &str, pointer: &str) -> Self {
        self.capture.push((var.to_owned(), pointer.to_owned()));
        self
    }
}

/// One server under test.
pub struct Server {
    pub base: String,
    pub admin_password: String,
    pub norm: Normalizer,
    pub vars: HashMap<String, String>,
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

impl Server {
    pub fn new(base: &str, admin_password: &str) -> Self {
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
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }

    /// `{{var}}` replaced in every string.
    fn fill(&self, v: &Value) -> Value {
        match v {
            Value::String(s) => Value::String(self.fill_str(s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.fill(x)).collect()),
            Value::Object(m) => {
                Value::Object(m.iter().map(|(k, x)| (k.clone(), self.fill(x))).collect())
            }
            other => other.clone(),
        }
    }

    fn fill_str(&self, s: &str) -> String {
        let mut out = s.to_owned();
        for (k, v) in &self.vars {
            out = out.replace(&format!("{{{{{k}}}}}"), v);
        }
        out
    }

    pub async fn run(&mut self, step: &Step) -> Answer {
        let url = format!("{}/xrpc/{}", self.base, step.nsid);
        let query: Vec<(&str, String)> = step
            .query
            .iter()
            .map(|(k, v)| (*k, self.fill_str(v)))
            .collect();
        let mut req = if step.get {
            self.client.get(&url)
        } else {
            self.client.post(&url)
        }
        .query(&query);
        req = match step.auth {
            Auth::None => req,
            Auth::User(u) => req.bearer_auth(
                self.vars
                    .get(&format!("{u}.access"))
                    .cloned()
                    .unwrap_or_default(),
            ),
            Auth::Refresh(u) => req.bearer_auth(
                self.vars
                    .get(&format!("{u}.refresh"))
                    .cloned()
                    .unwrap_or_default(),
            ),
            Auth::Bogus => {
                req.bearer_auth("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0.bm90LWEtc2lnbmF0dXJl")
            }
            Auth::Admin => req.basic_auth("admin", Some(&self.admin_password)),
        };
        req = match &step.body {
            Body::None => req,
            Body::Json(v) => req.json(&self.fill(v)),
            Body::Bytes(b, mime) => req.header("content-type", *mime).body(b.to_vec()),
        };
        let (status, raw) = match req.send().await {
            Ok(r) => {
                let status = r.status().as_u16();
                let ct = r
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_owned();
                let bytes = r.bytes().await.unwrap_or_default();
                let body = if ct.starts_with("application/json") {
                    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
                } else if bytes.is_empty() {
                    Value::Null
                } else {
                    json!({"$bytes": ct.split(';').next().unwrap_or_default(), "len": "<n>"})
                };
                (status, body)
            }
            Err(e) => (0, json!({"transport": e.to_string()})),
        };
        for (var, pointer) in &step.capture {
            if let Some(v) = raw.pointer(pointer) {
                let s = v.as_str().map_or_else(|| v.to_string(), str::to_owned);
                self.vars.insert(var.clone(), s);
            }
        }
        // Error messages are free text; only the name is compared.
        let message = raw["message"].as_str().map(str::to_owned);
        let mut body = self.norm.value(&raw);
        if status >= 400
            && let Some(m) = body.as_object_mut()
            && m.contains_key("error")
        {
            m.remove("message");
        }
        Answer {
            status,
            body,
            message,
        }
    }
}

/// One allowed difference, with the design section that justifies it.
#[derive(Debug, Clone)]
pub struct Allowed {
    pub step: String,
    pub path: String,
    pub section: String,
    pub why: String,
}

pub fn load_allowlist(v: &Value) -> Vec<Allowed> {
    v.as_array()
        .into_iter()
        .flatten()
        .map(|e| Allowed {
            step: e["step"].as_str().unwrap_or_default().to_owned(),
            path: e["path"].as_str().unwrap_or_default().to_owned(),
            section: e["section"].as_str().unwrap_or_default().to_owned(),
            why: e["why"].as_str().unwrap_or_default().to_owned(),
        })
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
        };
        assert!(matches(&a, "server.describe", "/links"));
        assert!(matches(&a, "server.describe", "/links/privacyPolicy"));
        assert!(!matches(&a, "server.describe", "/linksX"));
        assert!(!matches(&a, "repo.create", "/links"));
    }
}
