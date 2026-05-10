//! Time-range primitive for admin/moderator endpoints (Arc 5
//! §9.4.3 / chainlink #126).
//!
//! `TimeRange` is a validated `(start, end)` pair, constructible
//! from either a preset name (`"last_hour"`, `"last_24h"`,
//! `"last_7d"`, `"last_30d"`) or an explicit `{start, end}` pair
//! of RFC 3339 timestamps. The wrapper rejects inverted ranges
//! (`start > end`) at construction time so handlers can trust
//! the value without re-validating.
//!
//! The custom `Deserialize` impl accepts both wire shapes and is
//! the single validation boundary. Equal start/end is allowed
//! (zero-duration ranges are valid queries that return empty
//! series).
//!
//! # Wire shapes
//!
//! As a JSON value:
//!
//! ```json
//! "last_24h"
//! ```
//!
//! ```json
//! {"start": "2026-05-09T00:00:00Z", "end": "2026-05-10T00:00:00Z"}
//! ```
//!
//! Query-string callers can use the preset form directly
//! (`?timeRange=last_24h`). The struct-form `{start, end}` is
//! intended for JSON-body consumers; query-string callers wanting
//! explicit windows continue to use sibling `start`/`end` keys at
//! the request struct level (per Arc 5 §9.4.3 backward-compat
//! handling on `getModerationMetrics`).

use chrono::{DateTime, Duration, Utc};
use serde::{de::Error as _, Deserialize, Deserializer};

/// Validated time range. Construction guarantees `start <= end`.
///
/// Prefer the constructors (`new`, `from_preset`, `from_rfc3339_pair`)
/// or `Deserialize` over direct field access — fields are private so
/// the validation boundary cannot be accidentally bypassed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeRange {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}

impl TimeRange {
    /// Construct a `TimeRange` from explicit start/end. Returns
    /// `Err` if `start > end`. Equal start/end is allowed.
    pub fn new(start: DateTime<Utc>, end: DateTime<Utc>) -> Result<Self, &'static str> {
        if start > end {
            Err("start must be <= end")
        } else {
            Ok(TimeRange { start, end })
        }
    }

    /// Resolve a preset name to a `(now - duration, now)` window.
    /// Returns `None` for unknown preset names. Callers driving
    /// from operator input typically use `Deserialize` instead;
    /// this helper is exposed so tests can exercise the resolution
    /// directly with a fixed `now`.
    pub fn from_preset(name: &str, now: DateTime<Utc>) -> Option<Self> {
        let start = match name {
            "last_hour" => now - Duration::hours(1),
            "last_24h" => now - Duration::hours(24),
            "last_7d" => now - Duration::days(7),
            "last_30d" => now - Duration::days(30),
            _ => return None,
        };
        // Constructed forward in time: start <= end always.
        Some(TimeRange { start, end: now })
    }

    /// The committed preset vocabulary. Update in lockstep with
    /// `from_preset` and the operator docs.
    pub const PRESETS: &'static [&'static str] = &["last_hour", "last_24h", "last_7d", "last_30d"];

    /// Parse a pair of RFC 3339 timestamps and validate. Used by
    /// the `getModerationMetrics` request-struct dispatcher to
    /// build a `TimeRange` from legacy `start`/`end` peer fields.
    pub fn from_rfc3339_pair(start: &str, end: &str) -> Result<Self, String> {
        let start_dt = DateTime::parse_from_rfc3339(start)
            .map_err(|e| format!("invalid 'start' RFC 3339 timestamp ({}): {}", start, e))?
            .with_timezone(&Utc);
        let end_dt = DateTime::parse_from_rfc3339(end)
            .map_err(|e| format!("invalid 'end' RFC 3339 timestamp ({}): {}", end, e))?
            .with_timezone(&Utc);
        TimeRange::new(start_dt, end_dt).map_err(|s| s.to_string())
    }

    pub fn start(&self) -> DateTime<Utc> {
        self.start
    }

    pub fn end(&self) -> DateTime<Utc> {
        self.end
    }
}

/// Helper struct deserialized as a stepping stone for the
/// `{start, end}` object wire shape. Field names match the wire
/// keys verbatim.
#[derive(Deserialize)]
struct RawWindow {
    start: String,
    end: String,
}

impl<'de> Deserialize<'de> for TimeRange {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Materialize a JSON value to dispatch on shape. Preset =>
        // string; explicit window => object with start/end.
        let value = serde_json::Value::deserialize(d)?;
        match value {
            serde_json::Value::String(name) => TimeRange::from_preset(&name, Utc::now())
                .ok_or_else(|| {
                    D::Error::custom(format!(
                        "unknown time-range preset {:?}; expected one of: {}",
                        name,
                        TimeRange::PRESETS.join(", "),
                    ))
                }),
            serde_json::Value::Object(_) => {
                let raw: RawWindow = serde_json::from_value(value).map_err(|e| {
                    D::Error::custom(format!(
                        "expected {{start, end}} time-range object: {}",
                        e
                    ))
                })?;
                TimeRange::from_rfc3339_pair(&raw.start, &raw.end).map_err(D::Error::custom)
            }
            other => Err(D::Error::custom(format!(
                "time_range must be a preset-name string (one of: {}) or a {{start, end}} object \
                 with RFC 3339 timestamps; got {}",
                TimeRange::PRESETS.join(", "),
                shape_label(&other),
            ))),
        }
    }
}

fn shape_label(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<TimeRange, serde_json::Error> {
        serde_json::from_str(json)
    }

    #[test]
    fn deserializes_last_hour_preset() {
        let tr = parse("\"last_hour\"").unwrap();
        let span = tr.end() - tr.start();
        // Window must equal exactly one hour.
        assert_eq!(span.num_seconds(), 3600);
    }

    #[test]
    fn deserializes_last_24h_preset() {
        let tr = parse("\"last_24h\"").unwrap();
        assert_eq!((tr.end() - tr.start()).num_hours(), 24);
    }

    #[test]
    fn deserializes_last_7d_preset() {
        let tr = parse("\"last_7d\"").unwrap();
        assert_eq!((tr.end() - tr.start()).num_days(), 7);
    }

    #[test]
    fn deserializes_last_30d_preset() {
        let tr = parse("\"last_30d\"").unwrap();
        assert_eq!((tr.end() - tr.start()).num_days(), 30);
    }

    #[test]
    fn deserializes_struct_form_with_rfc3339() {
        let tr = parse(
            r#"{"start": "2026-01-01T00:00:00Z", "end": "2026-01-02T00:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!((tr.end() - tr.start()).num_hours(), 24);
        assert_eq!(tr.start().to_rfc3339(), "2026-01-01T00:00:00+00:00");
    }

    #[test]
    fn struct_form_allows_equal_start_end() {
        // Zero-duration range — valid query, empty series.
        let tr = parse(
            r#"{"start": "2026-01-01T00:00:00Z", "end": "2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(tr.start(), tr.end());
    }

    #[test]
    fn rejects_start_greater_than_end_at_deserialize() {
        let err = parse(
            r#"{"start": "2026-01-02T00:00:00Z", "end": "2026-01-01T00:00:00Z"}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("start must be <= end"),
            "expected start-greater-than-end error; got: {err}"
        );
    }

    #[test]
    fn rejects_unknown_preset_with_clear_error() {
        let err = parse("\"last_5min\"").unwrap_err().to_string();
        assert!(
            err.contains("last_5min"),
            "error must include the unknown preset name; got: {err}"
        );
        assert!(
            err.contains("last_24h") && err.contains("last_7d"),
            "error must list valid presets; got: {err}"
        );
    }

    #[test]
    fn rejects_malformed_rfc3339_in_start_with_clear_error() {
        let err = parse(
            r#"{"start": "not-a-date", "end": "2026-01-01T00:00:00Z"}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("not-a-date") || err.contains("'start'"),
            "error must mention the bad value or the field name; got: {err}"
        );
    }

    #[test]
    fn rejects_malformed_rfc3339_in_end_with_clear_error() {
        let err = parse(
            r#"{"start": "2026-01-01T00:00:00Z", "end": "garbage"}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("garbage") || err.contains("'end'"),
            "error must mention the bad value or the field name; got: {err}"
        );
    }

    #[test]
    fn rejects_non_string_non_object_with_canonical_error() {
        let err = parse("42").unwrap_err().to_string();
        assert!(
            err.contains("preset") || err.contains("PRESET") || err.contains("preset-name"),
            "error must mention the preset alternative; got: {err}"
        );
        assert!(
            err.contains("start") && err.contains("end"),
            "error must mention the {{start, end}} alternative; got: {err}"
        );
    }

    #[test]
    fn new_rejects_inverted_range() {
        let now = Utc::now();
        let later = now + Duration::hours(1);
        assert!(TimeRange::new(later, now).is_err());
    }

    #[test]
    fn new_allows_equal_start_end() {
        let now = Utc::now();
        assert!(TimeRange::new(now, now).is_ok());
    }

    #[test]
    fn from_preset_unknown_returns_none() {
        let now = Utc::now();
        assert!(TimeRange::from_preset("not_a_preset", now).is_none());
    }

    #[test]
    fn from_preset_returns_now_anchored_window() {
        let now = "2026-05-09T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let tr = TimeRange::from_preset("last_24h", now).unwrap();
        assert_eq!(tr.end(), now);
        assert_eq!(tr.start(), now - Duration::hours(24));
    }
}
