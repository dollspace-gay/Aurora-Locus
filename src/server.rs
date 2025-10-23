/// HTTP server setup and routing
use crate::{
    api::middleware::check_account_moderation,
    context::AppContext,
    error::{PdsError, PdsResult},
    rate_limit::rate_limit_middleware,
};
use axum::{
    http::{header, Method, StatusCode},
    middleware,
    response::Json,
    routing::get,
    Router,
};
use serde_json::json;
use tower_http::{
    compression::CompressionLayer,
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};
use tracing::info;

/// Build the main application router
/// Returns Router<()> because state is already provided
pub fn build_router(ctx: AppContext) -> Router {
    // Create CORS layer
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]);

    // Build router with middleware
    Router::new()
        // Health check endpoint (no middleware)
        .route("/health", get(health_check))
        // Server description endpoint
        .route("/xrpc/com.atproto.server.describeServer", get(describe_server))
        // Well-known endpoints will be added in Phase 6
        // .route("/.well-known/did.json", get(well_known_did))
        // .route("/.well-known/atproto-did", get(well_known_atproto_did))
        // API routes (Phase 2) - merge before with_state
        .merge(crate::api::routes())
        // Provide state - converts Router<AppContext> to Router<()>
        .with_state(ctx.clone())
        // Apply moderation check middleware (checks if account is suspended/taken down)
        .layer(middleware::from_fn_with_state(ctx.clone(), check_account_moderation))
        // Apply rate limiting middleware (after state so it can access AppContext)
        .layer(middleware::from_fn_with_state(ctx, rate_limit_middleware))
        .layer(cors)
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .fallback(not_found)
}

/// Health check handler
async fn health_check() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

/// Server description handler (com.atproto.server.describeServer)
async fn describe_server(
    axum::extract::State(ctx): axum::extract::State<AppContext>,
) -> Json<serde_json::Value> {
    Json(json!({
        "did": ctx.service_did(),
        "availableUserDomains": ctx.config.identity.service_handle_domains,
        "inviteCodeRequired": ctx.config.invites.required,
        "links": {
            "privacyPolicy": null,
            "termsOfService": null
        }
    }))
}

/// 404 handler
async fn not_found() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "error": "NotFound",
            "message": "Endpoint not found"
        })),
    )
}

/// Start the HTTP server
pub async fn serve(ctx: AppContext) -> PdsResult<()> {
    let addr = format!("{}:{}", ctx.config.service.hostname, ctx.config.service.port);

    info!("🚀 Aurora Locus PDS listening on {}", addr);
    info!("   Service DID: {}", ctx.service_did());
    info!("   Service URL: {}", ctx.service_url());

    let app = build_router(ctx);

    // Create TCP listener
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| PdsError::Internal(format!("Failed to bind to {}: {}", addr, e)))?;

    // Axum 0.7: Router<()> can be passed directly to serve
    axum::serve(listener, app)
        .await
        .map_err(|e| PdsError::Internal(format!("Server error: {}", e)))?;

    Ok(())
}
