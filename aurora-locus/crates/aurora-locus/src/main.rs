//! The aurora-locus binary: `serve` runs a node; the other subcommands are
//! the operator CLI.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "aurora-locus", version, about = "An atproto PDS")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run a node.
    Serve,
    /// Print version information.
    Version,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    match Cli::parse().command {
        Command::Serve => {
            tracing::error!("serve is not implemented yet");
            std::process::exit(1);
        }
        Command::Version => println!("aurora-locus {}", env!("CARGO_PKG_VERSION")),
    }
}
