use std::net::SocketAddr;

use axum::Router;
use axum::routing::post;
use color_eyre::Result;
use color_eyre::eyre::{WrapErr as _, eyre};
use reqwest::{Client, Url};
use tokio::net::TcpListener;

use crate::proxy::chat_completions;

const SELF_COMPLETIONS_API_PATH: &str = "/v1/chat/completions";
const UPSTREAM_COMPLETIONS_API_PATH: &str = "/chat/completions";

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub(crate) bind_address: SocketAddr,
    pub(crate) upstream_chat_completions_url: String,
    pub(crate) upstream_api_key: Option<String>,
}

impl AppConfig {
    pub fn new(
        bind_address: SocketAddr,
        upstream_base_url: &str,
        upstream_api_key: Option<String>,
    ) -> Result<Self> {
        let upstream_chat_completions_url = normalize_upstream_url(upstream_base_url)
            .wrap_err("failed to normalize upstream chat completions URL")?;
        Ok(Self {
            bind_address,
            upstream_chat_completions_url,
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
    pub(crate) client: Client,
}

impl AppState {
    #[must_use]
    pub(crate) fn new(config: AppConfig, client: Client) -> Self {
        Self { config, client }
    }
}

pub async fn serve(config: AppConfig) -> Result<()> {
    let bind_address = config.bind_address();

    let router = build_router(config, Client::new());
    let listener = TcpListener::bind(bind_address)
        .await
        .wrap_err_with(|| format!("failed to bind listener on {bind_address}"))?;

    tracing::info!(%bind_address, "listening");

    axum::serve(listener, router)
        .await
        .wrap_err("rlm-anywhere server failed")?;
    Ok(())
}

pub fn build_router(config: AppConfig, client: Client) -> Router {
    let state = AppState::new(config, client);
    Router::new()
        .route(SELF_COMPLETIONS_API_PATH, post(chat_completions))
        .with_state(state)
}

fn normalize_upstream_url(upstream_base_url: &str) -> Result<String> {
    let trimmed = upstream_base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(eyre!("upstream base URL cannot be empty"));
    }

    let url = Url::parse(&format!("{trimmed}{UPSTREAM_COMPLETIONS_API_PATH}"))
        .wrap_err_with(|| format!("invalid upstream base URL: {trimmed}"))?;
    Ok(url.to_string())
}
