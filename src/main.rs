#![recursion_limit = "512"]

//! Aurora Locus - ATProto Personal Data Server
//!
//! A Rust implementation of an ATProto PDS, providing personal data storage
//! and federation capabilities for the AT Protocol network.

mod account;
mod actor_store; // Must come after read_after_write (uses its types)
mod admin;
mod api;
mod auth;
mod backup;
mod blob_store;
mod cache;
mod cascade;
mod cli;
mod config;
mod context;
mod crypto;
mod db;
mod distributed;
mod error;
mod federation;
mod holder_signing;
mod identity;
mod jobs;
mod kryphocron;
mod kryphocron_audit;
mod kryphocron_content;
mod kryphocron_oracle_activity;
mod kryphocron_override;
mod kryphocron_policy;
mod kryphocron_rewrite;
mod kryphocron_rotation;
mod mailer;
mod metrics;
mod oauth;
mod oauth_client;
mod rate_limit;
mod read_after_write;
mod rebuild;
mod repo_scan;
mod repository;
mod sequencer;
mod sequencer_recovery;
mod server;
mod service_auth;
mod themes;
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

    // Arc 8 Step 2 (chainlink #54): build the API router +
    // capability registry first so `AppContext::new` can park
    // the populated `Arc<RouteRegistry>` in the context the
    // request handlers see. The router itself is consumed by
    // `server::serve` further down; CLI commands that don't
    // start the server still construct it (cheap — no I/O) so
    // the context always has a coherent registry.
    let (api_router, route_registry) = api::routes();

    // Create application context
    let ctx = AppContext::new(config, route_registry).await?;
    let ctx = std::sync::Arc::new(ctx);

    // Handle CLI commands
    match cli.command {
        Some(Commands::Serve) | None => {
            // §5.5.4 Phase E (#349) — startup-trigger lexicon migration (§6.7):
            // runs after AppContext::new (audit-chain ready) and before serving
            // so any migrated category-config is visible to the first request.
            // Best-effort; never blocks boot.
            api::lexicon_migration::run_lexicon_migration(&ctx).await;

            // v0.9 Federation Pattern-1 Phase B (#352 / design §2.4, Step 1) —
            // boot-seed federation.policy.peer-allowlist from FederationConfig
            // when unset, emitting one federation.peer_seeded per peer. Placed
            // after AppContext::new (audit-chain ready) and before serving (per
            // §2.4: after audit-chain init, before request-handler acceptance).
            // Seed-if-absent + idempotent: a prior boot's runtime contents stay
            // authoritative. discovery-mode / relay-urls / pending-discoveries
            // stay unset (Phases C/D seed them). Best-effort; never blocks boot.
            // v0.9 Federation Pattern-1 Phase D (#354 / addendum §A6) — boot-seed
            // all four federation.policy.* keys (peer-allowlist, discovery-mode,
            // pending-discoveries, relay-urls) via independent seed-if-absent
            // wrappers. relay-urls is gated on federation.enabled. If any seed
            // fails, emits federation.boot_seed_failed + sets the boot-seed
            // refusal flag (the 8 mutation XRPCs + the discovery scheduler then
            // 503/skip). Placed before JobScheduler::start() so the scheduler
            // reads the post-seed mode + flag.
            api::federation_peers::run_federation_boot_seed(&ctx).await;

            // v0.9 Federation runtime-mutability arc §3.3 (#391) — process any
            // pending restart-coordination markers left by a save-and-restart
            // flow before this boot. Best-effort: a failure logs and boot
            // continues (markers are coordination, not a boot prerequisite).
            if let Err(e) = api::pending_restart::process_pending_restart_actions(&ctx).await {
                tracing::error!(error = %e, "pending-restart-action boot hook failed");
            }

            // Start background jobs
            let scheduler = std::sync::Arc::new(jobs::JobScheduler::new(Arc::clone(&ctx)));
            scheduler.start();

            // Start server
            server::serve((*ctx).clone(), api_router).await?;
        }

        Some(Commands::CreateAccount {
            email,
            handle,
            password,
            invite_code,
        }) => {
            cli::account::create_account(&ctx, &email, &handle, &password, invite_code.as_deref())
                .await?;
        }

        Some(Commands::MigrateOauth { did, yes, dry_run }) => {
            if !yes && !dry_run {
                println!(
                    "WARNING: This will revoke all active JWT sessions for {}",
                    did
                );
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
            println!(
                "Successful migrations: {}",
                results.iter().filter(|r| r.success).count()
            );
            println!("Skipped: {}", results.iter().filter(|r| !r.success).count());
            println!("════════════════════════════════════════════════════════\n");
        }

        Some(Commands::Backup {
            output,
            compress,
            all,
        }) => {
            cli::backup::backup_database(&ctx, &output, compress, all).await?;
        }

        Some(Commands::Restore { input, yes }) => {
            cli::backup::restore_database(&ctx, &input, yes).await?;
        }

        Some(Commands::GenerateDidKey {
            format,
            output,
            private,
        }) => {
            cli::keygen::generate_did_key(&format, output.as_deref(), private)?;
        }

        Some(Commands::GenerateServiceToken { aud, lifetime, lxm }) => {
            cli::service_token::generate_service_token(&ctx, &aud, lifetime, lxm.as_deref())
                .await?;
        }

        Some(Commands::HealthCheck { format }) => {
            cli::health::health_check(&ctx, &format).await?;
        }

        Some(Commands::ExportMetrics { format, output }) => {
            cli::metrics::export_metrics(&format, output.as_deref())?;
        }

        Some(Commands::PublishIdentity { dids, delay }) => {
            cli::publish_identity::publish_identity(&ctx, dids.clone(), delay).await?;
        }

        Some(Commands::PublishIdentityFile { file, delay }) => {
            cli::publish_identity::publish_identity_from_file(&ctx, &file, delay).await?;
        }

        Some(Commands::RotateKeys {
            dids,
            concurrency,
            rationale,
            public_key,
            private_key_hex,
        }) => {
            // clap `requires` guarantees both-or-neither; assemble the
            // operator-supplied keypair (gated + single-DID enforced in
            // rotate_keys) only when both halves are present.
            let operator_keypair = match (public_key, private_key_hex) {
                (Some(public_did_key), Some(private_key_hex)) => {
                    Some(crate::account::OperatorSuppliedKeypair {
                        public_did_key,
                        private_key_hex,
                    })
                }
                _ => None,
            };
            cli::rotate_keys::rotate_keys(&ctx, dids, concurrency, rationale, operator_keypair)
                .await?;
        }

        Some(Commands::RotateKeysFile { file, concurrency }) => {
            cli::rotate_keys::rotate_keys_from_file(&ctx, &file, concurrency).await?;
        }

        Some(Commands::ValidateConfig) => {
            cli::validate_config::validate_config(&ctx.config)?;
        }

        Some(Commands::GrantAdmin {
            did,
            role,
            notes,
            force,
        }) => {
            cli::admin::grant_admin(&ctx, did, role, notes, force).await?;
        }

        Some(Commands::GcSweep {
            dry_run,
            report_only,
            max_deletes,
            threshold_secs,
            page_size,
        }) => {
            cli::gc_sweep::run(
                &ctx,
                dry_run,
                report_only,
                max_deletes,
                threshold_secs,
                page_size,
            )
            .await?;
        }

        Some(Commands::Debug { subcommand }) => {
            use cli::DebugCommands;
            match subcommand {
                DebugCommands::InspectAccount { identifier, format } => {
                    cli::debug::inspect_account(&ctx, &identifier, &format).await?;
                }
                DebugCommands::InspectRepo { did, format } => {
                    cli::debug::inspect_repo(&ctx, &did, &format).await?;
                }
                DebugCommands::ListSessions { did, format } => {
                    cli::debug::list_sessions(&ctx, did.as_deref(), &format).await?;
                }
                DebugCommands::CheckBlobs { did, orphaned } => {
                    cli::debug::check_blobs(&ctx, did.as_deref(), orphaned).await?;
                }
                DebugCommands::ExportAccount { did, output } => {
                    cli::debug::export_account(&ctx, &did, &output).await?;
                }
                DebugCommands::VerifyAuditChain => {
                    let healthy = cli::debug::verify_audit_chain(&ctx).await?;
                    if !healthy {
                        std::process::exit(1);
                    }
                }
            }
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
