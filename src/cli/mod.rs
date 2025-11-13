/// CLI commands for Aurora Locus administration
///
/// Provides command-line tools for server administration, including:
/// - OAuth migration (JWT to OAuth 2.1)
/// - User management
/// - Database maintenance

pub mod migrate_oauth;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "aurora-locus")]
#[command(about = "ATProto Personal Data Server", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Run the PDS server (default)
    Serve,

    /// Migrate user from JWT sessions to OAuth 2.1
    MigrateOauth {
        /// DID of the user to migrate
        did: String,

        /// Skip confirmation prompt
        #[arg(short = 'y', long)]
        yes: bool,

        /// Dry run - show what would be done without making changes
        #[arg(short = 'n', long)]
        dry_run: bool,
    },

    /// Bulk migrate all users to OAuth (admin only)
    BulkMigrateOauth {
        /// Skip confirmation prompt
        #[arg(short = 'y', long)]
        yes: bool,

        /// Dry run - show what would be done without making changes
        #[arg(short = 'n', long)]
        dry_run: bool,
    },
}
