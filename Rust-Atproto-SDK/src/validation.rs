//! Lexicon Validation Engine for ATProto
//!
//! This module implements runtime validation of records against lexicon schemas.
//! It enforces all constraints defined in the ATProto lexicon specification including:
//!
//! - Required fields
//! - String length constraints (minLength, maxLength)
//! - String grapheme constraints (maxGraphemes)
//! - Format validation (did, handle, uri, datetime, etc.)
//! - Numeric constraints (minimum, maximum)
//! - Enum constraints (knownValues)
//! - Array constraints (minLength, maxLength)
//! - Reference type validation
//! - Union type validation
//!
//! # Example
//!
//! ```no_run
//! use atproto::validation::{Validator, LexiconSchema};
//! use serde_json::json;
//!
//! // Load schema
//! // let schema = LexiconSchema::from_file("lexicons/app/bsky/feed/post.json")?;
//!
//! // Validate a record
//! // let record = json!({
//! //     "$type": "app.bsky.feed.post",
//! //     "text": "Hello world!",
//! //     "createdAt": "2025-01-15T10:00:00Z"
//! // });
//!
//! // let result = schema.validate(&record);
//! ```

use crate::rich_text::unicode::UnicodeString;
use crate::syntax::is_valid_nsid;
use crate::types::Did;
use libipld::cid::Cid;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::str::FromStr;
use thiserror::Error;

/// Validation error types
#[derive(Error, Debug)]
pub enum ValidationError {
    #[error("Missing required field: {0}")]
    MissingRequiredField(String),

    #[error("Invalid type for field '{field}': expected {expected}, got {actual}")]
    InvalidType {
        field: String,
        expected: String,
        actual: String,
    },

    #[error("String too short: '{field}' must be at least {min} characters, got {actual}")]
    StringTooShort {
        field: String,
        min: usize,
        actual: usize,
    },

    #[error("String too long: '{field}' must be at most {max} characters, got {actual}")]
    StringTooLong {
        field: String,
        max: usize,
        actual: usize,
    },

    #[error(
        "Too many graphemes: '{field}' must be at most {max} graphemes, got {actual}"
    )]
    TooManyGraphemes {
        field: String,
        max: usize,
        actual: usize,
    },

    #[error("Invalid format for field '{field}': expected {format}, got '{value}'")]
    InvalidFormat {
        field: String,
        format: String,
        value: String,
    },

    #[error("Value too small: '{field}' must be at least {min}, got {actual}")]
    ValueTooSmall {
        field: String,
        min: i64,
        actual: i64,
    },

    #[error("Value too large: '{field}' must be at most {max}, got {actual}")]
    ValueTooLarge {
        field: String,
        max: i64,
        actual: i64,
    },

    #[error("Invalid enum value: '{field}' must be one of {allowed:?}, got '{value}'")]
    InvalidEnumValue {
        field: String,
        allowed: Vec<String>,
        value: String,
    },

    #[error("Array too short: '{field}' must have at least {min} items, got {actual}")]
    ArrayTooShort {
        field: String,
        min: usize,
        actual: usize,
    },

    #[error("Array too long: '{field}' must have at most {max} items, got {actual}")]
    ArrayTooLong {
        field: String,
        max: usize,
        actual: usize,
    },

    #[error("Schema error: {0}")]
    SchemaError(String),

    #[error("Reference not found: {0}")]
    ReferenceNotFound(String),

    #[error("Union validation failed: no matching type found for '{field}'")]
    UnionValidationFailed { field: String },

    #[error("Missing $type property")]
    MissingTypeProperty,

    #[error("Invalid $type: expected {expected}, got {actual}")]
    InvalidTypeProperty { expected: String, actual: String },
}

/// Result type for validation
pub type ValidationResult<T> = std::result::Result<T, ValidationError>;

/// String format validators
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StringFormat {
    Did,
    Handle,
    Uri,
    AtUri,
    Nsid,
    RecordKey,
    Cid,
    Datetime,
    AtIdentifier,
    Language,
}

impl StringFormat {
    /// Validate a string against this format
    pub fn validate(&self, value: &str) -> bool {
        match self {
            StringFormat::Did => Did::new(value).is_ok(),
            StringFormat::Handle => crate::handle::is_valid_handle(value),
            StringFormat::Uri | StringFormat::AtUri => {
                // Basic URI validation
                value.contains(':')
            }
            StringFormat::Nsid => is_valid_nsid(value),
            StringFormat::RecordKey => {
                // Record keys are typically TIDs or custom strings
                !value.is_empty() && value.len() <= 512
            }
            StringFormat::Cid => Cid::from_str(value).is_ok(),
            StringFormat::Datetime => {
                // Basic ISO 8601 datetime validation
                chrono::DateTime::parse_from_rfc3339(value).is_ok()
            }
            StringFormat::AtIdentifier => {
                // Can be either DID or handle
                Did::new(value).is_ok() || crate::handle::is_valid_handle(value)
            }
            StringFormat::Language => {
                // BCP 47 language tag validation (simplified)
                !value.is_empty() && value.len() <= 16
            }
        }
    }
}

/// Lexicon type definition
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LexiconType {
    String {
        #[serde(rename = "minLength", skip_serializing_if = "Option::is_none")]
        min_length: Option<usize>,
        #[serde(rename = "maxLength", skip_serializing_if = "Option::is_none")]
        max_length: Option<usize>,
        #[serde(rename = "maxGraphemes", skip_serializing_if = "Option::is_none")]
        max_graphemes: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        format: Option<StringFormat>,
        #[serde(rename = "knownValues", skip_serializing_if = "Option::is_none")]
        known_values: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        default: Option<String>,
    },
    Integer {
        #[serde(skip_serializing_if = "Option::is_none")]
        minimum: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        maximum: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        default: Option<i64>,
    },
    Boolean {
        #[serde(skip_serializing_if = "Option::is_none")]
        default: Option<bool>,
    },
    Array {
        items: Box<LexiconType>,
        #[serde(rename = "minLength", skip_serializing_if = "Option::is_none")]
        min_length: Option<usize>,
        #[serde(rename = "maxLength", skip_serializing_if = "Option::is_none")]
        max_length: Option<usize>,
    },
    Object {
        #[serde(skip_serializing_if = "Option::is_none")]
        required: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        nullable: Option<Vec<String>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        properties: Option<HashMap<String, LexiconType>>,
    },
    Ref {
        #[serde(rename = "ref")]
        ref_path: String,
    },
    Union {
        refs: Vec<String>,
    },
    Unknown,
    Blob {
        #[serde(skip_serializing_if = "Option::is_none")]
        accept: Option<Vec<String>>,
        #[serde(rename = "maxSize", skip_serializing_if = "Option::is_none")]
        max_size: Option<usize>,
    },
    Record {
        key: String,
        record: Box<LexiconType>,
    },
    Procedure {
        #[serde(skip_serializing_if = "Option::is_none")]
        input: Option<Box<LexiconInput>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<Box<LexiconOutput>>,
    },
}

/// Lexicon input definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LexiconInput {
    pub encoding: String,
    pub schema: LexiconType,
}

/// Lexicon output definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LexiconOutput {
    pub encoding: String,
    pub schema: LexiconType,
}

/// Lexicon definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LexiconDef {
    #[serde(flatten)]
    pub def_type: LexiconType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Lexicon document (root schema)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LexiconSchema {
    pub lexicon: u32,
    pub id: String,
    pub defs: HashMap<String, LexiconDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl LexiconSchema {
    /// Load a schema from JSON string
    pub fn from_json(json: &str) -> ValidationResult<Self> {
        serde_json::from_str(json)
            .map_err(|e| ValidationError::SchemaError(format!("Failed to parse schema: {}", e)))
    }

    /// Get a definition by name
    pub fn get_def(&self, name: &str) -> Option<&LexiconDef> {
        self.defs.get(name)
    }

    /// Validate a value against this schema
    pub fn validate(&self, value: &Value) -> ValidationResult<()> {
        // Check for $type property
        if let Some(type_prop) = value.get("$type") {
            let type_str = type_prop
                .as_str()
                .ok_or(ValidationError::MissingTypeProperty)?;

            // Parse the type string (format: "id" or "id#def")
            let expected_type = if type_str.contains('#') {
                type_str.to_string()
            } else {
                format!("{}#main", type_str)
            };

            let actual_type = format!("{}#main", self.id);

            if expected_type != actual_type && !expected_type.starts_with(&self.id) {
                return Err(ValidationError::InvalidTypeProperty {
                    expected: actual_type,
                    actual: expected_type,
                });
            }
        }

        // Validate against main definition
        if let Some(main_def) = self.get_def("main") {
            self.validate_type(&main_def.def_type, value, "$root")
        } else {
            Err(ValidationError::SchemaError(
                "No main definition found".to_string(),
            ))
        }
    }

    /// Validate a value against a specific type
    fn validate_type(
        &self,
        type_def: &LexiconType,
        value: &Value,
        field_path: &str,
    ) -> ValidationResult<()> {
        match type_def {
            LexiconType::String {
                min_length,
                max_length,
                max_graphemes,
                format,
                known_values,
                ..
            } => {
                let s = value.as_str().ok_or(ValidationError::InvalidType {
                    field: field_path.to_string(),
                    expected: "string".to_string(),
                    actual: format!("{:?}", value),
                })?;

                // Check length constraints
                if let Some(min) = min_length {
                    if s.len() < *min {
                        return Err(ValidationError::StringTooShort {
                            field: field_path.to_string(),
                            min: *min,
                            actual: s.len(),
                        });
                    }
                }

                if let Some(max) = max_length {
                    if s.len() > *max {
                        return Err(ValidationError::StringTooLong {
                            field: field_path.to_string(),
                            max: *max,
                            actual: s.len(),
                        });
                    }
                }

                // Check grapheme constraints
                if let Some(max) = max_graphemes {
                    let grapheme_count = UnicodeString::from(s.to_string()).grapheme_len();
                    if grapheme_count > *max {
                        return Err(ValidationError::TooManyGraphemes {
                            field: field_path.to_string(),
                            max: *max,
                            actual: grapheme_count,
                        });
                    }
                }

                // Check format
                if let Some(fmt) = format {
                    if !fmt.validate(s) {
                        return Err(ValidationError::InvalidFormat {
                            field: field_path.to_string(),
                            format: format!("{:?}", fmt),
                            value: s.to_string(),
                        });
                    }
                }

                // Check enum values
                if let Some(allowed) = known_values {
                    if !allowed.contains(&s.to_string()) {
                        return Err(ValidationError::InvalidEnumValue {
                            field: field_path.to_string(),
                            allowed: allowed.clone(),
                            value: s.to_string(),
                        });
                    }
                }

                Ok(())
            }

            LexiconType::Integer {
                minimum, maximum, ..
            } => {
                let num = value.as_i64().ok_or(ValidationError::InvalidType {
                    field: field_path.to_string(),
                    expected: "integer".to_string(),
                    actual: format!("{:?}", value),
                })?;

                if let Some(min) = minimum {
                    if num < *min {
                        return Err(ValidationError::ValueTooSmall {
                            field: field_path.to_string(),
                            min: *min,
                            actual: num,
                        });
                    }
                }

                if let Some(max) = maximum {
                    if num > *max {
                        return Err(ValidationError::ValueTooLarge {
                            field: field_path.to_string(),
                            max: *max,
                            actual: num,
                        });
                    }
                }

                Ok(())
            }

            LexiconType::Boolean { .. } => {
                value.as_bool().ok_or(ValidationError::InvalidType {
                    field: field_path.to_string(),
                    expected: "boolean".to_string(),
                    actual: format!("{:?}", value),
                })?;
                Ok(())
            }

            LexiconType::Array {
                items,
                min_length,
                max_length,
            } => {
                let arr = value.as_array().ok_or(ValidationError::InvalidType {
                    field: field_path.to_string(),
                    expected: "array".to_string(),
                    actual: format!("{:?}", value),
                })?;

                if let Some(min) = min_length {
                    if arr.len() < *min {
                        return Err(ValidationError::ArrayTooShort {
                            field: field_path.to_string(),
                            min: *min,
                            actual: arr.len(),
                        });
                    }
                }

                if let Some(max) = max_length {
                    if arr.len() > *max {
                        return Err(ValidationError::ArrayTooLong {
                            field: field_path.to_string(),
                            max: *max,
                            actual: arr.len(),
                        });
                    }
                }

                // Validate each item
                for (i, item) in arr.iter().enumerate() {
                    let item_path = format!("{}[{}]", field_path, i);
                    self.validate_type(items, item, &item_path)?;
                }

                Ok(())
            }

            LexiconType::Object {
                required,
                nullable,
                properties,
            } => {
                let obj = value.as_object().ok_or(ValidationError::InvalidType {
                    field: field_path.to_string(),
                    expected: "object".to_string(),
                    actual: format!("{:?}", value),
                })?;

                // Check required fields
                if let Some(req_fields) = required {
                    for field_name in req_fields {
                        if !obj.contains_key(field_name) {
                            return Err(ValidationError::MissingRequiredField(
                                format!("{}.{}", field_path, field_name),
                            ));
                        }
                    }
                }

                // Validate properties
                if let Some(props) = properties {
                    for (prop_name, prop_type) in props {
                        if let Some(prop_value) = obj.get(prop_name) {
                            // Check if null is allowed
                            if prop_value.is_null() {
                                if let Some(nullable_fields) = nullable {
                                    if !nullable_fields.contains(prop_name) {
                                        return Err(ValidationError::InvalidType {
                                            field: format!("{}.{}", field_path, prop_name),
                                            expected: "non-null value".to_string(),
                                            actual: "null".to_string(),
                                        });
                                    }
                                }
                                continue;
                            }

                            let prop_path = if field_path == "$root" {
                                prop_name.clone()
                            } else {
                                format!("{}.{}", field_path, prop_name)
                            };
                            self.validate_type(prop_type, prop_value, &prop_path)?;
                        }
                    }
                }

                Ok(())
            }

            LexiconType::Ref { ref_path } => {
                // Resolve reference
                self.validate_ref(ref_path, value, field_path)
            }

            LexiconType::Union { refs } => {
                // Try each union member - at least one must succeed
                for ref_path in refs {
                    if self.validate_ref(ref_path, value, field_path).is_ok() {
                        return Ok(());
                    }
                }
                Err(ValidationError::UnionValidationFailed {
                    field: field_path.to_string(),
                })
            }

            LexiconType::Unknown => {
                // Unknown type accepts anything
                Ok(())
            }

            LexiconType::Blob { .. } => {
                // Blob validation would check accept types and max size
                // For now, accept any object with $type: "blob"
                if let Some(type_prop) = value.get("$type") {
                    if type_prop.as_str() == Some("blob") {
                        return Ok(());
                    }
                }
                Err(ValidationError::InvalidType {
                    field: field_path.to_string(),
                    expected: "blob".to_string(),
                    actual: format!("{:?}", value),
                })
            }

            LexiconType::Record { record, .. } => {
                // Validate the record object
                self.validate_type(record, value, field_path)
            }

            LexiconType::Procedure { .. } => {
                // Procedures are validated differently (input/output)
                Ok(())
            }
        }
    }

    /// Validate a reference type
    fn validate_ref(
        &self,
        ref_path: &str,
        value: &Value,
        field_path: &str,
    ) -> ValidationResult<()> {
        // Parse reference: "lex:namespace#def" or "#def"
        let def_name = if ref_path.starts_with("lex:") {
            // Cross-namespace reference - not supported yet
            return Err(ValidationError::ReferenceNotFound(ref_path.to_string()));
        } else if let Some(hash_pos) = ref_path.rfind('#') {
            &ref_path[hash_pos + 1..]
        } else {
            ref_path
        };

        // Get the referenced definition
        let def = self
            .get_def(def_name)
            .ok_or_else(|| ValidationError::ReferenceNotFound(ref_path.to_string()))?;

        self.validate_type(&def.def_type, value, field_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_string_format_did() {
        let format = StringFormat::Did;
        assert!(format.validate("did:plc:test123"));
        assert!(!format.validate("not-a-did"));
    }

    #[test]
    fn test_string_format_handle() {
        let format = StringFormat::Handle;
        assert!(format.validate("test.bsky.social"));
        assert!(!format.validate("invalid..handle"));
    }

    #[test]
    fn test_string_length_validation() {
        let schema_json = json!({
            "lexicon": 1,
            "id": "test.schema",
            "defs": {
                "main": {
                    "type": "object",
                    "required": ["text"],
                    "properties": {
                        "text": {
                            "type": "string",
                            "minLength": 1,
                            "maxLength": 10
                        }
                    }
                }
            }
        });

        let schema: LexiconSchema = serde_json::from_value(schema_json).unwrap();

        // Valid
        let valid = json!({"text": "hello"});
        assert!(schema.validate(&valid).is_ok());

        // Too short
        let too_short = json!({"text": ""});
        assert!(schema.validate(&too_short).is_err());

        // Too long
        let too_long = json!({"text": "hello world this is too long"});
        assert!(schema.validate(&too_long).is_err());
    }

    #[test]
    fn test_required_fields() {
        let schema_json = json!({
            "lexicon": 1,
            "id": "test.schema",
            "defs": {
                "main": {
                    "type": "object",
                    "required": ["field1", "field2"],
                    "properties": {
                        "field1": {"type": "string"},
                        "field2": {"type": "string"}
                    }
                }
            }
        });

        let schema: LexiconSchema = serde_json::from_value(schema_json).unwrap();

        // Valid
        let valid = json!({"field1": "value1", "field2": "value2"});
        assert!(schema.validate(&valid).is_ok());

        // Missing field
        let missing = json!({"field1": "value1"});
        assert!(matches!(
            schema.validate(&missing),
            Err(ValidationError::MissingRequiredField(_))
        ));
    }

    #[test]
    fn test_integer_constraints() {
        let schema_json = json!({
            "lexicon": 1,
            "id": "test.schema",
            "defs": {
                "main": {
                    "type": "object",
                    "required": ["count"],
                    "properties": {
                        "count": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": 100
                        }
                    }
                }
            }
        });

        let schema: LexiconSchema = serde_json::from_value(schema_json).unwrap();

        // Valid
        let valid = json!({"count": 50});
        assert!(schema.validate(&valid).is_ok());

        // Too small
        let too_small = json!({"count": 0});
        assert!(matches!(
            schema.validate(&too_small),
            Err(ValidationError::ValueTooSmall { .. })
        ));

        // Too large
        let too_large = json!({"count": 101});
        assert!(matches!(
            schema.validate(&too_large),
            Err(ValidationError::ValueTooLarge { .. })
        ));
    }

    #[test]
    fn test_enum_validation() {
        let schema_json = json!({
            "lexicon": 1,
            "id": "test.schema",
            "defs": {
                "main": {
                    "type": "object",
                    "required": ["status"],
                    "properties": {
                        "status": {
                            "type": "string",
                            "knownValues": ["active", "inactive", "pending"]
                        }
                    }
                }
            }
        });

        let schema: LexiconSchema = serde_json::from_value(schema_json).unwrap();

        // Valid
        let valid = json!({"status": "active"});
        assert!(schema.validate(&valid).is_ok());

        // Invalid value
        let invalid = json!({"status": "unknown"});
        assert!(matches!(
            schema.validate(&invalid),
            Err(ValidationError::InvalidEnumValue { .. })
        ));
    }

    #[test]
    fn test_array_validation() {
        let schema_json = json!({
            "lexicon": 1,
            "id": "test.schema",
            "defs": {
                "main": {
                    "type": "object",
                    "required": ["tags"],
                    "properties": {
                        "tags": {
                            "type": "array",
                            "minLength": 1,
                            "maxLength": 5,
                            "items": {
                                "type": "string",
                                "maxLength": 20
                            }
                        }
                    }
                }
            }
        });

        let schema: LexiconSchema = serde_json::from_value(schema_json).unwrap();

        // Valid
        let valid = json!({"tags": ["tag1", "tag2", "tag3"]});
        assert!(schema.validate(&valid).is_ok());

        // Too few items
        let too_few = json!({"tags": []});
        assert!(matches!(
            schema.validate(&too_few),
            Err(ValidationError::ArrayTooShort { .. })
        ));

        // Too many items
        let too_many = json!({"tags": ["t1", "t2", "t3", "t4", "t5", "t6"]});
        assert!(matches!(
            schema.validate(&too_many),
            Err(ValidationError::ArrayTooLong { .. })
        ));
    }

    #[test]
    fn test_reference_validation() {
        let schema_json = json!({
            "lexicon": 1,
            "id": "test.schema",
            "defs": {
                "main": {
                    "type": "object",
                    "required": ["user"],
                    "properties": {
                        "user": {
                            "type": "ref",
                            "ref": "#userInfo"
                        }
                    }
                },
                "userInfo": {
                    "type": "object",
                    "required": ["name"],
                    "properties": {
                        "name": {"type": "string"}
                    }
                }
            }
        });

        let schema: LexiconSchema = serde_json::from_value(schema_json).unwrap();

        // Valid
        let valid = json!({"user": {"name": "Alice"}});
        assert!(schema.validate(&valid).is_ok());

        // Missing required field in ref
        let invalid = json!({"user": {}});
        assert!(matches!(
            schema.validate(&invalid),
            Err(ValidationError::MissingRequiredField(_))
        ));
    }

    #[test]
    fn test_grapheme_validation() {
        let schema_json = json!({
            "lexicon": 1,
            "id": "test.schema",
            "defs": {
                "main": {
                    "type": "object",
                    "required": ["text"],
                    "properties": {
                        "text": {
                            "type": "string",
                            "maxGraphemes": 5
                        }
                    }
                }
            }
        });

        let schema: LexiconSchema = serde_json::from_value(schema_json).unwrap();

        // Valid - 5 graphemes
        let valid = json!({"text": "Hello"});
        assert!(schema.validate(&valid).is_ok());

        // Valid - emoji counts as 1 grapheme
        let emoji = json!({"text": "👋🌍✨🎉🚀"});
        assert!(schema.validate(&emoji).is_ok());

        // Invalid - too many graphemes
        let too_many = json!({"text": "Hello World"});
        assert!(matches!(
            schema.validate(&too_many),
            Err(ValidationError::TooManyGraphemes { .. })
        ));
    }
}
