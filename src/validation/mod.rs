/// Record validation module
///
/// Validates records against ATProto lexicon schemas
use crate::error::PdsError;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;

/// Validation error detail
#[derive(Debug, Clone)]
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
    // Try to parse as RFC3339
    DateTime::parse_from_rfc3339(datetime_str).is_ok()
}

/// Record validator
pub struct RecordValidator {
    /// Collection-specific validators
    validators: HashMap<String, Box<dyn Fn(&Value) -> ValidationResult + Send + Sync>>,
}

impl RecordValidator {
    /// Create a new record validator
    pub fn new() -> Self {
        let mut validator = Self {
            validators: HashMap::new(),
        };

        // Register built-in validators
        validator.register_post_validator();
        validator.register_profile_validator();
        validator.register_like_validator();
        validator.register_follow_validator();
        validator.register_repost_validator();

        validator
    }

    /// Validate a record against its collection schema
    pub fn validate(&self, collection: &str, record: &Value) -> ValidationResult {
        // Check if we have a specific validator for this collection
        if let Some(validator_fn) = self.validators.get(collection) {
            return validator_fn(record);
        }

        // No specific validator - do basic validation
        self.validate_basic(record)
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
                            // Max length: 3000 characters
                            if s.len() > 3000 {
                                errors.push(ValidationError {
                                    path: "$.text".to_string(),
                                    message: format!("Text exceeds maximum length of 3000 characters: {}", s.len()),
                                });
                            }
                            // Max graphemes: 300
                            let grapheme_count = s.chars().count();
                            if grapheme_count > 300 {
                                errors.push(ValidationError {
                                    path: "$.text".to_string(),
                                    message: format!("Text exceeds maximum of 300 graphemes: {}", grapheme_count),
                                });
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

                // Optional: tags (array with max 8 items, each max 640 chars/64 graphemes)
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
                                if s.len() > 640 {
                                    errors.push(ValidationError {
                                        path: format!("$.tags[{}]", i),
                                        message: format!("Tag exceeds maximum length of 640 characters: {}", s.len()),
                                    });
                                }
                                if s.chars().count() > 64 {
                                    errors.push(ValidationError {
                                        path: format!("$.tags[{}]", i),
                                        message: format!("Tag exceeds maximum of 64 graphemes: {}", s.chars().count()),
                                    });
                                }
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

    /// Register app.bsky.actor.profile validator
    fn register_profile_validator(&mut self) {
        self.validators.insert(
            "app.bsky.actor.profile".to_string(),
            Box::new(|record: &Value| {
                let mut errors = Vec::new();

                // Optional: displayName (max 640 chars, 64 graphemes)
                if let Some(display_name) = record.get("displayName") {
                    if let Some(s) = display_name.as_str() {
                        if s.len() > 640 {
                            errors.push(ValidationError {
                                path: "$.displayName".to_string(),
                                message: format!("displayName exceeds maximum length of 640 characters: {}", s.len()),
                            });
                        }
                        if s.chars().count() > 64 {
                            errors.push(ValidationError {
                                path: "$.displayName".to_string(),
                                message: format!("displayName exceeds maximum of 64 graphemes: {}", s.chars().count()),
                            });
                        }
                    }
                }

                // Optional: description (max 2560 chars, 256 graphemes)
                if let Some(description) = record.get("description") {
                    if let Some(s) = description.as_str() {
                        if s.len() > 2560 {
                            errors.push(ValidationError {
                                path: "$.description".to_string(),
                                message: format!("description exceeds maximum length of 2560 characters: {}", s.len()),
                            });
                        }
                        if s.chars().count() > 256 {
                            errors.push(ValidationError {
                                path: "$.description".to_string(),
                                message: format!("description exceeds maximum of 256 graphemes: {}", s.chars().count()),
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

    PdsError::Validation(format!("Record validation failed:\n  - {}", messages.join("\n  - ")))
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
            assert!(errors.iter().any(|e| e.path == "$.createdAt" && e.message.contains("RFC3339")));
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
}
