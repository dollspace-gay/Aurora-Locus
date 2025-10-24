//! Generated type definitions for app.bsky.graph.defs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListView {
    pub uri: crate::syntax::AtUri,
    pub purpose: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "descriptionFacets")]
    pub description_facets: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "listItemCount")]
    pub list_item_count: Option<i64>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewer: Option<serde_json::Value>,
    #[serde(rename = "indexedAt")]
    pub indexed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub cid: String,
    pub creator: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<serde_json::Value>,
}


/// A list of actors used for curation purposes such as list feeds or interaction gating.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Curatelist;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListItemView {
    pub subject: serde_json::Value,
    pub uri: crate::syntax::AtUri,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListViewerState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocked: Option<crate::syntax::AtUri>,
}


/// indicates that a handle or DID could not be resolved
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotFoundActor {
    /// indicates that a handle or DID could not be resolved
    pub actor: String,
    /// indicates that a handle or DID could not be resolved
    #[serde(rename = "notFound")]
    pub not_found: bool,
}


pub type ListPurpose = String;

/// lists the bi-directional graph relationships between one actor (not indicated in the object), and the target actors (the DID included in the object)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    /// lists the bi-directional graph relationships between one actor (not indicated in the object), and the target actors (the DID included in the object)
    pub did: crate::types::Did,
    /// if the actor follows this DID, this is the AT-URI of the follow record
    /// lists the bi-directional graph relationships between one actor (not indicated in the object), and the target actors (the DID included in the object)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub following: Option<crate::syntax::AtUri>,
    /// if the actor is followed by this DID, contains the AT-URI of the follow record
    /// lists the bi-directional graph relationships between one actor (not indicated in the object), and the target actors (the DID included in the object)
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "followedBy")]
    pub followed_by: Option<crate::syntax::AtUri>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListViewBasic {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub viewer: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "indexedAt")]
    pub indexed_at: Option<String>,
    pub cid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    pub purpose: serde_json::Value,
    pub uri: crate::syntax::AtUri,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "listItemCount")]
    pub list_item_count: Option<i64>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StarterPackViewBasic {
    pub cid: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "joinedAllTimeCount")]
    pub joined_all_time_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "joinedWeekCount")]
    pub joined_week_count: Option<i64>,
    #[serde(rename = "indexedAt")]
    pub indexed_at: String,
    pub uri: crate::syntax::AtUri,
    pub record: serde_json::Value,
    pub creator: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "listItemCount")]
    pub list_item_count: Option<i64>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StarterPackView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feeds: Option<serde_json::Value>,
    pub cid: String,
    #[serde(rename = "indexedAt")]
    pub indexed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "listItemsSample")]
    pub list_items_sample: Option<serde_json::Value>,
    pub record: serde_json::Value,
    pub uri: crate::syntax::AtUri,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "joinedAllTimeCount")]
    pub joined_all_time_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub labels: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "joinedWeekCount")]
    pub joined_week_count: Option<i64>,
    pub creator: serde_json::Value,
}


/// A list of actors used for only for reference purposes such as within a starter pack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Referencelist;

/// A list of actors to apply an aggregate moderation action (mute/block) on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Modlist;

