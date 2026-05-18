use clap::{Parser, Subcommand};
use rlm_anywhere::SettingsOverrides;

#[derive(Debug, Parser)]
#[command(version, about = "RLM as a language model api")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Command>,

    #[arg(long)]
    pub(crate) port: Option<u16>,

    #[arg(long)]
    pub(crate) upstream_base_url: Option<String>,

    #[arg(long)]
    pub(crate) upstream_api_key: Option<String>,
}

impl Cli {
    pub(crate) fn settings_overrides(&self) -> SettingsOverrides {
        SettingsOverrides {
            port: self.port,
            upstream_base_url: self.upstream_base_url.clone(),
            upstream_api_key: self.upstream_api_key.clone(),
        }
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    Serve,
}
