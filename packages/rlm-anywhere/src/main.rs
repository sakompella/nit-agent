use std::net::SocketAddr;
use std::time::Duration;

use clap::Parser as _;
use color_eyre::Result;
use color_eyre::eyre::WrapErr as _;
use figment::Figment;
use figment::providers::Serialized;
use rlm_anywhere::rlm::{RlmLoopConfig, sandbox::SandboxLimits};
use rlm_anywhere::{AppConfig, load_settings, serve};

mod cli;

use cli::{Cli, Command};
#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init()?;
    match cli.command.as_ref().unwrap_or(&Command::Serve) {
        Command::Serve => {
            let settings = load_settings(Figment::from(Serialized::defaults(&cli)))?;
            // todo make this configurable
            let bind_address = SocketAddr::from(([127, 0, 0, 1], settings.port));
            let config = AppConfig::new_with_provider(
                bind_address,
                settings.mode,
                settings.upstream_provider,
                &settings.upstream_base_url,
                settings.upstream_api_key,
                Duration::from_millis(settings.upstream_timeout_ms),
            )
            .wrap_err("failed to build rlm-anywhere app config")?;
            let rlm = RlmLoopConfig {
                max_steps: settings.rlm_max_steps,
                max_subcalls: settings.rlm_max_subcalls,
                max_wall: Duration::from_millis(settings.rlm_max_wall_ms),
                tool_result_preview_bytes: settings.rlm_tool_result_preview_bytes,
                sandbox_limits: SandboxLimits::default(),
            };
            serve(config.with_rlm(rlm)).await
        }
    }
}

fn init() -> Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    Ok(())
}
