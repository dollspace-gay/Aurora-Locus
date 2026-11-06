//! Collects a server's firehose while the scenarios run, as structural
//! summaries: event kind, ops (action and path), whether blocks and
//! `prevData` are present, account status, handle.

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use proto_blue_lex_data::LexValue;
use proto_blue_ws::frame::Frame;
use serde_json::{Value, json};

pub type Events = Arc<Mutex<Vec<Value>>>;

fn s(m: &std::collections::BTreeMap<String, LexValue>, k: &str) -> Value {
    m.get(k)
        .and_then(LexValue::as_str)
        .map_or(Value::Null, |v| Value::String(v.to_owned()))
}

fn summary(frame: Frame) -> Value {
    let m = match frame {
        Frame::Error(e) => return json!({"error": e.error}),
        Frame::Message(m) => m,
    };
    let kind = m.r#type.unwrap_or_default();
    let Some(body) = m.body.as_map() else {
        return json!({"t": kind});
    };
    let mut out = json!({"t": kind, "repo": s(body, "repo").as_str().map_or_else(|| s(body, "did"), |d| json!(d))});
    match kind.as_str() {
        "#commit" => {
            let ops: Vec<Value> = body
                .get("ops")
                .and_then(LexValue::as_array)
                .into_iter()
                .flatten()
                .filter_map(|o| {
                    let o = o.as_map()?;
                    Some(json!({
                        "action": s(o, "action"),
                        "path": s(o, "path"),
                        "cid": o.get("cid").is_some_and(|c| !matches!(c, LexValue::Null)),
                        "prev": o.get("prev").is_some_and(|c| !matches!(c, LexValue::Null)),
                    }))
                })
                .collect();
            let blocks = matches!(body.get("blocks"), Some(LexValue::Bytes(b)) if !b.is_empty());
            out["ops"] = json!(ops);
            out["blocks"] = json!(blocks);
            out["prevData"] = json!(
                body.get("prevData")
                    .is_some_and(|c| !matches!(c, LexValue::Null))
            );
            out["tooBig"] = json!(body.get("tooBig").and_then(LexValue::as_bool));
        }
        "#account" => {
            out["active"] = json!(body.get("active").and_then(LexValue::as_bool));
            out["status"] = s(body, "status");
        }
        "#identity" => {
            out["handle"] = s(body, "handle");
        }
        "#info" => {
            out["name"] = s(body, "name");
        }
        "#sync" => {
            out["blocks"] =
                json!(matches!(body.get("blocks"), Some(LexValue::Bytes(b)) if !b.is_empty()));
        }
        _ => {}
    }
    out
}

fn seq(frame: &Frame) -> Option<i64> {
    match frame {
        Frame::Message(m) => m
            .body
            .as_map()
            .and_then(|b| b.get("seq"))
            .and_then(|s| match s {
                LexValue::Integer(i) => Some(*i),
                _ => None,
            }),
        Frame::Error(_) => None,
    }
}

/// Subscribes from `cursor` (empty: none) and collects frames until the
/// stream is quiet for a second or closes, at most 10 s. Returns the
/// summaries and the last seq seen. A closed stream ends with `{"closed":
/// true}`.
pub async fn backfill(base: &str, cursor: &str) -> (Vec<Value>, Option<i64>) {
    use std::time::Duration;
    let mut url = format!(
        "{}/xrpc/com.atproto.sync.subscribeRepos",
        base.replacen("http", "ws", 1)
    );
    if !cursor.is_empty() {
        url = format!("{url}?cursor={cursor}");
    }
    let mut ws = match tokio_tungstenite::connect_async(&url).await {
        Ok((ws, _)) => ws,
        Err(e) => return (vec![json!({"connect": e.to_string()})], None),
    };
    let (mut out, mut last) = (Vec::new(), None);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let wait = Duration::from_millis(1000).min(deadline - tokio::time::Instant::now());
        match tokio::time::timeout(wait, ws.next()).await {
            Ok(Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(b)))) => {
                if let Ok(f) = Frame::decode(&b) {
                    last = seq(&f).or(last);
                    out.push(summary(f));
                }
            }
            Ok(
                Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None | Some(Err(_)),
            ) => {
                out.push(json!({"closed": true}));
                break;
            }
            Ok(Some(Ok(_))) => {}
            Err(_) => break,
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
    }
    (out, last)
}

/// Subscribes and keeps collecting until the process ends.
pub async fn collect(base: &str) -> Result<Events, String> {
    let url = format!(
        "{}/xrpc/com.atproto.sync.subscribeRepos",
        base.replacen("http", "ws", 1)
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| format!("{url}: {e}"))?;
    let events: Events = Arc::default();
    let sink = Arc::clone(&events);
    tokio::spawn(async move {
        while let Some(Ok(msg)) = ws.next().await {
            if let tokio_tungstenite::tungstenite::Message::Binary(b) = msg
                && let Ok(f) = Frame::decode(&b)
                && let Ok(mut v) = sink.lock()
            {
                v.push(summary(f));
            }
        }
    });
    Ok(events)
}
