/// HTTP server setup and routing
use crate::{
    api::middleware::{
        atproto_oauth_gate, check_account_moderation, federation_enabled_gate,
        jwt_deprecation_headers, namespace_scope_check,
    },
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

/// Build the main application router.
///
/// `api_router` is the pre-built `Router<AppContext>` returned by
/// `crate::api::routes()`. Arc 8 Step 2 (chainlink #54): the
/// api routes are constructed before `AppContext::new` so the
/// `RouteRegistry` produced by `aurora_route_builder()` is
/// available to thread into the context. Splitting the router
/// construction out of this function keeps that ordering
/// explicit at the `main.rs` callsite.
///
/// Returns `Router<()>` because state is already provided.
pub fn build_router(ctx: AppContext, api_router: Router<AppContext>) -> Router {
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
        .merge(api_router)
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
        // Arc 2 ε.3 — atproto-OAuth bearer gate (registry-gated). Resolves a
        // `DPoP`-scheme bearer to a DID (validate + DPoP proof + registered
        // device) and stamps the trusted internal header the fn-based auth
        // resolvers (require_auth / require_auth_unified) read. Layered OUTER of
        // the scope-check + moderation layers (and the handlers), so the
        // resolved DID is set before any of them run. Strips the inbound
        // internal header unconditionally — spoof defense.
        .layer(middleware::from_fn_with_state(ctx.clone(), atproto_oauth_gate))
        // v0.9 Federation runtime-mutability arc §3.7 (#395) — request-layer
        // short-circuit: 503 the inbound federation operational endpoints when
        // federation.enabled resolves false (incident response, effective before
        // restart). No-op (path check only) for every other route.
        .layer(middleware::from_fn_with_state(ctx.clone(), federation_enabled_gate))
        // Apply rate limiting middleware (after state so it can access AppContext)
        .layer(middleware::from_fn_with_state(ctx.clone(), rate_limit_middleware))
        // JWT-deprecation observability per Arc 6 Step 8: emits
        // Deprecation/Sunset/Warning/X-OAuth-Migration-Guide
        // response headers + increments the
        // jwt_deprecation_warnings_total counter when the request
        // carries a JWT-shaped bearer token (detected
        // structurally — see token_looks_like_jwt). Outermost of
        // the per-request layers so the headers reach the client
        // verbatim.
        .layer(middleware::from_fn_with_state(ctx, jwt_deprecation_headers))
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

/// v0.9 Federation runtime-mutability arc §3.1 (#390) — graceful-shutdown drain
/// deadline. After the shutdown signal fires, in-flight connections get this
/// long to close before the watchdog force-exits the process. Bounds the wait
/// the long-lived `subscribeRepos` WebSockets (which never close on their own)
/// would otherwise make unbounded.
const SHUTDOWN_DRAIN_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// Post-signal drain watchdog (§3.1, R3-verified). Blocks until the shutdown
/// signal fires, THEN enforces the drain deadline; returns once the deadline
/// elapses *after* the signal, at which point the caller force-exits.
///
/// Critically it does NOT arm the deadline until the signal fires: a freshly
/// `subscribe()`d `watch` receiver treats the channel's current value as already
/// seen, so `changed()` waits for the *next* `send`. In normal operation the
/// signal never fires and this future stays parked for the process lifetime,
/// doing nothing — so the watchdog never causes a boot-time exit (the R1.1
/// defect R2 caught and R2.1 corrected).
async fn shutdown_drain_watchdog(
    mut rx: tokio::sync::watch::Receiver<()>,
    deadline: std::time::Duration,
) {
    // Block until the shutdown signal. May never resolve under normal operation.
    let _ = rx.changed().await;
    // Signal fired — arm the deadline.
    tokio::time::sleep(deadline).await;
}

/// Start the HTTP server.
///
/// `api_router` is the pre-built `Router<AppContext>` produced by
/// `crate::api::routes()`; see [`build_router`] for why this
/// construction is hoisted out of the context flow.
pub async fn serve(ctx: AppContext, api_router: Router<AppContext>) -> PdsResult<()> {
    // Bind to 0.0.0.0 to listen on all interfaces (IPv4 and IPv6)
    let bind_addr = format!("0.0.0.0:{}", ctx.config.service.port);

    info!("🚀 Aurora Locus PDS listening on {}", bind_addr);
    info!("   Service DID: {}", ctx.service_did());
    info!("   Service URL: {}", ctx.service_url());

    // Acquire the PDS-liveness lock before binding the listener.
    // `_liveness_lock` lives for the entire `serve` scope; dropping
    // on return releases the lock (Postgres session close or kernel
    // flock release). The forthcoming `grant-admin` CLI (Step 4)
    // probes this same lock to fast-fail when a PDS is running. See
    // `src/db/liveness_lock.rs`.
    let _liveness_lock = crate::db::liveness_lock::LivenessLock::acquire(&ctx.config).await?;

    // v0.9 Federation runtime-mutability arc §3.1 (#390) — graceful-shutdown
    // wiring. Subscribe receivers from the context's shutdown trigger BEFORE
    // `build_router` moves `ctx` into router state. The save-and-restart
    // handlers landing in the D-phase fire `ctx.shutdown_trigger.send(())`;
    // `with_graceful_shutdown` then drains in-flight connections, and the
    // watchdog force-exits if the drain exceeds the deadline (the long-lived
    // `subscribeRepos` WebSockets would otherwise hang the drain indefinitely).
    let mut serve_shutdown_rx = ctx.shutdown_trigger.subscribe();
    let watchdog_rx = ctx.shutdown_trigger.subscribe();
    tokio::spawn(async move {
        shutdown_drain_watchdog(watchdog_rx, SHUTDOWN_DRAIN_DEADLINE).await;
        tracing::warn!(
            "graceful-shutdown drain exceeded {:?}; force-exiting process",
            SHUTDOWN_DRAIN_DEADLINE
        );
        std::process::exit(0);
    });

    let app = build_router(ctx, api_router);

    // Create TCP listener
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .map_err(|e| PdsError::Internal(format!("Failed to bind to {}: {}", bind_addr, e)))?;

    // Axum 0.7: Router<()> can be passed directly to serve. The graceful-
    // shutdown future is a real `async` block whose `changed().await` is polled
    // when the future runs (not pre-awaited), per §3.1 LB-1.
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = serve_shutdown_rx.changed().await;
        })
        .await
        .map_err(|e| PdsError::Internal(format!("Server error: {}", e)))?;

    // serve(...) returned: the shutdown signal fired and the drain completed
    // within the deadline (otherwise the watchdog above already force-exited).
    // `_liveness_lock` drops here, releasing the lock, then `main` returns and
    // the process exits naturally.
    Ok(())
}

#[cfg(test)]
mod shutdown_wiring_tests {
    //! v0.9 Federation runtime-mutability arc §3.1 (#390) — the watchdog's two
    //! R3-critical properties. Time is paused so the drain deadline advances
    //! deterministically without real sleeping. These tests exercise
    //! `shutdown_drain_watchdog` directly; the `std::process::exit(0)` that
    //! follows it in `serve` is left to integration / manual verification.
    use super::shutdown_drain_watchdog;
    use std::time::Duration;

    /// Regression for the R1.1 boot-exit defect: with no signal ever sent, the
    /// watchdog must stay parked on `changed()` — it must never reach the drain
    /// sleep, even after virtual time advances far past the deadline.
    #[tokio::test(start_paused = true)]
    async fn watchdog_does_not_arm_without_a_signal() {
        let (tx, _) = tokio::sync::watch::channel(());
        let rx = tx.subscribe();
        let res = tokio::time::timeout(
            Duration::from_secs(60),
            shutdown_drain_watchdog(rx, Duration::from_secs(10)),
        )
        .await;
        assert!(
            res.is_err(),
            "watchdog armed without a shutdown signal (boot-time-exit hazard)"
        );
        // Keep the sender alive until here so `changed()` never observes a close.
        drop(tx);
    }

    /// Positive path: once the signal fires, the watchdog arms the deadline and
    /// completes when it elapses (the caller would then force-exit).
    #[tokio::test(start_paused = true)]
    async fn watchdog_arms_and_completes_after_signal() {
        let (tx, _) = tokio::sync::watch::channel(());
        let rx = tx.subscribe();
        // Fire the shutdown signal: a send landing after `subscribe` makes the
        // receiver's `changed()` resolve, arming the drain deadline.
        tx.send(()).expect("receiver is alive");
        let res = tokio::time::timeout(
            Duration::from_secs(30),
            shutdown_drain_watchdog(rx, Duration::from_secs(10)),
        )
        .await;
        assert!(
            res.is_ok(),
            "watchdog should complete within the drain deadline after a signal"
        );
    }
}
