use clap::{Parser, Subcommand};
use rlm_anywhere::{RequestMode, UpstreamProvider};
use serde::Serialize;

/// A proxy + agent that lets you interact with RLMs as if they were any other LLM API.
#[derive(Debug, Parser, Serialize)]
#[command(version)]
pub(crate) struct Cli {
    #[serde(skip)]
    #[command(subcommand)]
    pub(crate) command: Option<Command>,

    /// Port on localhost to bind to
    #[serde(skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub(crate) port: Option<u16>,

    /// Base URL of the upstream LLM API
    #[serde(skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub(crate) upstream_base_url: Option<String>,

    /// Request handling mode: rlm or passthrough
    #[serde(skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub(crate) mode: Option<RequestMode>,

    /// Provider adapter used for upstream LLM API calls
    #[serde(skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub(crate) upstream_provider: Option<UpstreamProvider>,

    /// API key for the upstream LLM API
    #[serde(skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub(crate) upstream_api_key: Option<String>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    Serve,
}
