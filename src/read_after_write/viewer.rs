//! LocalViewer for formatting local records into view objects

use crate::{
    account::AccountManager,
    actor_store::ActorStore,
    error::{PdsError, PdsResult},
};
use std::sync::Arc;

use super::types::{PostView, ProfileViewBasic, RecordDescript};

/// LocalViewer formats local records into view objects
///
/// This is used during read-after-write to format records that haven't
/// been indexed by AppView yet. It creates the same view structures
/// that AppView would return, but from local data.
pub struct LocalViewer {
    /// The DID of the user viewing their own records
    pub did: String,

    /// Account manager for getting handle/profile info
    pub account_manager: Arc<AccountManager>,

    /// Actor store for accessing record data
    pub actor_store: Arc<ActorStore>,

    /// Service URL for building image URLs
    pub service_url: String,

    /// AppView URL for fetching quote posts (optional)
    pub appview_url: Option<String>,
}

impl LocalViewer {
    /// Create a new LocalViewer for a specific user
    pub fn new(
        did: String,
        account_manager: Arc<AccountManager>,
        actor_store: Arc<ActorStore>,
        service_url: String,
    ) -> Self {
        Self {
            did,
            account_manager,
            actor_store,
            service_url,
            appview_url: None,
        }
    }

    /// Create a new LocalViewer with AppView URL for quote post resolution
    pub fn with_appview(
        did: String,
        account_manager: Arc<AccountManager>,
        actor_store: Arc<ActorStore>,
        service_url: String,
        appview_url: Option<String>,
    ) -> Self {
        Self {
            did,
            account_manager,
            actor_store,
            service_url,
            appview_url,
        }
    }

    /// Get the user's basic profile view
    pub async fn get_profile_basic(&self) -> PdsResult<Option<ProfileViewBasic>> {
        // Get account info for handle
        let account = self.account_manager.get_account(&self.did).await?;

        // Try to get profile record for display name and avatar
        let profile_record = self.get_profile_record().await?;

        let mut profile = ProfileViewBasic {
            did: self.did.clone(),
            handle: account.handle,
            display_name: None,
            avatar: None,
        };

        if let Some(record) = profile_record {
            if let Some(display_name) = record.get("displayName").and_then(|v| v.as_str()) {
                profile.display_name = Some(display_name.to_string());
            }

            if let Some(avatar) = record.get("avatar") {
                if let Some(ref_obj) = avatar.get("ref") {
                    if let Some(cid) = ref_obj.get("$link").and_then(|v| v.as_str()) {
                        profile.avatar = Some(self.get_image_url("avatar", cid));
                    }
                }
            }
        }

        Ok(Some(profile))
    }

    /// Format a post record into a PostView
    pub async fn get_post(&self, descript: &RecordDescript) -> PdsResult<Option<serde_json::Value>> {
        let author = match self.get_profile_basic().await? {
            Some(a) => a,
            None => return Ok(None),
        };

        // Format embed if present
        let embed = if let Some(embed_value) = descript.record.get("embed") {
            self.format_post_embed(embed_value)?
        } else {
            None
        };

        let post_view = PostView {
            uri: descript.uri.clone(),
            cid: descript.cid.clone(),
            author,
            record: descript.record.clone(),
            embed,
            indexed_at: descript.indexed_at.clone(),
            like_count: 0,    // Presumed 0 for new posts
            reply_count: 0,
            repost_count: 0,
            quote_count: 0,
        };

        Ok(Some(serde_json::to_value(post_view).map_err(|e| {
            PdsError::Internal(format!("Failed to serialize post view: {}", e))
        })?))
    }

    /// Fetch a post from AppView by URI
    async fn fetch_post_from_appview(&self, uri: &str) -> PdsResult<Option<serde_json::Value>> {
        let appview_url = match &self.appview_url {
            Some(url) => url,
            None => return Ok(None), // No AppView configured, can't fetch
        };

        // Parse AT-URI to extract collection and rkey
        // Format: at://did:plc:abc/app.bsky.feed.post/xyz
        let parts: Vec<&str> = uri.split('/').collect();
        if parts.len() < 5 {
            return Ok(None); // Invalid URI format
        }

        let did = &parts[2];
        let collection = &parts[3];
        let rkey = &parts[4];

        // Fetch from AppView using getRecord endpoint
        let url = format!(
            "{}/xrpc/app.bsky.feed.getPostThread?uri={}",
            appview_url,
            urlencoding::encode(uri)
        );

        let client = reqwest::Client::new();
        let response = match client.get(&url).send().await {
            Ok(resp) => resp,
            Err(_) => return Ok(None), // AppView unreachable, return None
        };

        if !response.status().is_success() {
            return Ok(None); // Post not found or error
        }

        let data: serde_json::Value = match response.json().await {
            Ok(json) => json,
            Err(_) => return Ok(None),
        };

        // Extract the post view from thread response
        Ok(data.get("thread").and_then(|t| t.get("post")).cloned())
    }

    /// Format a post embed (images, external links, quotes, etc.)
    fn format_post_embed(&self, embed: &serde_json::Value) -> PdsResult<Option<serde_json::Value>> {
        let embed_type = embed
            .get("$type")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        match embed_type {
            "app.bsky.embed.images" => self.format_images_embed(embed),
            "app.bsky.embed.external" => self.format_external_embed(embed),
            "app.bsky.embed.record" => {
                // Quote posts - fetch from AppView if available
                let uri = embed
                    .get("record")
                    .and_then(|r| r.get("uri"))
                    .and_then(|u| u.as_str())
                    .unwrap_or("");

                // Try to fetch from AppView (blocking call)
                let quoted_post = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(self.fetch_post_from_appview(uri))
                });

                match quoted_post {
                    Ok(Some(post)) => {
                        // Successfully fetched - return viewRecord
                        Ok(Some(serde_json::json!({
                            "$type": "app.bsky.embed.record#view",
                            "record": {
                                "$type": "app.bsky.embed.record#viewRecord",
                                "uri": uri,
                                "cid": post.get("cid"),
                                "author": post.get("author"),
                                "value": post.get("record"),
                                "indexedAt": post.get("indexedAt"),
                                "embeds": post.get("embed").map(|e| vec![e.clone()]),
                            }
                        })))
                    }
                    _ => {
                        // Fallback to viewNotFound
                        Ok(Some(serde_json::json!({
                            "$type": "app.bsky.embed.record#view",
                            "record": {
                                "$type": "app.bsky.embed.record#viewNotFound",
                                "uri": uri
                            }
                        })))
                    }
                }
            }
            "app.bsky.embed.recordWithMedia" => {
                // Complex embed - media + quoted record
                let media = embed.get("media");
                let formatted_media = if let Some(m) = media {
                    let media_type = m.get("$type").and_then(|v| v.as_str()).unwrap_or("");
                    match media_type {
                        "app.bsky.embed.images" => self.format_images_embed(m)?,
                        "app.bsky.embed.external" => self.format_external_embed(m)?,
                        _ => None,
                    }
                } else {
                    None
                };

                // Fetch quoted record
                let uri = embed
                    .get("record")
                    .and_then(|r| r.get("record"))
                    .and_then(|r| r.get("uri"))
                    .and_then(|u| u.as_str())
                    .unwrap_or("");

                let quoted_post = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current()
                        .block_on(self.fetch_post_from_appview(uri))
                });

                let record = match quoted_post {
                    Ok(Some(post)) => serde_json::json!({
                        "$type": "app.bsky.embed.record#viewRecord",
                        "uri": uri,
                        "cid": post.get("cid"),
                        "author": post.get("author"),
                        "value": post.get("record"),
                        "indexedAt": post.get("indexedAt"),
                        "embeds": post.get("embed").map(|e| vec![e.clone()]),
                    }),
                    _ => serde_json::json!({
                        "$type": "app.bsky.embed.record#viewNotFound",
                        "uri": uri
                    }),
                };

                Ok(Some(serde_json::json!({
                    "$type": "app.bsky.embed.recordWithMedia#view",
                    "media": formatted_media,
                    "record": record
                })))
            }
            _ => Ok(None),
        }
    }

    /// Format an images embed
    fn format_images_embed(&self, embed: &serde_json::Value) -> PdsResult<Option<serde_json::Value>> {
        let images = embed.get("images").and_then(|v| v.as_array());

        if let Some(img_array) = images {
            let formatted_images: Vec<serde_json::Value> = img_array
                .iter()
                .filter_map(|img| {
                    let image_ref = img.get("image")?.get("ref")?;
                    let cid = image_ref.get("$link")?.as_str()?;

                    Some(serde_json::json!({
                        "thumb": self.get_image_url("feed_thumbnail", cid),
                        "fullsize": self.get_image_url("feed_fullsize", cid),
                        "alt": img.get("alt").and_then(|v| v.as_str()).unwrap_or(""),
                        "aspectRatio": img.get("aspectRatio")
                    }))
                })
                .collect();

            Ok(Some(serde_json::json!({
                "$type": "app.bsky.embed.images#view",
                "images": formatted_images
            })))
        } else {
            Ok(None)
        }
    }

    /// Format an external link embed
    fn format_external_embed(&self, embed: &serde_json::Value) -> PdsResult<Option<serde_json::Value>> {
        // If any required field is missing, return None rather than an error
        let external = match embed.get("external") {
            Some(e) => e,
            None => return Ok(None),
        };

        let uri = match external.get("uri").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => return Ok(None),
        };

        let title = match external.get("title").and_then(|v| v.as_str()) {
            Some(t) => t,
            None => return Ok(None),
        };

        let description = match external.get("description").and_then(|v| v.as_str()) {
            Some(d) => d,
            None => return Ok(None),
        };

        let thumb = external
            .get("thumb")
            .and_then(|t| t.get("ref"))
            .and_then(|r| r.get("$link"))
            .and_then(|cid| cid.as_str())
            .map(|cid| self.get_image_url("feed_thumbnail", cid));

        Ok(Some(serde_json::json!({
            "$type": "app.bsky.embed.external#view",
            "external": {
                "uri": uri,
                "title": title,
                "description": description,
                "thumb": thumb
            }
        })))
    }

    /// Build an image URL for a blob CID
    fn get_image_url(&self, pattern: &str, cid: &str) -> String {
        // Pattern can be: avatar, banner, feed_thumbnail, feed_fullsize
        // Format: {service_url}/xrpc/com.atproto.sync.getBlob?did={did}&cid={cid}
        format!(
            "{}/xrpc/com.atproto.sync.getBlob?did={}&cid={}",
            self.service_url, self.did, cid
        )
    }

    /// Get the user's profile record (if exists)
    async fn get_profile_record(&self) -> PdsResult<Option<serde_json::Value>> {
        // Construct profile record URI
        let profile_uri = format!("at://{}/app.bsky.actor.profile/self", self.did);

        // Try to get the record from actor store
        let record = match self.actor_store.get_record(&self.did, &profile_uri).await {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(None),
            Err(_) => return Ok(None), // Ignore errors, profile might not exist
        };

        // Get the record value from the block store using the CID
        match self.actor_store.get_block(&self.did, &record.cid).await {
            Ok(Some(content)) => {
                // Decode CBOR to JSON
                let value: serde_json::Value = serde_cbor::from_slice(&content)
                    .map_err(|e| PdsError::Internal(format!("Failed to decode profile CBOR: {}", e)))?;
                Ok(Some(value))
            }
            Ok(None) => Ok(None),
            Err(_) => Ok(None),
        }
    }
}
