use clap::{Parser, Subcommand};
use rlm_anywhere::{DEFAULT_PORT, DEFAULT_UPSTREAM_BASE_URL};

#[derive(Debug, Parser)]
#[command(version, about = "RLM as a language model api")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Command>,

    #[arg(
        long,
        // env = "RLM_ANYWHERE_PORT",
        default_value = DEFAULT_PORT
    )]
    pub(crate) port: u16,

    #[arg(
        long,
        // env = "RLM_ANYWHERE_UPSTREAM_BASE_URL",
        default_value = DEFAULT_UPSTREAM_BASE_URL
    )]
    pub(crate) upstream_base_url: String,

    #[arg(
        long,
        //, env = "RLM_ANYWHERE_UPSTREAM_API_KEY"
    )]
    pub(crate) upstream_api_key: Option<String>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    Serve,
}
