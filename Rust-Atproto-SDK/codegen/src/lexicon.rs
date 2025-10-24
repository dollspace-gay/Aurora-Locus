//! Lexicon schema types for ATProto code generation
//!
//! This module defines the structure of ATProto Lexicon schemas
//! for parsing JSON lexicon files and generating Rust code.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level lexicon document
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LexiconDoc {
    /// Lexicon version (should be 1)
    pub lexicon: u32,

    /// NSID identifier (e.g., "com.atproto.identity.resolveHandle")
    pub id: String,

    /// Schema definitions
    pub defs: HashMap<String, LexiconDef>,
}

/// A definition within a lexicon
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LexiconDef {
    /// XRPC query endpoint (GET)
    Query {
        #[serde(default)]
        description: Option<String>,

        #[serde(default)]
        parameters: Option<LexiconParams>,

        #[serde(default)]
        output: Option<LexiconOutput>,

        #[serde(default)]
        errors: Vec<LexiconError>,
    },

    /// XRPC procedure endpoint (POST)
    Procedure {
        #[serde(default)]
        description: Option<String>,

        #[serde(default)]
        parameters: Option<LexiconParams>,

        #[serde(default)]
        input: Option<LexiconInput>,

        #[serde(default)]
        output: Option<LexiconOutput>,

        #[serde(default)]
        errors: Vec<LexiconError>,
    },

    /// Record type
    Record {
        #[serde(default)]
        description: Option<String>,

        #[serde(default)]
        key: Option<String>,

        record: LexiconObject,
    },

    /// Object type
    Object {
        #[serde(default)]
        description: Option<String>,

        #[serde(default)]
        required: Vec<String>,

        #[serde(default)]
        properties: HashMap<String, LexiconProperty>,
    },

    /// Array type
    Array {
        #[serde(default)]
        description: Option<String>,

        items: Box<LexiconProperty>,
    },

    /// Token type
    Token {
        #[serde(default)]
        description: Option<String>,
    },

    /// String type (for simple string definitions)
    String {
        #[serde(default)]
        description: Option<String>,

        #[serde(rename = "maxLength")]
        #[serde(default)]
        max_length: Option<usize>,

        #[serde(rename = "maxGraphemes")]
        #[serde(default)]
        max_graphemes: Option<usize>,

        #[serde(default)]
        format: Option<String>,
    },

    /// Blob type
    Blob {
        #[serde(default)]
        description: Option<String>,

        #[serde(default)]
        accept: Vec<String>,

        #[serde(rename = "maxSize")]
        #[serde(default)]
        max_size: Option<usize>,
    },

    /// Subscription type (WebSocket/event streams)
    Subscription {
        #[serde(default)]
        description: Option<String>,

        #[serde(default)]
        parameters: Option<LexiconParams>,

        #[serde(default)]
        message: Option<serde_json::Value>,

        #[serde(default)]
        errors: Vec<LexiconError>,
    },
}

/// Query/procedure parameters
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LexiconParams {
    #[serde(rename = "type")]
    pub type_name: String, // Should be "params"

    #[serde(default)]
    pub required: Vec<String>,

    pub properties: HashMap<String, LexiconProperty>,
}

/// Input for procedures
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LexiconInput {
    pub encoding: String,

    #[serde(default)]
    pub schema: Option<LexiconObject>,
}

/// Output for queries/procedures
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LexiconOutput {
    pub encoding: String,

    #[serde(default)]
    pub schema: Option<LexiconObject>,
}

/// Object schema
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LexiconObject {
    #[serde(rename = "type")]
    pub type_name: String, // Should be "object"

    #[serde(default)]
    pub required: Vec<String>,

    #[serde(default)]
    pub properties: HashMap<String, LexiconProperty>,
}

/// Property definition
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum LexiconProperty {
    /// Simple type
    Simple {
        #[serde(rename = "type")]
        type_name: String,

        #[serde(default)]
        description: Option<String>,

        #[serde(default)]
        format: Option<String>,

        #[serde(default)]
        default: Option<serde_json::Value>,

        #[serde(default)]
        minimum: Option<i64>,

        #[serde(default)]
        maximum: Option<i64>,
    },

    /// Array type
    Array {
        #[serde(rename = "type")]
        type_name: String, // Should be "array"

        #[serde(default)]
        description: Option<String>,

        items: Box<LexiconProperty>,

        #[serde(rename = "minLength")]
        #[serde(default)]
        min_length: Option<usize>,

        #[serde(rename = "maxLength")]
        #[serde(default)]
        max_length: Option<usize>,
    },

    /// Reference to another type
    Ref {
        #[serde(rename = "type")]
        type_name: String, // Should be "ref"

        #[serde(rename = "ref")]
        ref_path: String,

        #[serde(default)]
        description: Option<String>,
    },

    /// Union of types
    Union {
        #[serde(rename = "type")]
        type_name: String, // Should be "union"

        refs: Vec<String>,

        #[serde(default)]
        description: Option<String>,
    },
}

/// Error definition
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LexiconError {
    pub name: String,

    #[serde(default)]
    pub description: Option<String>,
}

impl LexiconDoc {
    /// Get the main definition
    pub fn main_def(&self) -> Option<&LexiconDef> {
        self.defs.get("main")
    }

    /// Get the namespace parts from the NSID
    /// e.g., "com.atproto.identity.resolveHandle" -> ["com", "atproto", "identity"]
    pub fn namespace_parts(&self) -> Vec<&str> {
        let parts: Vec<&str> = self.id.split('.').collect();
        if parts.len() > 1 {
            parts[..parts.len() - 1].to_vec()
        } else {
            vec![]
        }
    }

    /// Get the method name from the NSID
    /// e.g., "com.atproto.identity.resolveHandle" -> "resolveHandle"
    pub fn method_name(&self) -> &str {
        self.id.split('.').last().unwrap_or(&self.id)
    }
}

impl LexiconProperty {
    /// Get the Rust type name for this property
    pub fn rust_type(&self) -> String {
        match self {
            LexiconProperty::Simple { type_name, format, .. } => {
                match type_name.as_str() {
                    "string" => {
                        if let Some(fmt) = format {
                            match fmt.as_str() {
                                "did" => "crate::types::Did".to_string(),
                                "handle" => "String".to_string(),
                                "at-uri" => "crate::syntax::AtUri".to_string(),
                                "datetime" => "String".to_string(), // TODO: Use chrono
                                _ => "String".to_string(),
                            }
                        } else {
                            "String".to_string()
                        }
                    }
                    "integer" => "i64".to_string(),
                    "boolean" => "bool".to_string(),
                    _ => "serde_json::Value".to_string(),
                }
            }
            LexiconProperty::Array { items, .. } => {
                format!("Vec<{}>", items.rust_type())
            }
            LexiconProperty::Ref { ref_path, .. } => {
                // Convert ref path to Rust module path
                // e.g., "app.bsky.feed.defs#postView" -> "app::bsky::feed::defs::PostView"
                // e.g., "#replyRef" -> "ReplyRef" (local reference)
                if ref_path.starts_with('#') {
                    // Local reference within the same file
                    heck::AsPascalCase(&ref_path[1..]).to_string()
                } else if ref_path.contains('#') {
                    let parts: Vec<&str> = ref_path.split('#').collect();
                    let module = parts[0].replace('.', "::");
                    let type_name = heck::AsPascalCase(parts[1]).to_string();
                    format!("{}::{}", module, type_name)
                } else {
                    // Full module path reference
                    ref_path.replace('.', "::")
                }
            }
            LexiconProperty::Union { .. } => {
                // For now, use serde_json::Value for unions
                // In the future, we could generate proper enum types
                "serde_json::Value".to_string()
            }
        }
    }

    /// Check if this property is optional (not required)
    pub fn is_optional(&self) -> bool {
        match self {
            LexiconProperty::Simple { default, .. } => default.is_some(),
            _ => false,
        }
    }
}
