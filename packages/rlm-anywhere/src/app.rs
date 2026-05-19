use std::net::SocketAddr;

use axum::Router;
use axum::routing::post;
use color_eyre::Result;
use color_eyre::eyre::{WrapErr as _, eyre};
use reqwest::{Client, Url};
use tokio::net::TcpListener;

use crate::proxy::chat_completions;

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub(crate) listen: SocketAddr,
    pub(crate) upstream_chat_completions_url: String,
    pub(crate) upstream_api_key: Option<String>,
}

impl AppConfig {
    pub fn new(
        listen: SocketAddr,
        upstream_base_url: &str,
        upstream_api_key: Option<String>,
    ) -> Result<Self> {
        let upstream_chat_completions_url = normalize_upstream_url(upstream_base_url)
            .wrap_err("failed to normalize upstream chat completions URL")?;
        Ok(Self {
            listen,
            upstream_chat_completions_url,
            upstream_api_key,
        })
    }

    #[must_use]
    pub fn listen(&self) -> SocketAddr {
        self.listen
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

pub fn build_router(config: AppConfig) -> Router {
    build_router_from_state(AppState::new(config, Client::new()))
}

fn build_router_from_state(state: AppState) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .with_state(state)
}

pub async fn serve(config: AppConfig) -> Result<()> {
    let listen = config.listen();

    let router = build_router(config);
    let listener = TcpListener::bind(listen)
        .await
        .wrap_err_with(|| format!("failed to bind listener on {listen}"))?;
    tracing::info!(%listen, "listening");
    axum::serve(listener, router)
        .await
        .wrap_err("rlm-anywhere server failed")?;
    Ok(())
}

fn normalize_upstream_url(upstream_base_url: &str) -> Result<String> {
    let trimmed = upstream_base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(eyre!("upstream base URL cannot be empty"));
    }

    let url = Url::parse(&format!("{trimmed}/chat/completions"))
        .wrap_err_with(|| format!("invalid upstream base URL: {trimmed}"))?;
    Ok(url.to_string())
}
