/// Aurora Locus - ATProto Personal Data Server
///
/// A Rust implementation of an ATProto PDS, providing personal data storage
/// and federation capabilities for the AT Protocol network.

mod account;
mod admin;
mod api;
mod auth;
mod backup;
mod blob_store;
mod cache;
mod car;
mod cli;
mod config;
mod context;
mod crypto;
mod db;
mod error;
mod federation;
mod identity;
mod jobs;
mod oauth;
mod mailer;
mod metrics;
mod rate_limit;
mod read_after_write;
mod actor_store;  // Must come after read_after_write (uses its types)
mod sequencer;
mod server;
mod validation;

use clap::Parser;
use cli::{Cli, Commands};
use config::ServerConfig;
use context::AppContext;
use error::PdsResult;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> PdsResult<()> {
    // Initialize logging with JSON support
    let log_format = std::env::var("LOG_FORMAT").unwrap_or_else(|_| "text".to_string());

    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "aurora_locus=info,tower_http=info".into());

    if log_format == "json" {
        // JSON logging for production
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().json())
            .init();
    } else {
        // Pretty text logging for development
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().pretty())
            .init();
    }

    // Parse CLI arguments
    let cli = Cli::parse();

    // Print banner
    print_banner();

    // Load configuration
    let config = ServerConfig::from_env()?;

    // Create application context
    let ctx = AppContext::new(config).await?;
    let ctx = std::sync::Arc::new(ctx);

    // Handle CLI commands
    match cli.command {
        Some(Commands::Serve) | None => {
            // Start background jobs
            let scheduler = std::sync::Arc::new(jobs::JobScheduler::new(Arc::clone(&ctx)));
            scheduler.start();

            // Start server
            server::serve((*ctx).clone()).await?;
        }

        Some(Commands::MigrateOauth { did, yes, dry_run }) => {
            if !yes && !dry_run {
                println!("WARNING: This will revoke all active JWT sessions for {}", did);
                println!("The user will need to re-authenticate using OAuth 2.1");
                print!("\nContinue? [y/N]: ");
                use std::io::{self, Write};
                io::stdout().flush().unwrap();

                let mut input = String::new();
                io::stdin().read_line(&mut input).unwrap();

                if !input.trim().eq_ignore_ascii_case("y") {
                    println!("Migration cancelled.");
                    return Ok(());
                }
            }

            let result = cli::migrate_oauth::migrate_user(&ctx, &did, dry_run).await?;
            cli::migrate_oauth::print_migration_result(&result);
        }

        Some(Commands::BulkMigrateOauth { yes, dry_run }) => {
            if !yes && !dry_run {
                println!("WARNING: This will revoke all active JWT sessions for ALL users");
                println!("All users will need to re-authenticate using OAuth 2.1");
                print!("\nContinue? [y/N]: ");
                use std::io::{self, Write};
                io::stdout().flush().unwrap();

                let mut input = String::new();
                io::stdin().read_line(&mut input).unwrap();

                if !input.trim().eq_ignore_ascii_case("y") {
                    println!("Bulk migration cancelled.");
                    return Ok(());
                }
            }

            let results = cli::migrate_oauth::bulk_migrate_users(&ctx, dry_run).await?;

            println!("\n════════════════════════════════════════════════════════");
            println!("  Bulk Migration Summary");
            println!("════════════════════════════════════════════════════════");
            println!("Total users processed: {}", results.len());
            println!("Successful migrations: {}", results.iter().filter(|r| r.success).count());
            println!("Skipped: {}", results.iter().filter(|r| !r.success).count());
            println!("════════════════════════════════════════════════════════\n");
        }
    }

    Ok(())
}

fn print_banner() {
    println!(
        r#"
    ___                                   __
   /   | __  ___________  _________ _   / /   ____  _______  _______
  / /| |/ / / / ___/ __ \/ ___/ __ `/  / /   / __ \/ ___/ / / / ___/
 / ___ / /_/ / /  / /_/ / /  / /_/ /  / /___/ /_/ / /__/ /_/ (__  )
/_/  |_\__,_/_/   \____/_/   \__,_/  /_____/\____/\___/\__,_/____/

        ATProto Personal Data Server v{}
        "#,
        env!("CARGO_PKG_VERSION")
    );
}
