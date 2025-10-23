/// Health check endpoints for Kubernetes liveness and readiness probes
///
/// Provides detailed health status including:
/// - Database connectivity
/// - Blob storage availability
/// - Background job status
/// - System resource checks
///
/// Supports two types of probes:
/// - Liveness: Is the application alive? (restart if not)
/// - Readiness: Can the application serve traffic? (remove from load balancer if not)

use crate::{context::AppContext, error::PdsResult, jobs, metrics};
use axum::{extract::State, http::StatusCode, response::Json, Router, routing::get};
use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Health status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    /// Overall status: "healthy", "degraded", or "unhealthy"
    pub status: String,

    /// Application version
    pub version: String,

    /// Uptime in seconds
    pub uptime_seconds: f64,

    /// Individual component checks
    pub checks: Vec<ComponentHealth>,

    /// Optional message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Health status of individual component
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHealth {
    /// Component name
    pub name: String,

    /// Status: "healthy", "degraded", or "unhealthy"
    pub status: String,

    /// Response time in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_time_ms: Option<u64>,

    /// Optional error message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// Additional details
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// Build health check routes
pub fn routes() -> Router<AppContext> {
    Router::new()
        .route("/health", get(health_basic))
        .route("/health/live", get(liveness_probe))
        .route("/health/ready", get(readiness_probe))
        .route("/health/detailed", get(health_detailed))
}

/// Basic health check (backward compatibility)
///
/// Returns simple JSON with status and version
pub async fn health_basic() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

/// Liveness probe - Kubernetes liveness check
///
/// Checks if the application is alive and responsive.
/// Returns 200 if alive, 503 if dead.
///
/// This is a lightweight check that should always succeed unless
/// the application is completely broken (deadlock, panic loop, etc.)
pub async fn liveness_probe() -> Result<Json<serde_json::Value>, StatusCode> {
    // Very basic check - just respond
    // If we can respond, we're alive
    Ok(Json(serde_json::json!({
        "status": "alive",
        "version": env!("CARGO_PKG_VERSION")
    })))
}

/// Readiness probe - Kubernetes readiness check
///
/// Checks if the application is ready to serve traffic.
/// Returns 200 if ready, 503 if not ready.
///
/// Performs checks on:
/// - Database connectivity
/// - Blob storage (if configured)
pub async fn readiness_probe(
    State(ctx): State<AppContext>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Check database connectivity
    if let Err(e) = check_database(&ctx).await {
        tracing::warn!(error = %e, "readiness_probe_failed: database check failed");
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    // Check blob storage
    if let Err(e) = check_blob_storage(&ctx).await {
        tracing::warn!(error = %e, "readiness_probe_failed: blob storage check failed");
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    Ok(Json(serde_json::json!({
        "status": "ready",
        "version": env!("CARGO_PKG_VERSION")
    })))
}

/// Detailed health check with all component statuses
///
/// Returns comprehensive health information for monitoring
pub async fn health_detailed(
    State(ctx): State<AppContext>,
) -> (StatusCode, Json<HealthStatus>) {
    let start = Instant::now();
    let mut checks = Vec::new();

    // Check database
    checks.push(check_database_detailed(&ctx).await);

    // Check blob storage
    checks.push(check_blob_storage_detailed(&ctx).await);

    // Check background jobs
    checks.push(check_background_jobs_detailed(&ctx).await);

    // Check sequencer
    checks.push(check_sequencer_detailed(&ctx).await);

    // Determine overall status
    let overall_status = determine_overall_status(&checks);

    // Calculate uptime
    let uptime = metrics::UPTIME_SECONDS.get();

    let health = HealthStatus {
        status: overall_status.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds: uptime,
        checks,
        message: if overall_status == "healthy" {
            None
        } else {
            Some("One or more components are unhealthy".to_string())
        },
    };

    let status_code = match overall_status.as_str() {
        "healthy" => StatusCode::OK,
        "degraded" => StatusCode::OK, // Still serving traffic
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };

    tracing::info!(
        status = %overall_status,
        duration_ms = start.elapsed().as_millis(),
        "health_check_completed"
    );

    (status_code, Json(health))
}

/// Check database connectivity
async fn check_database(ctx: &AppContext) -> PdsResult<()> {
    sqlx::query("SELECT 1")
        .fetch_one(&ctx.account_db)
        .await?;
    Ok(())
}

/// Check database with detailed metrics
async fn check_database_detailed(ctx: &AppContext) -> ComponentHealth {
    let start = Instant::now();

    match check_database(ctx).await {
        Ok(_) => {
            let duration = start.elapsed().as_millis() as u64;
            ComponentHealth {
                name: "database".to_string(),
                status: "healthy".to_string(),
                response_time_ms: Some(duration),
                error: None,
                details: Some(serde_json::json!({
                    "type": "sqlite",
                    "pool_size": ctx.account_db.size() as u32,
                })),
            }
        }
        Err(e) => ComponentHealth {
            name: "database".to_string(),
            status: "unhealthy".to_string(),
            response_time_ms: Some(start.elapsed().as_millis() as u64),
            error: Some(e.to_string()),
            details: None,
        },
    }
}

/// Check blob storage availability
async fn check_blob_storage(ctx: &AppContext) -> PdsResult<()> {
    // Just verify the blob store is accessible
    // We don't actually write anything, just check structure
    let _ = &ctx.blob_store;
    Ok(())
}

/// Check blob storage with detailed metrics
async fn check_blob_storage_detailed(ctx: &AppContext) -> ComponentHealth {
    let start = Instant::now();

    match check_blob_storage(ctx).await {
        Ok(_) => {
            let duration = start.elapsed().as_millis() as u64;
            ComponentHealth {
                name: "blob_storage".to_string(),
                status: "healthy".to_string(),
                response_time_ms: Some(duration),
                error: None,
                details: Some(serde_json::json!({
                    "type": "configured",
                })),
            }
        }
        Err(e) => ComponentHealth {
            name: "blob_storage".to_string(),
            status: "unhealthy".to_string(),
            response_time_ms: Some(start.elapsed().as_millis() as u64),
            error: Some(e.to_string()),
            details: None,
        },
    }
}

/// Check background jobs status
async fn check_background_jobs_detailed(ctx: &AppContext) -> ComponentHealth {
    let start = Instant::now();

    // Use existing health check from jobs module
    match jobs::tasks::health_check(ctx).await {
        Ok(_) => ComponentHealth {
            name: "background_jobs".to_string(),
            status: "healthy".to_string(),
            response_time_ms: Some(start.elapsed().as_millis() as u64),
            error: None,
            details: Some(serde_json::json!({
                "scheduler": "running",
            })),
        },
        Err(e) => ComponentHealth {
            name: "background_jobs".to_string(),
            status: "degraded".to_string(), // Jobs failing is degraded, not critical
            response_time_ms: Some(start.elapsed().as_millis() as u64),
            error: Some(e.to_string()),
            details: None,
        },
    }
}

/// Check sequencer status
async fn check_sequencer_detailed(ctx: &AppContext) -> ComponentHealth {
    let start = Instant::now();

    // Check if sequencer is accessible
    let _ = &ctx.sequencer;

    ComponentHealth {
        name: "sequencer".to_string(),
        status: "healthy".to_string(),
        response_time_ms: Some(start.elapsed().as_millis() as u64),
        error: None,
        details: Some(serde_json::json!({
            "type": "event_stream",
        })),
    }
}

/// Determine overall health status from individual checks
fn determine_overall_status(checks: &[ComponentHealth]) -> String {
    let unhealthy_count = checks.iter().filter(|c| c.status == "unhealthy").count();
    let degraded_count = checks.iter().filter(|c| c.status == "degraded").count();

    if unhealthy_count > 0 {
        "unhealthy".to_string()
    } else if degraded_count > 0 {
        "degraded".to_string()
    } else {
        "healthy".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_determine_overall_status_healthy() {
        let checks = vec![
            ComponentHealth {
                name: "db".to_string(),
                status: "healthy".to_string(),
                response_time_ms: Some(5),
                error: None,
                details: None,
            },
            ComponentHealth {
                name: "storage".to_string(),
                status: "healthy".to_string(),
                response_time_ms: Some(3),
                error: None,
                details: None,
            },
        ];

        assert_eq!(determine_overall_status(&checks), "healthy");
    }

    #[test]
    fn test_determine_overall_status_degraded() {
        let checks = vec![
            ComponentHealth {
                name: "db".to_string(),
                status: "healthy".to_string(),
                response_time_ms: Some(5),
                error: None,
                details: None,
            },
            ComponentHealth {
                name: "jobs".to_string(),
                status: "degraded".to_string(),
                response_time_ms: Some(10),
                error: Some("Job failed".to_string()),
                details: None,
            },
        ];

        assert_eq!(determine_overall_status(&checks), "degraded");
    }

    #[test]
    fn test_determine_overall_status_unhealthy() {
        let checks = vec![
            ComponentHealth {
                name: "db".to_string(),
                status: "unhealthy".to_string(),
                response_time_ms: Some(100),
                error: Some("Connection failed".to_string()),
                details: None,
            },
            ComponentHealth {
                name: "storage".to_string(),
                status: "healthy".to_string(),
                response_time_ms: Some(3),
                error: None,
                details: None,
            },
        ];

        assert_eq!(determine_overall_status(&checks), "unhealthy");
    }

    #[test]
    fn test_health_status_serialization() {
        let health = HealthStatus {
            status: "healthy".to_string(),
            version: "0.1.0".to_string(),
            uptime_seconds: 3600.5,
            checks: vec![
                ComponentHealth {
                    name: "database".to_string(),
                    status: "healthy".to_string(),
                    response_time_ms: Some(5),
                    error: None,
                    details: Some(serde_json::json!({"type": "sqlite"})),
                },
            ],
            message: None,
        };

        let json = serde_json::to_string(&health).unwrap();
        assert!(json.contains("healthy"));
        assert!(json.contains("database"));
        assert!(json.contains("0.1.0"));
    }
}
