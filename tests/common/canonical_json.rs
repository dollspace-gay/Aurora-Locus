//! Canonical JSON serialization for contract snapshot tests
//! (Arc 2 §6.3.6).
//!
//! Sorts object keys alphabetically to produce deterministic byte
//! output regardless of struct field declaration order. Used by the
//! Subject vocabulary snapshots (§6.4.1, Step 1), the action-ID
//! contract tests (§6.4.2, Step 2), and the describeCapabilities
//! snapshot (§6.4.3, Step 3).
//!
//! ## Reach across the unit/integration boundary
//!
//! This file lives under `tests/common/` so integration tests can
//! reach it via `mod common; use common::canonical_json::...;`. Unit
//! tests inside `src/` reach it via
//! `#[path = "../../tests/common/canonical_json.rs"] mod canonical_json;`
//! — the source-of-truth lives here; src/-side users include the
//! same file by path so any helper change updates both call sites in
//! lock-step.
//!
//! Lift-and-shift from the inline helper Step 0.5 added to
//! `src/api/admin.rs`'s test module. Behaviour-equivalent.

/// Canonical-JSON serialize: sorted keys, no whitespace, standard
/// JSON escaping.
pub fn canonical_json<T: serde::Serialize>(value: &T) -> String {
    let v = serde_json::to_value(value).expect("serializable");
    canonicalize(&v)
}

fn canonicalize(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Object(map) => {
            let mut sorted: Vec<(&String, &serde_json::Value)> = map.iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(b.0));
            let parts: Vec<String> = sorted
                .iter()
                .map(|(k, val)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap(),
                        canonicalize(val)
                    )
                })
                .collect();
            format!("{{{}}}", parts.join(","))
        }
        serde_json::Value::Array(arr) => {
            let parts: Vec<String> = arr.iter().map(canonicalize).collect();
            format!("[{}]", parts.join(","))
        }
        other => serde_json::to_string(other).unwrap(),
    }
}
