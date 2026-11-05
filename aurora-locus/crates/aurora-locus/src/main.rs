//! The aurora-locus binary: `serve` runs a node; the other subcommands are
//! the operator CLI.

#[cfg(unix)]
mod admin_cli;
mod logging;
mod node;
mod setup_cli;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use locus_core::bootstrap::{Bootstrap, LogFormat};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(name = "aurora-locus", version, about = "An atproto PDS")]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// A node's URL, to use the admin API over HTTP instead of the socket.
    #[arg(long, global = true, env = "LOCUS_ADMIN_URL")]
    url: Option<String>,
    /// An operator API token, with --url.
    #[arg(long, global = true, env = "LOCUS_ADMIN_TOKEN", hide_env_values = true)]
    token: Option<String>,
    /// The admin socket (default: <data>/admin.sock).
    #[arg(long, global = true, env = "LOCUS_ADMIN_SOCKET")]
    socket: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Run a node.
    Serve,
    /// First-run setup: interactive, or from a file with --config.
    Setup {
        /// A setup.toml with every answer, for automation.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Accept the final checks' warnings without asking.
        #[arg(long)]
        accept_warnings: bool,
        /// Replace the setup token (for the web wizard) and print it.
        #[arg(long)]
        new_token: bool,
    },
    /// GET /xrpc/_health; exits 0 on 200 (the compose health check).
    Healthcheck {
        #[arg(long, default_value = "http://127.0.0.1:3000")]
        url: String,
    },
    /// Print version information.
    Version,
    #[cfg(unix)]
    #[command(flatten)]
    Admin(admin_cli::AdminCmd),
}

fn bootstrap_or_exit() -> Bootstrap {
    match Bootstrap::from_env() {
        Ok(b) => b,
        Err(e) => {
            let _ = logging::init(LogFormat::Pretty);
            tracing::error!("{e}");
            std::process::exit(2);
        }
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Version => println!("aurora-locus {}", env!("CARGO_PKG_VERSION")),
        Command::Healthcheck { url } => std::process::exit(admin_cli::healthcheck(&url).await),
        #[cfg(unix)]
        Command::Admin(cmd) => {
            let client = match (cli.url, cli.token) {
                (Some(url), Some(token)) => admin_cli::Client::Http { url, token },
                (Some(_), None) => {
                    eprintln!("--url needs --token (an operator API token)");
                    std::process::exit(2);
                }
                _ => admin_cli::Client::Socket(cli.socket.unwrap_or_else(|| {
                    bootstrap_or_exit()
                        .admin_socket
                        .unwrap_or_else(|| PathBuf::from("admin.sock"))
                })),
            };
            if let Err(e) = admin_cli::run(&client, cmd).await {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        Command::Setup {
            config,
            accept_warnings,
            new_token,
        } => {
            let bootstrap = bootstrap_or_exit();
            let _log = logging::init(LogFormat::Pretty);
            let code = setup_cli::run(bootstrap, config, accept_warnings, new_token).await;
            std::process::exit(code);
        }
        Command::Serve => {
            let bootstrap = bootstrap_or_exit();
            let log = logging::init(bootstrap.log_format);
            let cancel = CancellationToken::new();
            node::cancel_on_signal(cancel.clone());
            // A node in setup mode starts again once setup completes.
            loop {
                match node::Node::start(bootstrap.clone(), log.clone(), cancel.clone()).await {
                    Ok(n) => match n.run_until_stopped().await {
                        node::Ended::Restart => {
                            tracing::info!("starting again after setup");
                        }
                        node::Ended::Stopped => break,
                    },
                    Err(e) => {
                        tracing::error!("failed to start: {e}");
                        std::process::exit(1);
                    }
                }
            }
        }
    }
}
