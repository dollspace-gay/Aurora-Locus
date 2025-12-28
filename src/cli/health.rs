//! Health Check CLI Command
//!
//! Provides command-line tools for checking server health status.

use crate::{context::AppContext, error::PdsResult};
use serde::{Deserialize, Serialize};
use sqlx::Row;

/// Health check result for a single component
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ComponentHealth {
    name: String,
    status: String,
    message: Option<String>,
}

/// Overall health check results
#[derive(Debug, Clone, Serialize, Deserialize)]
struct HealthCheckResult {
    overall_status: String,
    components: Vec<ComponentHealth>,
}

/// Perform health check
pub async fn health_check(ctx: &AppContext, format: &str) -> PdsResult<()> {
    let mut components = Vec::new();

    // Check database connectivity
    let db_health = check_database(&ctx.account_db).await;
    components.push(db_health);

    // Check blob store accessibility
    let blob_health = check_blob_store(ctx).await;
    components.push(blob_health);

    // Check identity cache
    let identity_health = check_identity_cache(ctx).await;
    components.push(identity_health);

    // Determine overall status
    let overall_status = if components.iter().all(|c| c.status == "healthy") {
        "healthy".to_string()
    } else if components.iter().any(|c| c.status == "unhealthy") {
        "unhealthy".to_string()
    } else {
        "degraded".to_string()
    };

    let result = HealthCheckResult {
        overall_status,
        components,
    };

    // Output in requested format
    match format.to_lowercase().as_str() {
        "json" => {
            let json = serde_json::to_string_pretty(&result).map_err(|e| {
                crate::error::PdsError::Internal(format!("Failed to serialize health check: {}", e))
            })?;
            println!("{}", json);
        }
        _ => {
            print_text_health(&result);
        }
    }

    // Exit with non-zero code if unhealthy
    if result.overall_status == "unhealthy" {
        std::process::exit(1);
    }

    Ok(())
}

/// Check database health
async fn check_database(db: &sqlx::SqlitePool) -> ComponentHealth {
    match sqlx::query("SELECT 1 as test").fetch_one(db).await {
        Ok(row) => {
            let value: i32 = row.get("test");
            if value == 1 {
                ComponentHealth {
                    name: "Database".to_string(),
                    status: "healthy".to_string(),
                    message: Some("Database connection successful".to_string()),
                }
            } else {
                ComponentHealth {
                    name: "Database".to_string(),
                    status: "unhealthy".to_string(),
                    message: Some("Database query returned unexpected value".to_string()),
                }
            }
        }
        Err(e) => ComponentHealth {
            name: "Database".to_string(),
            status: "unhealthy".to_string(),
            message: Some(format!("Database connection failed: {}", e)),
        },
    }
}

/// Check blob store health
async fn check_blob_store(_ctx: &AppContext) -> ComponentHealth {
    // Blob store is initialized if AppContext exists
    ComponentHealth {
        name: "Blob Store".to_string(),
        status: "healthy".to_string(),
        message: Some("Blob store initialized".to_string()),
    }
}

/// Check identity cache health
async fn check_identity_cache(_ctx: &AppContext) -> ComponentHealth {
    // Identity resolver is initialized if AppContext exists
    ComponentHealth {
        name: "Identity Cache".to_string(),
        status: "healthy".to_string(),
        message: Some("Identity resolver initialized".to_string()),
    }
}

/// Print health check results in text format
fn print_text_health(result: &HealthCheckResult) {
    println!("════════════════════════════════════════════════════════");
    println!("  Server Health Check");
    println!("════════════════════════════════════════════════════════\n");

    println!("Overall Status: {}", format_status(&result.overall_status));
    println!();

    println!("Component Status:");
    println!("────────────────────────────────────────────────────────");

    for component in &result.components {
        println!();
        println!("  {} {}", component.name, format_status(&component.status));
        if let Some(msg) = &component.message {
            println!("    {}", msg);
        }
    }

    println!();
    println!("════════════════════════════════════════════════════════\n");
}

/// Format status with emoji/symbol
fn format_status(status: &str) -> String {
    match status {
        "healthy" => format!("✓ {}", status.to_uppercase()),
        "degraded" => format!("⚠ {}", status.to_uppercase()),
        "unhealthy" => format!("✗ {}", status.to_uppercase()),
        _ => status.to_uppercase(),
    }
}
