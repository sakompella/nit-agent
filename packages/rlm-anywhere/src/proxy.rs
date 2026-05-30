use std::convert::Infallible;
use std::time::Duration;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::stream;
use secrecy::SecretString;
use serde_json::{Value, json};
use thiserror::Error;
use tokio_stream::StreamExt as _;

use crate::app::AppState;
use crate::config::PassthroughStatus;
use crate::transform::{lowercase_assistant_output, uppercase_request_message_text};
use crate::upstream::{ModelBackend as _, ModelError, ModelRequest};
use crate::validation::{self, ValidationError};

pub(crate) async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match state.config.mode() {
        PassthroughStatus::Rlm => rlm_chat_completions(state, headers, body).await,
        PassthroughStatus::Passthrough => passthrough_chat_completions(state, headers, body).await,
    }
}

async fn rlm_chat_completions(state: AppState, headers: HeaderMap, body: Bytes) -> Response {
    let mut request = match parse_chat_completion_request(&body) {
        Ok(request) => request,
        Err(error) => return invalid_request(error),
    };
    let wants_stream = request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    uppercase_request_message_text(&mut request);
    set_stream(&mut request, false);

    let Ok(caller_authorization) =
        caller_authorization(&headers, state.config.upstream_has_configured_api_key())
    else {
        return upstream_error(
            "upstream request failed: caller authorization header is not valid text".to_owned(),
        );
    };

    state
        .model_backend
        .complete(ModelRequest {
            body: request,
            caller_authorization,
        })
        .await
        .map_or_else(upstream_model_error, |mut response| {
            lowercase_assistant_output(&mut response);
            if wants_stream {
                stream_response(response)
            } else {
                Json(response).into_response()
            }
        })
}

async fn passthrough_chat_completions(
    state: AppState,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut request = match serde_json::from_slice::<Value>(&body) {
        Ok(request) => request,
        Err(error) => return invalid_request(InvalidRequestError::InvalidJson(error)),
    };
    let wants_stream = request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    // The current upstream backend only returns complete JSON responses. Keep
    // caller-facing stream compatibility by synthesizing SSE after a non-stream
    // upstream request.
    set_stream(&mut request, false);

    let Ok(caller_authorization) =
        caller_authorization(&headers, state.config.upstream_has_configured_api_key())
    else {
        return upstream_error(
            "upstream request failed: caller authorization header is not valid text".to_owned(),
        );
    };

    state
        .model_backend
        .complete(ModelRequest {
            body: request,
            caller_authorization,
        })
        .await
        .map_or_else(upstream_model_error, |response| {
            if wants_stream {
                stream_response(response)
            } else {
                Json(response).into_response()
            }
        })
}

fn caller_authorization(
    headers: &HeaderMap,
    has_configured_api_key: bool,
) -> Result<Option<SecretString>, ()> {
    if has_configured_api_key {
        return Ok(None);
    }

    headers
        .get(header::AUTHORIZATION)
        .map(|authorization| {
            authorization
                .to_str()
                .map(SecretString::from)
                .map_err(|_| ())
        })
        .transpose()
}

/// Keep the first milestone's fake-SSE behavior by forcing the upstream call to
/// be non-streaming while preserving the caller's original stream preference.
fn set_stream(request: &mut Value, stream: bool) {
    let Some(object) = request.as_object_mut() else {
        return;
    };
    object.insert("stream".to_owned(), Value::Bool(stream));
}

#[derive(Debug, Error)]
enum InvalidRequestError {
    #[error("invalid JSON chat completion request: {0}")]
    InvalidJson(serde_json::Error),
    #[error("invalid chat completion request schema: {0}")]
    InvalidSchema(serde_json::Error),
    #[error("unsupported request field: {path}")]
    UnsupportedField { path: String },
}

fn parse_chat_completion_request(body: &[u8]) -> Result<Value, InvalidRequestError> {
    let value = serde_json::from_slice::<Value>(body).map_err(InvalidRequestError::InvalidJson)?;
    validation::validate_chat_completion_request(value.clone()).map_err(validation_error)?;
    Ok(value)
}

fn validation_error(error: ValidationError) -> InvalidRequestError {
    match error {
        ValidationError::InvalidJson(error) => InvalidRequestError::InvalidJson(error),
        ValidationError::InvalidSchema(error) => InvalidRequestError::InvalidSchema(error),
        ValidationError::UnsupportedField { path } => {
            InvalidRequestError::UnsupportedField { path }
        }
    }
}

fn invalid_request(error: InvalidRequestError) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "error": {
                "type": "invalid_request",
                "message": error.to_string()
            }
        })),
    )
        .into_response()
}

/// Translate backend errors into the stable public 502 envelope.
fn upstream_model_error(error: ModelError) -> Response {
    let message = match error {
        ModelError::Request(message) => format!("upstream request failed: {message}"),
        ModelError::Api(message) => format!("upstream returned API error: {message}"),
        ModelError::InvalidJson(error) => format!("upstream returned invalid JSON: {error}"),
    };
    upstream_error(message)
}

fn upstream_error(message: String) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({
            "error": {
                "type": "upstream_error",
                "message": message
            }
        })),
    )
        .into_response()
}

fn stream_response(response: Value) -> Response {
    let id = response
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let model = response
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let created = response.get("created").and_then(Value::as_u64).unwrap_or(0);
    let tokens = response
        .get("choices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|choice| {
            choice
                .get("message")
                .and_then(|message| message.get("role"))
                .and_then(Value::as_str)
                == Some("assistant")
        })
        .find_map(|choice| {
            choice
                .get("message")
                .and_then(|message| message.get("content"))
                .and_then(Value::as_str)
        })
        .map(|text| {
            text.split_whitespace()
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut events: Vec<Result<Event, Infallible>> = Vec::with_capacity(tokens.len() + 2);
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
        .into_response()
}
