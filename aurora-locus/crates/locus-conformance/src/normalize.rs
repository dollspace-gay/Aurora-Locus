//! Values that legitimately differ between two servers (DIDs, TIDs, CIDs,
//! revs, timestamps, tokens, hosts, sequence numbers) become placeholders
//! before comparison. DIDs are numbered in order of first appearance on
//! each side, so the same account lines up on both. The server's own host
//! becomes `<host>` and the other server's `<peer>` (a migrated account's
//! document names its source).

use std::collections::HashMap;

use serde_json::{Map, Value};

/// Per-server state: the DIDs seen so far and this server's host.
#[derive(Default)]
pub struct Normalizer {
    dids: HashMap<String, usize>,
    /// `host:port` of the server, replaced with `<host>`.
    pub host: String,
    /// `host:port` of the other server, replaced with `<peer>`.
    pub peer: String,
}

/// Keys whose integer values are counters that differ between servers.
/// `repoBlocks` too: each server mints its own record keys, and how they
/// hash decides how many tree nodes the same records need.
const VOLATILE_INTS: &[&str] = &["seq", "cursor", "size", "time", "expires_in", "repoBlocks"];

fn is_tid(s: &str) -> bool {
    s.len() == 13
        && s.bytes().all(|b| matches!(b, b'2'..=b'7' | b'a'..=b'z'))
        && matches!(s.as_bytes()[0], b'2'..=b'7' | b'a'..=b'j')
}

fn is_cid(s: &str) -> bool {
    (s.starts_with("bafy") || s.starts_with("bafk"))
        && s.len() > 40
        && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn is_time(s: &str) -> bool {
    s.len() >= 20 && s.as_bytes().get(4) == Some(&b'-') && s.as_bytes().get(10) == Some(&b'T')
}

/// An app password: four groups of four, `xxxx-xxxx-xxxx-xxxx`.
fn is_app_password(s: &str) -> bool {
    s.len() == 19
        && s.split('-').count() == 4
        && s.split('-').all(|g| {
            g.len() == 4
                && g.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

fn is_jwt(s: &str) -> bool {
    s.starts_with("eyJ") && s.matches('.').count() == 2
}

impl Normalizer {
    #[must_use]
    pub fn new(host: &str) -> Self {
        Self {
            dids: HashMap::new(),
            host: host.to_owned(),
            peer: String::new(),
        }
    }

    fn did(&mut self, d: &str) -> String {
        if d == "did:web:localhost" {
            return "<service-did>".into();
        }
        let n = self.dids.len();
        let i = *self.dids.entry(d.to_owned()).or_insert(n);
        format!("<did:{i}>")
    }

    /// One string, piece by piece (at:// URIs and `/`-separated paths too).
    pub fn text(&mut self, s: &str) -> String {
        if let Some(rest) = s.strip_prefix("at://") {
            let parts: Vec<String> = rest.split('/').map(|p| self.atom(p)).collect();
            return format!("at://{}", parts.join("/"));
        }
        if s.contains('/') && !s.contains("://") && !s.contains(' ') {
            let parts: Vec<String> = s.split('/').map(|p| self.atom(p)).collect();
            return parts.join("/");
        }
        let mut s = s.replace(&self.host, "<host>");
        if !self.peer.is_empty() {
            s = s.replace(&self.peer, "<peer>");
        }
        self.atom(&s)
    }

    fn atom(&mut self, s: &str) -> String {
        // A keyset cursor (`<ms>::<did>`).
        if let Some((a, b)) = s.split_once("::")
            && a.bytes().all(|c| c.is_ascii_digit())
        {
            return format!("<n>::{}", self.atom(b));
        }
        if (s.starts_with("did:plc:") || s.starts_with("did:web:"))
            && let Some((d, frag)) = s.split_once('#')
        {
            return format!("{}#{frag}", self.did(d));
        }
        if s.starts_with("did:plc:") || s.starts_with("did:web:") {
            self.did(s)
        } else if s.starts_with("urn:ietf:params:oauth:request_uri:") {
            "<request-uri>".into()
        } else if (s.starts_with("zQ3sh") || s.starts_with("zDnae")) && s.len() > 40 {
            "<multikey>".into()
        } else if s.starts_with("did:key:") {
            "<did:key>".into()
        } else if is_tid(s) {
            "<tid>".into()
        } else if is_cid(s) {
            "<cid>".into()
        } else if is_time(s) {
            "<time>".into()
        } else if is_jwt(s) {
            "<jwt>".into()
        } else if is_app_password(s) {
            "<app-password>".into()
        } else {
            s.to_owned()
        }
    }

    /// A JSON value with every volatile part replaced.
    pub fn value(&mut self, v: &Value) -> Value {
        self.value_at(v, None)
    }

    fn value_at(&mut self, v: &Value, key: Option<&str>) -> Value {
        match v {
            Value::String(s) => Value::String(self.text(s)),
            Value::Number(_) if key.is_some_and(|k| VOLATILE_INTS.contains(&k)) => {
                Value::String("<n>".into())
            }
            Value::Array(a) => Value::Array(a.iter().map(|x| self.value_at(x, None)).collect()),
            Value::Object(m) => {
                let mut out = Map::new();
                for (k, x) in m {
                    out.insert(k.clone(), self.value_at(x, Some(k)));
                }
                Value::Object(out)
            }
            other => other.clone(),
        }
    }
}

/// One difference: a JSON pointer and each side's value there.
#[derive(Debug, Clone)]
pub struct Diff {
    pub path: String,
    pub locus: Value,
    pub reference: Value,
}

/// Every place the two values differ (missing keys show as null).
#[must_use]
pub fn diff(locus: &Value, reference: &Value) -> Vec<Diff> {
    let mut out = Vec::new();
    walk("", locus, reference, &mut out);
    out
}

fn walk(path: &str, a: &Value, b: &Value, out: &mut Vec<Diff>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let mut keys: Vec<&String> = x.keys().chain(y.keys()).collect();
            keys.sort();
            keys.dedup();
            for k in keys {
                let p = format!("{path}/{k}");
                walk(
                    &p,
                    x.get(k).unwrap_or(&Value::Null),
                    y.get(k).unwrap_or(&Value::Null),
                    out,
                );
            }
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                walk(&format!("{path}/{i}"), p, q, out);
            }
        }
        _ if a == b => {}
        _ => out.push(Diff {
            path: if path.is_empty() {
                "/".into()
            } else {
                path.to_owned()
            },
            locus: a.clone(),
            reference: b.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn volatile_values_become_placeholders() {
        let mut n = Normalizer::new("localhost:3000");
        let v = n.value(&json!({
            "did": "did:plc:abcdefghijklmnopqrstuvwx",
            "uri": "at://did:plc:abcdefghijklmnopqrstuvwx/app.bsky.feed.post/3l4kqz7xyzab2",
            "cid": "bafyreie5737gdxlw5i64vzichcalba3z2v5n6icifvx5xytvske7mr3hpm",
            "rev": "3l4kqz7xyzab2",
            "other": "did:plc:zzzzzzzzzzzzzzzzzzzzzzzz",
            "at": "2026-11-06T00:00:00.000Z",
            "accessJwt": "eyJhbGciOi.eyJzdWIi.sig",
            "seq": 41,
            "count": 3,
            "url": "http://localhost:3000/x",
        }));
        assert_eq!(v["did"], "<did:0>");
        assert_eq!(v["uri"], "at://<did:0>/app.bsky.feed.post/<tid>");
        assert_eq!(v["cid"], "<cid>");
        assert_eq!(v["rev"], "<tid>");
        assert_eq!(v["other"], "<did:1>");
        assert_eq!(v["at"], "<time>");
        assert_eq!(v["accessJwt"], "<jwt>");
        assert_eq!(v["seq"], "<n>");
        assert_eq!(v["count"], 3);
        assert_eq!(v["url"], "http://<host>/x");
    }

    #[test]
    fn diffs_name_the_paths() {
        let d = diff(
            &json!({"a": 1, "b": [1, 2], "c": {"x": 1}}),
            &json!({"a": 1, "b": [1, 3], "d": 2}),
        );
        let paths: Vec<&str> = d.iter().map(|x| x.path.as_str()).collect();
        assert_eq!(paths, vec!["/b/1", "/c", "/d"]);
    }
}
