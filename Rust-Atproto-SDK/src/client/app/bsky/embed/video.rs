//! Generated type definitions for app.bsky.embed.video

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Main {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "aspectRatio")]
    pub aspect_ratio: Option<serde_json::Value>,
    /// The mp4 video file. May be up to 100mb, formerly limited to 50mb.
    pub video: serde_json::Value,
    /// Alt text description of the video, for accessibility.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captions: Option<serde_json::Value>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Caption {
    pub file: serde_json::Value,
    pub lang: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct View {
    pub cid: String,
    pub playlist: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "aspectRatio")]
    pub aspect_ratio: Option<serde_json::Value>,
}


