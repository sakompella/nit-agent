use std::net::SocketAddr;

use clap::{Parser, Subcommand};
use rlm_anywhere::{AppConfig, DEFAULT_PORT, DEFAULT_UPSTREAM_BASE_URL, serve};

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => {
            let listen = SocketAddr::from(([127, 0, 0, 1], cli.port));
            let config = AppConfig::new(listen, cli.upstream_base_url, cli.upstream_api_key)
                .map_err(|error| color_eyre::eyre::eyre!(error))?;
            serve(config).await
        }
    }
}

#[derive(Debug, Parser)]
#[command(version, about = "RLM as a language model api")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(
        long,
        // env = "RLM_ANYWHERE_PORT",
        default_value = DEFAULT_PORT
    )]
    port: u16,

    #[arg(
        long,
        // env = "RLM_ANYWHERE_UPSTREAM_BASE_URL",
        default_value = DEFAULT_UPSTREAM_BASE_URL
    )]
    upstream_base_url: String,

    #[arg(
        long,
        //, env = "RLM_ANYWHERE_UPSTREAM_API_KEY"
    )]
    upstream_api_key: Option<String>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Serve,
}
