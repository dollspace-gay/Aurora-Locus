/// HTTP server setup and routing
use crate::{
    api::middleware::{check_account_moderation, namespace_scope_check},
    context::AppContext,
    error::{PdsError, PdsResult},
    metrics,
    rate_limit::rate_limit_middleware,
};
use axum::{
    extract::Request,
    http::{header, Method, StatusCode},
    middleware,
    middleware::Next,
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

    // Static file serving for admin panel.
    // Must come AFTER API routes to not conflict with /oauth/admin/* endpoints.
    //
    // /admin/debug.html is a developer affordance that renders the
    // bearer token from localStorage as visible page text. It must
    // not be reachable in production deployments — anyone with screen
    // access (shoulder-surfing, screen-share, malicious browser
    // extension reading DOM) would harvest the bearer token directly.
    // The opt-in PDS_ENABLE_DEBUG_PAGES env var keeps the page
    // available locally for development while 404'ing it everywhere
    // else. Default off; set to "true" or "1" to enable.
    let debug_pages_enabled = std::env::var("PDS_ENABLE_DEBUG_PAGES")
        .ok()
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);
    let admin_static = Router::new()
        .nest_service("/admin", ServeDir::new("static/admin"))
        .layer(middleware::from_fn(move |req: Request, next: Next| {
            // Capture into the async closure by value — bool is Copy
            // so this is a zero-cost copy per request.
            let enabled = debug_pages_enabled;
            async move {
                if !enabled && req.uri().path() == "/admin/debug.html" {
                    return (StatusCode::NOT_FOUND, "Not found").into_response();
                }
                next.run(req).await
            }
        }));

    // Build router with middleware
    Router::new()
        // Metrics endpoint (no middleware)
        .route("/metrics", get(metrics_handler))
        // API routes (Phase 2) - merge before with_state
        // Note: describeServer route is registered in api/server.rs
        .merge(crate::api::routes())
        // Provide state - converts Router<AppContext> to Router<()>
        .with_state(ctx.clone())
        // Merge admin static files (after with_state so it doesn't need state)
        .merge(admin_static)
        // Apply moderation check middleware (checks if account is suspended/taken down)
        .layer(middleware::from_fn_with_state(
            ctx.clone(),
            check_account_moderation,
        ))
        // Apply namespace scope-check middleware. No-op for non-admin paths
        // and session-token requests; enforces atproto:admin.* scope on
        // OAuth-authenticated requests to admin namespaces (chainlink #84).
        .layer(middleware::from_fn_with_state(
            ctx.clone(),
            namespace_scope_check,
        ))
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
        .header(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )
        .body(metrics_text.into())
        .unwrap()
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
    // Bind to 0.0.0.0 to listen on all interfaces (IPv4 and IPv6)
    let bind_addr = format!("0.0.0.0:{}", ctx.config.service.port);

    info!("🚀 Aurora Locus PDS listening on {}", bind_addr);
    info!("   Service DID: {}", ctx.service_did());
    info!("   Service URL: {}", ctx.service_url());

    let app = build_router(ctx);

    // Create TCP listener
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .map_err(|e| PdsError::Internal(format!("Failed to bind to {}: {}", bind_addr, e)))?;

    // Axum 0.7: Router<()> can be passed directly to serve
    axum::serve(listener, app)
        .await
        .map_err(|e| PdsError::Internal(format!("Server error: {}", e)))?;

    Ok(())
}
