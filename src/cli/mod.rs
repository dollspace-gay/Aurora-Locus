//! CLI commands for Aurora Locus administration
//!
//! Provides command-line tools for server administration, including:
//! - Account creation
//! - OAuth migration (JWT to OAuth 2.1)
//! - Database backup and restore
//! - DID key generation
//! - Service token generation
//! - Health checks
//! - Metrics export
//! - Configuration validation
//! - Debug utilities

pub mod account;
pub mod admin;
pub mod backup;
pub mod debug;
pub mod health;
pub mod keygen;
pub mod metrics;
pub mod migrate_oauth;
pub mod publish_identity;
pub mod rotate_keys;
pub mod service_token;
pub mod validate_config;

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

    /// Create a new account
    CreateAccount {
        /// Email address
        #[arg(short, long)]
        email: String,

        /// Handle (username)
        #[arg(short = 'u', long)]
        handle: String,

        /// Password
        #[arg(short, long)]
        password: String,

        /// Invite code (if required)
        #[arg(short, long)]
        invite_code: Option<String>,
    },

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

    /// Backup database to file
    Backup {
        /// Output file path
        #[arg(short, long)]
        output: String,

        /// Compress the backup with gzip
        #[arg(short, long)]
        compress: bool,

        /// Include all databases (account, sequencer, did cache)
        #[arg(short, long)]
        all: bool,
    },

    /// Restore database from backup
    Restore {
        /// Input backup file path
        #[arg(short, long)]
        input: String,

        /// Skip confirmation prompt
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Generate DID keypair
    GenerateDidKey {
        /// Output format: pem, jwk, did
        #[arg(short, long, default_value = "did")]
        format: String,

        /// Output file path (if not specified, prints to stdout)
        #[arg(short, long)]
        output: Option<String>,

        /// Include private key (for PEM and JWK formats only)
        #[arg(short = 'P', long)]
        private: bool,
    },

    /// Generate service authentication token
    GenerateServiceToken {
        /// Audience DID (the service that will verify this token)
        #[arg(short, long)]
        aud: String,

        /// Token lifetime in seconds (default: 3600)
        #[arg(short, long, default_value = "3600")]
        lifetime: i64,

        /// Optional lexicon method (e.g., com.atproto.repo.createRecord)
        #[arg(short = 'm', long)]
        lxm: Option<String>,
    },

    /// Check server health
    HealthCheck {
        /// Output format: text or json
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// Export metrics
    ExportMetrics {
        /// Output format: prometheus or json
        #[arg(short, long, default_value = "prometheus")]
        format: String,

        /// Output file path (if not specified, prints to stdout)
        #[arg(short, long)]
        output: Option<String>,
    },

    /// Publish identity event to sequencer
    PublishIdentity {
        /// DIDs to publish identity events for
        dids: Vec<String>,

        /// Delay between publishes in milliseconds (default: 0)
        #[arg(short, long, default_value = "0")]
        delay: u64,
    },

    /// Publish identity events from file (one DID per line)
    PublishIdentityFile {
        /// File containing DIDs (one per line)
        #[arg(short, long)]
        file: String,

        /// Delay between publishes in milliseconds (default: 5)
        #[arg(short, long, default_value = "5")]
        delay: u64,
    },

    /// Rotate DID signing keys and update PLC directory
    RotateKeys {
        /// DIDs to rotate keys for
        dids: Vec<String>,

        /// Number of concurrent rotations (default: 10)
        #[arg(short, long, default_value = "10")]
        concurrency: usize,
    },

    /// Rotate keys from file (one DID per line)
    RotateKeysFile {
        /// File containing DIDs (one per line)
        #[arg(short, long)]
        file: String,

        /// Number of concurrent rotations (default: 25)
        #[arg(short, long, default_value = "25")]
        concurrency: usize,
    },

    /// Validate configuration
    ValidateConfig,

    /// Grant an admin role to a DID. Step 3 (§5.4.3) scaffolding;
    /// the grant body itself lands in Step 4.
    GrantAdmin {
        /// DID to grant the role to (e.g. did:plc:abc, did:web:host).
        did: String,

        /// Role to grant: moderator, admin, or superadmin
        /// (case-insensitive).
        role: String,

        /// Optional human-readable note recorded with the grant.
        #[arg(long)]
        notes: Option<String>,

        /// Re-grant a DID with a previously revoked role. Offline-
        /// only; semantics land in Step 4.
        #[arg(long)]
        force: bool,
    },

    /// Debug utilities
    Debug {
        #[command(subcommand)]
        subcommand: DebugCommands,
    },
}

#[derive(Subcommand)]
pub enum DebugCommands {
    /// Inspect account details
    InspectAccount {
        /// Account identifier (DID or handle)
        identifier: String,

        /// Output format: text or json
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// Inspect repository state
    InspectRepo {
        /// DID of the repository
        did: String,

        /// Output format: text or json
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// List active sessions
    ListSessions {
        /// Filter by DID (optional)
        #[arg(short, long)]
        did: Option<String>,

        /// Output format: text or json
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// Check blob store integrity
    CheckBlobs {
        /// DID to check blobs for (optional, checks all if not specified)
        #[arg(short, long)]
        did: Option<String>,

        /// Check for orphaned temporary blobs
        #[arg(short, long)]
        orphaned: bool,
    },

    /// Export account data for debugging
    ExportAccount {
        /// DID of the account to export
        did: String,

        /// Output file path
        #[arg(short, long)]
        output: String,
    },

    /// Walk the audit chain from genesis forward, recomputing hashes
    /// per entry, and report pass/fail + entry count + any
    /// discontinuity. Read-only; exits 0 on healthy, 1 on
    /// discontinuity or DB error.
    VerifyAuditChain,
}
