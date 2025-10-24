//! Generated code for com.atproto.sync.subscribeRepos
//!
//! Repository event stream, aka Firehose endpoint. Outputs repo commits with diff data, and identity update events, for all repositories on the current server. See the atproto specifications for details around stream sequencing, repo versioning, CAR diff format, and more. Public and does not require auth; implemented by PDS and Relay.

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

/// Repository event stream, aka Firehose endpoint. Outputs repo commits with diff data, and identity update events, for all repositories on the current server. See the atproto specifications for details around stream sequencing, repo versioning, CAR diff format, and more. Public and does not require auth; implemented by PDS and Relay.
pub async fn subscribe_repos(
    client: &SubscriptionClient,
    params: QueryParams,
) -> SubscriptionResult<Pin<Box<dyn Stream<Item = SubscriptionResult<SubscriptionEvent>> + Send>>> {
    let mut req = XrpcRequest::query("com.atproto.sync.subscribeRepos");

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
