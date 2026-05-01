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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ValidationError {
    pub path: String,
    pub message: String,
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

    /// Validate a record against its collection schema
    pub fn validate(&self, collection: &str, record: &Value) -> ValidationResult {
        // Start timing for metrics
        let start = std::time::Instant::now();

        // If validation mode is None, skip all validation
        if self.mode == ValidationMode::None {
            return Ok(());
        }

        // Perform validation
        let result = if let Some(validator_fn) = self.validators.get(collection) {
            // Check if we have a specific validator for this collection
            validator_fn(record)
        } else {
            // No specific validator for this collection
            match self.mode {
                ValidationMode::Required => {
                    // In Required mode, reject unknown collections
                    Err(vec![ValidationError {
                        path: "$".to_string(),
                        message: format!(
                            "Unknown collection '{}' - validation required but no validator found",
                            collection
                        ),
                    }])
                }
                ValidationMode::Optimistic => {
                    // In Optimistic mode, fall back to basic validation
                    self.validate_basic(record)
                }
                ValidationMode::None => {
                    // Already handled above, but for completeness
                    Ok(())
                }
            }
        };

        // Record metrics
        let duration = start.elapsed().as_secs_f64();
        match &result {
            Ok(()) => {
                crate::metrics::record_validation(collection, true, duration);
            }
            Err(errors) => {
                crate::metrics::record_validation(collection, false, duration);
                // Record each error type
                for error in errors {
                    // Extract error type from message (first word or "unknown")
                    let error_type = error.message.split_whitespace().next().unwrap_or("unknown");
                    crate::metrics::record_validation_failure(collection, error_type);
                }
            }
        }

        result
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

/// Convert validation errors to PdsError
pub fn validation_errors_to_pds_error(errors: Vec<ValidationError>) -> PdsError {
    let messages: Vec<String> = errors
        .iter()
        .map(|e| format!("{}: {}", e.path, e.message))
        .collect();

    PdsError::Validation(format!(
        "Record validation failed:\n  - {}",
        messages.join("\n  - ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_validate_post_valid() {
        let validator = RecordValidator::new();

        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": "Hello world!",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.post", &post);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_post_missing_text() {
        let validator = RecordValidator::new();

        let post = json!({
            "$type": "app.bsky.feed.post",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.post", &post);
        assert!(result.is_err());

        if let Err(errors) = result {
            assert_eq!(errors.len(), 1);
            assert_eq!(errors[0].path, "$.text");
        }
    }

    #[test]
    fn test_validate_post_text_too_long() {
        let validator = RecordValidator::new();

        let long_text = "a".repeat(3001);
        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": long_text,
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.post", &post);
        assert!(result.is_err());

        if let Err(errors) = result {
            assert!(!errors.is_empty());
            assert!(errors.iter().any(|e| e.path == "$.text"));
        }
    }

    #[test]
    fn test_validate_post_too_many_tags() {
        let validator = RecordValidator::new();

        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": "Test post",
            "createdAt": "2025-01-10T12:00:00Z",
            "tags": ["tag1", "tag2", "tag3", "tag4", "tag5", "tag6", "tag7", "tag8", "tag9"]
        });

        let result = validator.validate("app.bsky.feed.post", &post);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_follow_valid() {
        let validator = RecordValidator::new();

        let follow = json!({
            "$type": "app.bsky.graph.follow",
            "subject": "did:plc:test123",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.follow", &follow);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_follow_invalid_did() {
        let validator = RecordValidator::new();

        let follow = json!({
            "$type": "app.bsky.graph.follow",
            "subject": "not-a-did",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.follow", &follow);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_datetime_valid_formats() {
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

    #[test]
    fn test_validate_datetime_invalid_formats() {
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

    #[test]
    fn test_validate_post_invalid_datetime() {
        let validator = RecordValidator::new();

        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": "Hello world!",
            "createdAt": "2025-01-10 12:00:00"  // Missing timezone, invalid format
        });

        let result = validator.validate("app.bsky.feed.post", &post);
        assert!(result.is_err());

        if let Err(errors) = result {
            assert!(errors
                .iter()
                .any(|e| e.path == "$.createdAt" && e.message.contains("RFC3339")));
        }
    }

    #[test]
    fn test_validate_post_valid_datetime_formats() {
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

            let result = validator.validate("app.bsky.feed.post", &post);
            assert!(result.is_ok(), "Failed for datetime: {}", datetime);
        }
    }

    // Embed validation tests

    #[test]
    fn test_validate_post_with_images_embed_valid() {
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

        let result = validator.validate("app.bsky.feed.post", &post);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_post_with_images_embed_missing_alt() {
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

        let result = validator.validate("app.bsky.feed.post", &post);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_post_with_images_embed_too_many() {
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

        let result = validator.validate("app.bsky.feed.post", &post);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_post_with_external_embed_valid() {
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

        let result = validator.validate("app.bsky.feed.post", &post);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_post_with_external_embed_invalid_uri() {
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

        let result = validator.validate("app.bsky.feed.post", &post);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_post_with_record_embed_valid() {
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

        let result = validator.validate("app.bsky.feed.post", &post);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_post_with_record_embed_invalid_uri() {
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

        let result = validator.validate("app.bsky.feed.post", &post);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_post_with_record_with_media_embed_valid() {
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

        let result = validator.validate("app.bsky.feed.post", &post);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_post_with_record_with_media_embed_missing_media() {
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

        let result = validator.validate("app.bsky.feed.post", &post);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_post_with_unknown_embed_type() {
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

        let result = validator.validate("app.bsky.feed.post", &post);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_like_invalid_datetime() {
        let validator = RecordValidator::new();

        let like = json!({
            "$type": "app.bsky.feed.like",
            "subject": {"uri": "at://did:plc:test/app.bsky.feed.post/123", "cid": "bafytest"},
            "createdAt": "invalid-datetime"
        });

        let result = validator.validate("app.bsky.feed.like", &like);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_follow_invalid_datetime() {
        let validator = RecordValidator::new();

        let follow = json!({
            "$type": "app.bsky.graph.follow",
            "subject": "did:plc:test123",
            "createdAt": "2025-01-10"  // Date only, missing time and timezone
        });

        let result = validator.validate("app.bsky.graph.follow", &follow);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_repost_invalid_datetime() {
        let validator = RecordValidator::new();

        let repost = json!({
            "$type": "app.bsky.feed.repost",
            "subject": {"uri": "at://did:plc:test/app.bsky.feed.post/123", "cid": "bafytest"},
            "createdAt": 1234567890  // Number instead of string
        });

        let result = validator.validate("app.bsky.feed.repost", &repost);
        assert!(result.is_err());
    }

    // Validation mode tests

    #[test]
    fn test_validation_mode_none_skips_all_validation() {
        let validator = RecordValidator::with_mode(ValidationMode::None);

        // Even completely invalid records should pass
        let invalid_post = json!({
            "$type": "app.bsky.feed.post"
            // Missing required fields: text, createdAt
        });

        let result = validator.validate("app.bsky.feed.post", &invalid_post);
        assert!(
            result.is_ok(),
            "ValidationMode::None should skip all validation"
        );
    }

    #[test]
    fn test_validation_mode_optimistic_validates_known_collections() {
        let validator = RecordValidator::with_mode(ValidationMode::Optimistic);

        // Known collection with invalid data should fail
        let invalid_post = json!({
            "$type": "app.bsky.feed.post",
            "createdAt": "2025-01-10T12:00:00Z"
            // Missing required field: text
        });

        let result = validator.validate("app.bsky.feed.post", &invalid_post);
        assert!(
            result.is_err(),
            "Optimistic mode should validate known collections"
        );
    }

    #[test]
    fn test_validation_mode_optimistic_accepts_unknown_collections() {
        let validator = RecordValidator::with_mode(ValidationMode::Optimistic);

        // Unknown collection with basic valid structure should pass
        let unknown_record = json!({
            "$type": "com.example.custom.record",
            "data": "some data"
        });

        let result = validator.validate("com.example.custom.record", &unknown_record);
        assert!(
            result.is_ok(),
            "Optimistic mode should accept unknown collections with basic validation"
        );
    }

    #[test]
    fn test_validation_mode_optimistic_rejects_malformed_unknown() {
        let validator = RecordValidator::with_mode(ValidationMode::Optimistic);

        // Unknown collection but not even an object
        let invalid_record = json!("not an object");

        let result = validator.validate("com.example.custom.record", &invalid_record);
        assert!(
            result.is_err(),
            "Optimistic mode should reject malformed unknown collections"
        );
    }

    #[test]
    fn test_validation_mode_required_validates_known_collections() {
        let validator = RecordValidator::with_mode(ValidationMode::Required);

        // Known collection with invalid data should fail
        let invalid_post = json!({
            "$type": "app.bsky.feed.post",
            "createdAt": "2025-01-10T12:00:00Z"
            // Missing required field: text
        });

        let result = validator.validate("app.bsky.feed.post", &invalid_post);
        assert!(
            result.is_err(),
            "Required mode should validate known collections"
        );
    }

    #[test]
    fn test_validation_mode_required_rejects_unknown_collections() {
        let validator = RecordValidator::with_mode(ValidationMode::Required);

        // Unknown collection should be rejected even if well-formed
        let unknown_record = json!({
            "$type": "com.example.custom.record",
            "data": "some data"
        });

        let result = validator.validate("com.example.custom.record", &unknown_record);
        assert!(
            result.is_err(),
            "Required mode should reject unknown collections"
        );

        if let Err(errors) = result {
            assert!(!errors.is_empty());
            assert!(errors[0].message.contains("Unknown collection"));
        }
    }

    #[test]
    fn test_validation_mode_from_str() {
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

    #[test]
    fn test_validation_mode_default() {
        let mode = ValidationMode::default();
        assert_eq!(
            mode,
            ValidationMode::Optimistic,
            "Default validation mode should be Optimistic"
        );
    }

    #[test]
    fn test_validator_mode_getter() {
        let validator_default = RecordValidator::new();
        assert_eq!(validator_default.mode(), ValidationMode::Optimistic);

        let validator_none = RecordValidator::with_mode(ValidationMode::None);
        assert_eq!(validator_none.mode(), ValidationMode::None);

        let validator_required = RecordValidator::with_mode(ValidationMode::Required);
        assert_eq!(validator_required.mode(), ValidationMode::Required);
    }

    // Tests for new collection validators

    #[test]
    fn test_validate_block_valid() {
        let validator = RecordValidator::new();

        let block = json!({
            "$type": "app.bsky.graph.block",
            "subject": "did:plc:test123",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.block", &block);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_block_missing_subject() {
        let validator = RecordValidator::new();

        let block = json!({
            "$type": "app.bsky.graph.block",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.block", &block);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_block_invalid_did() {
        let validator = RecordValidator::new();

        let block = json!({
            "$type": "app.bsky.graph.block",
            "subject": "not-a-did",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.block", &block);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_listitem_valid() {
        let validator = RecordValidator::new();

        let listitem = json!({
            "$type": "app.bsky.graph.listitem",
            "subject": "did:plc:test123",
            "list": "at://did:plc:owner/app.bsky.graph.list/abc123",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.listitem", &listitem);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_listitem_missing_list() {
        let validator = RecordValidator::new();

        let listitem = json!({
            "$type": "app.bsky.graph.listitem",
            "subject": "did:plc:test123",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.listitem", &listitem);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_listitem_invalid_at_uri() {
        let validator = RecordValidator::new();

        let listitem = json!({
            "$type": "app.bsky.graph.listitem",
            "subject": "did:plc:test123",
            "list": "not-an-at-uri",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.listitem", &listitem);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_list_valid() {
        let validator = RecordValidator::new();

        let list = json!({
            "$type": "app.bsky.graph.list",
            "name": "My Cool List",
            "purpose": "app.bsky.graph.defs#curatelist",
            "description": "A list of interesting accounts",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.list", &list);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_list_missing_name() {
        let validator = RecordValidator::new();

        let list = json!({
            "$type": "app.bsky.graph.list",
            "purpose": "app.bsky.graph.defs#curatelist",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.list", &list);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_list_invalid_purpose() {
        let validator = RecordValidator::new();

        let list = json!({
            "$type": "app.bsky.graph.list",
            "name": "My List",
            "purpose": "invalid-purpose",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.list", &list);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_list_name_too_long() {
        let validator = RecordValidator::new();

        let long_name = "a".repeat(641);
        let list = json!({
            "$type": "app.bsky.graph.list",
            "name": long_name,
            "purpose": "app.bsky.graph.defs#curatelist",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.graph.list", &list);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_threadgate_valid() {
        let validator = RecordValidator::new();

        let threadgate = json!({
            "$type": "app.bsky.feed.threadgate",
            "post": "at://did:plc:test/app.bsky.feed.post/abc123",
            "allow": [{"$type": "app.bsky.feed.threadgate#mentionRule"}],
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.threadgate", &threadgate);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_threadgate_missing_post() {
        let validator = RecordValidator::new();

        let threadgate = json!({
            "$type": "app.bsky.feed.threadgate",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.threadgate", &threadgate);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_threadgate_too_many_rules() {
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

        let result = validator.validate("app.bsky.feed.threadgate", &threadgate);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_postgate_valid() {
        let validator = RecordValidator::new();

        let postgate = json!({
            "$type": "app.bsky.feed.postgate",
            "post": "at://did:plc:test/app.bsky.feed.post/abc123",
            "embeddingRules": [{"$type": "app.bsky.feed.postgate#disableRule"}],
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.postgate", &postgate);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_postgate_missing_post() {
        let validator = RecordValidator::new();

        let postgate = json!({
            "$type": "app.bsky.feed.postgate",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.postgate", &postgate);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_postgate_too_many_detached_uris() {
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

        let result = validator.validate("app.bsky.feed.postgate", &postgate);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_generator_valid() {
        let validator = RecordValidator::new();

        let generator = json!({
            "$type": "app.bsky.feed.generator",
            "did": "did:web:feed.example.com",
            "displayName": "My Cool Feed",
            "description": "A custom feed generator",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.generator", &generator);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_generator_missing_did() {
        let validator = RecordValidator::new();

        let generator = json!({
            "$type": "app.bsky.feed.generator",
            "displayName": "My Feed",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.generator", &generator);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_generator_display_name_too_long() {
        let validator = RecordValidator::new();

        let long_name = "a".repeat(241);
        let generator = json!({
            "$type": "app.bsky.feed.generator",
            "did": "did:web:feed.example.com",
            "displayName": long_name,
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.generator", &generator);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_labeler_valid() {
        let validator = RecordValidator::new();

        let labeler = json!({
            "$type": "app.bsky.labeler.service",
            "policies": {
                "labelValues": ["porn", "nudity"],
                "labelValueDefinitions": []
            },
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.labeler.service", &labeler);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_labeler_missing_policies() {
        let validator = RecordValidator::new();

        let labeler = json!({
            "$type": "app.bsky.labeler.service",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.labeler.service", &labeler);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_labeler_invalid_labels() {
        let validator = RecordValidator::new();

        let labeler = json!({
            "$type": "app.bsky.labeler.service",
            "policies": {
                "labelValues": ["porn"]
            },
            "labels": "not-an-array",
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.labeler.service", &labeler);
        assert!(result.is_err());
    }

    // Grapheme counting tests

    #[test]
    fn test_validate_text_length_ascii() {
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

    #[test]
    fn test_validate_text_length_emoji() {
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

    #[test]
    fn test_validate_text_length_family_emoji() {
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

    #[test]
    fn test_validate_text_length_combining_characters() {
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

    #[test]
    fn test_validate_text_length_flag_emoji() {
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

    #[test]
    fn test_validate_post_with_emoji_text() {
        let validator = RecordValidator::new();

        // A post with emoji should count graphemes correctly
        let emoji_text = "Hello 👋 world 🌍!"; // 2 emojis, total ~13 graphemes
        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": emoji_text,
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.post", &post);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_post_with_too_many_graphemes() {
        let validator = RecordValidator::new();

        // Create a string with exactly 301 simple emojis (each is 1 grapheme)
        let emoji_text = "😀".repeat(301);
        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": emoji_text,
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.post", &post);
        assert!(result.is_err());

        if let Err(errors) = result {
            assert!(errors
                .iter()
                .any(|e| { e.path == "$.text" && e.message.contains("300 graphemes") }));
        }
    }

    #[test]
    fn test_validate_post_text_exactly_300_graphemes() {
        let validator = RecordValidator::new();

        // Create a string with exactly 300 emojis
        let emoji_text = "😀".repeat(300);
        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": emoji_text,
            "createdAt": "2025-01-10T12:00:00Z"
        });

        let result = validator.validate("app.bsky.feed.post", &post);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_profile_displayname_with_emoji() {
        let validator = RecordValidator::new();

        // Display name with emoji
        let profile = json!({
            "$type": "app.bsky.actor.profile",
            "displayName": "Alice 🎨 Smith",
        });

        let result = validator.validate("app.bsky.actor.profile", &profile);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_profile_displayname_too_many_graphemes() {
        let validator = RecordValidator::new();

        // Create a displayName with 65 emojis (exceeds 64 grapheme limit)
        let long_name = "😀".repeat(65);
        let profile = json!({
            "$type": "app.bsky.actor.profile",
            "displayName": long_name,
        });

        let result = validator.validate("app.bsky.actor.profile", &profile);
        assert!(result.is_err());

        if let Err(errors) = result {
            assert!(errors
                .iter()
                .any(|e| { e.path == "$.displayName" && e.message.contains("64 graphemes") }));
        }
    }

    #[test]
    fn test_validate_profile_description_with_unicode() {
        let validator = RecordValidator::new();

        // Description with various Unicode characters
        let profile = json!({
            "$type": "app.bsky.actor.profile",
            "description": "I love coding! 💻 こんにちは 🌸 Café ☕",
        });

        let result = validator.validate("app.bsky.actor.profile", &profile);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_profile_description_too_many_graphemes() {
        let validator = RecordValidator::new();

        // Create a description with 257 emojis (exceeds 256 grapheme limit)
        let long_desc = "😀".repeat(257);
        let profile = json!({
            "$type": "app.bsky.actor.profile",
            "description": long_desc,
        });

        let result = validator.validate("app.bsky.actor.profile", &profile);
        assert!(result.is_err());

        if let Err(errors) = result {
            assert!(errors
                .iter()
                .any(|e| { e.path == "$.description" && e.message.contains("256 graphemes") }));
        }
    }

    #[test]
    fn test_validate_tag_with_emoji() {
        let validator = RecordValidator::new();

        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": "Test post",
            "createdAt": "2025-01-10T12:00:00Z",
            "tags": ["coding", "rust🦀", "emoji😀"]
        });

        let result = validator.validate("app.bsky.feed.post", &post);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_tag_too_many_graphemes() {
        let validator = RecordValidator::new();

        // Create a tag with 65 emojis (exceeds 64 grapheme limit)
        let long_tag = "😀".repeat(65);
        let post = json!({
            "$type": "app.bsky.feed.post",
            "text": "Test post",
            "createdAt": "2025-01-10T12:00:00Z",
            "tags": [long_tag]
        });

        let result = validator.validate("app.bsky.feed.post", &post);
        assert!(result.is_err());

        if let Err(errors) = result {
            assert!(errors
                .iter()
                .any(|e| { e.path == "$.tags[0]" && e.message.contains("64 graphemes") }));
        }
    }

    #[test]
    fn test_validate_text_length_mixed_unicode() {
        // Mix of ASCII, Latin extended, emoji, and CJK. The string is
        // 21 graphemes (originally pegged at 20 by an off-by-one),
        // so the generous limit needs to accommodate it.
        let mixed = "Hello café 👋 こんにちは 世界";
        let result = validate_text_length(mixed, 100, 25);
        assert!(result.is_ok());

        let result = validate_text_length(mixed, 100, 10);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_text_length_skin_tone_emoji() {
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
}
