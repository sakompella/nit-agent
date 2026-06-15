use std::net::SocketAddr;

use axum::Router;
use axum::routing::post;
use color_eyre::Result;
use color_eyre::eyre::{WrapErr as _, eyre};
use reqwest::Url;
use secrecy::SecretString;
use tokio::net::TcpListener;

use crate::config::{RequestMode, UpstreamProvider};
use crate::proxy::chat_completions;
use crate::rlm::RlmLoopConfig;
use crate::upstream::RigModelBackend;

const CHAT_COMPLETIONS_API_PATH: &str = "/chat/completions";
const SELF_COMPLETIONS_API_PATH: &str = const_str::concat!("/v1", CHAT_COMPLETIONS_API_PATH);

#[derive(Clone, Debug)]
pub enum UpstreamConfig {
    OpenAiChatCompletions {
        base_url: String,
        api_key: Option<SecretString>,
    },
}

impl UpstreamConfig {
    /// # Errors
    /// Returns an error if the base URL is invalid or the upstream configuration fails to validate.
    pub fn open_ai_chat_completions(base_url: &str, api_key: Option<String>) -> Result<Self> {
        let base_url = normalize_upstream_base_url(base_url)
            .wrap_err("failed to normalize upstream base URL")?;
        let api_key = api_key.map(SecretString::from);
        let config = Self::OpenAiChatCompletions { base_url, api_key };
        config
            .model_backend()
            .wrap_err("failed to validate upstream configuration")?;
        Ok(config)
    }

    pub(crate) fn model_backend(&self) -> Result<RigModelBackend> {
        match self {
            Self::OpenAiChatCompletions { base_url, api_key } => {
                RigModelBackend::new(base_url.clone(), api_key.clone())
            }
        }
    }

    pub(crate) const fn has_configured_api_key(&self) -> bool {
        match self {
            Self::OpenAiChatCompletions { api_key, .. } => api_key.is_some(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub(crate) bind_address: SocketAddr,
    pub(crate) mode: RequestMode,
    pub(crate) upstream: UpstreamConfig,
    pub(crate) rlm: RlmLoopConfig,
}

impl AppConfig {
    /// # Errors
    /// Returns an error if the upstream configuration is invalid.
    pub fn new(
        bind_address: SocketAddr,
        upstream_base_url: &str,
        upstream_api_key: Option<String>,
    ) -> Result<Self> {
        Self::new_with_provider(
            bind_address,
            RequestMode::Rlm,
            UpstreamProvider::OpenAiCompatible,
            upstream_base_url,
            upstream_api_key,
        )
    }

    /// # Errors
    /// Returns an error if the upstream configuration is invalid.
    pub fn new_with_provider(
        bind_address: SocketAddr,
        mode: RequestMode,
        upstream_provider: UpstreamProvider,
        upstream_base_url: &str,
        upstream_api_key: Option<String>,
    ) -> Result<Self> {
        let upstream = match upstream_provider {
            UpstreamProvider::OpenAiCompatible => {
                UpstreamConfig::open_ai_chat_completions(upstream_base_url, upstream_api_key)?
            }
        };
        Ok(Self {
            bind_address,
            mode,
            upstream,
            rlm: RlmLoopConfig::default(),
        })
    }

    #[must_use]
    pub const fn bind_address(&self) -> SocketAddr {
        self.bind_address
    }

    #[must_use]
    pub const fn with_rlm(mut self, rlm: RlmLoopConfig) -> Self {
        self.rlm = rlm;
        self
    }

    pub(crate) fn upstream_has_configured_api_key(&self) -> bool {
        self.upstream.has_configured_api_key()
    }
}

#[derive(Clone)]
pub struct AppState {
    pub(crate) config: AppConfig,
    pub(crate) model_backend: RigModelBackend,
}

impl AppState {
    pub(crate) fn new(config: AppConfig) -> Result<Self> {
        let model_backend = config
            .upstream
            .model_backend()
            .wrap_err("failed to build upstream model backend")?;
        Ok(Self {
            config,
            model_backend,
        })
    }
}

/// # Errors
/// Returns an error if the router fails to build or the server fails to start.
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

/// # Errors
/// Returns an error if the upstream model backend fails to initialize.
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
    if !matches!(url.scheme(), "http" | "https") {
        return Err(eyre!("upstream base URL must use http or https: {trimmed}"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(eyre!(
            "upstream base URL cannot include query or fragment: {trimmed}"
        ));
    }
    Ok(url.to_string())
}
