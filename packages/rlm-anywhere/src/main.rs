use std::net::SocketAddr;

use clap::Parser as _;
use color_eyre::Result;
use color_eyre::eyre::WrapErr as _;
use rlm_anywhere::{AppConfig, load_settings, serve};

mod cli;

use cli::{Cli, Command};

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match cli.command.as_ref().unwrap_or(&Command::Serve) {
        Command::Serve => {
            let settings = load_settings(cli.settings_overrides())?;
            let listen = SocketAddr::from(([127, 0, 0, 1], settings.port));
            let config = AppConfig::new(
                listen,
                &settings.upstream_base_url,
                settings.upstream_api_key,
            )
            .wrap_err("failed to build rlm-anywhere app config")?;
            serve(config).await
        }
    }
}
