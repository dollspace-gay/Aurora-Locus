/// HTTP server setup and routing
use crate::{
    api::middleware::check_account_moderation,
    context::AppContext,
    error::{PdsError, PdsResult},
    metrics,
    rate_limit::rate_limit_middleware,
};
use axum::{
    http::{header, Method, StatusCode},
    middleware,
    response::{IntoResponse, Json, Response},
    routing::get,
    Router,
};
use serde_json::json;
use tower_http::{
    compression::CompressionLayer,
    cors::{Any, CorsLayer},
    services::ServeDir,
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

    // Static file serving for admin panel
    let admin_static = Router::new()
        .nest_service("/admin", ServeDir::new("static/admin"));

    // Build router with middleware
    Router::new()
        // Metrics endpoint (no middleware)
        .route("/metrics", get(metrics_handler))
        // Server description endpoint
        .route("/xrpc/com.atproto.server.describeServer", get(describe_server))
        // Well-known endpoints will be added in Phase 6
        // .route("/.well-known/did.json", get(well_known_did))
        // .route("/.well-known/atproto-did", get(well_known_atproto_did))
        // API routes (Phase 2) - merge before with_state
        .merge(crate::api::routes())
        // Provide state - converts Router<AppContext> to Router<()>
        .with_state(ctx.clone())
        // Merge admin static files (after with_state so it doesn't need state)
        .merge(admin_static)
        // Apply moderation check middleware (checks if account is suspended/taken down)
        .layer(middleware::from_fn_with_state(ctx.clone(), check_account_moderation))
        // Apply rate limiting middleware (after state so it can access AppContext)
        .layer(middleware::from_fn_with_state(ctx, rate_limit_middleware))
        .layer(cors)
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .fallback(not_found)
}

/// Metrics handler - Returns Prometheus-formatted metrics
async fn metrics_handler() -> Response {
    let metrics_text = metrics::render_metrics();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")
        .body(metrics_text.into())
        .unwrap()
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
