use color_eyre::Result;
use color_eyre::eyre::WrapErr as _;
use rig_core::client::{DebugExt, Nothing, Provider, ProviderBuilder, ProviderClientError};
use rig_core::http_client::{self, HttpClientExt as _};
use secrecy::{ExposeSecret as _, SecretString};
use serde_json::Value;
use std::fmt::Display;
use std::time::Duration;
use thiserror::Error;

pub const CHAT_COMPLETIONS_API_PATH: &str = "/chat/completions";

#[derive(Clone)]
pub struct ModelRequest {
    pub body: Value,
    pub caller_authorization: Option<SecretString>,
}

#[derive(Debug, Error)]
pub enum ModelError {
    #[error("upstream request failed: {0}")]
    Request(String),
    #[error("upstream returned API error: {message}")]
    Api {
        /// HTTP status, when the failure carried one.
        status: Option<u16>,
        message: String,
    },
    #[error("upstream returned invalid JSON: {0}")]
    InvalidJson(serde_json::Error),
}

/// Transient HTTP statuses worth retrying: request timeout, rate limit, and the
/// 5xx family commonly emitted during upstream restarts or overload.
const TRANSIENT_STATUSES: [u16; 6] = [408, 429, 500, 502, 503, 504];
const RETRY_BACKOFF_BASE_MS: u64 = 250;
const RETRY_BACKOFF_CAP_MS: u64 = 2_000;

/// Exponential backoff for retry `attempt` (0-based): base * 2^attempt, capped.
fn retry_backoff(attempt: usize) -> Duration {
    let factor = 1_u64.checked_shl(u32::try_from(attempt).unwrap_or(u32::MAX));
    let millis = factor
        .and_then(|factor| RETRY_BACKOFF_BASE_MS.checked_mul(factor))
        .unwrap_or(RETRY_BACKOFF_CAP_MS)
        .min(RETRY_BACKOFF_CAP_MS);
    Duration::from_millis(millis)
}

impl ModelError {
    /// Whether this failure should be retried on the buffered path: connection
    /// errors and the transient HTTP statuses. API errors with a non-transient
    /// status (e.g. 400) and JSON-decode failures are not retried.
    fn is_transient(&self) -> bool {
        match self {
            Self::Request(_) => true,
            Self::Api { status, .. } => status.is_some_and(|s| TRANSIENT_STATUSES.contains(&s)),
            Self::InvalidJson(_) => false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RigModelBackend {
    upstream_base_url: String,
    configured_api_key: Option<SecretString>,
    http_client: reqwest::Client,
    /// Client for streaming passthrough. Uses connect/read timeouts that bound
    /// hangs without imposing a whole-request deadline, so a legitimately long
    /// SSE stream is not aborted mid-flight the way `http_client.timeout` would.
    stream_client: reqwest::Client,
    default_client: NoAuthOpenAiClient,
    /// Maximum bounded retries on transient upstream failures for the buffered
    /// `complete` path. The streaming path never retries.
    max_retries: usize,
}

impl RigModelBackend {
    /// # Errors
    /// Returns an error if the upstream API key cannot be represented as an
    /// HTTP header or the default Rig client cannot be constructed.
    pub fn new(
        upstream_base_url: String,
        upstream_api_key: Option<&SecretString>,
        timeout: Duration,
        max_retries: usize,
    ) -> Result<Self> {
        let http_client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .wrap_err("failed to build upstream HTTP client")?;
        // The read timeout resets on each chunk, so it bounds an idle/hung
        // stream without capping total stream duration.
        let stream_client = reqwest::Client::builder()
            .connect_timeout(timeout)
            .read_timeout(timeout)
            .build()
            .wrap_err("failed to build upstream streaming HTTP client")?;
        let headers = configured_auth_headers(upstream_api_key)?;
        let default_client = Self::build_client(&upstream_base_url, http_client.clone(), headers)
            .wrap_err("failed to build default Rig client")?;

        Ok(Self {
            upstream_base_url,
            configured_api_key: upstream_api_key.cloned(),
            http_client,
            stream_client,
            default_client,
            max_retries,
        })
    }

    fn client(&self, caller_authorization: Option<SecretString>) -> Result<NoAuthOpenAiClient> {
        let Some(authorization) = caller_authorization else {
            return Ok(self.default_client.clone());
        };

        self.request_scoped_client(&authorization)
            .wrap_err("failed to build request-scoped Rig client")
    }

    fn request_scoped_client(&self, authorization: &SecretString) -> Result<NoAuthOpenAiClient> {
        Self::build_client(
            &self.upstream_base_url,
            self.http_client.clone(),
            caller_auth_headers(authorization)?,
        )
    }

    fn build_client(
        upstream_base_url: &str,
        http_client: reqwest::Client,
        headers: http_client::HeaderMap,
    ) -> Result<NoAuthOpenAiClient> {
        NoAuthOpenAiClient::builder()
            .api_key(Nothing)
            .base_url(upstream_base_url)
            .http_client(http_client)
            .http_headers(headers)
            .build()
            .map_err(ProviderClientError::from)
            .wrap_err("failed to build Rig client")
    }
}

impl RigModelBackend {
    /// # Errors
    /// Returns [`ModelError`] if the request cannot be built or sent, the
    /// upstream returns a non-success status, or the body is not valid JSON.
    pub async fn complete(&self, request: ModelRequest) -> Result<Value, ModelError> {
        let client = self
            .client(request.caller_authorization)
            .map_err(ModelError::request)?;
        let body = serde_json::to_vec(&request.body).map_err(ModelError::request)?;

        // Bounded retry on transient upstream failures. Total attempts =
        // 1 + max_retries; backoff doubles from a base, capped per the limit.
        let mut attempt: usize = 0;
        loop {
            let outcome = self.send_buffered(&client, &body).await;
            let error = match outcome {
                Ok(value) => return Ok(value),
                Err(error) => error,
            };

            if attempt >= self.max_retries || !error.is_transient() {
                return Err(error);
            }

            tokio::time::sleep(retry_backoff(attempt)).await;
            attempt += 1;
        }
    }

    /// One buffered attempt: build, send, and parse the response body.
    async fn send_buffered(
        &self,
        client: &NoAuthOpenAiClient,
        body: &[u8],
    ) -> Result<Value, ModelError> {
        let request = client
            .post(CHAT_COMPLETIONS_API_PATH)
            .map_err(ModelError::request)?
            .body(body.to_vec())
            .map_err(ModelError::request)?;

        let response = match client.send::<_, Vec<u8>>(request).await {
            Ok(response) => response,
            Err(http_client::Error::InvalidStatusCodeWithMessage(status, message)) => {
                return Err(ModelError::Api {
                    status: Some(status.as_u16()),
                    message,
                });
            }
            Err(error) => return Err(ModelError::Request(error.to_string())),
        };
        let text = http_client::text(response)
            .await
            .map_err(ModelError::request)?;
        serde_json::from_str(&text).map_err(ModelError::InvalidJson)
    }

    /// Sends `request` to the upstream chat-completions endpoint and returns the
    /// raw [`reqwest::Response`] without buffering its body, so streaming callers
    /// can pipe the upstream SSE stream through unmodified.
    ///
    /// # Errors
    /// Returns [`ModelError::Request`] if the request cannot be built or sent,
    /// or [`ModelError::Api`] if the upstream returns a non-success status.
    pub async fn stream(&self, request: ModelRequest) -> Result<reqwest::Response, ModelError> {
        let url = format!("{}{CHAT_COMPLETIONS_API_PATH}", self.upstream_base_url);
        let mut builder = self.stream_client.post(url).json(&request.body);

        if let Some(authorization) = stream_authorization(
            request.caller_authorization.as_ref(),
            self.configured_api_key.as_ref(),
        )? {
            builder = builder.header(reqwest::header::AUTHORIZATION, authorization);
        }

        let response = builder.send().await.map_err(ModelError::request)?;
        if response.status().is_success() {
            return Ok(response);
        }

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        Err(ModelError::Api {
            status: Some(status.as_u16()),
            message: format!("{status}: {body}"),
        })
    }
}

/// Resolves the authorization header value for a streaming passthrough request,
/// preferring the configured upstream key over the caller's header to match
/// `complete`'s precedence.
fn stream_authorization(
    caller_authorization: Option<&SecretString>,
    configured_api_key: Option<&SecretString>,
) -> Result<Option<reqwest::header::HeaderValue>, ModelError> {
    let raw = match (configured_api_key, caller_authorization) {
        (Some(api_key), _) => format!("Bearer {}", api_key.expose_secret()),
        (None, Some(caller)) => caller.expose_secret().to_owned(),
        (None, None) => return Ok(None),
    };

    let mut value = reqwest::header::HeaderValue::from_str(&raw).map_err(|_| {
        ModelError::Request(
            "authorization header cannot be represented as an HTTP header".to_owned(),
        )
    })?;
    value.set_sensitive(true);
    Ok(Some(value))
}

impl ModelError {
    fn request(error: impl Display) -> Self {
        Self::Request(error.to_string())
    }
}

fn configured_auth_headers(api_key: Option<&SecretString>) -> Result<http_client::HeaderMap> {
    let mut headers = http_client::HeaderMap::new();
    if let Some(api_key) = api_key {
        let (name, value) = http_client::make_auth_header(api_key.expose_secret())
            .wrap_err("upstream API key cannot be represented as an HTTP header")?;
        headers.insert(name, value);
    }
    Ok(headers)
}

fn caller_auth_headers(authorization: &SecretString) -> Result<http_client::HeaderMap> {
    let mut headers = http_client::HeaderMap::new();
    let value = http_client::HeaderValue::from_str(authorization.expose_secret())
        .wrap_err("caller authorization header cannot be represented as an HTTP header")?;
    headers.insert("authorization", value);
    Ok(headers)
}

type NoAuthOpenAiClient = rig_core::client::Client<NoAuthOpenAiCompatibleExt, reqwest::Client>;

#[derive(Debug, Default, Clone, Copy)]
struct NoAuthOpenAiCompatibleExt;

#[derive(Debug, Default, Clone, Copy)]
struct NoAuthOpenAiCompatibleExtBuilder;

impl Provider for NoAuthOpenAiCompatibleExt {
    type Builder = NoAuthOpenAiCompatibleExtBuilder;
    const VERIFY_PATH: &'static str = "/models";
}

impl DebugExt for NoAuthOpenAiCompatibleExt {}

impl ProviderBuilder for NoAuthOpenAiCompatibleExtBuilder {
    type Extension<H>
        = NoAuthOpenAiCompatibleExt
    where
        H: http_client::HttpClientExt;
    type ApiKey = Nothing;

    const BASE_URL: &'static str = "https://api.openai.com/v1";

    fn build<H>(
        _builder: &rig_core::client::ClientBuilder<Self, Self::ApiKey, H>,
    ) -> http_client::Result<Self::Extension<H>>
    where
        H: http_client::HttpClientExt,
    {
        Ok(NoAuthOpenAiCompatibleExt)
    }
}
