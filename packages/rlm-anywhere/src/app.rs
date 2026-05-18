use std::net::SocketAddr;

use axum::Router;
use axum::routing::post;
use reqwest::{Client, Url};
use tokio::net::TcpListener;

use crate::config::DEFAULT_UPSTREAM_BASE_URL;
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
        upstream_base_url: impl AsRef<str>,
        upstream_api_key: Option<String>,
    ) -> Result<Self, String> {
        let upstream_chat_completions_url = normalize_upstream_url(upstream_base_url.as_ref())?;
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

impl Default for AppConfig {
    fn default() -> Self {
        let listen = SocketAddr::from(([127, 0, 0, 1], 3000));

        Self::new(listen, DEFAULT_UPSTREAM_BASE_URL, None).unwrap_or_else(|_| Self {
            listen,
            upstream_chat_completions_url: format!("{DEFAULT_UPSTREAM_BASE_URL}/chat/completions"),
            upstream_api_key: None,
        })
    }
}

#[derive(Clone)]
pub(crate) struct ChatProxyState {
    pub(crate) config: AppConfig,
    pub(crate) client: Client,
}

impl ChatProxyState {
    #[must_use]
    pub(crate) fn new(config: AppConfig, client: Client) -> Self {
        Self { config, client }
    }
}

pub(crate) fn build_router(state: ChatProxyState) -> Router {
    Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .with_state(state)
}

pub async fn serve(config: AppConfig) -> color_eyre::Result<()> {
    let listen = config.listen();

    let router = build_router(ChatProxyState::new(config, Client::new()));
    let listener = TcpListener::bind(listen).await?;
    tracing::info!(%listen, "listening");
    axum::serve(listener, router).await?;
    Ok(())
}

fn normalize_upstream_url(upstream_base_url: &str) -> Result<String, String> {
    let trimmed = upstream_base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("upstream base URL cannot be empty".to_owned());
    }

    let url = Url::parse(&format!("{trimmed}/chat/completions"))
        .map_err(|error| format!("invalid upstream base URL: {error}"))?;
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::normalize_upstream_url;

    #[test]
    fn normalizes_upstream_chat_completions_url() {
        assert_eq!(
            normalize_upstream_url("http://localhost:20128/v1").as_deref(),
            Ok("http://localhost:20128/v1/chat/completions")
        );
        assert_eq!(
            normalize_upstream_url("http://localhost:20128/v1/").as_deref(),
            Ok("http://localhost:20128/v1/chat/completions")
        );
    }
}
