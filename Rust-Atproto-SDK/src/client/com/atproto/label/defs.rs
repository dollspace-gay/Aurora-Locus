//! Generated type definitions for com.atproto.label.defs

use serde::{Deserialize, Serialize};

/// Strings which describe the label in the UI, localized into a specific language.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelValueDefinitionStrings {
    /// A short human-readable name for the label.
    /// Strings which describe the label in the UI, localized into a specific language.
    pub name: String,
    /// The code of the language these strings are written in.
    /// Strings which describe the label in the UI, localized into a specific language.
    pub lang: String,
    /// A longer description of what the label means and why it might be applied.
    /// Strings which describe the label in the UI, localized into a specific language.
    pub description: String,
}


pub type LabelValue = String;

/// Metadata tags on an atproto record, published by the author within the record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfLabels {
    /// Metadata tags on an atproto record, published by the author within the record.
    pub values: serde_json::Value,
}


/// Declares a label value and its expected interpretations and behaviors.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelValueDefinition {
    /// Does the user need to have adult content enabled in order to configure this label?
    /// Declares a label value and its expected interpretations and behaviors.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "adultOnly")]
    pub adult_only: Option<bool>,
    /// Declares a label value and its expected interpretations and behaviors.
    pub locales: serde_json::Value,
    /// The default setting for this label.
    /// Declares a label value and its expected interpretations and behaviors.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "defaultSetting")]
    pub default_setting: Option<String>,
    /// How should a client visually convey this label? 'inform' means neutral and informational; 'alert' means negative and warning; 'none' means show nothing.
    /// Declares a label value and its expected interpretations and behaviors.
    pub severity: String,
    /// What should this label hide in the UI, if applied? 'content' hides all of the target; 'media' hides the images/video/audio; 'none' hides nothing.
    /// Declares a label value and its expected interpretations and behaviors.
    pub blurs: String,
    /// The value of the label being defined. Must only include lowercase ascii and the '-' character ([a-z-]+).
    /// Declares a label value and its expected interpretations and behaviors.
    pub identifier: String,
}


/// Metadata tag on an atproto resource (eg, repo or record).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    /// The AT Protocol version of the label object.
    /// Metadata tag on an atproto resource (eg, repo or record).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ver: Option<i64>,
    /// Signature of dag-cbor encoded label.
    /// Metadata tag on an atproto resource (eg, repo or record).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sig: Option<serde_json::Value>,
    /// AT URI of the record, repository (account), or other resource that this label applies to.
    /// Metadata tag on an atproto resource (eg, repo or record).
    pub uri: String,
    /// If true, this is a negation label, overwriting a previous label.
    /// Metadata tag on an atproto resource (eg, repo or record).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub neg: Option<bool>,
    /// DID of the actor who created this label.
    /// Metadata tag on an atproto resource (eg, repo or record).
    pub src: crate::types::Did,
    /// Optionally, CID specifying the specific version of 'uri' resource this label applies to.
    /// Metadata tag on an atproto resource (eg, repo or record).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    /// Timestamp when this label was created.
    /// Metadata tag on an atproto resource (eg, repo or record).
    pub cts: String,
    /// The short string name of the value or type of this label.
    /// Metadata tag on an atproto resource (eg, repo or record).
    pub val: String,
    /// Timestamp at which this label expires (no longer applies).
    /// Metadata tag on an atproto resource (eg, repo or record).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp: Option<String>,
}


/// Metadata tag on an atproto record, published by the author within the record. Note that schemas should use #selfLabels, not #selfLabel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfLabel {
    /// The short string name of the value or type of this label.
    /// Metadata tag on an atproto record, published by the author within the record. Note that schemas should use #selfLabels, not #selfLabel.
    pub val: String,
}


