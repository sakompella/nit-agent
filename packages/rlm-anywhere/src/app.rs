use std::net::SocketAddr;

use async_openai::Client as OpenAIClient;
use axum::Router;
use axum::routing::post;
use color_eyre::Result;
use color_eyre::eyre::{WrapErr as _, eyre};
use reqwest::Url;
use secrecy::SecretString;
use tokio::net::TcpListener;

use crate::proxy::chat_completions;
use crate::upstream::{UpstreamClient, UpstreamConfig};

const CHAT_COMPLETIONS_API_PATH: &str = "/chat/completions";
const SELF_COMPLETIONS_API_PATH: &str = const_str::concat!("/v1", CHAT_COMPLETIONS_API_PATH);

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub(crate) bind_address: SocketAddr,
    pub(crate) upstream_base_url: String,
    pub(crate) upstream_api_key: Option<SecretString>,
}

impl AppConfig {
    pub fn new(
        bind_address: SocketAddr,
        upstream_base_url: &str,
        upstream_api_key: Option<String>,
    ) -> Result<Self> {
        let upstream_base_url = normalize_upstream_base_url(upstream_base_url)
            .wrap_err("failed to normalize upstream base URL")?;
        let upstream_api_key = upstream_api_key.map(SecretString::from);
        UpstreamConfig::new(upstream_base_url.clone(), upstream_api_key.clone())
            .wrap_err("failed to validate upstream configuration")?;
        Ok(Self {
            bind_address,
            upstream_base_url,
            upstream_api_key,
        })
    }

    #[must_use]
    pub fn bind_address(&self) -> SocketAddr {
        self.bind_address
    }
}

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) config: AppConfig,
    pub(crate) client: UpstreamClient,
}

impl AppState {
    pub(crate) fn new(config: AppConfig) -> Result<Self> {
        let upstream_config = UpstreamConfig::new(
            config.upstream_base_url.clone(),
            config.upstream_api_key.clone(),
        )
        .wrap_err("failed to build upstream client configuration")?;
        let client = OpenAIClient::with_config(upstream_config);
        Ok(Self { config, client })
    }
}

pub async fn serve(config: AppConfig) -> Result<()> {
    let bind_address = config.bind_address();

    let router = build_router(config).wrap_err("failed to build rlm-anywhere router")?;
    let listener = TcpListener::bind(bind_address)
        .await
        .wrap_err_with(|| format!("failed to bind listener on {bind_address}"))?;

    tracing::info!(%bind_address, "listening");

    axum::serve(listener, router)
        .await
        .wrap_err("rlm-anywhere server failed")?;
    Ok(())
}

pub fn build_router(config: AppConfig) -> Result<Router> {
    let state = AppState::new(config)?;
    // todo set up further routes?
    Ok(Router::new()
        .route(SELF_COMPLETIONS_API_PATH, post(chat_completions))
        .with_state(state))
}

fn normalize_upstream_base_url(upstream_base_url: &str) -> Result<String> {
    let trimmed = upstream_base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        // todo use bail! here
        return Err(eyre!("upstream base URL cannot be empty"));
    }

    let url =
        Url::parse(trimmed).wrap_err_with(|| format!("invalid upstream base URL: {trimmed}"))?;
    Ok(url.to_string())
}
