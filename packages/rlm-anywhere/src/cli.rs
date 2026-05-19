use clap::{Parser, Subcommand};
use serde::Serialize;

#[derive(Debug, Parser, Serialize)]
#[command(version, about = "RLM as a language model api")]
pub(crate) struct Cli {
    #[serde(skip)]
    #[command(subcommand)]
    pub(crate) command: Option<Command>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub(crate) port: Option<u16>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub(crate) upstream_base_url: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub(crate) upstream_api_key: Option<String>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    Serve,
}
