//! Namespace wrappers for the Agent
//!
//! These wrappers provide access to the auto-generated API client code
//! through the Agent, ensuring proper authentication and header management.

use std::sync::Arc;
use crate::xrpc::XrpcClientImpl;

/// Wrapper for com.atproto.* APIs
#[derive(Clone)]
pub struct ComNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

impl ComNS {
    /// Access the com.atproto namespace
    pub fn atproto(&self) -> ComAtprotoNS {
        ComAtprotoNS {
            client: self.client.clone(),
        }
    }
}

/// Wrapper for com.atproto.* APIs
#[derive(Clone)]
pub struct ComAtprotoNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

impl ComAtprotoNS {
    /// Access com.atproto.server.* APIs
    pub fn server(&self) -> ComAtprotoServerNS {
        ComAtprotoServerNS {
            client: self.client.clone(),
        }
    }

    /// Access com.atproto.repo.* APIs
    pub fn repo(&self) -> ComAtprotoRepoNS {
        ComAtprotoRepoNS {
            client: self.client.clone(),
        }
    }

    /// Access com.atproto.identity.* APIs
    pub fn identity(&self) -> ComAtprotoIdentityNS {
        ComAtprotoIdentityNS {
            client: self.client.clone(),
        }
    }

    /// Access com.atproto.sync.* APIs
    pub fn sync(&self) -> ComAtprotoSyncNS {
        ComAtprotoSyncNS {
            client: self.client.clone(),
        }
    }

    /// Access com.atproto.admin.* APIs
    pub fn admin(&self) -> ComAtprotoAdminNS {
        ComAtprotoAdminNS {
            client: self.client.clone(),
        }
    }

    /// Access com.atproto.moderation.* APIs
    pub fn moderation(&self) -> ComAtprotoModerationNS {
        ComAtprotoModerationNS {
            client: self.client.clone(),
        }
    }

    /// Access com.atproto.label.* APIs
    pub fn label(&self) -> ComAtprotoLabelNS {
        ComAtprotoLabelNS {
            client: self.client.clone(),
        }
    }

    /// Access com.atproto.temp.* APIs
    pub fn temp(&self) -> ComAtprotoTempNS {
        ComAtprotoTempNS {
            client: self.client.clone(),
        }
    }
}

/// com.atproto.server.* namespace
#[derive(Clone)]
pub struct ComAtprotoServerNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// com.atproto.repo.* namespace
#[derive(Clone)]
pub struct ComAtprotoRepoNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// com.atproto.identity.* namespace
#[derive(Clone)]
pub struct ComAtprotoIdentityNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// com.atproto.sync.* namespace
#[derive(Clone)]
pub struct ComAtprotoSyncNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// com.atproto.admin.* namespace
#[derive(Clone)]
pub struct ComAtprotoAdminNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// com.atproto.moderation.* namespace
#[derive(Clone)]
pub struct ComAtprotoModerationNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// com.atproto.label.* namespace
#[derive(Clone)]
pub struct ComAtprotoLabelNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// com.atproto.temp.* namespace
#[derive(Clone)]
pub struct ComAtprotoTempNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// Wrapper for app.bsky.* APIs
#[derive(Clone)]
pub struct AppNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

impl AppNS {
    /// Access the app.bsky namespace
    pub fn bsky(&self) -> AppBskyNS {
        AppBskyNS {
            client: self.client.clone(),
        }
    }
}

/// Wrapper for app.bsky.* APIs
#[derive(Clone)]
pub struct AppBskyNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

impl AppBskyNS {
    /// Access app.bsky.actor.* APIs
    pub fn actor(&self) -> AppBskyActorNS {
        AppBskyActorNS {
            client: self.client.clone(),
        }
    }

    /// Access app.bsky.feed.* APIs
    pub fn feed(&self) -> AppBskyFeedNS {
        AppBskyFeedNS {
            client: self.client.clone(),
        }
    }

    /// Access app.bsky.graph.* APIs
    pub fn graph(&self) -> AppBskyGraphNS {
        AppBskyGraphNS {
            client: self.client.clone(),
        }
    }

    /// Access app.bsky.notification.* APIs
    pub fn notification(&self) -> AppBskyNotificationNS {
        AppBskyNotificationNS {
            client: self.client.clone(),
        }
    }

    /// Access app.bsky.labeler.* APIs
    pub fn labeler(&self) -> AppBskyLabelerNS {
        AppBskyLabelerNS {
            client: self.client.clone(),
        }
    }

    /// Access app.bsky.unspecced.* APIs
    pub fn unspecced(&self) -> AppBskyUnspeccedNS {
        AppBskyUnspeccedNS {
            client: self.client.clone(),
        }
    }

    /// Access app.bsky.video.* APIs
    pub fn video(&self) -> AppBskyVideoNS {
        AppBskyVideoNS {
            client: self.client.clone(),
        }
    }

    /// Access app.bsky.bookmark.* APIs
    pub fn bookmark(&self) -> AppBskyBookmarkNS {
        AppBskyBookmarkNS {
            client: self.client.clone(),
        }
    }
}

/// app.bsky.actor.* namespace
#[derive(Clone)]
pub struct AppBskyActorNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// app.bsky.feed.* namespace
#[derive(Clone)]
pub struct AppBskyFeedNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// app.bsky.graph.* namespace
#[derive(Clone)]
pub struct AppBskyGraphNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// app.bsky.notification.* namespace
#[derive(Clone)]
pub struct AppBskyNotificationNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// app.bsky.labeler.* namespace
#[derive(Clone)]
pub struct AppBskyLabelerNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// app.bsky.unspecced.* namespace
#[derive(Clone)]
pub struct AppBskyUnspeccedNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// app.bsky.video.* namespace
#[derive(Clone)]
pub struct AppBskyVideoNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// app.bsky.bookmark.* namespace
#[derive(Clone)]
pub struct AppBskyBookmarkNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// Wrapper for chat.bsky.* APIs
#[derive(Clone)]
pub struct ChatNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

impl ChatNS {
    /// Access the chat.bsky namespace
    pub fn bsky(&self) -> ChatBskyNS {
        ChatBskyNS {
            client: self.client.clone(),
        }
    }
}

/// Wrapper for chat.bsky.* APIs
#[derive(Clone)]
pub struct ChatBskyNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

impl ChatBskyNS {
    /// Access chat.bsky.actor.* APIs
    pub fn actor(&self) -> ChatBskyActorNS {
        ChatBskyActorNS {
            client: self.client.clone(),
        }
    }

    /// Access chat.bsky.convo.* APIs
    pub fn convo(&self) -> ChatBskyConvoNS {
        ChatBskyConvoNS {
            client: self.client.clone(),
        }
    }

    /// Access chat.bsky.moderation.* APIs
    pub fn moderation(&self) -> ChatBskyModerationNS {
        ChatBskyModerationNS {
            client: self.client.clone(),
        }
    }
}

/// chat.bsky.actor.* namespace
#[derive(Clone)]
pub struct ChatBskyActorNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// chat.bsky.convo.* namespace
#[derive(Clone)]
pub struct ChatBskyConvoNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// chat.bsky.moderation.* namespace
#[derive(Clone)]
pub struct ChatBskyModerationNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// Wrapper for tools.ozone.* APIs
#[derive(Clone)]
pub struct ToolsNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

impl ToolsNS {
    /// Access the tools.ozone namespace
    pub fn ozone(&self) -> ToolsOzoneNS {
        ToolsOzoneNS {
            client: self.client.clone(),
        }
    }
}

/// Wrapper for tools.ozone.* APIs
#[derive(Clone)]
pub struct ToolsOzoneNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

impl ToolsOzoneNS {
    /// Access tools.ozone.communication.* APIs
    pub fn communication(&self) -> ToolsOzoneCommunicationNS {
        ToolsOzoneCommunicationNS {
            client: self.client.clone(),
        }
    }

    /// Access tools.ozone.moderation.* APIs
    pub fn moderation(&self) -> ToolsOzoneModerationNS {
        ToolsOzoneModerationNS {
            client: self.client.clone(),
        }
    }

    /// Access tools.ozone.server.* APIs
    pub fn server(&self) -> ToolsOzoneServerNS {
        ToolsOzoneServerNS {
            client: self.client.clone(),
        }
    }

    /// Access tools.ozone.team.* APIs
    pub fn team(&self) -> ToolsOzoneTeamNS {
        ToolsOzoneTeamNS {
            client: self.client.clone(),
        }
    }

    /// Access tools.ozone.set.* APIs
    pub fn set(&self) -> ToolsOzoneSetNS {
        ToolsOzoneSetNS {
            client: self.client.clone(),
        }
    }

    /// Access tools.ozone.setting.* APIs
    pub fn setting(&self) -> ToolsOzoneSettingNS {
        ToolsOzoneSettingNS {
            client: self.client.clone(),
        }
    }

    /// Access tools.ozone.signature.* APIs
    pub fn signature(&self) -> ToolsOzoneSignatureNS {
        ToolsOzoneSignatureNS {
            client: self.client.clone(),
        }
    }
}

/// tools.ozone.communication.* namespace
#[derive(Clone)]
pub struct ToolsOzoneCommunicationNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// tools.ozone.moderation.* namespace
#[derive(Clone)]
pub struct ToolsOzoneModerationNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// tools.ozone.server.* namespace
#[derive(Clone)]
pub struct ToolsOzoneServerNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// tools.ozone.team.* namespace
#[derive(Clone)]
pub struct ToolsOzoneTeamNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// tools.ozone.set.* namespace
#[derive(Clone)]
pub struct ToolsOzoneSetNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// tools.ozone.setting.* namespace
#[derive(Clone)]
pub struct ToolsOzoneSettingNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}

/// tools.ozone.signature.* namespace
#[derive(Clone)]
pub struct ToolsOzoneSignatureNS {
    pub(crate) client: Arc<XrpcClientImpl>,
}
// Auto-generated impl blocks for namespace structs
// Append these to src/namespaces.rs

impl AppBskyActorNS {
    /// Call crate::client::app::bsky::actor::get_preferences::get_preferences
    pub async fn get_preferences(&self, params: crate::client::app::bsky::actor::get_preferences::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::actor::get_preferences::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::actor::get_preferences::get_preferences(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::actor::get_profile::get_profile
    pub async fn get_profile(&self, params: crate::client::app::bsky::actor::get_profile::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::actor::get_profile::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::actor::get_profile::get_profile(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::actor::get_profiles::get_profiles
    pub async fn get_profiles(&self, params: crate::client::app::bsky::actor::get_profiles::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::actor::get_profiles::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::actor::get_profiles::get_profiles(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::actor::get_suggestions::get_suggestions
    pub async fn get_suggestions(&self, params: crate::client::app::bsky::actor::get_suggestions::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::actor::get_suggestions::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::actor::get_suggestions::get_suggestions(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::actor::put_preferences::put_preferences
    pub async fn put_preferences(&self, input: crate::client::app::bsky::actor::put_preferences::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::actor::put_preferences::put_preferences(&*self.client, input).await
    }

    /// Call crate::client::app::bsky::actor::search_actors::search_actors
    pub async fn search_actors(&self, params: crate::client::app::bsky::actor::search_actors::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::actor::search_actors::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::actor::search_actors::search_actors(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::actor::search_actors_typeahead::search_actors_typeahead
    pub async fn search_actors_typeahead(&self, params: crate::client::app::bsky::actor::search_actors_typeahead::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::actor::search_actors_typeahead::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::actor::search_actors_typeahead::search_actors_typeahead(&*self.client, params).await
    }

}

impl AppBskyBookmarkNS {
    /// Call crate::client::app::bsky::bookmark::create_bookmark::create_bookmark
    pub async fn create_bookmark(&self, input: crate::client::app::bsky::bookmark::create_bookmark::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::bookmark::create_bookmark::create_bookmark(&*self.client, input).await
    }

    /// Call crate::client::app::bsky::bookmark::delete_bookmark::delete_bookmark
    pub async fn delete_bookmark(&self, input: crate::client::app::bsky::bookmark::delete_bookmark::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::bookmark::delete_bookmark::delete_bookmark(&*self.client, input).await
    }

    /// Call crate::client::app::bsky::bookmark::get_bookmarks::get_bookmarks
    pub async fn get_bookmarks(&self, params: crate::client::app::bsky::bookmark::get_bookmarks::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::bookmark::get_bookmarks::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::bookmark::get_bookmarks::get_bookmarks(&*self.client, params).await
    }

}

impl AppBskyFeedNS {
    /// Call crate::client::app::bsky::feed::describe_feed_generator::describe_feed_generator
    pub async fn describe_feed_generator(&self) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::feed::describe_feed_generator::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::feed::describe_feed_generator::describe_feed_generator(&*self.client).await
    }

    /// Call crate::client::app::bsky::feed::get_actor_feeds::get_actor_feeds
    pub async fn get_actor_feeds(&self, params: crate::client::app::bsky::feed::get_actor_feeds::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::feed::get_actor_feeds::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::feed::get_actor_feeds::get_actor_feeds(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::feed::get_actor_likes::get_actor_likes
    pub async fn get_actor_likes(&self, params: crate::client::app::bsky::feed::get_actor_likes::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::feed::get_actor_likes::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::feed::get_actor_likes::get_actor_likes(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::feed::get_author_feed::get_author_feed
    pub async fn get_author_feed(&self, params: crate::client::app::bsky::feed::get_author_feed::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::feed::get_author_feed::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::feed::get_author_feed::get_author_feed(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::feed::get_feed::get_feed
    pub async fn get_feed(&self, params: crate::client::app::bsky::feed::get_feed::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::feed::get_feed::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::feed::get_feed::get_feed(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::feed::get_feed_generator::get_feed_generator
    pub async fn get_feed_generator(&self, params: crate::client::app::bsky::feed::get_feed_generator::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::feed::get_feed_generator::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::feed::get_feed_generator::get_feed_generator(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::feed::get_feed_generators::get_feed_generators
    pub async fn get_feed_generators(&self, params: crate::client::app::bsky::feed::get_feed_generators::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::feed::get_feed_generators::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::feed::get_feed_generators::get_feed_generators(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::feed::get_feed_skeleton::get_feed_skeleton
    pub async fn get_feed_skeleton(&self, params: crate::client::app::bsky::feed::get_feed_skeleton::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::feed::get_feed_skeleton::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::feed::get_feed_skeleton::get_feed_skeleton(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::feed::get_likes::get_likes
    pub async fn get_likes(&self, params: crate::client::app::bsky::feed::get_likes::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::feed::get_likes::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::feed::get_likes::get_likes(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::feed::get_list_feed::get_list_feed
    pub async fn get_list_feed(&self, params: crate::client::app::bsky::feed::get_list_feed::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::feed::get_list_feed::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::feed::get_list_feed::get_list_feed(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::feed::get_post_thread::get_post_thread
    pub async fn get_post_thread(&self, params: crate::client::app::bsky::feed::get_post_thread::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::feed::get_post_thread::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::feed::get_post_thread::get_post_thread(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::feed::get_posts::get_posts
    pub async fn get_posts(&self, params: crate::client::app::bsky::feed::get_posts::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::feed::get_posts::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::feed::get_posts::get_posts(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::feed::get_quotes::get_quotes
    pub async fn get_quotes(&self, params: crate::client::app::bsky::feed::get_quotes::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::feed::get_quotes::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::feed::get_quotes::get_quotes(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::feed::get_reposted_by::get_reposted_by
    pub async fn get_reposted_by(&self, params: crate::client::app::bsky::feed::get_reposted_by::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::feed::get_reposted_by::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::feed::get_reposted_by::get_reposted_by(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::feed::get_suggested_feeds::get_suggested_feeds
    pub async fn get_suggested_feeds(&self, params: crate::client::app::bsky::feed::get_suggested_feeds::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::feed::get_suggested_feeds::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::feed::get_suggested_feeds::get_suggested_feeds(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::feed::get_timeline::get_timeline
    pub async fn get_timeline(&self, params: crate::client::app::bsky::feed::get_timeline::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::feed::get_timeline::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::feed::get_timeline::get_timeline(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::feed::search_posts::search_posts
    pub async fn search_posts(&self, params: crate::client::app::bsky::feed::search_posts::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::feed::search_posts::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::feed::search_posts::search_posts(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::feed::send_interactions::send_interactions
    pub async fn send_interactions(&self, input: crate::client::app::bsky::feed::send_interactions::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::feed::send_interactions::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::feed::send_interactions::send_interactions(&*self.client, input).await
    }

}

impl AppBskyGraphNS {
    /// Call crate::client::app::bsky::graph::get_actor_starter_packs::get_actor_starter_packs
    pub async fn get_actor_starter_packs(&self, params: crate::client::app::bsky::graph::get_actor_starter_packs::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::graph::get_actor_starter_packs::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::get_actor_starter_packs::get_actor_starter_packs(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::graph::get_blocks::get_blocks
    pub async fn get_blocks(&self, params: crate::client::app::bsky::graph::get_blocks::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::graph::get_blocks::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::get_blocks::get_blocks(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::graph::get_followers::get_followers
    pub async fn get_followers(&self, params: crate::client::app::bsky::graph::get_followers::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::graph::get_followers::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::get_followers::get_followers(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::graph::get_follows::get_follows
    pub async fn get_follows(&self, params: crate::client::app::bsky::graph::get_follows::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::graph::get_follows::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::get_follows::get_follows(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::graph::get_known_followers::get_known_followers
    pub async fn get_known_followers(&self, params: crate::client::app::bsky::graph::get_known_followers::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::graph::get_known_followers::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::get_known_followers::get_known_followers(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::graph::get_list::get_list
    pub async fn get_list(&self, params: crate::client::app::bsky::graph::get_list::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::graph::get_list::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::get_list::get_list(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::graph::get_list_blocks::get_list_blocks
    pub async fn get_list_blocks(&self, params: crate::client::app::bsky::graph::get_list_blocks::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::graph::get_list_blocks::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::get_list_blocks::get_list_blocks(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::graph::get_list_mutes::get_list_mutes
    pub async fn get_list_mutes(&self, params: crate::client::app::bsky::graph::get_list_mutes::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::graph::get_list_mutes::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::get_list_mutes::get_list_mutes(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::graph::get_lists::get_lists
    pub async fn get_lists(&self, params: crate::client::app::bsky::graph::get_lists::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::graph::get_lists::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::get_lists::get_lists(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::graph::get_lists_with_membership::get_lists_with_membership
    pub async fn get_lists_with_membership(&self, params: crate::client::app::bsky::graph::get_lists_with_membership::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::graph::get_lists_with_membership::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::get_lists_with_membership::get_lists_with_membership(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::graph::get_mutes::get_mutes
    pub async fn get_mutes(&self, params: crate::client::app::bsky::graph::get_mutes::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::graph::get_mutes::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::get_mutes::get_mutes(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::graph::get_relationships::get_relationships
    pub async fn get_relationships(&self, params: crate::client::app::bsky::graph::get_relationships::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::graph::get_relationships::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::get_relationships::get_relationships(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::graph::get_starter_pack::get_starter_pack
    pub async fn get_starter_pack(&self, params: crate::client::app::bsky::graph::get_starter_pack::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::graph::get_starter_pack::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::get_starter_pack::get_starter_pack(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::graph::get_starter_packs::get_starter_packs
    pub async fn get_starter_packs(&self, params: crate::client::app::bsky::graph::get_starter_packs::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::graph::get_starter_packs::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::get_starter_packs::get_starter_packs(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::graph::get_starter_packs_with_membership::get_starter_packs_with_membership
    pub async fn get_starter_packs_with_membership(&self, params: crate::client::app::bsky::graph::get_starter_packs_with_membership::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::graph::get_starter_packs_with_membership::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::get_starter_packs_with_membership::get_starter_packs_with_membership(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::graph::get_suggested_follows_by_actor::get_suggested_follows_by_actor
    pub async fn get_suggested_follows_by_actor(&self, params: crate::client::app::bsky::graph::get_suggested_follows_by_actor::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::graph::get_suggested_follows_by_actor::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::get_suggested_follows_by_actor::get_suggested_follows_by_actor(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::graph::mute_actor::mute_actor
    pub async fn mute_actor(&self, input: crate::client::app::bsky::graph::mute_actor::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::mute_actor::mute_actor(&*self.client, input).await
    }

    /// Call crate::client::app::bsky::graph::mute_actor_list::mute_actor_list
    pub async fn mute_actor_list(&self, input: crate::client::app::bsky::graph::mute_actor_list::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::mute_actor_list::mute_actor_list(&*self.client, input).await
    }

    /// Call crate::client::app::bsky::graph::mute_thread::mute_thread
    pub async fn mute_thread(&self, input: crate::client::app::bsky::graph::mute_thread::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::mute_thread::mute_thread(&*self.client, input).await
    }

    /// Call crate::client::app::bsky::graph::search_starter_packs::search_starter_packs
    pub async fn search_starter_packs(&self, params: crate::client::app::bsky::graph::search_starter_packs::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::graph::search_starter_packs::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::search_starter_packs::search_starter_packs(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::graph::unmute_actor::unmute_actor
    pub async fn unmute_actor(&self, input: crate::client::app::bsky::graph::unmute_actor::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::unmute_actor::unmute_actor(&*self.client, input).await
    }

    /// Call crate::client::app::bsky::graph::unmute_actor_list::unmute_actor_list
    pub async fn unmute_actor_list(&self, input: crate::client::app::bsky::graph::unmute_actor_list::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::unmute_actor_list::unmute_actor_list(&*self.client, input).await
    }

    /// Call crate::client::app::bsky::graph::unmute_thread::unmute_thread
    pub async fn unmute_thread(&self, input: crate::client::app::bsky::graph::unmute_thread::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::graph::unmute_thread::unmute_thread(&*self.client, input).await
    }

}

impl AppBskyLabelerNS {
    /// Call crate::client::app::bsky::labeler::get_services::get_services
    pub async fn get_services(&self, params: crate::client::app::bsky::labeler::get_services::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::labeler::get_services::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::labeler::get_services::get_services(&*self.client, params).await
    }

}

impl AppBskyNotificationNS {
    /// Call crate::client::app::bsky::notification::get_preferences::get_preferences
    pub async fn get_preferences(&self, params: crate::client::app::bsky::notification::get_preferences::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::notification::get_preferences::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::notification::get_preferences::get_preferences(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::notification::get_unread_count::get_unread_count
    pub async fn get_unread_count(&self, params: crate::client::app::bsky::notification::get_unread_count::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::notification::get_unread_count::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::notification::get_unread_count::get_unread_count(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::notification::list_activity_subscriptions::list_activity_subscriptions
    pub async fn list_activity_subscriptions(&self, params: crate::client::app::bsky::notification::list_activity_subscriptions::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::notification::list_activity_subscriptions::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::notification::list_activity_subscriptions::list_activity_subscriptions(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::notification::list_notifications::list_notifications
    pub async fn list_notifications(&self, params: crate::client::app::bsky::notification::list_notifications::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::notification::list_notifications::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::notification::list_notifications::list_notifications(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::notification::put_activity_subscription::put_activity_subscription
    pub async fn put_activity_subscription(&self, input: crate::client::app::bsky::notification::put_activity_subscription::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::notification::put_activity_subscription::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::notification::put_activity_subscription::put_activity_subscription(&*self.client, input).await
    }

    /// Call crate::client::app::bsky::notification::put_preferences::put_preferences
    pub async fn put_preferences(&self, input: crate::client::app::bsky::notification::put_preferences::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::notification::put_preferences::put_preferences(&*self.client, input).await
    }

    /// Call crate::client::app::bsky::notification::put_preferences_v2::put_preferences_v2
    pub async fn put_preferences_v2(&self, input: crate::client::app::bsky::notification::put_preferences_v2::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::notification::put_preferences_v2::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::notification::put_preferences_v2::put_preferences_v2(&*self.client, input).await
    }

    /// Call crate::client::app::bsky::notification::register_push::register_push
    pub async fn register_push(&self, input: crate::client::app::bsky::notification::register_push::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::notification::register_push::register_push(&*self.client, input).await
    }

    /// Call crate::client::app::bsky::notification::unregister_push::unregister_push
    pub async fn unregister_push(&self, input: crate::client::app::bsky::notification::unregister_push::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::notification::unregister_push::unregister_push(&*self.client, input).await
    }

    /// Call crate::client::app::bsky::notification::update_seen::update_seen
    pub async fn update_seen(&self, input: crate::client::app::bsky::notification::update_seen::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::notification::update_seen::update_seen(&*self.client, input).await
    }

}

impl AppBskyUnspeccedNS {
    /// Call crate::client::app::bsky::unspecced::get_age_assurance_state::get_age_assurance_state
    pub async fn get_age_assurance_state(&self) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::get_age_assurance_state::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::get_age_assurance_state::get_age_assurance_state(&*self.client).await
    }

    /// Call crate::client::app::bsky::unspecced::get_config::get_config
    pub async fn get_config(&self) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::get_config::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::get_config::get_config(&*self.client).await
    }

    /// Call crate::client::app::bsky::unspecced::get_onboarding_suggested_starter_packs::get_onboarding_suggested_starter_packs
    pub async fn get_onboarding_suggested_starter_packs(&self, params: crate::client::app::bsky::unspecced::get_onboarding_suggested_starter_packs::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::get_onboarding_suggested_starter_packs::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::get_onboarding_suggested_starter_packs::get_onboarding_suggested_starter_packs(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::unspecced::get_onboarding_suggested_starter_packs_skeleton::get_onboarding_suggested_starter_packs_skeleton
    pub async fn get_onboarding_suggested_starter_packs_skeleton(&self, params: crate::client::app::bsky::unspecced::get_onboarding_suggested_starter_packs_skeleton::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::get_onboarding_suggested_starter_packs_skeleton::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::get_onboarding_suggested_starter_packs_skeleton::get_onboarding_suggested_starter_packs_skeleton(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::unspecced::get_popular_feed_generators::get_popular_feed_generators
    pub async fn get_popular_feed_generators(&self, params: crate::client::app::bsky::unspecced::get_popular_feed_generators::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::get_popular_feed_generators::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::get_popular_feed_generators::get_popular_feed_generators(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::unspecced::get_post_thread_other_v2::get_post_thread_other_v2
    pub async fn get_post_thread_other_v2(&self, params: crate::client::app::bsky::unspecced::get_post_thread_other_v2::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::get_post_thread_other_v2::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::get_post_thread_other_v2::get_post_thread_other_v2(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::unspecced::get_post_thread_v2::get_post_thread_v2
    pub async fn get_post_thread_v2(&self, params: crate::client::app::bsky::unspecced::get_post_thread_v2::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::get_post_thread_v2::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::get_post_thread_v2::get_post_thread_v2(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::unspecced::get_suggested_feeds::get_suggested_feeds
    pub async fn get_suggested_feeds(&self, params: crate::client::app::bsky::unspecced::get_suggested_feeds::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::get_suggested_feeds::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::get_suggested_feeds::get_suggested_feeds(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::unspecced::get_suggested_feeds_skeleton::get_suggested_feeds_skeleton
    pub async fn get_suggested_feeds_skeleton(&self, params: crate::client::app::bsky::unspecced::get_suggested_feeds_skeleton::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::get_suggested_feeds_skeleton::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::get_suggested_feeds_skeleton::get_suggested_feeds_skeleton(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::unspecced::get_suggested_starter_packs::get_suggested_starter_packs
    pub async fn get_suggested_starter_packs(&self, params: crate::client::app::bsky::unspecced::get_suggested_starter_packs::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::get_suggested_starter_packs::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::get_suggested_starter_packs::get_suggested_starter_packs(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::unspecced::get_suggested_starter_packs_skeleton::get_suggested_starter_packs_skeleton
    pub async fn get_suggested_starter_packs_skeleton(&self, params: crate::client::app::bsky::unspecced::get_suggested_starter_packs_skeleton::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::get_suggested_starter_packs_skeleton::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::get_suggested_starter_packs_skeleton::get_suggested_starter_packs_skeleton(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::unspecced::get_suggested_users::get_suggested_users
    pub async fn get_suggested_users(&self, params: crate::client::app::bsky::unspecced::get_suggested_users::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::get_suggested_users::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::get_suggested_users::get_suggested_users(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::unspecced::get_suggested_users_skeleton::get_suggested_users_skeleton
    pub async fn get_suggested_users_skeleton(&self, params: crate::client::app::bsky::unspecced::get_suggested_users_skeleton::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::get_suggested_users_skeleton::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::get_suggested_users_skeleton::get_suggested_users_skeleton(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::unspecced::get_suggestions_skeleton::get_suggestions_skeleton
    pub async fn get_suggestions_skeleton(&self, params: crate::client::app::bsky::unspecced::get_suggestions_skeleton::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::get_suggestions_skeleton::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::get_suggestions_skeleton::get_suggestions_skeleton(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::unspecced::get_tagged_suggestions::get_tagged_suggestions
    pub async fn get_tagged_suggestions(&self, params: crate::client::app::bsky::unspecced::get_tagged_suggestions::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::get_tagged_suggestions::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::get_tagged_suggestions::get_tagged_suggestions(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::unspecced::get_trending_topics::get_trending_topics
    pub async fn get_trending_topics(&self, params: crate::client::app::bsky::unspecced::get_trending_topics::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::get_trending_topics::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::get_trending_topics::get_trending_topics(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::unspecced::get_trends::get_trends
    pub async fn get_trends(&self, params: crate::client::app::bsky::unspecced::get_trends::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::get_trends::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::get_trends::get_trends(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::unspecced::get_trends_skeleton::get_trends_skeleton
    pub async fn get_trends_skeleton(&self, params: crate::client::app::bsky::unspecced::get_trends_skeleton::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::get_trends_skeleton::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::get_trends_skeleton::get_trends_skeleton(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::unspecced::init_age_assurance::init_age_assurance
    pub async fn init_age_assurance(&self, input: crate::client::app::bsky::unspecced::init_age_assurance::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::init_age_assurance::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::init_age_assurance::init_age_assurance(&*self.client, input).await
    }

    /// Call crate::client::app::bsky::unspecced::search_actors_skeleton::search_actors_skeleton
    pub async fn search_actors_skeleton(&self, params: crate::client::app::bsky::unspecced::search_actors_skeleton::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::search_actors_skeleton::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::search_actors_skeleton::search_actors_skeleton(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::unspecced::search_posts_skeleton::search_posts_skeleton
    pub async fn search_posts_skeleton(&self, params: crate::client::app::bsky::unspecced::search_posts_skeleton::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::search_posts_skeleton::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::search_posts_skeleton::search_posts_skeleton(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::unspecced::search_starter_packs_skeleton::search_starter_packs_skeleton
    pub async fn search_starter_packs_skeleton(&self, params: crate::client::app::bsky::unspecced::search_starter_packs_skeleton::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::unspecced::search_starter_packs_skeleton::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::unspecced::search_starter_packs_skeleton::search_starter_packs_skeleton(&*self.client, params).await
    }

}

impl AppBskyVideoNS {
    /// Call crate::client::app::bsky::video::get_job_status::get_job_status
    pub async fn get_job_status(&self, params: crate::client::app::bsky::video::get_job_status::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::video::get_job_status::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::video::get_job_status::get_job_status(&*self.client, params).await
    }

    /// Call crate::client::app::bsky::video::get_upload_limits::get_upload_limits
    pub async fn get_upload_limits(&self) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::video::get_upload_limits::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::video::get_upload_limits::get_upload_limits(&*self.client).await
    }

    /// Call crate::client::app::bsky::video::upload_video::upload_video
    pub async fn upload_video(&self, input: crate::client::app::bsky::video::upload_video::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::app::bsky::video::upload_video::Output>, crate::xrpc::XrpcError> {
        crate::client::app::bsky::video::upload_video::upload_video(&*self.client, input).await
    }

}

impl ChatBskyActorNS {
    /// Call crate::client::chat::bsky::actor::delete_account::delete_account
    pub async fn delete_account(&self) -> Result<crate::xrpc::XrpcResponse<crate::client::chat::bsky::actor::delete_account::Output>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::actor::delete_account::delete_account(&*self.client).await
    }

    /// Call crate::client::chat::bsky::actor::export_account_data::export_account_data
    pub async fn export_account_data(&self) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::actor::export_account_data::export_account_data(&*self.client).await
    }

}

impl ChatBskyConvoNS {
    /// Call crate::client::chat::bsky::convo::accept_convo::accept_convo
    pub async fn accept_convo(&self, input: crate::client::chat::bsky::convo::accept_convo::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::chat::bsky::convo::accept_convo::Output>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::convo::accept_convo::accept_convo(&*self.client, input).await
    }

    /// Call crate::client::chat::bsky::convo::add_reaction::add_reaction
    pub async fn add_reaction(&self, input: crate::client::chat::bsky::convo::add_reaction::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::chat::bsky::convo::add_reaction::Output>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::convo::add_reaction::add_reaction(&*self.client, input).await
    }

    /// Call crate::client::chat::bsky::convo::delete_message_for_self::delete_message_for_self
    pub async fn delete_message_for_self(&self, input: crate::client::chat::bsky::convo::delete_message_for_self::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::chat::bsky::convo::delete_message_for_self::Output>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::convo::delete_message_for_self::delete_message_for_self(&*self.client, input).await
    }

    /// Call crate::client::chat::bsky::convo::get_convo::get_convo
    pub async fn get_convo(&self, params: crate::client::chat::bsky::convo::get_convo::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::chat::bsky::convo::get_convo::Output>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::convo::get_convo::get_convo(&*self.client, params).await
    }

    /// Call crate::client::chat::bsky::convo::get_convo_availability::get_convo_availability
    pub async fn get_convo_availability(&self, params: crate::client::chat::bsky::convo::get_convo_availability::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::chat::bsky::convo::get_convo_availability::Output>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::convo::get_convo_availability::get_convo_availability(&*self.client, params).await
    }

    /// Call crate::client::chat::bsky::convo::get_convo_for_members::get_convo_for_members
    pub async fn get_convo_for_members(&self, params: crate::client::chat::bsky::convo::get_convo_for_members::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::chat::bsky::convo::get_convo_for_members::Output>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::convo::get_convo_for_members::get_convo_for_members(&*self.client, params).await
    }

    /// Call crate::client::chat::bsky::convo::get_log::get_log
    pub async fn get_log(&self, params: crate::client::chat::bsky::convo::get_log::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::chat::bsky::convo::get_log::Output>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::convo::get_log::get_log(&*self.client, params).await
    }

    /// Call crate::client::chat::bsky::convo::get_messages::get_messages
    pub async fn get_messages(&self, params: crate::client::chat::bsky::convo::get_messages::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::chat::bsky::convo::get_messages::Output>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::convo::get_messages::get_messages(&*self.client, params).await
    }

    /// Call crate::client::chat::bsky::convo::leave_convo::leave_convo
    pub async fn leave_convo(&self, input: crate::client::chat::bsky::convo::leave_convo::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::chat::bsky::convo::leave_convo::Output>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::convo::leave_convo::leave_convo(&*self.client, input).await
    }

    /// Call crate::client::chat::bsky::convo::list_convos::list_convos
    pub async fn list_convos(&self, params: crate::client::chat::bsky::convo::list_convos::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::chat::bsky::convo::list_convos::Output>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::convo::list_convos::list_convos(&*self.client, params).await
    }

    /// Call crate::client::chat::bsky::convo::mute_convo::mute_convo
    pub async fn mute_convo(&self, input: crate::client::chat::bsky::convo::mute_convo::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::chat::bsky::convo::mute_convo::Output>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::convo::mute_convo::mute_convo(&*self.client, input).await
    }

    /// Call crate::client::chat::bsky::convo::remove_reaction::remove_reaction
    pub async fn remove_reaction(&self, input: crate::client::chat::bsky::convo::remove_reaction::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::chat::bsky::convo::remove_reaction::Output>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::convo::remove_reaction::remove_reaction(&*self.client, input).await
    }

    /// Call crate::client::chat::bsky::convo::send_message::send_message
    pub async fn send_message(&self, input: crate::client::chat::bsky::convo::send_message::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::chat::bsky::convo::send_message::Output>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::convo::send_message::send_message(&*self.client, input).await
    }

    /// Call crate::client::chat::bsky::convo::send_message_batch::send_message_batch
    pub async fn send_message_batch(&self, input: crate::client::chat::bsky::convo::send_message_batch::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::chat::bsky::convo::send_message_batch::Output>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::convo::send_message_batch::send_message_batch(&*self.client, input).await
    }

    /// Call crate::client::chat::bsky::convo::unmute_convo::unmute_convo
    pub async fn unmute_convo(&self, input: crate::client::chat::bsky::convo::unmute_convo::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::chat::bsky::convo::unmute_convo::Output>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::convo::unmute_convo::unmute_convo(&*self.client, input).await
    }

    /// Call crate::client::chat::bsky::convo::update_all_read::update_all_read
    pub async fn update_all_read(&self, input: crate::client::chat::bsky::convo::update_all_read::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::chat::bsky::convo::update_all_read::Output>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::convo::update_all_read::update_all_read(&*self.client, input).await
    }

    /// Call crate::client::chat::bsky::convo::update_read::update_read
    pub async fn update_read(&self, input: crate::client::chat::bsky::convo::update_read::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::chat::bsky::convo::update_read::Output>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::convo::update_read::update_read(&*self.client, input).await
    }

}

impl ChatBskyModerationNS {
    /// Call crate::client::chat::bsky::moderation::get_actor_metadata::get_actor_metadata
    pub async fn get_actor_metadata(&self, params: crate::client::chat::bsky::moderation::get_actor_metadata::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::chat::bsky::moderation::get_actor_metadata::Output>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::moderation::get_actor_metadata::get_actor_metadata(&*self.client, params).await
    }

    /// Call crate::client::chat::bsky::moderation::get_message_context::get_message_context
    pub async fn get_message_context(&self, params: crate::client::chat::bsky::moderation::get_message_context::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::chat::bsky::moderation::get_message_context::Output>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::moderation::get_message_context::get_message_context(&*self.client, params).await
    }

    /// Call crate::client::chat::bsky::moderation::update_actor_access::update_actor_access
    pub async fn update_actor_access(&self, input: crate::client::chat::bsky::moderation::update_actor_access::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::chat::bsky::moderation::update_actor_access::update_actor_access(&*self.client, input).await
    }

}

impl ComAtprotoAdminNS {
    /// Call crate::client::com::atproto::admin::delete_account::delete_account
    pub async fn delete_account(&self, input: crate::client::com::atproto::admin::delete_account::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::admin::delete_account::delete_account(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::admin::disable_account_invites::disable_account_invites
    pub async fn disable_account_invites(&self, input: crate::client::com::atproto::admin::disable_account_invites::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::admin::disable_account_invites::disable_account_invites(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::admin::disable_invite_codes::disable_invite_codes
    pub async fn disable_invite_codes(&self, input: crate::client::com::atproto::admin::disable_invite_codes::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::admin::disable_invite_codes::disable_invite_codes(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::admin::enable_account_invites::enable_account_invites
    pub async fn enable_account_invites(&self, input: crate::client::com::atproto::admin::enable_account_invites::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::admin::enable_account_invites::enable_account_invites(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::admin::get_account_info::get_account_info
    pub async fn get_account_info(&self, params: crate::client::com::atproto::admin::get_account_info::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::admin::get_account_info::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::admin::get_account_info::get_account_info(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::admin::get_account_infos::get_account_infos
    pub async fn get_account_infos(&self, params: crate::client::com::atproto::admin::get_account_infos::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::admin::get_account_infos::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::admin::get_account_infos::get_account_infos(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::admin::get_invite_codes::get_invite_codes
    pub async fn get_invite_codes(&self, params: crate::client::com::atproto::admin::get_invite_codes::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::admin::get_invite_codes::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::admin::get_invite_codes::get_invite_codes(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::admin::get_subject_status::get_subject_status
    pub async fn get_subject_status(&self, params: crate::client::com::atproto::admin::get_subject_status::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::admin::get_subject_status::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::admin::get_subject_status::get_subject_status(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::admin::search_accounts::search_accounts
    pub async fn search_accounts(&self, params: crate::client::com::atproto::admin::search_accounts::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::admin::search_accounts::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::admin::search_accounts::search_accounts(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::admin::send_email::send_email
    pub async fn send_email(&self, input: crate::client::com::atproto::admin::send_email::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::admin::send_email::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::admin::send_email::send_email(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::admin::update_account_email::update_account_email
    pub async fn update_account_email(&self, input: crate::client::com::atproto::admin::update_account_email::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::admin::update_account_email::update_account_email(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::admin::update_account_handle::update_account_handle
    pub async fn update_account_handle(&self, input: crate::client::com::atproto::admin::update_account_handle::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::admin::update_account_handle::update_account_handle(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::admin::update_account_password::update_account_password
    pub async fn update_account_password(&self, input: crate::client::com::atproto::admin::update_account_password::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::admin::update_account_password::update_account_password(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::admin::update_account_signing_key::update_account_signing_key
    pub async fn update_account_signing_key(&self, input: crate::client::com::atproto::admin::update_account_signing_key::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::admin::update_account_signing_key::update_account_signing_key(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::admin::update_subject_status::update_subject_status
    pub async fn update_subject_status(&self, input: crate::client::com::atproto::admin::update_subject_status::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::admin::update_subject_status::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::admin::update_subject_status::update_subject_status(&*self.client, input).await
    }

}

impl ComAtprotoIdentityNS {
    /// Call crate::client::com::atproto::identity::get_recommended_did_credentials::get_recommended_did_credentials
    pub async fn get_recommended_did_credentials(&self) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::identity::get_recommended_did_credentials::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::identity::get_recommended_did_credentials::get_recommended_did_credentials(&*self.client).await
    }

    /// Call crate::client::com::atproto::identity::refresh_identity::refresh_identity
    pub async fn refresh_identity(&self, input: crate::client::com::atproto::identity::refresh_identity::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::identity::refresh_identity::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::identity::refresh_identity::refresh_identity(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::identity::request_plc_operation_signature::request_plc_operation_signature
    pub async fn request_plc_operation_signature(&self) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::identity::request_plc_operation_signature::request_plc_operation_signature(&*self.client).await
    }

    /// Call crate::client::com::atproto::identity::resolve_did::resolve_did
    pub async fn resolve_did(&self, params: crate::client::com::atproto::identity::resolve_did::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::identity::resolve_did::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::identity::resolve_did::resolve_did(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::identity::resolve_handle::resolve_handle
    pub async fn resolve_handle(&self, params: crate::client::com::atproto::identity::resolve_handle::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::identity::resolve_handle::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::identity::resolve_handle::resolve_handle(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::identity::resolve_identity::resolve_identity
    pub async fn resolve_identity(&self, params: crate::client::com::atproto::identity::resolve_identity::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::identity::resolve_identity::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::identity::resolve_identity::resolve_identity(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::identity::sign_plc_operation::sign_plc_operation
    pub async fn sign_plc_operation(&self, input: crate::client::com::atproto::identity::sign_plc_operation::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::identity::sign_plc_operation::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::identity::sign_plc_operation::sign_plc_operation(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::identity::submit_plc_operation::submit_plc_operation
    pub async fn submit_plc_operation(&self, input: crate::client::com::atproto::identity::submit_plc_operation::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::identity::submit_plc_operation::submit_plc_operation(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::identity::update_handle::update_handle
    pub async fn update_handle(&self, input: crate::client::com::atproto::identity::update_handle::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::identity::update_handle::update_handle(&*self.client, input).await
    }

}

impl ComAtprotoLabelNS {
    /// Call crate::client::com::atproto::label::query_labels::query_labels
    pub async fn query_labels(&self, params: crate::client::com::atproto::label::query_labels::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::label::query_labels::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::label::query_labels::query_labels(&*self.client, params).await
    }

}

impl ComAtprotoModerationNS {
    /// Call crate::client::com::atproto::moderation::create_report::create_report
    pub async fn create_report(&self, input: crate::client::com::atproto::moderation::create_report::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::moderation::create_report::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::moderation::create_report::create_report(&*self.client, input).await
    }

}

impl ComAtprotoRepoNS {
    /// Call crate::client::com::atproto::repo::apply_writes::apply_writes
    pub async fn apply_writes(&self, input: crate::client::com::atproto::repo::apply_writes::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::repo::apply_writes::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::repo::apply_writes::apply_writes(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::repo::create_record::create_record
    pub async fn create_record(&self, input: crate::client::com::atproto::repo::create_record::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::repo::create_record::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::repo::create_record::create_record(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::repo::delete_record::delete_record
    pub async fn delete_record(&self, input: crate::client::com::atproto::repo::delete_record::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::repo::delete_record::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::repo::delete_record::delete_record(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::repo::describe_repo::describe_repo
    pub async fn describe_repo(&self, params: crate::client::com::atproto::repo::describe_repo::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::repo::describe_repo::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::repo::describe_repo::describe_repo(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::repo::get_record::get_record
    pub async fn get_record(&self, params: crate::client::com::atproto::repo::get_record::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::repo::get_record::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::repo::get_record::get_record(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::repo::import_repo::import_repo
    pub async fn import_repo(&self, input: crate::client::com::atproto::repo::import_repo::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::repo::import_repo::import_repo(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::repo::list_missing_blobs::list_missing_blobs
    pub async fn list_missing_blobs(&self, params: crate::client::com::atproto::repo::list_missing_blobs::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::repo::list_missing_blobs::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::repo::list_missing_blobs::list_missing_blobs(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::repo::list_records::list_records
    pub async fn list_records(&self, params: crate::client::com::atproto::repo::list_records::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::repo::list_records::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::repo::list_records::list_records(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::repo::put_record::put_record
    pub async fn put_record(&self, input: crate::client::com::atproto::repo::put_record::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::repo::put_record::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::repo::put_record::put_record(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::repo::upload_blob::upload_blob
    pub async fn upload_blob(&self, input: crate::client::com::atproto::repo::upload_blob::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::repo::upload_blob::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::repo::upload_blob::upload_blob(&*self.client, input).await
    }

}

impl ComAtprotoServerNS {
    /// Call crate::client::com::atproto::server::activate_account::activate_account
    pub async fn activate_account(&self) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::activate_account::activate_account(&*self.client).await
    }

    /// Call crate::client::com::atproto::server::check_account_status::check_account_status
    pub async fn check_account_status(&self) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::server::check_account_status::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::check_account_status::check_account_status(&*self.client).await
    }

    /// Call crate::client::com::atproto::server::confirm_email::confirm_email
    pub async fn confirm_email(&self, input: crate::client::com::atproto::server::confirm_email::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::confirm_email::confirm_email(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::server::create_account::create_account
    pub async fn create_account(&self, input: crate::client::com::atproto::server::create_account::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::server::create_account::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::create_account::create_account(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::server::create_app_password::create_app_password
    pub async fn create_app_password(&self, input: crate::client::com::atproto::server::create_app_password::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::server::create_app_password::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::create_app_password::create_app_password(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::server::create_invite_code::create_invite_code
    pub async fn create_invite_code(&self, input: crate::client::com::atproto::server::create_invite_code::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::server::create_invite_code::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::create_invite_code::create_invite_code(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::server::create_invite_codes::create_invite_codes
    pub async fn create_invite_codes(&self, input: crate::client::com::atproto::server::create_invite_codes::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::server::create_invite_codes::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::create_invite_codes::create_invite_codes(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::server::create_session::create_session
    pub async fn create_session(&self, input: crate::client::com::atproto::server::create_session::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::server::create_session::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::create_session::create_session(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::server::deactivate_account::deactivate_account
    pub async fn deactivate_account(&self, input: crate::client::com::atproto::server::deactivate_account::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::deactivate_account::deactivate_account(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::server::delete_account::delete_account
    pub async fn delete_account(&self, input: crate::client::com::atproto::server::delete_account::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::delete_account::delete_account(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::server::delete_session::delete_session
    pub async fn delete_session(&self) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::delete_session::delete_session(&*self.client).await
    }

    /// Call crate::client::com::atproto::server::describe_server::describe_server
    pub async fn describe_server(&self) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::server::describe_server::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::describe_server::describe_server(&*self.client).await
    }

    /// Call crate::client::com::atproto::server::get_account_invite_codes::get_account_invite_codes
    pub async fn get_account_invite_codes(&self, params: crate::client::com::atproto::server::get_account_invite_codes::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::server::get_account_invite_codes::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::get_account_invite_codes::get_account_invite_codes(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::server::get_service_auth::get_service_auth
    pub async fn get_service_auth(&self, params: crate::client::com::atproto::server::get_service_auth::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::server::get_service_auth::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::get_service_auth::get_service_auth(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::server::get_session::get_session
    pub async fn get_session(&self) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::server::get_session::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::get_session::get_session(&*self.client).await
    }

    /// Call crate::client::com::atproto::server::list_app_passwords::list_app_passwords
    pub async fn list_app_passwords(&self) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::server::list_app_passwords::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::list_app_passwords::list_app_passwords(&*self.client).await
    }

    /// Call crate::client::com::atproto::server::refresh_session::refresh_session
    pub async fn refresh_session(&self) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::server::refresh_session::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::refresh_session::refresh_session(&*self.client).await
    }

    /// Call crate::client::com::atproto::server::request_account_delete::request_account_delete
    pub async fn request_account_delete(&self) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::request_account_delete::request_account_delete(&*self.client).await
    }

    /// Call crate::client::com::atproto::server::request_email_confirmation::request_email_confirmation
    pub async fn request_email_confirmation(&self) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::request_email_confirmation::request_email_confirmation(&*self.client).await
    }

    /// Call crate::client::com::atproto::server::request_email_update::request_email_update
    pub async fn request_email_update(&self) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::server::request_email_update::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::request_email_update::request_email_update(&*self.client).await
    }

    /// Call crate::client::com::atproto::server::request_password_reset::request_password_reset
    pub async fn request_password_reset(&self, input: crate::client::com::atproto::server::request_password_reset::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::request_password_reset::request_password_reset(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::server::reserve_signing_key::reserve_signing_key
    pub async fn reserve_signing_key(&self, input: crate::client::com::atproto::server::reserve_signing_key::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::server::reserve_signing_key::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::reserve_signing_key::reserve_signing_key(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::server::reset_password::reset_password
    pub async fn reset_password(&self, input: crate::client::com::atproto::server::reset_password::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::reset_password::reset_password(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::server::revoke_app_password::revoke_app_password
    pub async fn revoke_app_password(&self, input: crate::client::com::atproto::server::revoke_app_password::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::revoke_app_password::revoke_app_password(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::server::update_email::update_email
    pub async fn update_email(&self, input: crate::client::com::atproto::server::update_email::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::server::update_email::update_email(&*self.client, input).await
    }

}

impl ComAtprotoSyncNS {
    /// Call crate::client::com::atproto::sync::get_blob::get_blob
    pub async fn get_blob(&self, params: crate::client::com::atproto::sync::get_blob::QueryParams) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::sync::get_blob::get_blob(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::sync::get_blocks::get_blocks
    pub async fn get_blocks(&self, params: crate::client::com::atproto::sync::get_blocks::QueryParams) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::sync::get_blocks::get_blocks(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::sync::get_checkout::get_checkout
    pub async fn get_checkout(&self, params: crate::client::com::atproto::sync::get_checkout::QueryParams) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::sync::get_checkout::get_checkout(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::sync::get_head::get_head
    pub async fn get_head(&self, params: crate::client::com::atproto::sync::get_head::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::sync::get_head::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::sync::get_head::get_head(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::sync::get_host_status::get_host_status
    pub async fn get_host_status(&self, params: crate::client::com::atproto::sync::get_host_status::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::sync::get_host_status::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::sync::get_host_status::get_host_status(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::sync::get_latest_commit::get_latest_commit
    pub async fn get_latest_commit(&self, params: crate::client::com::atproto::sync::get_latest_commit::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::sync::get_latest_commit::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::sync::get_latest_commit::get_latest_commit(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::sync::get_record::get_record
    pub async fn get_record(&self, params: crate::client::com::atproto::sync::get_record::QueryParams) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::sync::get_record::get_record(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::sync::get_repo::get_repo
    pub async fn get_repo(&self, params: crate::client::com::atproto::sync::get_repo::QueryParams) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::sync::get_repo::get_repo(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::sync::get_repo_status::get_repo_status
    pub async fn get_repo_status(&self, params: crate::client::com::atproto::sync::get_repo_status::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::sync::get_repo_status::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::sync::get_repo_status::get_repo_status(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::sync::list_blobs::list_blobs
    pub async fn list_blobs(&self, params: crate::client::com::atproto::sync::list_blobs::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::sync::list_blobs::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::sync::list_blobs::list_blobs(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::sync::list_hosts::list_hosts
    pub async fn list_hosts(&self, params: crate::client::com::atproto::sync::list_hosts::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::sync::list_hosts::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::sync::list_hosts::list_hosts(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::sync::list_repos::list_repos
    pub async fn list_repos(&self, params: crate::client::com::atproto::sync::list_repos::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::sync::list_repos::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::sync::list_repos::list_repos(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::sync::list_repos_by_collection::list_repos_by_collection
    pub async fn list_repos_by_collection(&self, params: crate::client::com::atproto::sync::list_repos_by_collection::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::sync::list_repos_by_collection::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::sync::list_repos_by_collection::list_repos_by_collection(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::sync::notify_of_update::notify_of_update
    pub async fn notify_of_update(&self, input: crate::client::com::atproto::sync::notify_of_update::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::sync::notify_of_update::notify_of_update(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::sync::request_crawl::request_crawl
    pub async fn request_crawl(&self, input: crate::client::com::atproto::sync::request_crawl::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::sync::request_crawl::request_crawl(&*self.client, input).await
    }

}

impl ComAtprotoTempNS {
    /// Call crate::client::com::atproto::temp::add_reserved_handle::add_reserved_handle
    pub async fn add_reserved_handle(&self, input: crate::client::com::atproto::temp::add_reserved_handle::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::temp::add_reserved_handle::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::temp::add_reserved_handle::add_reserved_handle(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::temp::check_handle_availability::check_handle_availability
    pub async fn check_handle_availability(&self, params: crate::client::com::atproto::temp::check_handle_availability::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::temp::check_handle_availability::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::temp::check_handle_availability::check_handle_availability(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::temp::check_signup_queue::check_signup_queue
    pub async fn check_signup_queue(&self) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::temp::check_signup_queue::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::temp::check_signup_queue::check_signup_queue(&*self.client).await
    }

    /// Call crate::client::com::atproto::temp::dereference_scope::dereference_scope
    pub async fn dereference_scope(&self, params: crate::client::com::atproto::temp::dereference_scope::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::temp::dereference_scope::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::temp::dereference_scope::dereference_scope(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::temp::fetch_labels::fetch_labels
    pub async fn fetch_labels(&self, params: crate::client::com::atproto::temp::fetch_labels::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::com::atproto::temp::fetch_labels::Output>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::temp::fetch_labels::fetch_labels(&*self.client, params).await
    }

    /// Call crate::client::com::atproto::temp::request_phone_verification::request_phone_verification
    pub async fn request_phone_verification(&self, input: crate::client::com::atproto::temp::request_phone_verification::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::temp::request_phone_verification::request_phone_verification(&*self.client, input).await
    }

    /// Call crate::client::com::atproto::temp::revoke_account_credentials::revoke_account_credentials
    pub async fn revoke_account_credentials(&self, input: crate::client::com::atproto::temp::revoke_account_credentials::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::com::atproto::temp::revoke_account_credentials::revoke_account_credentials(&*self.client, input).await
    }

}

impl ToolsOzoneCommunicationNS {
    /// Call crate::client::tools::ozone::communication::create_template::create_template
    pub async fn create_template(&self, input: crate::client::tools::ozone::communication::create_template::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::communication::create_template::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::communication::create_template::create_template(&*self.client, input).await
    }

    /// Call crate::client::tools::ozone::communication::delete_template::delete_template
    pub async fn delete_template(&self, input: crate::client::tools::ozone::communication::delete_template::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::communication::delete_template::delete_template(&*self.client, input).await
    }

    /// Call crate::client::tools::ozone::communication::list_templates::list_templates
    pub async fn list_templates(&self) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::communication::list_templates::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::communication::list_templates::list_templates(&*self.client).await
    }

    /// Call crate::client::tools::ozone::communication::update_template::update_template
    pub async fn update_template(&self, input: crate::client::tools::ozone::communication::update_template::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::communication::update_template::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::communication::update_template::update_template(&*self.client, input).await
    }

}

impl ToolsOzoneModerationNS {
    /// Call crate::client::tools::ozone::moderation::cancel_scheduled_actions::cancel_scheduled_actions
    pub async fn cancel_scheduled_actions(&self, input: crate::client::tools::ozone::moderation::cancel_scheduled_actions::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::moderation::cancel_scheduled_actions::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::moderation::cancel_scheduled_actions::cancel_scheduled_actions(&*self.client, input).await
    }

    /// Call crate::client::tools::ozone::moderation::emit_event::emit_event
    pub async fn emit_event(&self, input: crate::client::tools::ozone::moderation::emit_event::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::moderation::emit_event::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::moderation::emit_event::emit_event(&*self.client, input).await
    }

    /// Call crate::client::tools::ozone::moderation::get_account_timeline::get_account_timeline
    pub async fn get_account_timeline(&self, params: crate::client::tools::ozone::moderation::get_account_timeline::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::moderation::get_account_timeline::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::moderation::get_account_timeline::get_account_timeline(&*self.client, params).await
    }

    /// Call crate::client::tools::ozone::moderation::get_event::get_event
    pub async fn get_event(&self, params: crate::client::tools::ozone::moderation::get_event::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::moderation::get_event::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::moderation::get_event::get_event(&*self.client, params).await
    }

    /// Call crate::client::tools::ozone::moderation::get_record::get_record
    pub async fn get_record(&self, params: crate::client::tools::ozone::moderation::get_record::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::moderation::get_record::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::moderation::get_record::get_record(&*self.client, params).await
    }

    /// Call crate::client::tools::ozone::moderation::get_records::get_records
    pub async fn get_records(&self, params: crate::client::tools::ozone::moderation::get_records::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::moderation::get_records::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::moderation::get_records::get_records(&*self.client, params).await
    }

    /// Call crate::client::tools::ozone::moderation::get_repo::get_repo
    pub async fn get_repo(&self, params: crate::client::tools::ozone::moderation::get_repo::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::moderation::get_repo::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::moderation::get_repo::get_repo(&*self.client, params).await
    }

    /// Call crate::client::tools::ozone::moderation::get_reporter_stats::get_reporter_stats
    pub async fn get_reporter_stats(&self, params: crate::client::tools::ozone::moderation::get_reporter_stats::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::moderation::get_reporter_stats::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::moderation::get_reporter_stats::get_reporter_stats(&*self.client, params).await
    }

    /// Call crate::client::tools::ozone::moderation::get_repos::get_repos
    pub async fn get_repos(&self, params: crate::client::tools::ozone::moderation::get_repos::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::moderation::get_repos::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::moderation::get_repos::get_repos(&*self.client, params).await
    }

    /// Call crate::client::tools::ozone::moderation::get_subjects::get_subjects
    pub async fn get_subjects(&self, params: crate::client::tools::ozone::moderation::get_subjects::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::moderation::get_subjects::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::moderation::get_subjects::get_subjects(&*self.client, params).await
    }

    /// Call crate::client::tools::ozone::moderation::list_scheduled_actions::list_scheduled_actions
    pub async fn list_scheduled_actions(&self, input: crate::client::tools::ozone::moderation::list_scheduled_actions::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::moderation::list_scheduled_actions::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::moderation::list_scheduled_actions::list_scheduled_actions(&*self.client, input).await
    }

    /// Call crate::client::tools::ozone::moderation::query_events::query_events
    pub async fn query_events(&self, params: crate::client::tools::ozone::moderation::query_events::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::moderation::query_events::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::moderation::query_events::query_events(&*self.client, params).await
    }

    /// Call crate::client::tools::ozone::moderation::query_statuses::query_statuses
    pub async fn query_statuses(&self, params: crate::client::tools::ozone::moderation::query_statuses::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::moderation::query_statuses::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::moderation::query_statuses::query_statuses(&*self.client, params).await
    }

    /// Call crate::client::tools::ozone::moderation::schedule_action::schedule_action
    pub async fn schedule_action(&self, input: crate::client::tools::ozone::moderation::schedule_action::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::moderation::schedule_action::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::moderation::schedule_action::schedule_action(&*self.client, input).await
    }

    /// Call crate::client::tools::ozone::moderation::search_repos::search_repos
    pub async fn search_repos(&self, params: crate::client::tools::ozone::moderation::search_repos::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::moderation::search_repos::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::moderation::search_repos::search_repos(&*self.client, params).await
    }

}

impl ToolsOzoneServerNS {
    /// Call crate::client::tools::ozone::server::get_config::get_config
    pub async fn get_config(&self) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::server::get_config::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::server::get_config::get_config(&*self.client).await
    }

}

impl ToolsOzoneSetNS {
    /// Call crate::client::tools::ozone::set::add_values::add_values
    pub async fn add_values(&self, input: crate::client::tools::ozone::set::add_values::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::set::add_values::add_values(&*self.client, input).await
    }

    /// Call crate::client::tools::ozone::set::delete_set::delete_set
    pub async fn delete_set(&self, input: crate::client::tools::ozone::set::delete_set::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::set::delete_set::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::set::delete_set::delete_set(&*self.client, input).await
    }

    /// Call crate::client::tools::ozone::set::delete_values::delete_values
    pub async fn delete_values(&self, input: crate::client::tools::ozone::set::delete_values::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::set::delete_values::delete_values(&*self.client, input).await
    }

    /// Call crate::client::tools::ozone::set::get_values::get_values
    pub async fn get_values(&self, params: crate::client::tools::ozone::set::get_values::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::set::get_values::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::set::get_values::get_values(&*self.client, params).await
    }

    /// Call crate::client::tools::ozone::set::query_sets::query_sets
    pub async fn query_sets(&self, params: crate::client::tools::ozone::set::query_sets::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::set::query_sets::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::set::query_sets::query_sets(&*self.client, params).await
    }

    /// Call crate::client::tools::ozone::set::upsert_set::upsert_set
    pub async fn upsert_set(&self, input: crate::client::tools::ozone::set::upsert_set::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::set::upsert_set::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::set::upsert_set::upsert_set(&*self.client, input).await
    }

}

impl ToolsOzoneSettingNS {
    /// Call crate::client::tools::ozone::setting::list_options::list_options
    pub async fn list_options(&self, params: crate::client::tools::ozone::setting::list_options::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::setting::list_options::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::setting::list_options::list_options(&*self.client, params).await
    }

    /// Call crate::client::tools::ozone::setting::remove_options::remove_options
    pub async fn remove_options(&self, input: crate::client::tools::ozone::setting::remove_options::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::setting::remove_options::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::setting::remove_options::remove_options(&*self.client, input).await
    }

    /// Call crate::client::tools::ozone::setting::upsert_option::upsert_option
    pub async fn upsert_option(&self, input: crate::client::tools::ozone::setting::upsert_option::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::setting::upsert_option::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::setting::upsert_option::upsert_option(&*self.client, input).await
    }

}

impl ToolsOzoneSignatureNS {
    /// Call crate::client::tools::ozone::signature::find_correlation::find_correlation
    pub async fn find_correlation(&self, params: crate::client::tools::ozone::signature::find_correlation::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::signature::find_correlation::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::signature::find_correlation::find_correlation(&*self.client, params).await
    }

    /// Call crate::client::tools::ozone::signature::find_related_accounts::find_related_accounts
    pub async fn find_related_accounts(&self, params: crate::client::tools::ozone::signature::find_related_accounts::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::signature::find_related_accounts::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::signature::find_related_accounts::find_related_accounts(&*self.client, params).await
    }

    /// Call crate::client::tools::ozone::signature::search_accounts::search_accounts
    pub async fn search_accounts(&self, params: crate::client::tools::ozone::signature::search_accounts::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::signature::search_accounts::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::signature::search_accounts::search_accounts(&*self.client, params).await
    }

}

impl ToolsOzoneTeamNS {
    /// Call crate::client::tools::ozone::team::add_member::add_member
    pub async fn add_member(&self, input: crate::client::tools::ozone::team::add_member::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::team::add_member::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::team::add_member::add_member(&*self.client, input).await
    }

    /// Call crate::client::tools::ozone::team::delete_member::delete_member
    pub async fn delete_member(&self, input: crate::client::tools::ozone::team::delete_member::Input) -> Result<crate::xrpc::XrpcResponse<()>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::team::delete_member::delete_member(&*self.client, input).await
    }

    /// Call crate::client::tools::ozone::team::list_members::list_members
    pub async fn list_members(&self, params: crate::client::tools::ozone::team::list_members::QueryParams) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::team::list_members::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::team::list_members::list_members(&*self.client, params).await
    }

    /// Call crate::client::tools::ozone::team::update_member::update_member
    pub async fn update_member(&self, input: crate::client::tools::ozone::team::update_member::Input) -> Result<crate::xrpc::XrpcResponse<crate::client::tools::ozone::team::update_member::Output>, crate::xrpc::XrpcError> {
        crate::client::tools::ozone::team::update_member::update_member(&*self.client, input).await
    }

}

