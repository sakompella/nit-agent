pub mod config;

use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use futures_util::stream::{self, Stream};
use reqwest::{Client, Url};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_stream::StreamExt as _;

const DEFAULT_UPSTREAM_BASE_URL: &str = "http://localhost:20128/v1";
const DEFAULT_LISTEN_ADDRESS: &str = "127.0.0.1:3000";

#[derive(Clone, Debug)]
pub struct AppConfig {
    listen: SocketAddr,
    upstream_chat_completions_url: String,
    upstream_api_key: Option<String>,
}

impl AppConfig {
    /// Creates a parsed config so request handlers do not have to re-validate static inputs.
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
        let listen = DEFAULT_LISTEN_ADDRESS
            .parse()
            .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], 3000)));

        Self::new(listen, DEFAULT_UPSTREAM_BASE_URL, None).unwrap_or_else(|_| Self {
            listen,
            upstream_chat_completions_url: format!("{DEFAULT_UPSTREAM_BASE_URL}/chat/completions"),
            upstream_api_key: None,
        })
    }
}

#[derive(Clone)]
pub struct ChatProxyState {
    config: AppConfig,
    client: Client,
}

impl ChatProxyState {
    #[must_use]
    pub fn new(config: AppConfig, client: Client) -> Self {
        Self { config, client }
    }
}

pub fn build_router(state: ChatProxyState) -> Router {
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

pub fn normalize_upstream_url(upstream_base_url: &str) -> Result<String, String> {
    let trimmed = upstream_base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("upstream base URL cannot be empty".to_owned());
    }

    let url = Url::parse(&format!("{trimmed}/chat/completions"))
        .map_err(|error| format!("invalid upstream base URL: {error}"))?;
    Ok(url.to_string())
}

pub fn uppercase_request_message_text(request: &mut Value) {
    let Some(messages) = request.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };

    for message in messages {
        let Some(content) = message.get_mut("content") else {
            continue;
        };
        transform_content_text(content, |text| text.to_uppercase());
    }
}

pub fn lowercase_assistant_output(response: &mut Value) {
    let Some(choices) = response.get_mut("choices").and_then(Value::as_array_mut) else {
        return;
    };

    for choice in choices {
        let Some(message) = choice.get_mut("message") else {
            continue;
        };
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(content) = message.get_mut("content") else {
            continue;
        };
        transform_content_text(content, |text| text.to_lowercase());
    }
}

pub fn tokenize_whitespace(text: &str) -> Vec<String> {
    text.split_whitespace().map(ToOwned::to_owned).collect()
}

fn transform_content_text(content: &mut Value, transform: fn(&str) -> String) {
    match content {
        Value::String(text) => {
            *text = transform(text);
        }
        Value::Array(parts) => {
            for part in parts {
                let is_text_part = part.get("type").and_then(Value::as_str) == Some("text");
                if !is_text_part {
                    continue;
                }
                if let Some(Value::String(text)) = part.get_mut("text") {
                    *text = transform(text);
                }
            }
        }
        _ => {}
    }
}

async fn chat_completions(
    State(state): State<ChatProxyState>,
    headers: HeaderMap,
    Json(mut request): Json<Value>,
) -> Response {
    let wants_stream = request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    uppercase_request_message_text(&mut request);
    request["stream"] = Value::Bool(false);

    let upstream = forward_to_upstream(&state, &headers, &request).await;
    let mut response = match upstream {
        Ok(response) => response,
        Err(error) => return gateway_error("upstream_error", error).into_response(),
    };

    lowercase_assistant_output(&mut response);

    if wants_stream {
        stream_response(response).into_response()
    } else {
        Json(response).into_response()
    }
}

async fn forward_to_upstream(
    state: &ChatProxyState,
    headers: &HeaderMap,
    request: &Value,
) -> Result<Value, String> {
    let mut builder = state
        .client
        .post(&state.config.upstream_chat_completions_url)
        .json(request);

    if let Some(api_key) = &state.config.upstream_api_key {
        builder = builder.bearer_auth(api_key);
    } else if let Some(authorization) = headers.get(header::AUTHORIZATION) {
        builder = builder.header(header::AUTHORIZATION, authorization);
    }

    let response = builder
        .send()
        .await
        .map_err(|error| format!("upstream request failed: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("upstream returned {status}: {body}"));
    }

    response
        .json::<Value>()
        .await
        .map_err(|error| format!("upstream returned invalid JSON: {error}"))
}

fn stream_response(response: Value) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let id = response
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("chatcmpl-proxy")
        .to_owned();
    let model = response
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let created = response.get("created").and_then(Value::as_i64).unwrap_or(0);
    let tokens = assistant_text(&response)
        .map(tokenize_whitespace)
        .unwrap_or_default();

    let mut events = Vec::with_capacity(tokens.len() + 2);
    for token in tokens {
        let chunk = json!({
            "id": id,
            "object": "chat.completion.chunk",
            "created": created,
            "model": model,
            "choices": [
                {
                    "index": 0,
                    "delta": { "content": token },
                    "finish_reason": null
                }
            ]
        });
        events.push(Ok(Event::default().data(chunk.to_string())));
    }

    let done = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [
            {
                "index": 0,
                "delta": {},
                "finish_reason": "stop"
            }
        ]
    });
    events.push(Ok(Event::default().data(done.to_string())));
    events.push(Ok(Event::default().data("[DONE]")));

    Sse::new(stream::iter(events).throttle(Duration::from_millis(100)))
        .keep_alive(KeepAlive::default())
}

fn assistant_text(response: &Value) -> Option<&str> {
    response
        .get("choices")?
        .as_array()?
        .iter()
        .filter_map(|choice| choice.get("message"))
        .find(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))?
        .get("content")?
        .as_str()
}

fn gateway_error(kind: &str, message: String) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({
            "error": {
                "type": kind,
                "message": message
            }
        })),
    )
}
