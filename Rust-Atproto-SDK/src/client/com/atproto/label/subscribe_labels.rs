//! Generated code for com.atproto.label.subscribeLabels
//!
//! Subscribe to stream of labels (and negations). Public endpoint implemented by mod services. Uses same sequencing scheme as repo event stream.

use crate::xrpc::{XrpcRequest, XrpcError};
use crate::xrpc_subscription::{SubscriptionClient, SubscriptionEvent, SubscriptionResult};
use serde::{Deserialize, Serialize};
use futures::stream::Stream;
use std::pin::Pin;

/// Query parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryParams {
    /// The last known event seq number to backfill from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<i64>,
}

/// Subscribe to stream of labels (and negations). Public endpoint implemented by mod services. Uses same sequencing scheme as repo event stream.
pub async fn subscribe_labels(
    client: &SubscriptionClient,
    params: QueryParams,
) -> SubscriptionResult<Pin<Box<dyn Stream<Item = SubscriptionResult<SubscriptionEvent>> + Send>>> {
    let mut req = XrpcRequest::query("com.atproto.label.subscribeLabels");

    // Add query parameters
    let params_json = serde_json::to_value(&params)
        .map_err(XrpcError::Serialization)?;

    if let Some(obj) = params_json.as_object() {
        for (key, value) in obj {
            if let Some(s) = value.as_str() {
                req.params.insert(key.clone(), s.to_string());
            } else {
                req.params.insert(key.clone(), value.to_string());
            }
        }
    }

    client.subscribe(req).await
}
