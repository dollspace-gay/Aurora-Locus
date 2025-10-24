//! Generated type definitions for chat.bsky.convo.defs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogBeginConvo {
    #[serde(rename = "convoId")]
    pub convo_id: String,
    pub rev: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogCreateMessage {
    pub message: serde_json::Value,
    #[serde(rename = "convoId")]
    pub convo_id: String,
    pub rev: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogAddReaction {
    pub rev: String,
    #[serde(rename = "convoId")]
    pub convo_id: String,
    pub reaction: serde_json::Value,
    pub message: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogRemoveReaction {
    pub reaction: serde_json::Value,
    #[serde(rename = "convoId")]
    pub convo_id: String,
    pub message: serde_json::Value,
    pub rev: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvoView {
    pub rev: String,
    pub members: serde_json::Value,
    #[serde(rename = "unreadCount")]
    pub unread_count: i64,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "lastMessage")]
    pub last_message: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "lastReaction")]
    pub last_reaction: Option<serde_json::Value>,
    pub muted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageView {
    /// Annotations of text (mentions, URLs, hashtags, etc)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facets: Option<serde_json::Value>,
    pub rev: String,
    pub text: String,
    /// Reactions to this message, in ascending order of creation time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reactions: Option<serde_json::Value>,
    pub sender: serde_json::Value,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embed: Option<serde_json::Value>,
    #[serde(rename = "sentAt")]
    pub sent_at: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogAcceptConvo {
    pub rev: String,
    #[serde(rename = "convoId")]
    pub convo_id: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletedMessageView {
    pub sender: serde_json::Value,
    pub id: String,
    #[serde(rename = "sentAt")]
    pub sent_at: String,
    pub rev: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogDeleteMessage {
    pub rev: String,
    pub message: serde_json::Value,
    #[serde(rename = "convoId")]
    pub convo_id: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogUnmuteConvo {
    pub rev: String,
    #[serde(rename = "convoId")]
    pub convo_id: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogLeaveConvo {
    pub rev: String,
    #[serde(rename = "convoId")]
    pub convo_id: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embed: Option<serde_json::Value>,
    pub text: String,
    /// Annotations of text (mentions, URLs, hashtags, etc)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facets: Option<serde_json::Value>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactionViewSender {
    pub did: crate::types::Did,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactionView {
    #[serde(rename = "createdAt")]
    pub created_at: String,
    pub value: String,
    pub sender: serde_json::Value,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogMuteConvo {
    pub rev: String,
    #[serde(rename = "convoId")]
    pub convo_id: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogReadMessage {
    pub message: serde_json::Value,
    pub rev: String,
    #[serde(rename = "convoId")]
    pub convo_id: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageViewSender {
    pub did: crate::types::Did,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRef {
    pub did: crate::types::Did,
    #[serde(rename = "convoId")]
    pub convo_id: String,
    #[serde(rename = "messageId")]
    pub message_id: String,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageAndReactionView {
    pub message: serde_json::Value,
    pub reaction: serde_json::Value,
}


