//! ATProto Moderation System
//!
//! This module provides label interpretation and content filtering for ATProto content.
//! It implements the same moderation logic as the official TypeScript SDK.
//!
//! # Overview
//!
//! The moderation system consists of several key components:
//!
//! - **Labels**: Metadata applied to content by labelers (e.g., "porn", "sexual", "graphic-media")
//! - **Moderation Causes**: Reasons why content is moderated (blocking, labels, muted words, etc.)
//! - **Moderation Behaviors**: How to display content in different UI contexts
//! - **Moderation Decisions**: Aggregated decision about how to handle content
//! - **Moderation UI**: Final output for what to show in the UI
//!
//! # Example
//!
//! ```rust
//! use atproto::moderation::{ModerationPrefs, moderate_post};
//!
//! // Configure user preferences
//! let prefs = ModerationPrefs {
//!     adult_content_enabled: true,
//!     labels: [("porn".to_string(), "hide".to_string())].into_iter().collect(),
//!     labelers: vec![],
//!     muted_words: vec![],
//!     hidden_posts: vec![],
//! };
//!
//! // Get moderation decision for content
//! // let decision = moderate_post(&post_view, &prefs, Some("did:plc:user"));
//!
//! // Check UI behavior for different contexts
//! // let list_ui = decision.ui("contentList");
//! // if list_ui.filter { /* hide from list */ }
//! // if list_ui.blur { /* show blurred */ }
//! // if list_ui.alert { /* show warning */ }
//! ```

pub mod types;
pub mod labels;
pub mod decision;
pub mod ui;
// pub mod subjects; // TODO: Needs complex type integration with generated JSON types

pub use types::*;
pub use labels::*;
pub use decision::*;
pub use ui::*;
// pub use subjects::*;
