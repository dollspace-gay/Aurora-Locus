/// Aurora Locus - ATProto Personal Data Server
///
/// A Rust implementation of an ATProto PDS, providing personal data storage
/// and federation capabilities for the AT Protocol network.

mod account;
mod actor_store;
mod admin;
mod api;
mod auth;
mod blob_store;
mod car;
mod config;
mod context;
mod db;
mod error;
mod identity;
mod jobs;
mod mailer;
mod rate_limit;
mod sequencer;
mod server;

use config::ServerConfig;
use context::AppContext;
use error::PdsResult;
use std::sync::Arc;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> PdsResult<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "aurora_locus=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Print banner
    print_banner();

    // Load configuration
    let config = ServerConfig::from_env()?;

    // Create application context
    let ctx = AppContext::new(config).await?;
    let ctx = std::sync::Arc::new(ctx);

    // Start background jobs
    let scheduler = std::sync::Arc::new(jobs::JobScheduler::new(Arc::clone(&ctx)));
    scheduler.start();

    // Start server
    server::serve((*ctx).clone()).await?;

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
