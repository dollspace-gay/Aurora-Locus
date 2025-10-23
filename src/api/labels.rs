/// com.atproto.label.* endpoints
use crate::{
    admin::labels::Label,
    context::AppContext,
    error::PdsResult,
};
use axum::{
    extract::{Query, State},
    routing::get,
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Request parameters for queryLabels
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryLabelsParams {
    /// List of AT-URIs to query labels for
    pub uri_patterns: Vec<String>,
    /// Optional sources (DIDs) to filter by
    #[serde(default)]
    pub sources: Vec<String>,
    /// Optional limit (default: 50, max: 250)
    pub limit: Option<i64>,
    /// Optional cursor for pagination
    pub cursor: Option<String>,
}

/// Response for queryLabels
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryLabelsResponse {
    pub labels: Vec<LabelView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Label view for API responses
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabelView {
    /// AT-URI of the labeled content
    pub uri: String,
    /// Optional CID of the labeled content version
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<String>,
    /// Label value (e.g., "porn", "spam", "nsfw")
    pub val: String,
    /// Negation - if true, this label removes a previous label
    #[serde(skip_serializing_if = "is_false")]
    pub neg: bool,
    /// DID of the labeler
    pub src: String,
    /// Timestamp when label was created
    #[serde(rename = "cts")]
    pub created_at: String,
    /// Optional expiration timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp: Option<String>,
    /// Optional signature
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sig: Option<String>,
}

fn is_false(val: &bool) -> bool {
    !val
}

impl From<Label> for LabelView {
    fn from(label: Label) -> Self {
        Self {
            uri: label.uri,
            cid: label.cid,
            val: label.val,
            neg: label.neg,
            src: label.src,
            created_at: label.created_at.to_rfc3339(),
            exp: label.expires_at.map(|dt| dt.to_rfc3339()),
            sig: label.sig.map(|bytes| base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                bytes
            )),
        }
    }
}

/// Query labels for content
///
/// Implements com.atproto.label.queryLabels
pub async fn query_labels(
    State(ctx): State<AppContext>,
    Query(params): Query<QueryLabelsParams>,
) -> PdsResult<Json<QueryLabelsResponse>> {
    let limit = params.limit.unwrap_or(50).min(250);
    let mut all_labels = Vec::new();

    // Query labels for each URI pattern
    for uri_pattern in &params.uri_patterns {
        let labels = ctx.label_manager.get_labels(uri_pattern).await?;

        // Filter by sources if specified
        let filtered_labels: Vec<Label> = if params.sources.is_empty() {
            labels
        } else {
            labels.into_iter()
                .filter(|label| params.sources.contains(&label.src))
                .collect()
        };

        all_labels.extend(filtered_labels);
    }

    // Filter out expired labels
    let now = Utc::now();
    all_labels.retain(|label| {
        label.expires_at.map_or(true, |exp| exp > now)
    });

    // Sort by creation time (newest first)
    all_labels.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    // Apply limit and pagination
    let total = all_labels.len();
    let labels: Vec<LabelView> = all_labels
        .into_iter()
        .take(limit as usize)
        .map(LabelView::from)
        .collect();

    // Simple cursor implementation (could be improved with actual cursor logic)
    let cursor = if total > limit as usize {
        Some(format!("{}", limit))
    } else {
        None
    };

    Ok(Json(QueryLabelsResponse { labels, cursor }))
}

/// Build labels API routes
pub fn routes() -> Router<AppContext> {
    Router::new()
        .route("/xrpc/com.atproto.label.queryLabels", get(query_labels))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_label_view_serialization() {
        let label = Label {
            id: 1,
            uri: "at://did:plc:test/app.bsky.feed.post/123".to_string(),
            cid: Some("bafyreie5cvv4h45feadgeuwhbcutmh6t2ceseocckahdoe6uat64zmz454".to_string()),
            val: "porn".to_string(),
            neg: false,
            src: "did:plc:labeler".to_string(),
            created_at: Utc::now(),
            created_by: "did:plc:admin".to_string(),
            expires_at: None,
            sig: None,
        };

        let view: LabelView = label.into();
        let json = serde_json::to_string(&view).unwrap();

        assert!(json.contains("porn"));
        assert!(json.contains("at://did:plc:test"));
    }

    #[test]
    fn test_query_params_deserialization() {
        let json = r#"{"uriPatterns":["at://did:plc:test/*"],"sources":["did:plc:labeler"],"limit":100}"#;
        let params: QueryLabelsParams = serde_json::from_str(json).unwrap();

        assert_eq!(params.uri_patterns.len(), 1);
        assert_eq!(params.sources.len(), 1);
        assert_eq!(params.limit, Some(100));
    }
}
