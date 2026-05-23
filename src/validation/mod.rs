/// Record validation module
///
/// Validates records against ATProto lexicon schemas
use crate::error::PdsError;
use chrono::DateTime;
use serde_json::Value;
use std::collections::HashMap;
use std::str::FromStr;
use unicode_segmentation::UnicodeSegmentation;

/// Type alias for collection validator functions
type ValidatorFn = Box<dyn Fn(&Value) -> ValidationResult + Send + Sync>;

/// Validation mode determines how strictly records are validated
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ValidationMode {
    /// Strict mode - reject unknown collections
    Required,
    /// Validate known collections, warn on unknown (default)
    #[default]
    Optimistic,
    /// No validation performed
    None,
}

impl FromStr for ValidationMode {
    type Err = PdsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "required" => Ok(ValidationMode::Required),
            "optimistic" => Ok(ValidationMode::Optimistic),
            "none" => Ok(ValidationMode::None),
            _ => Err(PdsError::Validation(format!(
                "Invalid validation mode: {}",
                s
            ))),
        }
    }
}

/// Validation error detail
///
/// The wire shape is `{path, message}` to preserve the 152 hand-coded
/// validator bodies (Arc 17 §17.4 Step 5 byte-identical audit baseline).
/// Arc 17's structured variants per §17.3.6 — `NamespaceDenied`,
/// `LexiconFetchFailed`, `SchemaViolation`, etc. — encode into the same
/// struct shape via path-sentinel routing: the `path` field carries an
/// `@lexicon/<variant>` tag that [`validation_errors_to_pds_error`]
/// matches on to surface the right [`PdsError`] variant for HTTP wire
/// mapping. Hand-coded validators keep their plain JSON-pointer paths
/// (`$.text`, `$.embed.images[0].image`, etc.) and remain
/// indistinguishable from the v1 shape.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ValidationError {
    pub path: String,
    pub message: String,
}

/// Sentinel prefix used by Arc 17 §17.3.6 variants in
/// [`ValidationError::path`]. Anything not starting with this prefix
/// is a hand-coded-validator error (preserved-as-is on the wire);
/// anything starting with it is parsed by
/// [`validation_errors_to_pds_error`].
pub(crate) const LEXICON_VARIANT_PREFIX: &str = "@lexicon/";

/// Arc 17 §17.3.4 / round-1 F4 closure — should validation fire for
/// this write? The full matrix:
///
/// | `write.validate`     | lexicon.enabled | validate_imports | fires? |
/// |----------------------|-----------------|------------------|--------|
/// | `None`               | any             | any              | yes    |
/// | `Some(true)`         | any             | any              | yes    |
/// | `Some(false)` (CAR)  | true            | true             | yes (override) |
/// | `Some(false)` (CAR)  | true            | false            | no     |
/// | `Some(false)` (CAR)  | false           | any              | no     |
/// | `Some(false)` (CAR)  | (no lex config) | n/a              | no     |
///
/// The override only fires when BOTH `lexicon.enabled` and
/// `validate_imports` are true. Disabling the lexicon globally
/// (`enabled = false`) restores pre-Arc-17 semantics regardless of
/// the import-validation flag.
pub fn should_validate_per_lexicon_imports(
    write_validate: Option<bool>,
    lexicon_config: Option<&crate::config::LexiconConfig>,
) -> bool {
    let override_fires = lexicon_config
        .map(|cfg| cfg.enabled && cfg.validate_imports)
        .unwrap_or(false);
    match write_validate {
        Some(false) => override_fires,
        _ => true,
    }
}

/// Arc 17 §17.3.3 Phase B bug #2 — given a non-empty error list and
/// the active [`ValidationMode`], should `validate_write` propagate
/// the errors (reject the write), or can Optimistic mode absorb them?
///
/// Returns `true` to propagate, `false` to absorb. The matrix:
///
/// | mode       | fetch-class variant present | propagate? |
/// |------------|-----------------------------|------------|
/// | Required   | any                         | yes        |
/// | None       | any                         | yes (defensive — None short-circuits before Err in practice) |
/// | Optimistic | yes                         | yes (§17.3.3 HardFail/authority/deny/InvalidNsid bypass) |
/// | Optimistic | no                          | no (absorb — v1 SchemaViolation / hand-coded Optimistic contract) |
///
/// "fetch-class variant" is anything for which
/// [`ValidationError::is_fetch_class_lexicon_variant`] returns true.
/// The fetch-class set deliberately excludes `SchemaViolation` so the
/// v1 unknown-NSID-extended-to-mismatched-record absorption contract
/// is preserved.
pub fn should_propagate_validation_errors(
    errors: &[ValidationError],
    mode: ValidationMode,
) -> bool {
    if mode != ValidationMode::Optimistic {
        return true;
    }
    errors.iter().any(|e| e.is_fetch_class_lexicon_variant())
}

impl ValidationError {
    /// Arc 17 §17.3.6 — NSID matches the configured denylist; record
    /// rejected outright. Maps to [`PdsError::NamespaceDenied`].
    pub fn namespace_denied(nsid: &str) -> Self {
        Self {
            path: format!("{LEXICON_VARIANT_PREFIX}NamespaceDenied"),
            message: serde_json::json!({ "nsid": nsid }).to_string(),
        }
    }

    /// Arc 17 §17.3.6 — lexicon fetch (DNS / DID-resolve / HTTP)
    /// exhausted retries or surfaced a terminal failure. Maps to
    /// [`PdsError::LexiconFetchFailed`].
    pub fn lexicon_fetch_failed(nsid: &str, failure_class: &'static str, source_detail: &str) -> Self {
        Self {
            path: format!("{LEXICON_VARIANT_PREFIX}LexiconFetchFailed"),
            message: serde_json::json!({
                "nsid": nsid,
                "failure_class": failure_class,
                "source_detail": source_detail,
            })
            .to_string(),
        }
    }

    /// Arc 17 §17.3.6 — fetched lexicon document failed schema
    /// validation inside `proto_blue::lexicon::Lexicons::add`. Maps to
    /// [`PdsError::LexiconInvalidSchema`].
    pub fn lexicon_invalid_schema(nsid: &str, detail: &str) -> Self {
        Self {
            path: format!("{LEXICON_VARIANT_PREFIX}LexiconInvalidSchema"),
            message: serde_json::json!({ "nsid": nsid, "detail": detail }).to_string(),
        }
    }

    /// Arc 17 §17.3.6 — DNS TXT returned a DID different from the
    /// lexicon record's hosting DID. Maps to
    /// [`PdsError::LexiconAuthorityMismatch`].
    pub fn lexicon_authority_mismatch(nsid: &str, expected: &str, found: &str) -> Self {
        Self {
            path: format!("{LEXICON_VARIANT_PREFIX}LexiconAuthorityMismatch"),
            message: serde_json::json!({
                "nsid": nsid,
                "expected": expected,
                "found": found,
            })
            .to_string(),
        }
    }

    /// Arc 17 §17.3.6 — multiple TXT records or multiple `did=` entries.
    /// Maps to [`PdsError::LexiconAuthorityAmbiguous`].
    pub fn lexicon_authority_ambiguous(nsid: &str, candidates: &[String]) -> Self {
        Self {
            path: format!("{LEXICON_VARIANT_PREFIX}LexiconAuthorityAmbiguous"),
            message: serde_json::json!({
                "nsid": nsid,
                "candidates": candidates,
            })
            .to_string(),
        }
    }

    /// Arc 17 §17.3.6 — authority DID is tombstoned in PLC. Maps to
    /// [`PdsError::LexiconAuthorityTombstoned`].
    pub fn lexicon_authority_tombstoned(nsid: &str, did: &str) -> Self {
        Self {
            path: format!("{LEXICON_VARIANT_PREFIX}LexiconAuthorityTombstoned"),
            message: serde_json::json!({ "nsid": nsid, "did": did }).to_string(),
        }
    }

    /// Arc 17 §17.3.6 — NSID fails ATProto spec segment validation
    /// (`[a-z][a-z0-9-]*[a-z0-9]`, ≥ 3 segments). Maps to
    /// [`PdsError::LexiconInvalidNsid`].
    pub fn lexicon_invalid_nsid(nsid: &str) -> Self {
        Self {
            path: format!("{LEXICON_VARIANT_PREFIX}LexiconInvalidNsid"),
            message: serde_json::json!({ "nsid": nsid }).to_string(),
        }
    }

    /// Arc 17 §17.3.6 — record failed lexicon-driven schema validation.
    /// `field_path` is structured (JSON-pointer-style, from proto-blue's
    /// `ValidationError::InvalidValue.path`); `expected` is heuristic-
    /// on-message for v0.5 per Step 0.0b finding (proto-blue's
    /// structured-field shape may enrich in v0.6+). Maps to
    /// [`PdsError::SchemaViolation`].
    pub fn schema_violation(
        collection: &str,
        field_path: &str,
        expected: Option<&str>,
        actual_summary: Option<&str>,
        detail: &str,
    ) -> Self {
        Self {
            path: format!("{LEXICON_VARIANT_PREFIX}SchemaViolation"),
            message: serde_json::json!({
                "collection": collection,
                "field_path": field_path,
                "expected": expected,
                "actual_summary": actual_summary,
                "detail": detail,
            })
            .to_string(),
        }
    }

    /// Returns true if this error encodes an Arc 17 §17.3.6 variant
    /// (i.e. [`ValidationError::path`] starts with `@lexicon/`).
    pub fn is_lexicon_variant(&self) -> bool {
        self.path.starts_with(LEXICON_VARIANT_PREFIX)
    }

    /// Arc 17 §17.3.3 Phase B bug #2 — does this error encode a
    /// FETCH/AUTHORITY/INPUT-class lexicon hard-failure variant?
    /// These reject the write even under [`ValidationMode::Optimistic`]
    /// because the §17.3.3 contract ("HardFail propagates; record
    /// validation fails") and the operator-configured deny / pre-I/O
    /// hard errors are strictly stronger than Optimistic's accept-on-
    /// failure precedent.
    ///
    /// In the bypass set (these REJECT even in Optimistic mode):
    /// - `LexiconFetchFailed` — `fetch_failure_behavior = HardFail`
    ///   contract per §17.3.3.
    /// - `LexiconAuthorityTombstoned` / `LexiconAuthorityAmbiguous` —
    ///   authority cannot be trusted; absorbing would silently accept
    ///   records under an unverifiable schema source.
    /// - `NamespaceDenied` — operator-configured deny rule (§17.3.3
    ///   PRIORITY 3); a deliberate rejection Optimistic must not
    ///   soft-circumvent.
    /// - `LexiconInvalidNsid` — pre-I/O hard error (NSID failed
    ///   ATProto spec segment validation); an invalid identifier is
    ///   not absorbable.
    ///
    /// NOT in the bypass set (Optimistic still absorbs):
    /// - `SchemaViolation` — lexicon fetched fine, record just doesn't
    ///   match. This is the v1 unknown-NSID precedent extended; the
    ///   Optimistic contract is preserved.
    ///
    /// `LexiconAuthorityMismatch` and `LexiconInvalidSchema` are
    /// presently NOT in the bypass set (neither Phase B Scenario 6a
    /// nor the bug #2 spec named them). The predicate's tag list is
    /// the single change-point if a future scenario reclassifies them.
    pub fn is_fetch_class_lexicon_variant(&self) -> bool {
        // The path-sentinel format is `@lexicon/<Variant>`; match on
        // the tag rather than the full path so the predicate doesn't
        // accidentally accept a hand-coded validator path that happens
        // to contain a variant name as a substring.
        const FETCH_CLASS_TAGS: &[&str] = &[
            "LexiconFetchFailed",
            "LexiconAuthorityTombstoned",
            "LexiconAuthorityAmbiguous",
            "NamespaceDenied",
            "LexiconInvalidNsid",
        ];
        self.path
            .strip_prefix(LEXICON_VARIANT_PREFIX)
            .map(|tag| FETCH_CLASS_TAGS.contains(&tag))
            .unwrap_or(false)
    }
}

/// Validation result with detailed errors
pub type ValidationResult = Result<(), Vec<ValidationError>>;

/// Validate a datetime string in RFC3339 format
///
/// ATProto requires datetime strings to be in RFC3339 format with timezone.
/// Examples of valid formats:
/// - `2025-01-10T12:00:00Z`
/// - `2025-01-10T12:00:00.123Z`
/// - `2025-01-10T12:00:00+00:00`
/// - `2025-01-10T12:00:00-05:00`
fn validate_datetime(datetime_str: &str) -> bool {
    // chrono's parse_from_rfc3339 accepts a space between date and time
    // (RFC 3339 §5.6 allows lowercase `t` or a space as the separator),
    // but the AT Protocol spec requires the canonical upper-case `T`.
    // Reject space-separated forms before delegating.
    if !datetime_str.contains('T') {
        return false;
    }
    DateTime::parse_from_rfc3339(datetime_str).is_ok()
}

/// Validate text length using both byte length and grapheme count
///
/// ATProto validates text fields by grapheme count (user-perceived characters),
/// not byte length. However, we also check byte length as a secondary limit.
///
/// # Arguments
/// * `text` - The text to validate
/// * `max_bytes` - Maximum byte length (UTF-8 encoded)
/// * `max_graphemes` - Maximum grapheme count
///
/// # Returns
/// * `Ok(())` if text is within limits
/// * `Err((byte_len, grapheme_count))` if text exceeds limits
///
/// # Examples
/// * `"hello"` - 5 bytes, 5 graphemes
/// * `"👨‍👩‍👧‍👦"` - 25 bytes, 1 grapheme (family emoji)
/// * `"café"` - 5 bytes, 4 graphemes (é is one grapheme)
fn validate_text_length(
    text: &str,
    max_bytes: usize,
    max_graphemes: usize,
) -> Result<(), (usize, usize)> {
    let byte_len = text.len();
    let grapheme_count = text.graphemes(true).count();

    if byte_len > max_bytes || grapheme_count > max_graphemes {
        Err((byte_len, grapheme_count))
    } else {
        Ok(())
    }
}

/// Record validator
pub struct RecordValidator {
    /// Validation mode
    mode: ValidationMode,
    /// Collection-specific validators
    validators: HashMap<String, ValidatorFn>,
    /// Arc 17 §17.3.3 — optional lexicon fall-through. `None` = legacy
    /// behavior (Optimistic/Required modes only, no lexicon fetch).
    /// `Some` = unknown-collection writes route through the §17.3.1
    /// flow (denylist → allowlist → resolve_and_fetch →
    /// validate_against_lexicon). Constructed via
    /// [`RecordValidator::with_lexicon`].
    lexicon_config: Option<crate::config::LexiconConfig>,
    /// Arc 17 §17.3.2 — shared lexicon resolver. Paired with
    /// `lexicon_config`; both must be Some for the lexicon path to
    /// fire.
    lexicon_resolver: Option<std::sync::Arc<crate::federation::lexicon_resolver::LexResolver>>,
}

impl RecordValidator {
    /// Create a new record validator with default (Optimistic) mode
    pub fn new() -> Self {
        Self::with_mode(ValidationMode::default())
    }

    /// Create a new record validator with specified mode
    pub fn with_mode(mode: ValidationMode) -> Self {
        let mut validator = Self {
            mode,
            validators: HashMap::new(),
            lexicon_config: None,
            lexicon_resolver: None,
        };

        // Register built-in validators
        validator.register_post_validator();
        validator.register_profile_validator();
        validator.register_like_validator();
        validator.register_follow_validator();
        validator.register_repost_validator();
        validator.register_block_validator();
        validator.register_listitem_validator();
        validator.register_list_validator();
        validator.register_threadgate_validator();
        validator.register_postgate_validator();
        validator.register_generator_validator();
        validator.register_labeler_validator();

        validator
    }

    /// Get the current validation mode
    pub fn mode(&self) -> ValidationMode {
        self.mode
    }

    /// Arc 17 §17.4 Step 1.5 wiring: attach the lexicon resolver +
    /// config so unknown-collection writes route through the
    /// §17.3.1 flow. Without this call the validator behaves
    /// identically to v1 (Optimistic / Required modes only).
    #[must_use]
    pub fn with_lexicon(
        mut self,
        resolver: std::sync::Arc<crate::federation::lexicon_resolver::LexResolver>,
        config: crate::config::LexiconConfig,
    ) -> Self {
        self.lexicon_config = Some(config);
        self.lexicon_resolver = Some(resolver);
        self
    }

    /// Arc 17 §17.3.3 Pattern B — validate a record against its
    /// collection schema. The outer dispatcher is `async` so the
    /// lexicon fall-through can `.await`; the inner hand-coded
    /// validator closures stay sync (called directly inside this
    /// async fn). The 152 `register_*_validator` closure signatures
    /// (`Fn(&Value) -> ValidationResult`) are UNCHANGED — Arc 17
    /// §17.4 Step 5 audit baseline.
    pub async fn validate(&self, collection: &str, record: &Value) -> ValidationResult {
        // Start timing for metrics
        let start = std::time::Instant::now();

        // If validation mode is None, skip all validation
        if self.mode == ValidationMode::None {
            return Ok(());
        }

        let result = self.dispatch(collection, record).await;

        // Record metrics
        let duration = start.elapsed().as_secs_f64();
        match &result {
            Ok(()) => {
                crate::metrics::record_validation(collection, true, duration);
            }
            Err(errors) => {
                crate::metrics::record_validation(collection, false, duration);
                for error in errors {
                    let error_type = if error.is_lexicon_variant() {
                        // Arc 17 §17.3.6 path-sentinel encoding: the
                        // tag after `@lexicon/` IS the error type;
                        // avoids parsing the JSON message just for
                        // metrics granularity.
                        error.path.trim_start_matches(LEXICON_VARIANT_PREFIX)
                    } else {
                        error.message.split_whitespace().next().unwrap_or("unknown")
                    };
                    crate::metrics::record_validation_failure(collection, error_type);
                }
            }
        }

        result
    }

    /// Inner dispatch — separates the §17.3.3 priority ladder from
    /// the metrics bookkeeping so the logic stays readable.
    async fn dispatch(&self, collection: &str, record: &Value) -> ValidationResult {
        // PRIORITY 1 (§17.3.3): hand-coded validator (sync, preserved).
        if let Some(validator_fn) = self.validators.get(collection) {
            return validator_fn(record);
        }

        // PRIORITY 2: lexicon fall-through gate. If lexicon is not
        // configured OR disabled in config, defer to the legacy
        // unknown-collection behavior (Optimistic / Required).
        let (lex_config, lex_resolver) = match (&self.lexicon_config, &self.lexicon_resolver) {
            (Some(cfg), Some(resolver)) if cfg.enabled => (cfg, resolver),
            _ => return self.handle_unknown(collection, record),
        };

        // PRIORITY 3 (§17.3.3 / round-1 F2 closure): denylist check
        // BEFORE allowlist. Denylist hit → reject; matches first per
        // §4.9 trust-boundary semantics (Intent B). Both-match → deny
        // wins because deny is checked first.
        if let Some(deny) = lex_config.namespace_denylist.as_ref() {
            if deny.iter().any(|p| collection.starts_with(p)) {
                return Err(vec![ValidationError::namespace_denied(collection)]);
            }
        }

        // PRIORITY 4 (§17.3.3 / round-1 F2 closure): allowlist
        // exclusion → fall through to Optimistic (Intent A —
        // fetch-restriction, NOT rejection). Empty allowlist (None)
        // means "no restriction; every unknown NSID is fetchable".
        if let Some(allow) = lex_config.namespace_allowlist.as_ref() {
            if !allow.iter().any(|p| collection.starts_with(p)) {
                return self.handle_unknown(collection, record);
            }
        }

        // PRIORITY 5: resolve_and_fetch via the lexicon resolver.
        // Caller's task only awaits while the single-flight gate is
        // contended OR while DNS / HTTP fires on a miss.
        let fetched = lex_resolver.resolve_and_fetch(collection).await;

        match fetched {
            Ok(cached) => self.validate_against_lexicon(collection, record, &cached),
            Err(err) => self.handle_fetch_error(collection, record, err, lex_config),
        }
    }

    /// §17.3.3 — branch on fetch_failure_behavior. HardFail surfaces
    /// the PdsError-shaped LexiconFetchFailed; Warn emits a WARN log
    /// and delegates to Optimistic fall-through (existing precedent).
    /// No Quarantine path per §17.5.7 / round-1 F1 closure.
    fn handle_fetch_error(
        &self,
        collection: &str,
        record: &Value,
        err: crate::error::PdsError,
        lex_config: &crate::config::LexiconConfig,
    ) -> ValidationResult {
        use crate::config::FetchFailureBehavior;
        use crate::error::PdsError;

        // Classify into the round-1 F14 forensic-log taxonomy. The
        // resolver already routes specific variants; we surface
        // them verbatim as Arc 17 ValidationError structs.
        let ve = match &err {
            PdsError::LexiconAuthorityAmbiguous { nsid, candidates } => {
                ValidationError::lexicon_authority_ambiguous(nsid, candidates)
            }
            PdsError::LexiconAuthorityTombstoned { nsid, did } => {
                ValidationError::lexicon_authority_tombstoned(nsid, did)
            }
            PdsError::LexiconAuthorityMismatch { nsid, expected, found } => {
                ValidationError::lexicon_authority_mismatch(nsid, expected, found)
            }
            PdsError::LexiconInvalidNsid { nsid } => ValidationError::lexicon_invalid_nsid(nsid),
            PdsError::LexiconInvalidSchema { nsid, detail } => {
                ValidationError::lexicon_invalid_schema(nsid, detail)
            }
            PdsError::LexiconFetchFailed { nsid, failure_class, source_detail } => {
                ValidationError::lexicon_fetch_failed(nsid, failure_class, source_detail)
            }
            other => ValidationError::lexicon_fetch_failed(
                collection,
                "unknown",
                &other.to_string(),
            ),
        };

        match lex_config.fetch_failure_behavior {
            FetchFailureBehavior::HardFail => Err(vec![ve]),
            FetchFailureBehavior::Warn => {
                tracing::warn!(
                    event = "lexicon_fetch_failed_warn_fallback",
                    collection = %collection,
                    error = ?ve,
                    "lexicon fetch failed; falling back to Optimistic per fetch_failure_behavior=Warn"
                );
                self.handle_unknown(collection, record)
            }
        }
    }

    /// §17.3.3 — bind the fetched lexicon doc to proto-blue's
    /// `validate_record` and convert any failure into Arc 17's
    /// structured `SchemaViolation`. Step 0.0b finding: proto-blue's
    /// `ValidationError::InvalidValue { path, message }` gives
    /// structured `path` (→ field_path) and a descriptive `message`
    /// (→ actual_summary). `expected_type` is heuristic-on-message
    /// for v0.5 (acceptable; v0.6+ candidate to enrich if proto-blue
    /// adds structured-field accessors).
    fn validate_against_lexicon(
        &self,
        collection: &str,
        record: &Value,
        cached: &crate::federation::lexicon_cache::CachedLexicon,
    ) -> ValidationResult {
        use proto_blue::lex_json::json_to_lex;
        use proto_blue::lexicon::{validate_record, LexUserType};

        // Locate the `main` def. Arc 17 only validates record
        // collections; a non-record `main` (query/procedure/etc.) is
        // a misconfigured fetch and surfaces as InvalidSchema.
        let main_def = cached.doc.defs.get("main").ok_or_else(|| {
            vec![ValidationError::lexicon_invalid_schema(
                collection,
                "lexicon doc has no `main` definition",
            )]
        })?;
        let record_def = match main_def {
            LexUserType::Record(r) => r,
            _ => {
                return Err(vec![ValidationError::lexicon_invalid_schema(
                    collection,
                    "lexicon `main` def is not a record",
                )]);
            }
        };

        // Bridge serde_json::Value → LexValue. proto-blue's validate_record
        // operates on the lex-typed value (which distinguishes blob refs,
        // CIDs, etc. that lossily round-trip through plain JSON).
        let lex_value = json_to_lex(record);

        match validate_record(&cached.lexicons, record_def, &lex_value) {
            Ok(()) => Ok(()),
            Err(e) => {
                use proto_blue::lexicon::ValidationError as PbErr;
                let ve = match e {
                    PbErr::InvalidValue { path, message } => ValidationError::schema_violation(
                        collection,
                        &path,
                        None,
                        Some(&message),
                        &message,
                    ),
                    other => ValidationError::schema_violation(
                        collection,
                        "$",
                        None,
                        None,
                        &other.to_string(),
                    ),
                };
                Err(vec![ve])
            }
        }
    }

    /// Legacy unknown-collection handler. Wires into the Required /
    /// Optimistic / None mode matrix that pre-Arc-17 v1 used. The
    /// Arc 17 fall-through reaches this when (a) lexicon is
    /// disabled, (b) the NSID is excluded by allowlist, or (c)
    /// `fetch_failure_behavior = Warn` triggered a fall-back.
    fn handle_unknown(&self, collection: &str, record: &Value) -> ValidationResult {
        match self.mode {
            ValidationMode::Required => Err(vec![ValidationError {
                path: "$".to_string(),
                message: format!(
                    "Unknown collection '{}' - validation required but no validator found",
                    collection
                ),
            }]),
            ValidationMode::Optimistic => self.validate_basic(record),
            ValidationMode::None => Ok(()),
        }
    }

    /// Basic validation for all records
    fn validate_basic(&self, record: &Value) -> ValidationResult {
        let mut errors = Vec::new();

        // Must be an object
        if !record.is_object() {
            errors.push(ValidationError {
                path: "$".to_string(),
                message: "Record must be an object".to_string(),
            });
            return Err(errors);
        }

        // Should have $type field
        if record.get("$type").is_none() {
            errors.push(ValidationError {
                path: "$.type".to_string(),
                message: "Record should have $type field".to_string(),
            });
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Validate app.bsky.embed.images embed
    fn validate_images_embed(embed: &Value, errors: &mut Vec<ValidationError>) {
        // Required: images array (max 4 items)
        match embed.get("images") {
            None => errors.push(ValidationError {
                path: "$.embed.images".to_string(),
                message: "Required field 'images' is missing".to_string(),
            }),
            Some(images) => {
                if let Some(arr) = images.as_array() {
                    if arr.is_empty() {
                        errors.push(ValidationError {
                            path: "$.embed.images".to_string(),
                            message: "Array 'images' must contain at least 1 item".to_string(),
                        });
                    }
                    if arr.len() > 4 {
                        errors.push(ValidationError {
                            path: "$.embed.images".to_string(),
                            message: format!(
                                "Array 'images' exceeds maximum length of 4: {}",
                                arr.len()
                            ),
                        });
                    }
                    // Validate each image
                    for (i, image) in arr.iter().enumerate() {
                        if let Some(obj) = image.as_object() {
                            // Required: image (blob reference)
                            if !obj.contains_key("image") {
                                errors.push(ValidationError {
                                    path: format!("$.embed.images[{}].image", i),
                                    message: "Required field 'image' is missing".to_string(),
                                });
                            }
                            // Required: alt (max 10000 chars)
                            match obj.get("alt") {
                                None => errors.push(ValidationError {
                                    path: format!("$.embed.images[{}].alt", i),
                                    message: "Required field 'alt' is missing".to_string(),
                                }),
                                Some(alt) => {
                                    if let Some(s) = alt.as_str() {
                                        if s.len() > 10000 {
                                            errors.push(ValidationError {
                                                path: format!("$.embed.images[{}].alt", i),
                                                message: format!("Field 'alt' exceeds maximum length of 10000 characters: {}", s.len()),
                                            });
                                        }
                                    }
                                }
                            }
                            // Optional: aspectRatio
                            if let Some(aspect_ratio) = obj.get("aspectRatio") {
                                if let Some(ar_obj) = aspect_ratio.as_object() {
                                    // Validate width and height are positive integers
                                    if let Some(width) = ar_obj.get("width") {
                                        if let Some(w) = width.as_i64() {
                                            if w <= 0 {
                                                errors.push(ValidationError {
                                                    path: format!(
                                                        "$.embed.images[{}].aspectRatio.width",
                                                        i
                                                    ),
                                                    message:
                                                        "Field 'width' must be a positive integer"
                                                            .to_string(),
                                                });
                                            }
                                        }
                                    }
                                    if let Some(height) = ar_obj.get("height") {
                                        if let Some(h) = height.as_i64() {
                                            if h <= 0 {
                                                errors.push(ValidationError {
                                                    path: format!(
                                                        "$.embed.images[{}].aspectRatio.height",
                                                        i
                                                    ),
                                                    message:
                                                        "Field 'height' must be a positive integer"
                                                            .to_string(),
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                    errors.push(ValidationError {
                        path: "$.embed.images".to_string(),
                        message: "Field 'images' must be an array".to_string(),
                    });
                }
            }
        }
    }

    /// Validate app.bsky.embed.external embed
    fn validate_external_embed(embed: &Value, errors: &mut Vec<ValidationError>) {
        // Required: external object
        match embed.get("external") {
            None => errors.push(ValidationError {
                path: "$.embed.external".to_string(),
                message: "Required field 'external' is missing".to_string(),
            }),
            Some(external) => {
                if let Some(obj) = external.as_object() {
                    // Required: uri (max 8000 chars)
                    match obj.get("uri") {
                        None => errors.push(ValidationError {
                            path: "$.embed.external.uri".to_string(),
                            message: "Required field 'uri' is missing".to_string(),
                        }),
                        Some(uri) => {
                            if let Some(s) = uri.as_str() {
                                if s.len() > 8000 {
                                    errors.push(ValidationError {
                                        path: "$.embed.external.uri".to_string(),
                                        message: format!("Field 'uri' exceeds maximum length of 8000 characters: {}", s.len()),
                                    });
                                }
                                // Basic URL validation
                                if !s.starts_with("http://") && !s.starts_with("https://") {
                                    errors.push(ValidationError {
                                        path: "$.embed.external.uri".to_string(),
                                        message: "Field 'uri' must be a valid HTTP/HTTPS URL"
                                            .to_string(),
                                    });
                                }
                            }
                        }
                    }
                    // Required: title (max 5000 chars)
                    match obj.get("title") {
                        None => errors.push(ValidationError {
                            path: "$.embed.external.title".to_string(),
                            message: "Required field 'title' is missing".to_string(),
                        }),
                        Some(title) => {
                            if let Some(s) = title.as_str() {
                                if s.len() > 5000 {
                                    errors.push(ValidationError {
                                        path: "$.embed.external.title".to_string(),
                                        message: format!("Field 'title' exceeds maximum length of 5000 characters: {}", s.len()),
                                    });
                                }
                            }
                        }
                    }
                    // Required: description (max 10000 chars)
                    match obj.get("description") {
                        None => errors.push(ValidationError {
                            path: "$.embed.external.description".to_string(),
                            message: "Required field 'description' is missing".to_string(),
                        }),
                        Some(description) => {
                            if let Some(s) = description.as_str() {
                                if s.len() > 10000 {
                                    errors.push(ValidationError {
                                        path: "$.embed.external.description".to_string(),
                                        message: format!("Field 'description' exceeds maximum length of 10000 characters: {}", s.len()),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Validate app.bsky.embed.record embed
    fn validate_record_embed(embed: &Value, errors: &mut Vec<ValidationError>) {
        // Required: record object with uri
        match embed.get("record") {
            None => errors.push(ValidationError {
                path: "$.embed.record".to_string(),
                message: "Required field 'record' is missing".to_string(),
            }),
            Some(record) => {
                if let Some(obj) = record.as_object() {
                    // Required: uri (AT-URI format)
                    match obj.get("uri") {
                        None => errors.push(ValidationError {
                            path: "$.embed.record.uri".to_string(),
                            message: "Required field 'uri' is missing".to_string(),
                        }),
                        Some(uri) => {
                            if let Some(s) = uri.as_str() {
                                // Validate AT-URI format (at://)
                                if !s.starts_with("at://") {
                                    errors.push(ValidationError {
                                        path: "$.embed.record.uri".to_string(),
                                        message: "Field 'uri' must be a valid AT-URI (starts with 'at://')".to_string(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Validate app.bsky.embed.recordWithMedia embed
    fn validate_record_with_media_embed(embed: &Value, errors: &mut Vec<ValidationError>) {
        // Required: record
        if let Some(record) = embed.get("record") {
            // Validate as record embed
            let mut record_embed = serde_json::Map::new();
            record_embed.insert("record".to_string(), record.clone());
            Self::validate_record_embed(&Value::Object(record_embed), errors);
        } else {
            errors.push(ValidationError {
                path: "$.embed.record".to_string(),
                message: "Required field 'record' is missing".to_string(),
            });
        }

        // Required: media (either images or external)
        if let Some(media) = embed.get("media") {
            if let Some(media_obj) = media.as_object() {
                // Check $type to determine media type
                if let Some(media_type) = media_obj.get("$type").and_then(|t| t.as_str()) {
                    match media_type {
                        "app.bsky.embed.images" => {
                            Self::validate_images_embed(media, errors);
                        }
                        "app.bsky.embed.external" => {
                            Self::validate_external_embed(media, errors);
                        }
                        _ => {
                            errors.push(ValidationError {
                                path: "$.embed.media.$type".to_string(),
                                message: format!("Invalid media type '{}', expected 'app.bsky.embed.images' or 'app.bsky.embed.external'", media_type),
                            });
                        }
                    }
                }
            }
        } else {
            errors.push(ValidationError {
                path: "$.embed.media".to_string(),
                message: "Required field 'media' is missing".to_string(),
            });
        }
    }

    /// Register app.bsky.feed.post validator
    fn register_post_validator(&mut self) {
        self.validators.insert(
            "app.bsky.feed.post".to_string(),
            Box::new(|record: &Value| {
                let mut errors = Vec::new();

                // Required: text
                match record.get("text") {
                    None => errors.push(ValidationError {
                        path: "$.text".to_string(),
                        message: "Required field 'text' is missing".to_string(),
                    }),
                    Some(text) => {
                        if let Some(s) = text.as_str() {
                            // Validate using both byte length (3000) and grapheme count (300)
                            if let Err((byte_len, grapheme_count)) = validate_text_length(s, 3000, 300) {
                                if byte_len > 3000 {
                                    errors.push(ValidationError {
                                        path: "$.text".to_string(),
                                        message: format!("Text exceeds maximum byte length of 3000: {}", byte_len),
                                    });
                                }
                                if grapheme_count > 300 {
                                    errors.push(ValidationError {
                                        path: "$.text".to_string(),
                                        message: format!("Text exceeds maximum of 300 graphemes: {}", grapheme_count),
                                    });
                                }
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.text".to_string(),
                                message: "Field 'text' must be a string".to_string(),
                            });
                        }
                    }
                }

                // Required: createdAt
                match record.get("createdAt") {
                    None => errors.push(ValidationError {
                        path: "$.createdAt".to_string(),
                        message: "Required field 'createdAt' is missing".to_string(),
                    }),
                    Some(created_at) => {
                        if let Some(s) = created_at.as_str() {
                            // Validate RFC3339 datetime format
                            if !validate_datetime(s) {
                                errors.push(ValidationError {
                                    path: "$.createdAt".to_string(),
                                    message: format!("Field 'createdAt' must be a valid RFC3339 datetime string: '{}'", s),
                                });
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.createdAt".to_string(),
                                message: "Field 'createdAt' must be a string (datetime)".to_string(),
                            });
                        }
                    }
                }

                // Optional: langs (array with max 3 items)
                if let Some(langs) = record.get("langs") {
                    if let Some(arr) = langs.as_array() {
                        if arr.len() > 3 {
                            errors.push(ValidationError {
                                path: "$.langs".to_string(),
                                message: format!("Array 'langs' exceeds maximum length of 3: {}", arr.len()),
                            });
                        }
                    } else {
                        errors.push(ValidationError {
                            path: "$.langs".to_string(),
                            message: "Field 'langs' must be an array".to_string(),
                        });
                    }
                }

                // Optional: tags (array with max 8 items, each max 640 bytes/64 graphemes)
                if let Some(tags) = record.get("tags") {
                    if let Some(arr) = tags.as_array() {
                        if arr.len() > 8 {
                            errors.push(ValidationError {
                                path: "$.tags".to_string(),
                                message: format!("Array 'tags' exceeds maximum length of 8: {}", arr.len()),
                            });
                        }
                        for (i, tag) in arr.iter().enumerate() {
                            if let Some(s) = tag.as_str() {
                                // Validate using both byte length (640) and grapheme count (64)
                                if let Err((byte_len, grapheme_count)) = validate_text_length(s, 640, 64) {
                                    if byte_len > 640 {
                                        errors.push(ValidationError {
                                            path: format!("$.tags[{}]", i),
                                            message: format!("Tag exceeds maximum byte length of 640: {}", byte_len),
                                        });
                                    }
                                    if grapheme_count > 64 {
                                        errors.push(ValidationError {
                                            path: format!("$.tags[{}]", i),
                                            message: format!("Tag exceeds maximum of 64 graphemes: {}", grapheme_count),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }

                // Optional: embed (validate based on $type)
                if let Some(embed) = record.get("embed") {
                    if let Some(embed_obj) = embed.as_object() {
                        // Check $type to determine embed type
                        if let Some(embed_type) = embed_obj.get("$type").and_then(|t| t.as_str()) {
                            match embed_type {
                                "app.bsky.embed.images" => {
                                    Self::validate_images_embed(embed, &mut errors);
                                }
                                "app.bsky.embed.external" => {
                                    Self::validate_external_embed(embed, &mut errors);
                                }
                                "app.bsky.embed.record" => {
                                    Self::validate_record_embed(embed, &mut errors);
                                }
                                "app.bsky.embed.recordWithMedia" => {
                                    Self::validate_record_with_media_embed(embed, &mut errors);
                                }
                                _ => {
                                    errors.push(ValidationError {
                                        path: "$.embed.$type".to_string(),
                                        message: format!("Unknown embed type: '{}'", embed_type),
                                    });
                                }
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.embed.$type".to_string(),
                                message: "Field 'embed' must have a '$type' field".to_string(),
                            });
                        }
                    } else {
                        errors.push(ValidationError {
                            path: "$.embed".to_string(),
                            message: "Field 'embed' must be an object".to_string(),
                        });
                    }
                }

                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(errors)
                }
            }),
        );
    }

    /// Register app.bsky.actor.profile validator
    fn register_profile_validator(&mut self) {
        self.validators.insert(
            "app.bsky.actor.profile".to_string(),
            Box::new(|record: &Value| {
                let mut errors = Vec::new();

                // Optional: displayName (max 640 bytes, 64 graphemes)
                if let Some(display_name) = record.get("displayName") {
                    if let Some(s) = display_name.as_str() {
                        // Validate using both byte length (640) and grapheme count (64)
                        if let Err((byte_len, grapheme_count)) = validate_text_length(s, 640, 64) {
                            if byte_len > 640 {
                                errors.push(ValidationError {
                                    path: "$.displayName".to_string(),
                                    message: format!(
                                        "displayName exceeds maximum byte length of 640: {}",
                                        byte_len
                                    ),
                                });
                            }
                            if grapheme_count > 64 {
                                errors.push(ValidationError {
                                    path: "$.displayName".to_string(),
                                    message: format!(
                                        "displayName exceeds maximum of 64 graphemes: {}",
                                        grapheme_count
                                    ),
                                });
                            }
                        }
                    }
                }

                // Optional: description (max 2560 bytes, 256 graphemes)
                if let Some(description) = record.get("description") {
                    if let Some(s) = description.as_str() {
                        // Validate using both byte length (2560) and grapheme count (256)
                        if let Err((byte_len, grapheme_count)) = validate_text_length(s, 2560, 256)
                        {
                            if byte_len > 2560 {
                                errors.push(ValidationError {
                                    path: "$.description".to_string(),
                                    message: format!(
                                        "description exceeds maximum byte length of 2560: {}",
                                        byte_len
                                    ),
                                });
                            }
                            if grapheme_count > 256 {
                                errors.push(ValidationError {
                                    path: "$.description".to_string(),
                                    message: format!(
                                        "description exceeds maximum of 256 graphemes: {}",
                                        grapheme_count
                                    ),
                                });
                            }
                        }
                    }
                }

                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(errors)
                }
            }),
        );
    }

    /// Register app.bsky.feed.like validator
    fn register_like_validator(&mut self) {
        self.validators.insert(
            "app.bsky.feed.like".to_string(),
            Box::new(|record: &Value| {
                let mut errors = Vec::new();

                // Required: subject
                if record.get("subject").is_none() {
                    errors.push(ValidationError {
                        path: "$.subject".to_string(),
                        message: "Required field 'subject' is missing".to_string(),
                    });
                }

                // Required: createdAt
                match record.get("createdAt") {
                    None => errors.push(ValidationError {
                        path: "$.createdAt".to_string(),
                        message: "Required field 'createdAt' is missing".to_string(),
                    }),
                    Some(created_at) => {
                        if let Some(s) = created_at.as_str() {
                            if !validate_datetime(s) {
                                errors.push(ValidationError {
                                    path: "$.createdAt".to_string(),
                                    message: format!("Field 'createdAt' must be a valid RFC3339 datetime string: '{}'", s),
                                });
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.createdAt".to_string(),
                                message: "Field 'createdAt' must be a string (datetime)".to_string(),
                            });
                        }
                    }
                }

                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(errors)
                }
            }),
        );
    }

    /// Register app.bsky.graph.follow validator
    fn register_follow_validator(&mut self) {
        self.validators.insert(
            "app.bsky.graph.follow".to_string(),
            Box::new(|record: &Value| {
                let mut errors = Vec::new();

                // Required: subject (DID)
                match record.get("subject") {
                    None => errors.push(ValidationError {
                        path: "$.subject".to_string(),
                        message: "Required field 'subject' is missing".to_string(),
                    }),
                    Some(subject) => {
                        if let Some(s) = subject.as_str() {
                            if !s.starts_with("did:") {
                                errors.push(ValidationError {
                                    path: "$.subject".to_string(),
                                    message: "Field 'subject' must be a valid DID".to_string(),
                                });
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.subject".to_string(),
                                message: "Field 'subject' must be a string".to_string(),
                            });
                        }
                    }
                }

                // Required: createdAt
                match record.get("createdAt") {
                    None => errors.push(ValidationError {
                        path: "$.createdAt".to_string(),
                        message: "Required field 'createdAt' is missing".to_string(),
                    }),
                    Some(created_at) => {
                        if let Some(s) = created_at.as_str() {
                            if !validate_datetime(s) {
                                errors.push(ValidationError {
                                    path: "$.createdAt".to_string(),
                                    message: format!("Field 'createdAt' must be a valid RFC3339 datetime string: '{}'", s),
                                });
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.createdAt".to_string(),
                                message: "Field 'createdAt' must be a string (datetime)".to_string(),
                            });
                        }
                    }
                }

                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(errors)
                }
            }),
        );
    }

    /// Register app.bsky.feed.repost validator
    fn register_repost_validator(&mut self) {
        self.validators.insert(
            "app.bsky.feed.repost".to_string(),
            Box::new(|record: &Value| {
                let mut errors = Vec::new();

                // Required: subject
                if record.get("subject").is_none() {
                    errors.push(ValidationError {
                        path: "$.subject".to_string(),
                        message: "Required field 'subject' is missing".to_string(),
                    });
                }

                // Required: createdAt
                match record.get("createdAt") {
                    None => errors.push(ValidationError {
                        path: "$.createdAt".to_string(),
                        message: "Required field 'createdAt' is missing".to_string(),
                    }),
                    Some(created_at) => {
                        if let Some(s) = created_at.as_str() {
                            if !validate_datetime(s) {
                                errors.push(ValidationError {
                                    path: "$.createdAt".to_string(),
                                    message: format!("Field 'createdAt' must be a valid RFC3339 datetime string: '{}'", s),
                                });
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.createdAt".to_string(),
                                message: "Field 'createdAt' must be a string (datetime)".to_string(),
                            });
                        }
                    }
                }

                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(errors)
                }
            }),
        );
    }

    /// Register app.bsky.graph.block validator
    fn register_block_validator(&mut self) {
        self.validators.insert(
            "app.bsky.graph.block".to_string(),
            Box::new(|record: &Value| {
                let mut errors = Vec::new();

                // Required: subject (DID)
                match record.get("subject") {
                    None => errors.push(ValidationError {
                        path: "$.subject".to_string(),
                        message: "Required field 'subject' is missing".to_string(),
                    }),
                    Some(subject) => {
                        if let Some(s) = subject.as_str() {
                            if !s.starts_with("did:") {
                                errors.push(ValidationError {
                                    path: "$.subject".to_string(),
                                    message: "Field 'subject' must be a valid DID".to_string(),
                                });
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.subject".to_string(),
                                message: "Field 'subject' must be a string".to_string(),
                            });
                        }
                    }
                }

                // Required: createdAt
                match record.get("createdAt") {
                    None => errors.push(ValidationError {
                        path: "$.createdAt".to_string(),
                        message: "Required field 'createdAt' is missing".to_string(),
                    }),
                    Some(created_at) => {
                        if let Some(s) = created_at.as_str() {
                            if !validate_datetime(s) {
                                errors.push(ValidationError {
                                    path: "$.createdAt".to_string(),
                                    message: format!("Field 'createdAt' must be a valid RFC3339 datetime string: '{}'", s),
                                });
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.createdAt".to_string(),
                                message: "Field 'createdAt' must be a string (datetime)".to_string(),
                            });
                        }
                    }
                }

                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(errors)
                }
            }),
        );
    }

    /// Register app.bsky.graph.listitem validator
    fn register_listitem_validator(&mut self) {
        self.validators.insert(
            "app.bsky.graph.listitem".to_string(),
            Box::new(|record: &Value| {
                let mut errors = Vec::new();

                // Required: subject (DID)
                match record.get("subject") {
                    None => errors.push(ValidationError {
                        path: "$.subject".to_string(),
                        message: "Required field 'subject' is missing".to_string(),
                    }),
                    Some(subject) => {
                        if let Some(s) = subject.as_str() {
                            if !s.starts_with("did:") {
                                errors.push(ValidationError {
                                    path: "$.subject".to_string(),
                                    message: "Field 'subject' must be a valid DID".to_string(),
                                });
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.subject".to_string(),
                                message: "Field 'subject' must be a string".to_string(),
                            });
                        }
                    }
                }

                // Required: list (AT-URI)
                match record.get("list") {
                    None => errors.push(ValidationError {
                        path: "$.list".to_string(),
                        message: "Required field 'list' is missing".to_string(),
                    }),
                    Some(list) => {
                        if let Some(s) = list.as_str() {
                            if !s.starts_with("at://") {
                                errors.push(ValidationError {
                                    path: "$.list".to_string(),
                                    message: "Field 'list' must be a valid AT-URI".to_string(),
                                });
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.list".to_string(),
                                message: "Field 'list' must be a string".to_string(),
                            });
                        }
                    }
                }

                // Required: createdAt
                match record.get("createdAt") {
                    None => errors.push(ValidationError {
                        path: "$.createdAt".to_string(),
                        message: "Required field 'createdAt' is missing".to_string(),
                    }),
                    Some(created_at) => {
                        if let Some(s) = created_at.as_str() {
                            if !validate_datetime(s) {
                                errors.push(ValidationError {
                                    path: "$.createdAt".to_string(),
                                    message: format!("Field 'createdAt' must be a valid RFC3339 datetime string: '{}'", s),
                                });
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.createdAt".to_string(),
                                message: "Field 'createdAt' must be a string (datetime)".to_string(),
                            });
                        }
                    }
                }

                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(errors)
                }
            }),
        );
    }

    /// Register app.bsky.graph.list validator
    fn register_list_validator(&mut self) {
        self.validators.insert(
            "app.bsky.graph.list".to_string(),
            Box::new(|record: &Value| {
                let mut errors = Vec::new();

                // Required: name (max 640 bytes, 64 graphemes)
                match record.get("name") {
                    None => errors.push(ValidationError {
                        path: "$.name".to_string(),
                        message: "Required field 'name' is missing".to_string(),
                    }),
                    Some(name) => {
                        if let Some(s) = name.as_str() {
                            // Validate using both byte length (640) and grapheme count (64)
                            if let Err((byte_len, grapheme_count)) = validate_text_length(s, 640, 64) {
                                if byte_len > 640 {
                                    errors.push(ValidationError {
                                        path: "$.name".to_string(),
                                        message: format!("name exceeds maximum byte length of 640: {}", byte_len),
                                    });
                                }
                                if grapheme_count > 64 {
                                    errors.push(ValidationError {
                                        path: "$.name".to_string(),
                                        message: format!("name exceeds maximum of 64 graphemes: {}", grapheme_count),
                                    });
                                }
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.name".to_string(),
                                message: "Field 'name' must be a string".to_string(),
                            });
                        }
                    }
                }

                // Required: purpose (enum: modlist, curatelist, referencelist)
                match record.get("purpose") {
                    None => errors.push(ValidationError {
                        path: "$.purpose".to_string(),
                        message: "Required field 'purpose' is missing".to_string(),
                    }),
                    Some(purpose) => {
                        if let Some(s) = purpose.as_str() {
                            if !["app.bsky.graph.defs#modlist", "app.bsky.graph.defs#curatelist", "app.bsky.graph.defs#referencelist"].contains(&s) {
                                errors.push(ValidationError {
                                    path: "$.purpose".to_string(),
                                    message: format!("Field 'purpose' must be one of: modlist, curatelist, referencelist (got: '{}')", s),
                                });
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.purpose".to_string(),
                                message: "Field 'purpose' must be a string".to_string(),
                            });
                        }
                    }
                }

                // Optional: description (max 3000 bytes, 300 graphemes)
                if let Some(description) = record.get("description") {
                    if let Some(s) = description.as_str() {
                        // Validate using both byte length (3000) and grapheme count (300)
                        if let Err((byte_len, grapheme_count)) = validate_text_length(s, 3000, 300) {
                            if byte_len > 3000 {
                                errors.push(ValidationError {
                                    path: "$.description".to_string(),
                                    message: format!("description exceeds maximum byte length of 3000: {}", byte_len),
                                });
                            }
                            if grapheme_count > 300 {
                                errors.push(ValidationError {
                                    path: "$.description".to_string(),
                                    message: format!("description exceeds maximum of 300 graphemes: {}", grapheme_count),
                                });
                            }
                        }
                    }
                }

                // Required: createdAt
                match record.get("createdAt") {
                    None => errors.push(ValidationError {
                        path: "$.createdAt".to_string(),
                        message: "Required field 'createdAt' is missing".to_string(),
                    }),
                    Some(created_at) => {
                        if let Some(s) = created_at.as_str() {
                            if !validate_datetime(s) {
                                errors.push(ValidationError {
                                    path: "$.createdAt".to_string(),
                                    message: format!("Field 'createdAt' must be a valid RFC3339 datetime string: '{}'", s),
                                });
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.createdAt".to_string(),
                                message: "Field 'createdAt' must be a string (datetime)".to_string(),
                            });
                        }
                    }
                }

                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(errors)
                }
            }),
        );
    }

    /// Register app.bsky.feed.threadgate validator
    fn register_threadgate_validator(&mut self) {
        self.validators.insert(
            "app.bsky.feed.threadgate".to_string(),
            Box::new(|record: &Value| {
                let mut errors = Vec::new();

                // Required: post (AT-URI)
                match record.get("post") {
                    None => errors.push(ValidationError {
                        path: "$.post".to_string(),
                        message: "Required field 'post' is missing".to_string(),
                    }),
                    Some(post) => {
                        if let Some(s) = post.as_str() {
                            if !s.starts_with("at://") {
                                errors.push(ValidationError {
                                    path: "$.post".to_string(),
                                    message: "Field 'post' must be a valid AT-URI".to_string(),
                                });
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.post".to_string(),
                                message: "Field 'post' must be a string".to_string(),
                            });
                        }
                    }
                }

                // Optional: allow (array with max 5 items)
                if let Some(allow) = record.get("allow") {
                    if let Some(arr) = allow.as_array() {
                        if arr.len() > 5 {
                            errors.push(ValidationError {
                                path: "$.allow".to_string(),
                                message: format!("Array 'allow' exceeds maximum length of 5: {}", arr.len()),
                            });
                        }
                    } else {
                        errors.push(ValidationError {
                            path: "$.allow".to_string(),
                            message: "Field 'allow' must be an array".to_string(),
                        });
                    }
                }

                // Required: createdAt
                match record.get("createdAt") {
                    None => errors.push(ValidationError {
                        path: "$.createdAt".to_string(),
                        message: "Required field 'createdAt' is missing".to_string(),
                    }),
                    Some(created_at) => {
                        if let Some(s) = created_at.as_str() {
                            if !validate_datetime(s) {
                                errors.push(ValidationError {
                                    path: "$.createdAt".to_string(),
                                    message: format!("Field 'createdAt' must be a valid RFC3339 datetime string: '{}'", s),
                                });
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.createdAt".to_string(),
                                message: "Field 'createdAt' must be a string (datetime)".to_string(),
                            });
                        }
                    }
                }

                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(errors)
                }
            }),
        );
    }

    /// Register app.bsky.feed.postgate validator
    fn register_postgate_validator(&mut self) {
        self.validators.insert(
            "app.bsky.feed.postgate".to_string(),
            Box::new(|record: &Value| {
                let mut errors = Vec::new();

                // Required: post (AT-URI)
                match record.get("post") {
                    None => errors.push(ValidationError {
                        path: "$.post".to_string(),
                        message: "Required field 'post' is missing".to_string(),
                    }),
                    Some(post) => {
                        if let Some(s) = post.as_str() {
                            if !s.starts_with("at://") {
                                errors.push(ValidationError {
                                    path: "$.post".to_string(),
                                    message: "Field 'post' must be a valid AT-URI".to_string(),
                                });
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.post".to_string(),
                                message: "Field 'post' must be a string".to_string(),
                            });
                        }
                    }
                }

                // Optional: embeddingRules (array)
                if let Some(embedding_rules) = record.get("embeddingRules") {
                    if !embedding_rules.is_array() {
                        errors.push(ValidationError {
                            path: "$.embeddingRules".to_string(),
                            message: "Field 'embeddingRules' must be an array".to_string(),
                        });
                    }
                }

                // Optional: detachedEmbeddingUris (array of AT-URIs, max 50)
                if let Some(uris) = record.get("detachedEmbeddingUris") {
                    if let Some(arr) = uris.as_array() {
                        if arr.len() > 50 {
                            errors.push(ValidationError {
                                path: "$.detachedEmbeddingUris".to_string(),
                                message: format!("Array 'detachedEmbeddingUris' exceeds maximum length of 50: {}", arr.len()),
                            });
                        }
                        for (i, uri) in arr.iter().enumerate() {
                            if let Some(s) = uri.as_str() {
                                if !s.starts_with("at://") {
                                    errors.push(ValidationError {
                                        path: format!("$.detachedEmbeddingUris[{}]", i),
                                        message: "URI must be a valid AT-URI".to_string(),
                                    });
                                }
                            }
                        }
                    } else {
                        errors.push(ValidationError {
                            path: "$.detachedEmbeddingUris".to_string(),
                            message: "Field 'detachedEmbeddingUris' must be an array".to_string(),
                        });
                    }
                }

                // Required: createdAt
                match record.get("createdAt") {
                    None => errors.push(ValidationError {
                        path: "$.createdAt".to_string(),
                        message: "Required field 'createdAt' is missing".to_string(),
                    }),
                    Some(created_at) => {
                        if let Some(s) = created_at.as_str() {
                            if !validate_datetime(s) {
                                errors.push(ValidationError {
                                    path: "$.createdAt".to_string(),
                                    message: format!("Field 'createdAt' must be a valid RFC3339 datetime string: '{}'", s),
                                });
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.createdAt".to_string(),
                                message: "Field 'createdAt' must be a string (datetime)".to_string(),
                            });
                        }
                    }
                }

                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(errors)
                }
            }),
        );
    }

    /// Register app.bsky.feed.generator validator
    fn register_generator_validator(&mut self) {
        self.validators.insert(
            "app.bsky.feed.generator".to_string(),
            Box::new(|record: &Value| {
                let mut errors = Vec::new();

                // Required: did (DID)
                match record.get("did") {
                    None => errors.push(ValidationError {
                        path: "$.did".to_string(),
                        message: "Required field 'did' is missing".to_string(),
                    }),
                    Some(did) => {
                        if let Some(s) = did.as_str() {
                            if !s.starts_with("did:") {
                                errors.push(ValidationError {
                                    path: "$.did".to_string(),
                                    message: "Field 'did' must be a valid DID".to_string(),
                                });
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.did".to_string(),
                                message: "Field 'did' must be a string".to_string(),
                            });
                        }
                    }
                }

                // Required: displayName (max 240 bytes, 24 graphemes)
                match record.get("displayName") {
                    None => errors.push(ValidationError {
                        path: "$.displayName".to_string(),
                        message: "Required field 'displayName' is missing".to_string(),
                    }),
                    Some(display_name) => {
                        if let Some(s) = display_name.as_str() {
                            // Validate using both byte length (240) and grapheme count (24)
                            if let Err((byte_len, grapheme_count)) = validate_text_length(s, 240, 24) {
                                if byte_len > 240 {
                                    errors.push(ValidationError {
                                        path: "$.displayName".to_string(),
                                        message: format!("displayName exceeds maximum byte length of 240: {}", byte_len),
                                    });
                                }
                                if grapheme_count > 24 {
                                    errors.push(ValidationError {
                                        path: "$.displayName".to_string(),
                                        message: format!("displayName exceeds maximum of 24 graphemes: {}", grapheme_count),
                                    });
                                }
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.displayName".to_string(),
                                message: "Field 'displayName' must be a string".to_string(),
                            });
                        }
                    }
                }

                // Optional: description (max 3000 bytes, 300 graphemes)
                if let Some(description) = record.get("description") {
                    if let Some(s) = description.as_str() {
                        // Validate using both byte length (3000) and grapheme count (300)
                        if let Err((byte_len, grapheme_count)) = validate_text_length(s, 3000, 300) {
                            if byte_len > 3000 {
                                errors.push(ValidationError {
                                    path: "$.description".to_string(),
                                    message: format!("description exceeds maximum byte length of 3000: {}", byte_len),
                                });
                            }
                            if grapheme_count > 300 {
                                errors.push(ValidationError {
                                    path: "$.description".to_string(),
                                    message: format!("description exceeds maximum of 300 graphemes: {}", grapheme_count),
                                });
                            }
                        }
                    }
                }

                // Required: createdAt
                match record.get("createdAt") {
                    None => errors.push(ValidationError {
                        path: "$.createdAt".to_string(),
                        message: "Required field 'createdAt' is missing".to_string(),
                    }),
                    Some(created_at) => {
                        if let Some(s) = created_at.as_str() {
                            if !validate_datetime(s) {
                                errors.push(ValidationError {
                                    path: "$.createdAt".to_string(),
                                    message: format!("Field 'createdAt' must be a valid RFC3339 datetime string: '{}'", s),
                                });
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.createdAt".to_string(),
                                message: "Field 'createdAt' must be a string (datetime)".to_string(),
                            });
                        }
                    }
                }

                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(errors)
                }
            }),
        );
    }

    /// Register app.bsky.labeler.service validator
    fn register_labeler_validator(&mut self) {
        self.validators.insert(
            "app.bsky.labeler.service".to_string(),
            Box::new(|record: &Value| {
                let mut errors = Vec::new();

                // Required: policies (object with labelValues, labelValueDefinitions)
                if record.get("policies").is_none() {
                    errors.push(ValidationError {
                        path: "$.policies".to_string(),
                        message: "Required field 'policies' is missing".to_string(),
                    });
                }

                // Optional: labels (array)
                if let Some(labels) = record.get("labels") {
                    if !labels.is_array() {
                        errors.push(ValidationError {
                            path: "$.labels".to_string(),
                            message: "Field 'labels' must be an array".to_string(),
                        });
                    }
                }

                // Required: createdAt
                match record.get("createdAt") {
                    None => errors.push(ValidationError {
                        path: "$.createdAt".to_string(),
                        message: "Required field 'createdAt' is missing".to_string(),
                    }),
                    Some(created_at) => {
                        if let Some(s) = created_at.as_str() {
                            if !validate_datetime(s) {
                                errors.push(ValidationError {
                                    path: "$.createdAt".to_string(),
                                    message: format!("Field 'createdAt' must be a valid RFC3339 datetime string: '{}'", s),
                                });
                            }
                        } else {
                            errors.push(ValidationError {
                                path: "$.createdAt".to_string(),
                                message: "Field 'createdAt' must be a string (datetime)".to_string(),
                            });
                        }
                    }
                }

                if errors.is_empty() {
                    Ok(())
                } else {
                    Err(errors)
                }
            }),
        );
    }
}

impl Default for RecordValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert validation errors to [`PdsError`].
///
/// Arc 17 §17.3.6 routing: if the first error's `path` starts with
/// `@lexicon/`, the variant tag identifies the specific PdsError
/// variant to surface (HTTP 502 for fetch-class failures, HTTP 400
/// for client-input failures, HTTP 500 for invalid-schema). The
/// JSON-encoded `message` carries the structured fields. Hand-coded
/// validator errors (which don't use the `@lexicon/` prefix) fall
/// through to the legacy `PdsError::Validation` aggregation that
/// surfaces as HTTP 400 with a multi-line error list.
///
/// Only the *first* error is routed to a typed variant — single-
/// origin errors (which the resolver always produces) carry one
/// entry; multi-error lists from hand-coded validators all share
/// the same Validation umbrella anyway.
pub fn validation_errors_to_pds_error(errors: Vec<ValidationError>) -> PdsError {
    if let Some(first) = errors.first() {
        if first.is_lexicon_variant() {
            if let Some(err) = parse_lexicon_variant(first) {
                return err;
            }
            // Path looked like @lexicon/X but JSON decode failed —
            // fall through to the umbrella Validation rather than
            // dropping the error. Worst case is an opaque 400; the
            // structured wire shape regresses but the rejection
            // itself doesn't disappear.
        }
    }

    let messages: Vec<String> = errors
        .iter()
        .map(|e| format!("{}: {}", e.path, e.message))
        .collect();

    PdsError::Validation(format!(
        "Record validation failed:\n  - {}",
        messages.join("\n  - ")
    ))
}

/// Arc 17 §17.3.6 — parse a `@lexicon/<variant>` sentinel into the
/// concrete [`PdsError`] variant. Returns `None` if the message JSON
/// fails to decode or the variant tag is unknown (caller falls back
/// to the umbrella `PdsError::Validation`).
fn parse_lexicon_variant(err: &ValidationError) -> Option<PdsError> {
    let tag = err.path.strip_prefix(LEXICON_VARIANT_PREFIX)?;
    let payload: serde_json::Value = serde_json::from_str(&err.message).ok()?;
    let s = |k: &str| -> Option<String> {
        payload.get(k).and_then(|v| v.as_str()).map(str::to_string)
    };

    match tag {
        "NamespaceDenied" => Some(PdsError::NamespaceDenied { nsid: s("nsid")? }),
        "LexiconInvalidNsid" => Some(PdsError::LexiconInvalidNsid { nsid: s("nsid")? }),
        "LexiconInvalidSchema" => Some(PdsError::LexiconInvalidSchema {
            nsid: s("nsid")?,
            detail: s("detail")?,
        }),
        "LexiconAuthorityTombstoned" => Some(PdsError::LexiconAuthorityTombstoned {
            nsid: s("nsid")?,
            did: s("did")?,
        }),
        "LexiconAuthorityMismatch" => Some(PdsError::LexiconAuthorityMismatch {
            nsid: s("nsid")?,
            expected: s("expected")?,
            found: s("found")?,
        }),
        "LexiconAuthorityAmbiguous" => {
            let nsid = s("nsid")?;
            let candidates: Vec<String> = payload
                .get("candidates")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            Some(PdsError::LexiconAuthorityAmbiguous { nsid, candidates })
        }
        "LexiconFetchFailed" => {
            let nsid = s("nsid")?;
            // failure_class must be a 'static str at the PdsError
            // level; map the parsed string back to the closed set of
            // round-1 F14 taxonomy values, defaulting to "unknown"
            // for shapes that escape the canonical list.
            let class_str = payload.get("failure_class").and_then(|v| v.as_str()).unwrap_or("unknown");
            let failure_class: &'static str = match class_str {
                "dns_fail" => "dns_fail",
                "did_fail" => "did_fail",
                "pds_unreachable" => "pds_unreachable",
                "http_5xx" => "http_5xx",
                "http_4xx" => "http_4xx",
                "timeout" => "timeout",
                "authority_tombstoned" => "authority_tombstoned",
                "authority_ambiguous" => "authority_ambiguous",
                "invalid_schema" => "invalid_schema",
                _ => "unknown",
            };
            let source_detail = s("source_detail").unwrap_or_default();
            Some(PdsError::LexiconFetchFailed {
                nsid,
                failure_class,
                source_detail,
            })
        }
        "SchemaViolation" => Some(PdsError::SchemaViolation {
            collection: s("collection")?,
            field_path: s("field_path")?,
            expected: s("expected"),
            actual_summary: s("actual_summary"),
            detail: s("detail")?,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_validate_post_valid() {
        let validator = RecordValidator::new();

        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": "Hello world!",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.post", &post).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_post_missing_text() {
        let validator = RecordValidator::new();

        let post = json!({
            "$type": "app.bsky.feed.post",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.post", &post).await;
        assert!(result.is_err());

        if let Err(errors) = result {
            assert_eq!(errors.len(), 1);
            assert_eq!(errors[0].path, "$.text");
        }
    }

    #[tokio::test]
    async fn test_validate_post_text_too_long() {
        let validator = RecordValidator::new();

        let long_text = "a".repeat(3001);
        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": long_text,
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.post", &post).await;
        assert!(result.is_err());

        if let Err(errors) = result {
            assert!(!errors.is_empty());
            assert!(errors.iter().any(|e| e.path == "$.text"));
        }
    }

    #[tokio::test]
    async fn test_validate_post_too_many_tags() {
        let validator = RecordValidator::new();

        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": "Test post",
            "createdAt": "2025-01-10T12:00:00Z",
            "tags": ["tag1", "tag2", "tag3", "tag4", "tag5", "tag6", "tag7", "tag8", "tag9"]
        });

        let result = validator.validate("app.bsky.feed.post", &post).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_follow_valid() {
        let validator = RecordValidator::new();

        let follow = json!({
            "$type": "app.bsky.graph.follow",
            "subject": "did:plc:test123",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.follow", &follow).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_follow_invalid_did() {
        let validator = RecordValidator::new();

        let follow = json!({
            "$type": "app.bsky.graph.follow",
            "subject": "not-a-did",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.follow", &follow).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_datetime_valid_formats() {
        // RFC3339 with Z timezone
        assert!(validate_datetime("2025-01-10T12:00:00Z"));

        // RFC3339 with milliseconds and Z
        assert!(validate_datetime("2025-01-10T12:00:00.123Z"));

        // RFC3339 with microseconds
        assert!(validate_datetime("2025-01-10T12:00:00.123456Z"));

        // RFC3339 with +00:00 timezone
        assert!(validate_datetime("2025-01-10T12:00:00+00:00"));

        // RFC3339 with -05:00 timezone (EST)
        assert!(validate_datetime("2025-01-10T12:00:00-05:00"));

        // RFC3339 with +09:30 timezone (Australia)
        assert!(validate_datetime("2025-01-10T12:00:00+09:30"));
    }

    #[tokio::test]
    async fn test_validate_datetime_invalid_formats() {
        // Missing timezone
        assert!(!validate_datetime("2025-01-10T12:00:00"));

        // Invalid format (no T separator)
        assert!(!validate_datetime("2025-01-10 12:00:00Z"));

        // Invalid date
        assert!(!validate_datetime("2025-13-45T12:00:00Z"));

        // Invalid time
        assert!(!validate_datetime("2025-01-10T25:00:00Z"));

        // Completely invalid
        assert!(!validate_datetime("not a date"));

        // Empty string
        assert!(!validate_datetime(""));
    }

    #[tokio::test]
    async fn test_validate_post_invalid_datetime() {
        let validator = RecordValidator::new();

        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": "Hello world!",
            "createdAt": "2025-01-10 12:00:00"  // Missing timezone, invalid format
        });

        let result = validator.validate("app.bsky.feed.post", &post).await;
        assert!(result.is_err());

        if let Err(errors) = result {
            assert!(errors
                .iter()
                .any(|e| e.path == "$.createdAt" && e.message.contains("RFC3339")));
        }
    }

    #[tokio::test]
    async fn test_validate_post_valid_datetime_formats() {
        let validator = RecordValidator::new();

        // Test various valid datetime formats
        let valid_datetimes = vec![
            "2025-01-10T12:00:00Z",
            "2025-01-10T12:00:00.123Z",
            "2025-01-10T12:00:00+00:00",
            "2025-01-10T12:00:00-05:00",
        ];

        for datetime in valid_datetimes {
            let post = json!({
                "$type": "app.bsky.feed.post",
                "text": "Hello world!",
                "createdAt": datetime
            });

            let result = validator.validate("app.bsky.feed.post", &post).await;
            assert!(result.is_ok(), "Failed for datetime: {}", datetime);
        }
    }

    // Embed validation tests

    #[tokio::test]
    async fn test_validate_post_with_images_embed_valid() {
        let validator = RecordValidator::new();

        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": "Check out these images!",
            "createdAt": "2025-01-10T12:00:00Z",
            "embed": {
                "$type": "app.bsky.embed.images",
                "images": [
                    {
                        "image": {"$type": "blob", "ref": "bafytest", "mimeType": "image/jpeg"},
                        "alt": "A beautiful sunset",
                        "aspectRatio": {"width": 1920, "height": 1080}
                    }
                ]
            }
        });

        let result = validator.validate("app.bsky.feed.post", &post).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_post_with_images_embed_missing_alt() {
        let validator = RecordValidator::new();

        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": "Images without alt",
            "createdAt": "2025-01-10T12:00:00Z",
            "embed": {
                "$type": "app.bsky.embed.images",
                "images": [
                    {
                        "image": {"$type": "blob", "ref": "bafytest", "mimeType": "image/jpeg"}
                        // Missing alt
                    }
                ]
            }
        });

        let result = validator.validate("app.bsky.feed.post", &post).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_post_with_images_embed_too_many() {
        let validator = RecordValidator::new();

        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": "Too many images",
            "createdAt": "2025-01-10T12:00:00Z",
            "embed": {
                "$type": "app.bsky.embed.images",
                "images": [
                    {"image": {"$type": "blob"}, "alt": "1"},
                    {"image": {"$type": "blob"}, "alt": "2"},
                    {"image": {"$type": "blob"}, "alt": "3"},
                    {"image": {"$type": "blob"}, "alt": "4"},
                    {"image": {"$type": "blob"}, "alt": "5"}  // More than 4
                ]
            }
        });

        let result = validator.validate("app.bsky.feed.post", &post).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_post_with_external_embed_valid() {
        let validator = RecordValidator::new();

        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": "Check out this link!",
            "createdAt": "2025-01-10T12:00:00Z",
            "embed": {
                "$type": "app.bsky.embed.external",
                "external": {
                    "uri": "https://example.com/article",
                    "title": "An Interesting Article",
                    "description": "This is a great article about something interesting.",
                    "thumb": {"$type": "blob", "ref": "bafytest"}
                }
            }
        });

        let result = validator.validate("app.bsky.feed.post", &post).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_post_with_external_embed_invalid_uri() {
        let validator = RecordValidator::new();

        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": "Invalid URI",
            "createdAt": "2025-01-10T12:00:00Z",
            "embed": {
                "$type": "app.bsky.embed.external",
                "external": {
                    "uri": "not-a-valid-url",  // Invalid - not HTTP/HTTPS
                    "title": "Title",
                    "description": "Description"
                }
            }
        });

        let result = validator.validate("app.bsky.feed.post", &post).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_post_with_record_embed_valid() {
        let validator = RecordValidator::new();

        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": "Quoting this post",
            "createdAt": "2025-01-10T12:00:00Z",
            "embed": {
                "$type": "app.bsky.embed.record",
                "record": {
                    "uri": "at://did:plc:test/app.bsky.feed.post/abc123",
                    "cid": "bafytest"
                }
            }
        });

        let result = validator.validate("app.bsky.feed.post", &post).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_post_with_record_embed_invalid_uri() {
        let validator = RecordValidator::new();

        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": "Invalid quote",
            "createdAt": "2025-01-10T12:00:00Z",
            "embed": {
                "$type": "app.bsky.embed.record",
                "record": {
                    "uri": "https://example.com/post",  // Invalid - not AT-URI
                    "cid": "bafytest"
                }
            }
        });

        let result = validator.validate("app.bsky.feed.post", &post).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_post_with_record_with_media_embed_valid() {
        let validator = RecordValidator::new();

        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": "Quote with images",
            "createdAt": "2025-01-10T12:00:00Z",
            "embed": {
                "$type": "app.bsky.embed.recordWithMedia",
                "record": {
                    "uri": "at://did:plc:test/app.bsky.feed.post/abc123",
                    "cid": "bafytest"
                },
                "media": {
                    "$type": "app.bsky.embed.images",
                    "images": [
                        {
                            "image": {"$type": "blob", "ref": "bafytest"},
                            "alt": "Image"
                        }
                    ]
                }
            }
        });

        let result = validator.validate("app.bsky.feed.post", &post).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_post_with_record_with_media_embed_missing_media() {
        let validator = RecordValidator::new();

        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": "Quote without media",
            "createdAt": "2025-01-10T12:00:00Z",
            "embed": {
                "$type": "app.bsky.embed.recordWithMedia",
                "record": {
                    "uri": "at://did:plc:test/app.bsky.feed.post/abc123",
                    "cid": "bafytest"
                }
                // Missing media field
            }
        });

        let result = validator.validate("app.bsky.feed.post", &post).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_post_with_unknown_embed_type() {
        let validator = RecordValidator::new();

        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": "Unknown embed type",
            "createdAt": "2025-01-10T12:00:00Z",
            "embed": {
                "$type": "app.bsky.embed.unknown",
                "data": "something"
            }
        });

        let result = validator.validate("app.bsky.feed.post", &post).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_like_invalid_datetime() {
        let validator = RecordValidator::new();

        let like = json!({
            "$type": "app.bsky.feed.like",
            "subject": {"uri": "at://did:plc:test/app.bsky.feed.post/123", "cid": "bafytest"},
            "createdAt": "invalid-datetime"
        });

        let result = validator.validate("app.bsky.feed.like", &like).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_follow_invalid_datetime() {
        let validator = RecordValidator::new();

        let follow = json!({
            "$type": "app.bsky.graph.follow",
            "subject": "did:plc:test123",
            "createdAt": "2025-01-10"  // Date only, missing time and timezone
        });

        let result = validator.validate("app.bsky.graph.follow", &follow).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_repost_invalid_datetime() {
        let validator = RecordValidator::new();

        let repost = json!({
            "$type": "app.bsky.feed.repost",
            "subject": {"uri": "at://did:plc:test/app.bsky.feed.post/123", "cid": "bafytest"},
            "createdAt": 1234567890  // Number instead of string
        });

        let result = validator.validate("app.bsky.feed.repost", &repost).await;
        assert!(result.is_err());
    }

    // Validation mode tests

    #[tokio::test]
    async fn test_validation_mode_none_skips_all_validation() {
        let validator = RecordValidator::with_mode(ValidationMode::None);

        // Even completely invalid records should pass
        let invalid_post = json!({
            "$type": "app.bsky.feed.post"
            // Missing required fields: text, createdAt
        });

        let result = validator.validate("app.bsky.feed.post", &invalid_post).await;
        assert!(
            result.is_ok(),
            "ValidationMode::None should skip all validation"
        );
    }

    #[tokio::test]
    async fn test_validation_mode_optimistic_validates_known_collections() {
        let validator = RecordValidator::with_mode(ValidationMode::Optimistic);

        // Known collection with invalid data should fail
        let invalid_post = json!({
            "$type": "app.bsky.feed.post",
            "createdAt": "2025-01-10T12:00:00Z"
            // Missing required field: text
        });

        let result = validator.validate("app.bsky.feed.post", &invalid_post).await;
        assert!(
            result.is_err(),
            "Optimistic mode should validate known collections"
        );
    }

    #[tokio::test]
    async fn test_validation_mode_optimistic_accepts_unknown_collections() {
        let validator = RecordValidator::with_mode(ValidationMode::Optimistic);

        // Unknown collection with basic valid structure should pass
        let unknown_record = json!({
            "$type": "com.example.custom.record",
            "data": "some data"
        });

        let result = validator.validate("com.example.custom.record", &unknown_record).await;
        assert!(
            result.is_ok(),
            "Optimistic mode should accept unknown collections with basic validation"
        );
    }

    #[tokio::test]
    async fn test_validation_mode_optimistic_rejects_malformed_unknown() {
        let validator = RecordValidator::with_mode(ValidationMode::Optimistic);

        // Unknown collection but not even an object
        let invalid_record = json!("not an object");

        let result = validator.validate("com.example.custom.record", &invalid_record).await;
        assert!(
            result.is_err(),
            "Optimistic mode should reject malformed unknown collections"
        );
    }

    #[tokio::test]
    async fn test_validation_mode_required_validates_known_collections() {
        let validator = RecordValidator::with_mode(ValidationMode::Required);

        // Known collection with invalid data should fail
        let invalid_post = json!({
            "$type": "app.bsky.feed.post",
            "createdAt": "2025-01-10T12:00:00Z"
            // Missing required field: text
        });

        let result = validator.validate("app.bsky.feed.post", &invalid_post).await;
        assert!(
            result.is_err(),
            "Required mode should validate known collections"
        );
    }

    #[tokio::test]
    async fn test_validation_mode_required_rejects_unknown_collections() {
        let validator = RecordValidator::with_mode(ValidationMode::Required);

        // Unknown collection should be rejected even if well-formed
        let unknown_record = json!({
            "$type": "com.example.custom.record",
            "data": "some data"
        });

        let result = validator.validate("com.example.custom.record", &unknown_record).await;
        assert!(
            result.is_err(),
            "Required mode should reject unknown collections"
        );

        if let Err(errors) = result {
            assert!(!errors.is_empty());
            assert!(errors[0].message.contains("Unknown collection"));
        }
    }

    #[tokio::test]
    async fn test_validation_mode_from_str() {
        assert_eq!(
            ValidationMode::from_str("required"),
            Ok(ValidationMode::Required)
        );
        assert_eq!(
            ValidationMode::from_str("Required"),
            Ok(ValidationMode::Required)
        );
        assert_eq!(
            ValidationMode::from_str("REQUIRED"),
            Ok(ValidationMode::Required)
        );

        assert_eq!(
            ValidationMode::from_str("optimistic"),
            Ok(ValidationMode::Optimistic)
        );
        assert_eq!(
            ValidationMode::from_str("Optimistic"),
            Ok(ValidationMode::Optimistic)
        );

        assert_eq!(ValidationMode::from_str("none"), Ok(ValidationMode::None));
        assert_eq!(ValidationMode::from_str("None"), Ok(ValidationMode::None));

        assert!(ValidationMode::from_str("invalid").is_err());
        assert!(ValidationMode::from_str("").is_err());
    }

    #[tokio::test]
    async fn test_validation_mode_default() {
        let mode = ValidationMode::default();
        assert_eq!(
            mode,
            ValidationMode::Optimistic,
            "Default validation mode should be Optimistic"
        );
    }

    #[tokio::test]
    async fn test_validator_mode_getter() {
        let validator_default = RecordValidator::new();
        assert_eq!(validator_default.mode(), ValidationMode::Optimistic);

        let validator_none = RecordValidator::with_mode(ValidationMode::None);
        assert_eq!(validator_none.mode(), ValidationMode::None);

        let validator_required = RecordValidator::with_mode(ValidationMode::Required);
        assert_eq!(validator_required.mode(), ValidationMode::Required);
    }

    // Tests for new collection validators

    #[tokio::test]
    async fn test_validate_block_valid() {
        let validator = RecordValidator::new();

        let block = json!({
            "$type": "app.bsky.graph.block",
            "subject": "did:plc:test123",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.block", &block).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_block_missing_subject() {
        let validator = RecordValidator::new();

        let block = json!({
            "$type": "app.bsky.graph.block",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.block", &block).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_block_invalid_did() {
        let validator = RecordValidator::new();

        let block = json!({
            "$type": "app.bsky.graph.block",
            "subject": "not-a-did",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.block", &block).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_listitem_valid() {
        let validator = RecordValidator::new();

        let listitem = json!({
            "$type": "app.bsky.graph.listitem",
            "subject": "did:plc:test123",
            "list": "at://did:plc:owner/app.bsky.graph.list/abc123",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.listitem", &listitem).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_listitem_missing_list() {
        let validator = RecordValidator::new();

        let listitem = json!({
            "$type": "app.bsky.graph.listitem",
            "subject": "did:plc:test123",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.listitem", &listitem).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_listitem_invalid_at_uri() {
        let validator = RecordValidator::new();

        let listitem = json!({
            "$type": "app.bsky.graph.listitem",
            "subject": "did:plc:test123",
            "list": "not-an-at-uri",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.listitem", &listitem).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_list_valid() {
        let validator = RecordValidator::new();

        let list = json!({
            "$type": "app.bsky.graph.list",
            "name": "My Cool List",
            "purpose": "app.bsky.graph.defs#curatelist",
            "description": "A list of interesting accounts",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.list", &list).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_list_missing_name() {
        let validator = RecordValidator::new();

        let list = json!({
            "$type": "app.bsky.graph.list",
            "purpose": "app.bsky.graph.defs#curatelist",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.list", &list).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_list_invalid_purpose() {
        let validator = RecordValidator::new();

        let list = json!({
            "$type": "app.bsky.graph.list",
            "name": "My List",
            "purpose": "invalid-purpose",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.list", &list).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_list_name_too_long() {
        let validator = RecordValidator::new();

        let long_name = "a".repeat(641);
        let list = json!({
            "$type": "app.bsky.graph.list",
            "name": long_name,
            "purpose": "app.bsky.graph.defs#curatelist",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.list", &list).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_threadgate_valid() {
        let validator = RecordValidator::new();

        let threadgate = json!({
            "$type": "app.bsky.feed.threadgate",
            "post": "at://did:plc:test/app.bsky.feed.post/abc123",
            "allow": [{"$type": "app.bsky.feed.threadgate#mentionRule"}],
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.threadgate", &threadgate).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_threadgate_missing_post() {
        let validator = RecordValidator::new();

        let threadgate = json!({
            "$type": "app.bsky.feed.threadgate",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.threadgate", &threadgate).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_threadgate_too_many_rules() {
        let validator = RecordValidator::new();

        let threadgate = json!({
            "$type": "app.bsky.feed.threadgate",
            "post": "at://did:plc:test/app.bsky.feed.post/abc123",
            "allow": [
                {"$type": "rule1"},
                {"$type": "rule2"},
                {"$type": "rule3"},
                {"$type": "rule4"},
                {"$type": "rule5"},
                {"$type": "rule6"}
            ],
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.threadgate", &threadgate).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_postgate_valid() {
        let validator = RecordValidator::new();

        let postgate = json!({
            "$type": "app.bsky.feed.postgate",
            "post": "at://did:plc:test/app.bsky.feed.post/abc123",
            "embeddingRules": [{"$type": "app.bsky.feed.postgate#disableRule"}],
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.postgate", &postgate).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_postgate_missing_post() {
        let validator = RecordValidator::new();

        let postgate = json!({
            "$type": "app.bsky.feed.postgate",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.postgate", &postgate).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_postgate_too_many_detached_uris() {
        let validator = RecordValidator::new();

        let mut uris = Vec::new();
        for i in 0..51 {
            uris.push(format!("at://did:plc:test/app.bsky.feed.post/{}", i));
        }

        let postgate = json!({
            "$type": "app.bsky.feed.postgate",
            "post": "at://did:plc:test/app.bsky.feed.post/abc123",
            "detachedEmbeddingUris": uris,
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.postgate", &postgate).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_generator_valid() {
        let validator = RecordValidator::new();

        let generator = json!({
            "$type": "app.bsky.feed.generator",
            "did": "did:web:feed.example.com",
            "displayName": "My Cool Feed",
            "description": "A custom feed generator",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.generator", &generator).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_generator_missing_did() {
        let validator = RecordValidator::new();

        let generator = json!({
            "$type": "app.bsky.feed.generator",
            "displayName": "My Feed",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.generator", &generator).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_generator_display_name_too_long() {
        let validator = RecordValidator::new();

        let long_name = "a".repeat(241);
        let generator = json!({
            "$type": "app.bsky.feed.generator",
            "did": "did:web:feed.example.com",
            "displayName": long_name,
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.generator", &generator).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_labeler_valid() {
        let validator = RecordValidator::new();

        let labeler = json!({
            "$type": "app.bsky.labeler.service",
            "policies": {
                "labelValues": ["porn", "nudity"],
                "labelValueDefinitions": []
            },
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.labeler.service", &labeler).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_labeler_missing_policies() {
        let validator = RecordValidator::new();

        let labeler = json!({
            "$type": "app.bsky.labeler.service",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.labeler.service", &labeler).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_labeler_invalid_labels() {
        let validator = RecordValidator::new();

        let labeler = json!({
            "$type": "app.bsky.labeler.service",
            "policies": {
                "labelValues": ["porn"]
            },
            "labels": "not-an-array",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.labeler.service", &labeler).await;
        assert!(result.is_err());
    }

    // Grapheme counting tests

    #[tokio::test]
    async fn test_validate_text_length_ascii() {
        // Simple ASCII text: 1 byte = 1 grapheme
        let result = validate_text_length("hello", 10, 10);
        assert!(result.is_ok());

        let result = validate_text_length("hello world", 10, 10);
        assert!(result.is_err());
        if let Err((byte_len, grapheme_count)) = result {
            assert_eq!(byte_len, 11);
            assert_eq!(grapheme_count, 11);
        }
    }

    #[tokio::test]
    async fn test_validate_text_length_emoji() {
        // Single emoji: multiple bytes, 1 grapheme
        let emoji = "👍";
        let result = validate_text_length(emoji, 100, 1);
        assert!(result.is_ok());

        // Emoji is 4 bytes but 1 grapheme
        let result = validate_text_length(emoji, 3, 1);
        assert!(result.is_err());
        if let Err((byte_len, _)) = result {
            assert_eq!(byte_len, 4);
        }
    }

    #[tokio::test]
    async fn test_validate_text_length_family_emoji() {
        // Family emoji with ZWJ (Zero Width Joiner): 25 bytes, 1 grapheme
        let family = "👨‍👩‍👧‍👦";
        let result = validate_text_length(family, 100, 1);
        assert!(result.is_ok());

        // Should fail on grapheme count
        let result = validate_text_length(family, 100, 0);
        assert!(result.is_err());
        if let Err((byte_len, grapheme_count)) = result {
            assert!(byte_len > 20); // Family emoji is ~25 bytes
            assert_eq!(grapheme_count, 1);
        }
    }

    #[tokio::test]
    async fn test_validate_text_length_combining_characters() {
        // "é" can be represented as e + combining acute accent
        let combined = "e\u{0301}"; // e + combining acute accent = é
        let result = validate_text_length(combined, 10, 1);
        assert!(result.is_ok());

        let result = validate_text_length(combined, 10, 0);
        assert!(result.is_err());
        if let Err((byte_len, grapheme_count)) = result {
            assert_eq!(byte_len, 3); // e (1 byte) + combining accent (2 bytes)
            assert_eq!(grapheme_count, 1); // But it's 1 grapheme
        }
    }

    #[tokio::test]
    async fn test_validate_text_length_flag_emoji() {
        // Flag emojis are regional indicator symbols
        let flag = "🇺🇸"; // US flag
        let result = validate_text_length(flag, 100, 1);
        assert!(result.is_ok());

        let result = validate_text_length(flag, 100, 0);
        assert!(result.is_err());
        if let Err((byte_len, grapheme_count)) = result {
            assert_eq!(byte_len, 8); // Two regional indicators
            assert_eq!(grapheme_count, 1); // But displayed as 1 flag
        }
    }

    #[tokio::test]
    async fn test_validate_post_with_emoji_text() {
        let validator = RecordValidator::new();

        // A post with emoji should count graphemes correctly
        let emoji_text = "Hello 👋 world 🌍!"; // 2 emojis, total ~13 graphemes
        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": emoji_text,
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.post", &post).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_post_with_too_many_graphemes() {
        let validator = RecordValidator::new();

        // Create a string with exactly 301 simple emojis (each is 1 grapheme)
        let emoji_text = "😀".repeat(301);
        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": emoji_text,
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.post", &post).await;
        assert!(result.is_err());

        if let Err(errors) = result {
            assert!(errors
                .iter()
                .any(|e| { e.path == "$.text" && e.message.contains("300 graphemes") }));
        }
    }

    #[tokio::test]
    async fn test_validate_post_text_exactly_300_graphemes() {
        let validator = RecordValidator::new();

        // Create a string with exactly 300 emojis
        let emoji_text = "😀".repeat(300);
        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": emoji_text,
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.post", &post).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_profile_displayname_with_emoji() {
        let validator = RecordValidator::new();

        // Display name with emoji
        let profile = json!({
            "$type": "app.bsky.actor.profile",
            "displayName": "Alice 🎨 Smith",
        });

        let result = validator.validate("app.bsky.actor.profile", &profile).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_profile_displayname_too_many_graphemes() {
        let validator = RecordValidator::new();

        // Create a displayName with 65 emojis (exceeds 64 grapheme limit)
        let long_name = "😀".repeat(65);
        let profile = json!({
            "$type": "app.bsky.actor.profile",
            "displayName": long_name,
        });

        let result = validator.validate("app.bsky.actor.profile", &profile).await;
        assert!(result.is_err());

        if let Err(errors) = result {
            assert!(errors
                .iter()
                .any(|e| { e.path == "$.displayName" && e.message.contains("64 graphemes") }));
        }
    }

    #[tokio::test]
    async fn test_validate_profile_description_with_unicode() {
        let validator = RecordValidator::new();

        // Description with various Unicode characters
        let profile = json!({
            "$type": "app.bsky.actor.profile",
            "description": "I love coding! 💻 こんにちは 🌸 Café ☕",
        });

        let result = validator.validate("app.bsky.actor.profile", &profile).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_profile_description_too_many_graphemes() {
        let validator = RecordValidator::new();

        // Create a description with 257 emojis (exceeds 256 grapheme limit)
        let long_desc = "😀".repeat(257);
        let profile = json!({
            "$type": "app.bsky.actor.profile",
            "description": long_desc,
        });

        let result = validator.validate("app.bsky.actor.profile", &profile).await;
        assert!(result.is_err());

        if let Err(errors) = result {
            assert!(errors
                .iter()
                .any(|e| { e.path == "$.description" && e.message.contains("256 graphemes") }));
        }
    }

    #[tokio::test]
    async fn test_validate_tag_with_emoji() {
        let validator = RecordValidator::new();

        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": "Test post",
            "createdAt": "2025-01-10T12:00:00Z",
            "tags": ["coding", "rust🦀", "emoji😀"]
        });

        let result = validator.validate("app.bsky.feed.post", &post).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_tag_too_many_graphemes() {
        let validator = RecordValidator::new();

        // Create a tag with 65 emojis (exceeds 64 grapheme limit)
        let long_tag = "😀".repeat(65);
        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": "Test post",
            "createdAt": "2025-01-10T12:00:00Z",
            "tags": [long_tag]
        });

        let result = validator.validate("app.bsky.feed.post", &post).await;
        assert!(result.is_err());

        if let Err(errors) = result {
            assert!(errors
                .iter()
                .any(|e| { e.path == "$.tags[0]" && e.message.contains("64 graphemes") }));
        }
    }

    #[tokio::test]
    async fn test_validate_text_length_mixed_unicode() {
        // Mix of ASCII, Latin extended, emoji, and CJK. The string is
        // 21 graphemes (originally pegged at 20 by an off-by-one),
        // so the generous limit needs to accommodate it.
        let mixed = "Hello café 👋 こんにちは 世界";
        let result = validate_text_length(mixed, 100, 25);
        assert!(result.is_ok());

        let result = validate_text_length(mixed, 100, 10);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_validate_text_length_skin_tone_emoji() {
        // Emoji with skin tone modifier: 1 grapheme but multiple code points
        let emoji_with_tone = "👋🏽"; // Waving hand with medium skin tone
        let result = validate_text_length(emoji_with_tone, 100, 1);
        assert!(result.is_ok());

        let result = validate_text_length(emoji_with_tone, 100, 0);
        assert!(result.is_err());
        if let Err((byte_len, grapheme_count)) = result {
            assert!(byte_len > 4); // Base emoji + modifier
            assert_eq!(grapheme_count, 1); // But displayed as 1 emoji
        }
    }

    // ──────────────────────────────────────────────────────────────
    // Arc 17 §17.3 — dynamic-lexicon dispatch matrix
    //
    // Covers the §17.3.4 validate_imports matrix (via the pure helper
    // `should_validate_per_lexicon_imports`), the §17.3.3 allowlist/
    // denylist split, the fetch_failure_behavior branch, and the
    // path-sentinel round-trip from ValidationError → PdsError.
    //
    // The lexicon-fetch path is mocked through the same
    // LexiconRecordFetcher + MockDnsTxtResolver seam unit-tested in
    // src/federation/lexicon_resolver.rs; the real DNS / PLC / HTTP
    // path is Phase B (Step 4) territory.
    // ──────────────────────────────────────────────────────────────

    mod arc17_matrix {
        use super::*;
        use crate::config::{FetchFailureBehavior, LexiconConfig};
        use crate::federation::dns_resolver::{DnsTxtResolver, MockDnsTxtResolver};
        use crate::federation::lexicon_cache::LexiconCache;
        use crate::federation::lexicon_resolver::{
            LexResolver, LexiconFetcherError, LexiconRecordFetcher,
        };
        use async_trait::async_trait;
        use std::sync::Arc;

        struct MockFetcher {
            response: Option<String>,
            error: Option<fn() -> LexiconFetcherError>,
        }

        impl MockFetcher {
            fn with_doc(json: &str) -> Self {
                Self {
                    response: Some(json.to_string()),
                    error: None,
                }
            }
            fn with_error(err_fn: fn() -> LexiconFetcherError) -> Self {
                Self {
                    response: None,
                    error: Some(err_fn),
                }
            }
        }

        #[async_trait]
        impl LexiconRecordFetcher for MockFetcher {
            async fn fetch(
                &self,
                _authority_did: &str,
                _nsid: &str,
            ) -> Result<String, LexiconFetcherError> {
                if let Some(err_fn) = self.error {
                    return Err(err_fn());
                }
                self.response.clone().ok_or(LexiconFetcherError::Http4xx("404".to_string()))
            }
        }

        fn sample_doc(nsid: &str) -> String {
            format!(
                r#"{{
                    "lexicon": 1,
                    "id": "{nsid}",
                    "defs": {{
                        "main": {{
                            "type": "record",
                            "key": "tid",
                            "record": {{
                                "type": "object",
                                "required": ["text"],
                                "properties": {{
                                    "text": {{ "type": "string" }}
                                }}
                            }}
                        }}
                    }}
                }}"#
            )
        }

        fn build_validator(
            config: LexiconConfig,
            fetcher: MockFetcher,
            dns_txt_for_nsid: Option<(&str, &str)>,
        ) -> RecordValidator {
            let dns = if let Some((auth, did)) = dns_txt_for_nsid {
                MockDnsTxtResolver::new()
                    .with_txt(&format!("_lexicon.{auth}"), vec![format!("did={did}")])
            } else {
                MockDnsTxtResolver::new()
            };
            let cache = Arc::new(LexiconCache::in_memory(60));
            let dns: Arc<dyn DnsTxtResolver> = Arc::new(dns);
            let fetcher: Arc<dyn LexiconRecordFetcher> = Arc::new(fetcher);
            let resolver = Arc::new(LexResolver::new(cache, dns, fetcher, config.clone()));
            RecordValidator::with_mode(ValidationMode::Optimistic).with_lexicon(resolver, config)
        }

        // ─── §17.3.4 validate_imports override matrix (pure helper) ───

        #[test]
        fn override_local_write_none_always_validates() {
            assert!(should_validate_per_lexicon_imports(None, None));
            assert!(should_validate_per_lexicon_imports(
                None,
                Some(&LexiconConfig::default())
            ));
        }

        #[test]
        fn override_local_write_explicit_true_always_validates() {
            assert!(should_validate_per_lexicon_imports(Some(true), None));
            let mut cfg = LexiconConfig::default();
            cfg.enabled = true;
            cfg.validate_imports = false;
            assert!(should_validate_per_lexicon_imports(Some(true), Some(&cfg)));
        }

        #[test]
        fn override_car_import_no_lexicon_config_honors_bypass() {
            // write.validate = Some(false), no lexicon config → bypass.
            assert!(!should_validate_per_lexicon_imports(Some(false), None));
        }

        #[test]
        fn override_car_import_lexicon_disabled_honors_bypass() {
            let mut cfg = LexiconConfig::default();
            cfg.enabled = false;
            cfg.validate_imports = true; // would fire if enabled
            assert!(!should_validate_per_lexicon_imports(Some(false), Some(&cfg)));
        }

        #[test]
        fn override_car_import_validate_imports_false_honors_bypass() {
            let mut cfg = LexiconConfig::default();
            cfg.enabled = true;
            cfg.validate_imports = false;
            assert!(!should_validate_per_lexicon_imports(Some(false), Some(&cfg)));
        }

        #[test]
        fn override_car_import_enabled_and_validate_imports_fires() {
            let mut cfg = LexiconConfig::default();
            cfg.enabled = true;
            cfg.validate_imports = true;
            assert!(should_validate_per_lexicon_imports(Some(false), Some(&cfg)));
        }

        // ─── §17.3.3 known-NSID short-circuit ───

        #[tokio::test]
        async fn known_nsid_uses_hand_coded_validator_not_lexicon_path() {
            // Build a validator with a lexicon resolver wired in but
            // also the default hand-coded validators (post-validator
            // is registered for app.bsky.feed.post). The hand-coded
            // path MUST win; lexicon fetch must not fire.
            let mut cfg = LexiconConfig::default();
            cfg.enabled = true;
            // Configure a fetcher that PANICS if called — the
            // hand-coded path must short-circuit before fetcher invoke.
            let fetcher = MockFetcher::with_error(|| {
                panic!("fetcher must not fire for known NSID");
            });
            let validator = build_validator(cfg, fetcher, None);
            let post = json!({
                "$type": "app.bsky.feed.post",
                "text": "hello",
                "createdAt": "2025-01-10T12:00:00Z"
            });
            // Hand-coded validator should accept the post.
            assert!(validator.validate("app.bsky.feed.post", &post).await.is_ok());
        }

        // ─── §17.3.3 allowlist/denylist split ───

        #[tokio::test]
        async fn denylist_hit_rejects_with_namespace_denied_sentinel() {
            let mut cfg = LexiconConfig::default();
            cfg.enabled = true;
            cfg.namespace_denylist = Some(vec!["com.evil.".to_string()]);
            let fetcher = MockFetcher::with_doc(&sample_doc("com.evil.thing"));
            let validator = build_validator(cfg, fetcher, Some(("evil.com", "did:plc:x")));
            let record = json!({"text": "hi"});
            let errors = validator
                .validate("com.evil.thing", &record)
                .await
                .unwrap_err();
            assert_eq!(errors.len(), 1);
            assert_eq!(errors[0].path, "@lexicon/NamespaceDenied");
            assert!(errors[0].message.contains("com.evil.thing"));
        }

        #[tokio::test]
        async fn allowlist_exclusion_falls_through_to_optimistic() {
            // allowlist non-empty, NSID not in it → Optimistic path
            // (Intent A — fetch-restriction, NOT rejection). The
            // record passes validate_basic (it's an object with $type).
            let mut cfg = LexiconConfig::default();
            cfg.enabled = true;
            cfg.namespace_allowlist = Some(vec!["com.allowed.".to_string()]);
            let fetcher = MockFetcher::with_error(|| {
                panic!("fetcher must not fire for allowlist-excluded NSID")
            });
            let validator = build_validator(cfg, fetcher, None);
            let record = json!({
                "$type": "com.example.other.thing",
                "data": "x"
            });
            assert!(validator
                .validate("com.example.other.thing", &record)
                .await
                .is_ok());
        }

        #[tokio::test]
        async fn deny_and_allow_both_match_deny_wins() {
            let mut cfg = LexiconConfig::default();
            cfg.enabled = true;
            cfg.namespace_denylist = Some(vec!["com.evil.".to_string()]);
            cfg.namespace_allowlist = Some(vec!["com.evil.".to_string()]);
            let fetcher = MockFetcher::with_doc(&sample_doc("com.evil.thing"));
            let validator = build_validator(cfg, fetcher, Some(("evil.com", "did:plc:x")));
            let record = json!({"text": "hi"});
            let errors = validator
                .validate("com.evil.thing", &record)
                .await
                .unwrap_err();
            assert_eq!(errors[0].path, "@lexicon/NamespaceDenied");
        }

        // ─── §17.3.3 fetch-failure branch ───

        #[tokio::test]
        async fn hardfail_fetch_failure_surfaces_lexicon_fetch_failed_sentinel() {
            let mut cfg = LexiconConfig::default();
            cfg.enabled = true;
            cfg.fetch_failure_behavior = FetchFailureBehavior::HardFail;
            let fetcher = MockFetcher::with_error(|| {
                LexiconFetcherError::Http5xx("503".to_string())
            });
            let validator =
                build_validator(cfg, fetcher, Some(("thing.example.com", "did:plc:x")));
            let record = json!({"text": "hi"});
            let errors = validator
                .validate("com.example.thing.foo", &record)
                .await
                .unwrap_err();
            assert_eq!(errors[0].path, "@lexicon/LexiconFetchFailed");
            assert!(errors[0].message.contains("http_5xx"));
        }

        #[tokio::test]
        async fn warn_fetch_failure_falls_through_to_optimistic() {
            let mut cfg = LexiconConfig::default();
            cfg.enabled = true;
            cfg.fetch_failure_behavior = FetchFailureBehavior::Warn;
            let fetcher = MockFetcher::with_error(|| {
                LexiconFetcherError::Http5xx("503".to_string())
            });
            let validator =
                build_validator(cfg, fetcher, Some(("thing.example.com", "did:plc:x")));
            let record = json!({
                "$type": "com.example.thing.foo",
                "data": "x"
            });
            // Warn mode + Optimistic fallback accepts the record.
            assert!(validator
                .validate("com.example.thing.foo", &record)
                .await
                .is_ok());
        }

        // ─── §17.3.3 happy-path lexicon validation + SchemaViolation ───

        #[tokio::test]
        async fn lexicon_validation_happy_path_accepts_well_formed_record() {
            let mut cfg = LexiconConfig::default();
            cfg.enabled = true;
            let fetcher = MockFetcher::with_doc(&sample_doc("com.example.thing.foo"));
            let validator =
                build_validator(cfg, fetcher, Some(("thing.example.com", "did:plc:x")));
            let record = json!({
                "$type": "com.example.thing.foo",
                "text": "hello"
            });
            assert!(validator
                .validate("com.example.thing.foo", &record)
                .await
                .is_ok());
        }

        #[tokio::test]
        async fn lexicon_validation_missing_required_field_surfaces_schema_violation() {
            let mut cfg = LexiconConfig::default();
            cfg.enabled = true;
            let fetcher = MockFetcher::with_doc(&sample_doc("com.example.thing.foo"));
            let validator =
                build_validator(cfg, fetcher, Some(("thing.example.com", "did:plc:x")));
            let record = json!({
                "$type": "com.example.thing.foo"
                // text missing — schema requires it
            });
            let errors = validator
                .validate("com.example.thing.foo", &record)
                .await
                .unwrap_err();
            assert_eq!(errors[0].path, "@lexicon/SchemaViolation");
        }

        // ─── PdsError round-trip via validation_errors_to_pds_error ───

        #[test]
        fn pds_error_roundtrip_namespace_denied() {
            let ve = ValidationError::namespace_denied("com.evil.thing");
            let pe = validation_errors_to_pds_error(vec![ve]);
            match pe {
                PdsError::NamespaceDenied { nsid } => assert_eq!(nsid, "com.evil.thing"),
                other => panic!("expected NamespaceDenied, got {other:?}"),
            }
        }

        #[test]
        fn pds_error_roundtrip_lexicon_fetch_failed_preserves_failure_class() {
            let ve = ValidationError::lexicon_fetch_failed(
                "app.bsky.feed.post",
                "http_5xx",
                "503 Service Unavailable",
            );
            let pe = validation_errors_to_pds_error(vec![ve]);
            match pe {
                PdsError::LexiconFetchFailed {
                    nsid,
                    failure_class,
                    source_detail,
                } => {
                    assert_eq!(nsid, "app.bsky.feed.post");
                    assert_eq!(failure_class, "http_5xx");
                    assert!(source_detail.contains("503"));
                }
                other => panic!("expected LexiconFetchFailed, got {other:?}"),
            }
        }

        #[test]
        fn pds_error_roundtrip_schema_violation_preserves_field_path() {
            let ve = ValidationError::schema_violation(
                "app.bsky.feed.post",
                "/text",
                Some("string"),
                Some("missing required field"),
                "Required field missing: text",
            );
            let pe = validation_errors_to_pds_error(vec![ve]);
            match pe {
                PdsError::SchemaViolation {
                    collection,
                    field_path,
                    expected,
                    actual_summary,
                    detail,
                } => {
                    assert_eq!(collection, "app.bsky.feed.post");
                    assert_eq!(field_path, "/text");
                    assert_eq!(expected.as_deref(), Some("string"));
                    assert!(actual_summary.is_some());
                    assert!(detail.contains("text"));
                }
                other => panic!("expected SchemaViolation, got {other:?}"),
            }
        }

        #[test]
        fn pds_error_roundtrip_authority_ambiguous_carries_candidates() {
            let candidates = vec!["did:plc:one".to_string(), "did:plc:two".to_string()];
            let ve = ValidationError::lexicon_authority_ambiguous("app.bsky.feed.post", &candidates);
            let pe = validation_errors_to_pds_error(vec![ve]);
            match pe {
                PdsError::LexiconAuthorityAmbiguous { nsid, candidates: c } => {
                    assert_eq!(nsid, "app.bsky.feed.post");
                    assert_eq!(c.len(), 2);
                }
                other => panic!("expected LexiconAuthorityAmbiguous, got {other:?}"),
            }
        }

        #[test]
        fn pds_error_legacy_validation_error_uses_umbrella_validation_variant() {
            // Hand-coded validator path (no @lexicon/ prefix) → umbrella.
            let ve = ValidationError {
                path: "$.text".to_string(),
                message: "Required field missing".to_string(),
            };
            let pe = validation_errors_to_pds_error(vec![ve]);
            assert!(matches!(pe, PdsError::Validation(_)));
        }

        // ─── §17.3.3 Phase B bug #2 — fetch-class predicate + propagate matrix ───
        //
        // The bug: a HardFail lexicon fetch failure was being absorbed
        // by `ValidationMode::Optimistic` at `repository.rs::validate_write`.
        // The fix introduces `is_fetch_class_lexicon_variant` + the
        // `should_propagate_validation_errors` matrix. These tests pin
        // both halves so future variants (e.g. an Arc-17.x reclassifying
        // `LexiconAuthorityMismatch` into the bypass set) flip the
        // predicate, not the surrounding plumbing.

        #[test]
        fn fetch_class_predicate_lexicon_fetch_failed_in_set() {
            let ve =
                ValidationError::lexicon_fetch_failed("com.example.foo", "pds_unreachable", "x");
            assert!(ve.is_fetch_class_lexicon_variant());
        }

        #[test]
        fn fetch_class_predicate_authority_tombstoned_in_set() {
            let ve =
                ValidationError::lexicon_authority_tombstoned("com.example.foo", "did:plc:gone");
            assert!(ve.is_fetch_class_lexicon_variant());
        }

        #[test]
        fn fetch_class_predicate_authority_ambiguous_in_set() {
            let ve = ValidationError::lexicon_authority_ambiguous(
                "com.example.foo",
                &["did:plc:a".to_string(), "did:plc:b".to_string()],
            );
            assert!(ve.is_fetch_class_lexicon_variant());
        }

        #[test]
        fn fetch_class_predicate_namespace_denied_in_set() {
            let ve = ValidationError::namespace_denied("com.evil.thing");
            assert!(ve.is_fetch_class_lexicon_variant());
        }

        #[test]
        fn fetch_class_predicate_lexicon_invalid_nsid_in_set() {
            let ve = ValidationError::lexicon_invalid_nsid("not-a-real-nsid");
            assert!(ve.is_fetch_class_lexicon_variant());
        }

        #[test]
        fn fetch_class_predicate_schema_violation_not_in_set() {
            // SchemaViolation = lexicon fetched fine, record doesn't
            // match. Optimistic absorption preserved.
            let ve = ValidationError::schema_violation(
                "com.example.foo",
                "/text",
                Some("string"),
                Some("number"),
                "text must be a string",
            );
            assert!(!ve.is_fetch_class_lexicon_variant());
        }

        #[test]
        fn fetch_class_predicate_authority_mismatch_not_in_set_currently() {
            // Documented in the predicate doc-comment: NOT in the
            // bypass set at v0.5; this test pins current behavior so
            // a future reclassification is intentional, not accidental.
            let ve = ValidationError::lexicon_authority_mismatch(
                "com.example.foo",
                "did:plc:expected",
                "did:plc:found",
            );
            assert!(!ve.is_fetch_class_lexicon_variant());
        }

        #[test]
        fn fetch_class_predicate_lexicon_invalid_schema_not_in_set_currently() {
            // Documented in the predicate doc-comment: NOT in the
            // bypass set at v0.5; future reclassification is the
            // single change-point in the predicate's tag list.
            let ve = ValidationError::lexicon_invalid_schema("com.example.foo", "broken doc");
            assert!(!ve.is_fetch_class_lexicon_variant());
        }

        #[test]
        fn fetch_class_predicate_hand_coded_validator_path_not_in_set() {
            // Plain JSON-pointer paths (the 152 hand-coded validators)
            // must never trip the predicate.
            let ve = ValidationError {
                path: "$.text".to_string(),
                message: "Required field missing".to_string(),
            };
            assert!(!ve.is_fetch_class_lexicon_variant());
        }

        #[test]
        fn fetch_class_predicate_path_with_variant_name_substring_not_in_set() {
            // Defense against accidental substring match. A
            // hand-coded path that happens to contain a variant name
            // must NOT trip the predicate (the prefix gate keeps
            // us honest).
            let ve = ValidationError {
                path: "$.LexiconFetchFailed".to_string(),
                message: "field path coincidence".to_string(),
            };
            assert!(!ve.is_fetch_class_lexicon_variant());
        }

        // ── propagate-matrix tests ──

        fn one_lex_fetch_failed() -> Vec<ValidationError> {
            vec![ValidationError::lexicon_fetch_failed(
                "com.example.foo",
                "pds_unreachable",
                "connection refused",
            )]
        }

        fn one_schema_violation() -> Vec<ValidationError> {
            vec![ValidationError::schema_violation(
                "com.example.foo",
                "/text",
                Some("string"),
                None,
                "missing text",
            )]
        }

        fn one_hand_coded_error() -> Vec<ValidationError> {
            vec![ValidationError {
                path: "$.text".to_string(),
                message: "Required field missing".to_string(),
            }]
        }

        #[test]
        fn propagate_matrix_required_always_propagates_fetch_class() {
            assert!(should_propagate_validation_errors(
                &one_lex_fetch_failed(),
                ValidationMode::Required
            ));
        }

        #[test]
        fn propagate_matrix_required_always_propagates_schema_violation() {
            assert!(should_propagate_validation_errors(
                &one_schema_violation(),
                ValidationMode::Required
            ));
        }

        #[test]
        fn propagate_matrix_required_always_propagates_hand_coded() {
            assert!(should_propagate_validation_errors(
                &one_hand_coded_error(),
                ValidationMode::Required
            ));
        }

        #[test]
        fn propagate_matrix_none_propagates_defensive() {
            // In practice None mode short-circuits before the Err
            // branch — but if it ever reaches here, propagate is the
            // defensive choice.
            assert!(should_propagate_validation_errors(
                &one_lex_fetch_failed(),
                ValidationMode::None
            ));
        }

        #[test]
        fn propagate_matrix_optimistic_propagates_fetch_class() {
            // THIS is bug #2's core assertion: HardFail + Optimistic
            // must propagate (not absorb).
            assert!(should_propagate_validation_errors(
                &one_lex_fetch_failed(),
                ValidationMode::Optimistic
            ));
        }

        #[test]
        fn propagate_matrix_optimistic_absorbs_schema_violation() {
            // v1 contract: SchemaViolation under Optimistic = warn-
            // and-accept. Bug #2's fix must NOT regress this.
            assert!(!should_propagate_validation_errors(
                &one_schema_violation(),
                ValidationMode::Optimistic
            ));
        }

        #[test]
        fn propagate_matrix_optimistic_absorbs_hand_coded() {
            // The 152 hand-coded validator errors retain their pre-
            // Arc-17 Optimistic absorption.
            assert!(!should_propagate_validation_errors(
                &one_hand_coded_error(),
                ValidationMode::Optimistic
            ));
        }

        #[test]
        fn propagate_matrix_optimistic_mixed_propagates_if_any_fetch_class() {
            // A single fetch-class variant in a multi-error vec
            // forces propagation — even if other errors would be
            // absorbable on their own.
            let mut errs = one_schema_violation();
            errs.extend(one_lex_fetch_failed());
            assert!(should_propagate_validation_errors(
                &errs,
                ValidationMode::Optimistic
            ));
        }

        #[test]
        fn propagate_matrix_optimistic_empty_absorbs() {
            // Defensive: the Err branch shouldn't see an empty vec
            // (validators always emit ≥1 error), but if it does,
            // Optimistic absorption is the consistent choice.
            assert!(!should_propagate_validation_errors(
                &[],
                ValidationMode::Optimistic
            ));
        }
    }
}
