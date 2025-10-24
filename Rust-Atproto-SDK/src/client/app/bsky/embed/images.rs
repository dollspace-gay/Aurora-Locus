//! Generated type definitions for app.bsky.embed.images

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewImage {
    /// Fully-qualified URL where a thumbnail of the image can be fetched. For example, CDN location provided by the App View.
    pub thumb: String,
    /// Alt text description of the image, for accessibility.
    pub alt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "aspectRatio")]
    pub aspect_ratio: Option<serde_json::Value>,
    /// Fully-qualified URL where a large version of the image can be fetched. May or may not be the exact original blob. For example, CDN location provided by the App View.
    pub fullsize: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Image {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "aspectRatio")]
    pub aspect_ratio: Option<serde_json::Value>,
    pub image: serde_json::Value,
    /// Alt text description of the image, for accessibility.
    pub alt: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct View {
    pub images: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Main {
    pub images: serde_json::Value,
}


