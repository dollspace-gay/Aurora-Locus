//! Rust code generation from Lexicon schemas

use crate::lexicon::*;
use heck::{ToSnakeCase, ToPascalCase};
use std::collections::HashMap;

pub struct CodeGenerator {
    /// Map of NSID to lexicon doc
    lexicons: HashMap<String, LexiconDoc>,
}

impl CodeGenerator {
    pub fn new() -> Self {
        Self {
            lexicons: HashMap::new(),
        }
    }

    pub fn add_lexicon(&mut self, doc: LexiconDoc) {
        self.lexicons.insert(doc.id.clone(), doc);
    }

    /// Escape Rust keywords for field names
    fn escape_keyword(name: &str) -> String {
        match name {
            "type" | "ref" | "mod" | "use" | "fn" | "let" | "mut" |
            "const" | "static" | "struct" | "enum" | "trait" | "impl" |
            "pub" | "priv" | "crate" | "self" | "super" | "async" | "await" |
            "match" | "if" | "else" | "for" | "while" | "loop" | "break" |
            "continue" | "return" | "move" | "box" | "where" | "unsafe" |
            "extern" | "as" | "in" | "dyn" | "true" | "false" => {
                format!("r#{}", name)
            }
            _ => name.to_string()
        }
    }

    /// Rename type names that conflict with std types
    fn rename_conflicting_type(name: &str) -> String {
        match name {
            "Option" => "OptionSetting".to_string(),
            "Result" => "ResultType".to_string(),
            "Box" => "BoxType".to_string(),
            "Vec" => "VecType".to_string(),
            "String" => "StringType".to_string(),
            _ => name.to_string()
        }
    }

    /// Generate field code with proper keyword escaping and serde annotations
    fn generate_field(&self, code: &mut String, json_name: &str, rust_type: &str, is_required: bool, description: Option<&str>) {
        // Add description if present
        if let Some(desc) = description {
            code.push_str("    /// ");
            code.push_str(desc);
            code.push_str("\n");
        }

        // Skip serialization if None for optional fields
        if !is_required {
            code.push_str("    #[serde(skip_serializing_if = \"Option::is_none\")]\n");
        }

        let field_name = json_name.to_snake_case();
        let escaped_field_name = Self::escape_keyword(&field_name);

        // Add #[serde(rename)] if needed (either escaped or camelCase)
        if escaped_field_name != field_name || field_name != json_name {
            code.push_str("    #[serde(rename = \"");
            code.push_str(json_name);
            code.push_str("\")]\n");
        }

        code.push_str("    pub ");
        code.push_str(&escaped_field_name);
        code.push_str(": ");

        if !is_required {
            code.push_str("Option<");
            code.push_str(rust_type);
            code.push_str(">");
        } else {
            code.push_str(rust_type);
        }

        code.push_str(",\n");
    }

    /// Generate Rust code for a single lexicon
    pub fn generate(&self, doc: &LexiconDoc) -> Result<String, String> {
        // Check if there's a main definition
        if let Some(main_def) = doc.main_def() {
            match main_def {
                LexiconDef::Query { .. } => self.generate_query(doc, main_def),
                LexiconDef::Procedure { .. } => self.generate_procedure(doc, main_def),
                LexiconDef::Record { .. } => self.generate_record(doc, main_def),
                LexiconDef::Subscription { .. } => self.generate_subscription(doc, main_def),
                // Object, String, Token, etc. in main are just type definitions
                _ => self.generate_defs_only(doc),
            }
        } else {
            // No main definition - this is a definitions-only file (like defs.json)
            self.generate_defs_only(doc)
        }
    }

    /// Generate code for a definitions-only lexicon (includes main definition if it's an object)
    fn generate_defs_only(&self, doc: &LexiconDoc) -> Result<String, String> {
        let mut code = String::new();

        // File header
        code.push_str("//! Generated type definitions for ");
        code.push_str(&doc.id);
        code.push_str("\n\n");

        // Check if we need serde (objects or tokens that use Serialize/Deserialize)
        let needs_serde = doc.defs.values().any(|def| matches!(def, LexiconDef::Object { .. } | LexiconDef::Token { .. }));

        // Imports
        if needs_serde {
            code.push_str("use serde::{Deserialize, Serialize};\n\n");
        }

        // Generate all definitions INCLUDING "main" now
        for (def_name, def_value) in &doc.defs {
            // For "main" definitions, use "Main" as the struct name
            let type_name = if def_name == "main" {
                "Main"
            } else {
                def_name
            };

            match def_value {
                LexiconDef::Object { description, required, properties } => {
                    code.push_str(&self.generate_object_type_from_fields(
                        type_name,
                        description.as_deref(),
                        required,
                        properties
                    )?);
                    code.push_str("\n\n");
                }
                LexiconDef::String { description, .. } => {
                    // Generate a type alias for string types
                    if let Some(desc) = description {
                        code.push_str("/// ");
                        code.push_str(desc);
                        code.push_str("\n");
                    }
                    code.push_str("pub type ");
                    code.push_str(&type_name.to_pascal_case());
                    code.push_str(" = String;\n\n");
                }
                LexiconDef::Array { .. } => {
                    // Skip array types for now - they're typically used inline
                    // We could generate type aliases if needed
                }
                LexiconDef::Token { description } => {
                    // Generate a unit struct for tokens
                    if let Some(desc) = description {
                        code.push_str("/// ");
                        code.push_str(desc);
                        code.push_str("\n");
                    }
                    code.push_str("#[derive(Debug, Clone, Serialize, Deserialize)]\n");
                    code.push_str("pub struct ");
                    code.push_str(&Self::rename_conflicting_type(&type_name.to_pascal_case()));
                    code.push_str(";\n\n");
                }
                _ => {
                    // Skip unsupported types for now
                }
            }
        }

        Ok(code)
    }

    /// Generate code for a query endpoint
    fn generate_query(&self, doc: &LexiconDoc, def: &LexiconDef) -> Result<String, String> {
        let LexiconDef::Query {
            description,
            parameters,
            output,
            errors,
        } = def
        else {
            return Err("Expected query definition".to_string());
        };

        let mut code = String::new();

        // File header
        code.push_str("//! Generated code for ");
        code.push_str(&doc.id);
        code.push_str("\n//!\n");
        if let Some(desc) = description {
            code.push_str("//! ");
            code.push_str(desc);
            code.push_str("\n");
        }
        code.push_str("\n");

        // Check if we need serde (params or output with schema)
        let needs_serde = parameters.is_some() ||
            (output.is_some() && output.as_ref().unwrap().schema.is_some());

        // Imports
        code.push_str("use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};\n");
        if needs_serde {
            code.push_str("use serde::{Deserialize, Serialize};\n");
        }
        code.push_str("\n");

        // Generate QueryParams if parameters exist
        if let Some(params) = parameters {
            code.push_str("/// Query parameters\n");
            code.push_str("#[derive(Debug, Clone, Serialize, Deserialize)]\n");
            code.push_str("pub struct QueryParams {\n");

            for (name, prop) in &params.properties {
                let is_required = params.required.contains(name);
                let rust_type = prop.rust_type();
                let description = self.get_property_description(prop);

                self.generate_field(&mut code, name, &rust_type, is_required, description);
            }

            code.push_str("}\n\n");
        }

        // Generate Output if it exists
        if let Some(out) = output {
            if let Some(schema) = &out.schema {
                code.push_str("/// Response output\n");
                code.push_str("#[derive(Debug, Clone, Serialize, Deserialize)]\n");
                code.push_str("pub struct Output {\n");

                for (name, prop) in &schema.properties {
                    let is_required = schema.required.contains(name);
                    let rust_type = prop.rust_type();
                    let description = self.get_property_description(prop);

                    self.generate_field(&mut code, name, &rust_type, is_required, description);
                }

                code.push_str("}\n\n");
            }
        }

        // Generate error types
        for error in errors {
            let error_name = error.name.to_pascal_case();
            code.push_str("/// Error: ");
            code.push_str(&error.name);
            code.push_str("\n");

            if let Some(desc) = &error.description {
                code.push_str("/// ");
                code.push_str(desc);
                code.push_str("\n");
            }

            code.push_str("#[derive(Debug, Clone, thiserror::Error)]\n");
            code.push_str("#[error(\"");
            code.push_str(&error.name);
            code.push_str("\")]\n");
            code.push_str("pub struct ");
            code.push_str(&error_name);
            code.push_str("Error;\n\n");
        }

        // Generate the function
        let method_name = doc.method_name().to_snake_case();

        code.push_str("/// ");
        if let Some(desc) = description {
            code.push_str(desc);
        } else {
            code.push_str(&doc.id);
        }
        code.push_str("\n");

        code.push_str("pub async fn ");
        code.push_str(&method_name);
        code.push_str("(\n");
        code.push_str("    client: &impl XrpcClient,\n");

        if parameters.is_some() {
            code.push_str("    params: QueryParams,\n");
        }

        // Use () if there's no output
        let has_output = output.as_ref().and_then(|o| o.schema.as_ref()).is_some();
        if has_output {
            code.push_str(") -> Result<XrpcResponse<Output>, XrpcError> {\n");
        } else {
            code.push_str(") -> Result<XrpcResponse<()>, XrpcError> {\n");
        }

        // Build the request
        // Only make it mutable if we have parameters to add
        if parameters.is_some() {
            code.push_str("    let mut req = XrpcRequest::query(\"");
        } else {
            code.push_str("    let req = XrpcRequest::query(\"");
        }
        code.push_str(&doc.id);
        code.push_str("\");\n\n");

        if parameters.is_some() {
            code.push_str("    // Add query parameters\n");
            code.push_str("    let params_json = serde_json::to_value(&params)\n");
            code.push_str("        .map_err(XrpcError::Serialization)?;\n\n");
            code.push_str("    if let Some(obj) = params_json.as_object() {\n");
            code.push_str("        for (key, value) in obj {\n");
            code.push_str("            if let Some(s) = value.as_str() {\n");
            code.push_str("                req.params.insert(key.clone(), s.to_string());\n");
            code.push_str("            } else {\n");
            code.push_str("                req.params.insert(key.clone(), value.to_string());\n");
            code.push_str("            }\n");
            code.push_str("        }\n");
            code.push_str("    }\n\n");
        }

        code.push_str("    client.request(req).await\n");
        code.push_str("}\n");

        Ok(code)
    }

    /// Generate code for a procedure endpoint
    fn generate_procedure(&self, doc: &LexiconDoc, def: &LexiconDef) -> Result<String, String> {
        let LexiconDef::Procedure {
            description,
            parameters: _,
            input,
            output,
            errors: _,
        } = def
        else {
            return Err("Expected procedure definition".to_string());
        };

        let mut code = String::new();

        // File header
        code.push_str("//! Generated code for ");
        code.push_str(&doc.id);
        code.push_str("\n//!\n");
        if let Some(desc) = description {
            code.push_str("//! ");
            code.push_str(desc);
            code.push_str("\n");
        }
        code.push_str("\n");

        // Check if we need serde (input or output with schema)
        let needs_serde = (input.is_some() && input.as_ref().unwrap().schema.is_some()) ||
            (output.is_some() && output.as_ref().unwrap().schema.is_some());

        // Imports
        code.push_str("use crate::xrpc::{XrpcClient, XrpcRequest, XrpcResponse, XrpcError};\n");
        if needs_serde {
            code.push_str("use serde::{Deserialize, Serialize};\n");
        }
        code.push_str("\n");

        // Generate Input if it exists
        if let Some(inp) = input {
            if let Some(schema) = &inp.schema {
                code.push_str("/// Request input\n");
                code.push_str("#[derive(Debug, Clone, Serialize, Deserialize)]\n");
                code.push_str("pub struct Input {\n");

                for (name, prop) in &schema.properties {
                    let is_required = schema.required.contains(name);
                    let rust_type = prop.rust_type();
                    let description = self.get_property_description(prop);

                    self.generate_field(&mut code, name, &rust_type, is_required, description);
                }

                code.push_str("}\n\n");
            } else {
                // Binary input (e.g., video/mp4)
                code.push_str("/// Request input (binary data)\n");
                code.push_str("pub type Input = Vec<u8>;\n\n");
            }
        }

        // Generate Output if it exists
        if let Some(out) = output {
            if let Some(schema) = &out.schema {
                code.push_str("/// Response output\n");
                code.push_str("#[derive(Debug, Clone, Serialize, Deserialize)]\n");
                code.push_str("pub struct Output {\n");

                for (name, prop) in &schema.properties {
                    let is_required = schema.required.contains(name);
                    let rust_type = prop.rust_type();
                    let description = self.get_property_description(prop);

                    self.generate_field(&mut code, name, &rust_type, is_required, description);
                }

                code.push_str("}\n\n");
            }
        }

        // Generate the function
        let method_name = doc.method_name().to_snake_case();

        code.push_str("/// ");
        if let Some(desc) = description {
            code.push_str(desc);
        } else {
            code.push_str(&doc.id);
        }
        code.push_str("\n");

        code.push_str("pub async fn ");
        code.push_str(&method_name);
        code.push_str("(\n");
        code.push_str("    client: &impl XrpcClient,\n");

        if input.is_some() {
            code.push_str("    input: Input,\n");
        }

        // Use () if there's no output
        let has_output = output.as_ref().and_then(|o| o.schema.as_ref()).is_some();
        if has_output {
            code.push_str(") -> Result<XrpcResponse<Output>, XrpcError> {\n");
        } else {
            code.push_str(") -> Result<XrpcResponse<()>, XrpcError> {\n");
        }

        // Build the request
        code.push_str("    let req = XrpcRequest::procedure(\"");
        code.push_str(&doc.id);
        code.push_str("\")");

        if input.is_some() {
            code.push_str(".data(&input)?");
        }

        code.push_str(";\n\n");

        code.push_str("    client.request(req).await\n");
        code.push_str("}\n");

        Ok(code)
    }

    /// Generate code for a subscription endpoint
    fn generate_subscription(&self, doc: &LexiconDoc, def: &LexiconDef) -> Result<String, String> {
        let LexiconDef::Subscription {
            description,
            parameters,
            message: _,
            errors: _,
        } = def
        else {
            return Err("Expected subscription definition".to_string());
        };

        let mut code = String::new();

        // File header
        code.push_str("//! Generated code for ");
        code.push_str(&doc.id);
        code.push_str("\n//!\n");
        if let Some(desc) = description {
            code.push_str("//! ");
            code.push_str(desc);
            code.push_str("\n");
        }
        code.push_str("\n");

        // Imports
        code.push_str("use crate::xrpc::{XrpcRequest, XrpcError};\n");
        code.push_str("use crate::xrpc_subscription::{SubscriptionClient, SubscriptionEvent, SubscriptionResult};\n");
        code.push_str("use serde::{Deserialize, Serialize};\n");
        code.push_str("use futures::stream::Stream;\n");
        code.push_str("use std::pin::Pin;\n\n");

        // Generate QueryParams if parameters exist
        if let Some(params) = parameters {
            code.push_str("/// Query parameters\n");
            code.push_str("#[derive(Debug, Clone, Serialize, Deserialize)]\n");
            code.push_str("pub struct QueryParams {\n");

            for (name, prop) in &params.properties {
                let is_required = params.required.contains(name);
                let rust_type = prop.rust_type();
                let description = self.get_property_description(prop);

                self.generate_field(&mut code, name, &rust_type, is_required, description);
            }

            code.push_str("}\n\n");
        }

        // Generate the subscription function
        let method_name = doc.method_name().to_snake_case();

        if let Some(desc) = description {
            code.push_str("/// ");
            code.push_str(desc);
            code.push_str("\n");
        }

        code.push_str("pub async fn ");
        code.push_str(&method_name);
        code.push_str("(\n");
        code.push_str("    client: &SubscriptionClient,\n");

        if parameters.is_some() {
            code.push_str("    params: QueryParams,\n");
        }

        code.push_str(") -> SubscriptionResult<Pin<Box<dyn Stream<Item = SubscriptionResult<SubscriptionEvent>> + Send>>> {\n");

        // Build the request
        // Only make it mutable if we have parameters to add
        if parameters.is_some() {
            code.push_str("    let mut req = XrpcRequest::query(\"");
        } else {
            code.push_str("    let req = XrpcRequest::query(\"");
        }
        code.push_str(&doc.id);
        code.push_str("\");\n\n");

        // Add parameters if they exist
        if parameters.is_some() {
            code.push_str("    // Add query parameters\n");
            code.push_str("    let params_json = serde_json::to_value(&params)\n");
            code.push_str("        .map_err(XrpcError::Serialization)?;\n\n");

            code.push_str("    if let Some(obj) = params_json.as_object() {\n");
            code.push_str("        for (key, value) in obj {\n");
            code.push_str("            if let Some(s) = value.as_str() {\n");
            code.push_str("                req.params.insert(key.clone(), s.to_string());\n");
            code.push_str("            } else {\n");
            code.push_str("                req.params.insert(key.clone(), value.to_string());\n");
            code.push_str("            }\n");
            code.push_str("        }\n");
            code.push_str("    }\n\n");
        }

        code.push_str("    client.subscribe(req).await\n");
        code.push_str("}\n");

        Ok(code)
    }

    /// Generate code for a record type
    fn generate_record(&self, doc: &LexiconDoc, def: &LexiconDef) -> Result<String, String> {
        let LexiconDef::Record {
            description,
            key: _,
            record,
        } = def
        else {
            return Err("Expected record definition".to_string());
        };

        let mut code = String::new();

        // File header
        code.push_str("//! Generated code for ");
        code.push_str(&doc.id);
        code.push_str("\n//!\n");
        if let Some(desc) = description {
            code.push_str("//! ");
            code.push_str(desc);
            code.push_str("\n");
        }
        code.push_str("\n");

        // Imports
        code.push_str("use serde::{Deserialize, Serialize};\n\n");

        // Generate the main record struct
        let record_name = doc.method_name().to_pascal_case();

        if let Some(desc) = description {
            code.push_str("/// ");
            code.push_str(desc);
            code.push_str("\n");
        }

        code.push_str("#[derive(Debug, Clone, Serialize, Deserialize)]\n");
        code.push_str("pub struct ");
        code.push_str(&Self::rename_conflicting_type(&record_name));
        code.push_str(" {\n");

        // Generate fields
        for (name, prop) in &record.properties {
            if let Some(desc) = self.get_property_description(prop) {
                code.push_str("    /// ");
                code.push_str(desc);
                code.push_str("\n");
            }

            let is_required = record.required.contains(name);
            let rust_type = prop.rust_type();

            // Handle serde rename for camelCase fields
            if name.contains(char::is_uppercase) || name.contains('_') {
                code.push_str("    #[serde(rename = \"");
                code.push_str(name);
                code.push_str("\")]\n");
            }

            // Skip serialization if None for optional fields
            if !is_required {
                code.push_str("    #[serde(skip_serializing_if = \"Option::is_none\")]\n");
            }

            code.push_str("    pub ");
            code.push_str(&name.to_snake_case());
            code.push_str(": ");

            if !is_required {
                code.push_str("Option<");
                code.push_str(&rust_type);
                code.push_str(">");
            } else {
                code.push_str(&rust_type);
            }

            code.push_str(",\n");
        }

        code.push_str("}\n\n");

        // Generate any additional object types defined in defs
        for (def_name, def_value) in &doc.defs {
            if def_name == "main" {
                continue;
            }

            if let LexiconDef::Object { description, required, properties } = def_value {
                code.push_str(&self.generate_object_type_from_fields(
                    def_name,
                    description.as_deref(),
                    required,
                    properties
                )?);
                code.push_str("\n");
            }
        }

        Ok(code)
    }

    /// Generate a standalone object type
    fn generate_object_type(&self, name: &str, obj: &LexiconObject) -> Result<String, String> {
        self.generate_object_type_from_fields(
            name,
            None,
            &obj.required,
            &obj.properties
        )
    }

    /// Generate a standalone object type from individual fields
    fn generate_object_type_from_fields(
        &self,
        name: &str,
        description: Option<&str>,
        required: &[String],
        properties: &HashMap<String, LexiconProperty>
    ) -> Result<String, String> {
        let mut code = String::new();

        let type_name = name.to_pascal_case();

        if let Some(desc) = description {
            code.push_str("/// ");
            code.push_str(desc);
            code.push_str("\n");
        }

        code.push_str("#[derive(Debug, Clone, Serialize, Deserialize)]\n");
        code.push_str("pub struct ");
        code.push_str(&Self::rename_conflicting_type(&type_name));
        code.push_str(" {\n");

        for (prop_name, prop) in properties {
            if let Some(desc) = self.get_property_description(prop) {
                code.push_str("    /// ");
                code.push_str(desc);
                code.push_str("\n");
            }

            let is_required = required.contains(prop_name);
            let rust_type = prop.rust_type();

            self.generate_field(&mut code, prop_name, &rust_type, is_required, description);
        }

        code.push_str("}\n");

        Ok(code)
    }

    /// Get description from a property
    fn get_property_description<'a>(&self, prop: &'a LexiconProperty) -> Option<&'a str> {
        match prop {
            LexiconProperty::Simple { description, .. } => description.as_deref(),
            LexiconProperty::Array { description, .. } => description.as_deref(),
            LexiconProperty::Ref { description, .. } => description.as_deref(),
            LexiconProperty::Union { description, .. } => description.as_deref(),
        }
    }
}
