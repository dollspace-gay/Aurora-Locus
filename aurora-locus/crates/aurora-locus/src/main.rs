//! The aurora-locus binary: `serve` runs a node; the other subcommands are
//! the operator CLI.

mod logging;
mod node;

use clap::{Parser, Subcommand};
use locus_core::bootstrap::{Bootstrap, LogFormat};
use tokio_util::sync::CancellationToken;

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
    let cli = Cli::parse();
    match cli.command {
        Command::Version => println!("aurora-locus {}", env!("CARGO_PKG_VERSION")),
        Command::Serve => {
            let bootstrap = match Bootstrap::from_env() {
                Ok(b) => b,
                Err(e) => {
                    let _ = logging::init(LogFormat::Pretty);
                    tracing::error!("{e}");
                    std::process::exit(2);
                }
            };
            let log = logging::init(bootstrap.log_format);
            let cancel = CancellationToken::new();
            node::cancel_on_signal(cancel.clone());
            match node::Node::start(bootstrap, log, cancel).await {
                Ok(n) => n.run_until_stopped().await,
                Err(e) => {
                    tracing::error!("failed to start: {e}");
                    std::process::exit(1);
                }
            }
        }
    }
}
