//! Theme manifest schema + parsing (§11.2).
//!
//! A theme directory carries a `manifest.json` declaring its identity, the
//! parent it `extends`, and the CSS files it provides. This module owns the
//! schema (serde shape, the required + optional fields of §11.2.1–11.2.3)
//! and the parse step (steps 1–2 of the §11.10 validation contract).

use crate::error::{PdsError, PdsResult};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The substrate version this build implements (§11.1 principle 5). A theme
/// targeting a newer substrate version is refused (validation step 4).
pub const SUBSTRATE_VERSION: &str = "1.0";

/// Theme manifest (§11.2). Field names are camelCase on the wire.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeManifest {
    pub schema_version: String,
    pub theme_id: String,
    pub theme_name: String,
    #[serde(default)]
    pub theme_version: Option<String>,
    pub substrate_version: String,
    /// Parent theme id. `None` for the inheritance root (`aurora-default`).
    #[serde(default)]
    pub extends: Option<String>,
    pub files: ThemeFiles,
    #[serde(default)]
    pub theme_author: Option<String>,
    #[serde(default)]
    pub theme_description: Option<String>,
    /// Extension points this theme declares (§11.7). The extension-point
    /// system itself is deferred to 0.9.1; the field is parsed now so 0.9.0
    /// themes can declare forward-compatibly.
    #[serde(default)]
    pub provided_extension_points: Vec<String>,
    /// Open-ended convenience object (preview color, tags, …).
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// The `files` object — `tokens` is required; the rest optional (§11.2.2).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeFiles {
    pub tokens: String,
    #[serde(default)]
    pub effects: Option<String>,
    #[serde(default)]
    pub extensions: Option<String>,
    #[serde(default)]
    pub preview: Option<String>,
}

impl ThemeManifest {
    /// Parse a `manifest.json` (validation steps 1–2: file readable + valid
    /// JSON matching the schema shape). Missing required fields or wrong
    /// types surface as `PdsError::Validation` with the path in the message.
    pub fn parse_file(path: &Path) -> PdsResult<Self> {
        let raw = std::fs::read_to_string(path).map_err(|e| {
            PdsError::Validation(format!(
                "theme.invalid.manifest: unreadable at {}: {e}",
                path.display()
            ))
        })?;
        serde_json::from_str(&raw).map_err(|e| {
            PdsError::Validation(format!(
                "theme.invalid.manifest: invalid JSON at {}: {e}",
                path.display()
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_minimal_manifest() {
        let dir = std::env::temp_dir().join("aurora-theme-manifest-test-min");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("manifest.json");
        std::fs::write(
            &path,
            r#"{
              "schemaVersion": "1.0",
              "themeId": "demo",
              "themeName": "Demo",
              "substrateVersion": "1.0",
              "extends": "aurora-default",
              "files": { "tokens": "tokens.css" }
            }"#,
        )
        .unwrap();
        let m = ThemeManifest::parse_file(&path).expect("parses");
        assert_eq!(m.theme_id, "demo");
        assert_eq!(m.extends.as_deref(), Some("aurora-default"));
        assert_eq!(m.files.tokens, "tokens.css");
        assert!(m.theme_version.is_none());
        assert!(m.provided_extension_points.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn root_manifest_has_no_extends() {
        let dir = std::env::temp_dir().join("aurora-theme-manifest-test-root");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("manifest.json");
        std::fs::write(
            &path,
            r#"{
              "schemaVersion": "1.0",
              "themeId": "aurora-default",
              "themeName": "Aurora Default",
              "substrateVersion": "1.0",
              "files": { "tokens": "tokens.css" }
            }"#,
        )
        .unwrap();
        let m = ThemeManifest::parse_file(&path).expect("parses");
        assert!(m.extends.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_non_json() {
        let dir = std::env::temp_dir().join("aurora-theme-manifest-test-bad");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("manifest.json");
        std::fs::write(&path, "not json {").unwrap();
        assert!(ThemeManifest::parse_file(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
