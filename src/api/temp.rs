/// com.atproto.temp.* endpoints
///
/// Temporary/experimental ATProto endpoints. These are subject to change
/// and may be deprecated in future versions of the protocol.
use crate::{
    auth::AuthContext,
    context::AppContext,
};
use axum::{
    extract::State,
    http::StatusCode,
    routing::get,
    Json, Router,
};
use serde::Serialize;

/// Build temp routes
pub fn routes() -> Router<AppContext> {
    Router::new()
        .route("/xrpc/com.atproto.temp.checkSignupQueue", get(check_signup_queue))
}

// ============================================================================
// Request/Response Types
// ============================================================================

/// Response for checkSignupQueue
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CheckSignupQueueResponse {
    /// Whether the account has been activated (approved through queue)
    activated: bool,
    /// Position in the signup queue (if not yet activated)
    #[serde(skip_serializing_if = "Option::is_none")]
    place_in_queue: Option<i64>,
    /// Estimated time until activation in milliseconds (if not yet activated)
    #[serde(skip_serializing_if = "Option::is_none")]
    estimated_time_ms: Option<i64>,
}

// ============================================================================
// Endpoint Handlers
// ============================================================================

/// Check the current signup queue status
///
/// Returns whether the authenticated user's account has been activated
/// (approved through the signup queue) or is still waiting.
///
/// For PDSes without a signup queue, this always returns activated=true.
async fn check_signup_queue(
    State(ctx): State<AppContext>,
    auth: AuthContext,
) -> Result<Json<CheckSignupQueueResponse>, (StatusCode, String)> {
    // Get the account to check activation status
    let account = ctx.account_manager
        .get_account(&auth.did)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, format!("Account not found: {}", e)))?;

    // Check if account is deactivated (in queue)
    // Accounts are considered "in queue" if they have a deactivated_at timestamp
    // and no takedown_ref (takedowns are different from queue status)
    let in_queue = account.deactivated_at.is_some() && account.takedown_ref.is_none();

    if in_queue {
        // Account is still in the signup queue
        // Get queue position by counting how many deactivated accounts were created before this one
        let place_in_queue = ctx.account_manager
            .get_signup_queue_position(&auth.did)
            .await
            .ok();

        // Estimate time based on position (rough estimate: 1 hour per position)
        let estimated_time_ms = place_in_queue.map(|pos| pos * 3600 * 1000);

        Ok(Json(CheckSignupQueueResponse {
            activated: false,
            place_in_queue,
            estimated_time_ms,
        }))
    } else {
        // Account is activated (not in queue or already approved)
        Ok(Json(CheckSignupQueueResponse {
            activated: true,
            place_in_queue: None,
            estimated_time_ms: None,
        }))
    }
}
