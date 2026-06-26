use clap::{Parser, Subcommand};
use rlm_anywhere::{RequestMode, UpstreamProvider};
use serde::Serialize;

/// A proxy + agent that lets you interact with RLMs as if they were any other LLM API.
#[derive(Debug, Parser, Serialize)]
#[command(version)]
pub struct Cli {
    #[serde(skip)]
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Port on localhost to bind to
    #[serde(skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub port: Option<u16>,

    /// Base URL of the upstream LLM API
    #[serde(skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub upstream_base_url: Option<String>,

    /// Request handling mode: rlm or passthrough
    #[serde(skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub mode: Option<RequestMode>,

    /// Provider adapter used for upstream LLM API calls
    #[serde(skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub upstream_provider: Option<UpstreamProvider>,

    /// API key for the upstream LLM API
    #[serde(skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub upstream_api_key: Option<String>,

    /// Maximum RLM loop steps per request
    #[serde(skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub rlm_max_steps: Option<u64>,

    /// Maximum RLM subcalls per request
    #[serde(skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub rlm_max_subcalls: Option<u64>,

    /// Maximum RLM wall-clock budget in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub rlm_max_wall_ms: Option<u64>,

    /// Maximum preview bytes for tool results
    #[serde(skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub rlm_tool_result_preview_bytes: Option<usize>,

    /// Maximum bytes allowed for a single tool call's arguments
    #[serde(skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub rlm_max_tool_arg_bytes: Option<usize>,

    /// Per-call upstream HTTP timeout in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub upstream_timeout_ms: Option<u64>,

    /// Maximum accepted caller request body size in bytes
    #[serde(skip_serializing_if = "Option::is_none")]
    #[arg(long)]
    pub max_request_body_bytes: Option<usize>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    Serve,
}
