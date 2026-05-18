use std::net::SocketAddr;

use clap::Parser as _;
use rlm_anywhere::{AppConfig, serve};

mod cli;

use cli::{Cli, Command};

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
