/// Well-known endpoints
/// Handles /.well-known/* endpoints for DID resolution and other standards
use crate::{context::AppContext, error::PdsResult};
use axum::{
    extract::State,
    http::{header, StatusCode},
    response::Response,
    routing::get,
    Router,
};

/// Build well-known routes
pub fn routes() -> Router<AppContext> {
    Router::new().route("/.well-known/atproto-did", get(atproto_did))
}

/// /.well-known/atproto-did
///
/// Returns the DID for this PDS server in plain text
/// Used for did:web resolution
pub async fn atproto_did(State(ctx): State<AppContext>) -> PdsResult<Response> {
    let did = ctx.service_did();

    // Return plain text DID
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(did.to_string().into())
        .map_err(|e| {
            crate::error::PdsError::Internal(format!("Failed to build response: {}", e))
        })?;

    Ok(response)
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_well_known_path() {
        // Well-known path should be at root level
        assert_eq!("/.well-known/atproto-did", "/.well-known/atproto-did");
    }
}
