use std::convert::Infallible;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
use crate::config::RequestMode;
use crate::rlm::driver::{
    LoopInput, RlmError, extract_sampling, run_loop, split_query_and_context,
};
use crate::upstream::{ModelError, ModelRequest};
use crate::validation::{self, ValidationError};

const INVALID_REQUEST_ERROR_TYPE: &str = "invalid_request";
const UPSTREAM_ERROR_TYPE: &str = "upstream_error";
const RLM_ERROR_TYPE: &str = "rlm_error";
const RLM_COMPLETION_ID: &str = "chatcmpl-rlm";

pub async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match state.config.mode {
        RequestMode::Rlm => rlm_chat_completions(state, headers, body).await,
        RequestMode::Passthrough => passthrough_chat_completions(state, headers, body).await,
    }
}

async fn rlm_chat_completions(state: AppState, headers: HeaderMap, body: Bytes) -> Response {
    let request = match parse_chat_completion_request(&body) {
        Ok(request) => request,
        Err(error) => return invalid_request(&error),
    };
    let wants_stream = request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let Ok(caller_authorization) =
        caller_authorization(&headers, state.config.upstream_has_configured_api_key())
    else {
        return upstream_error(
            "upstream request failed: caller authorization header is not valid text",
        );
    };

    if has_caller_tools(&request) {
        return forward_to_upstream(&state, request, caller_authorization, wants_stream).await;
    }

    let Some(model) = request
        .get("model")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
    else {
        return invalid_request(&InvalidRequestError::MissingField { field: "model" });
    };
    let Some(messages) = request.get("messages").and_then(Value::as_array) else {
        return invalid_request(&InvalidRequestError::MissingField { field: "messages" });
    };
    let Some(split) = split_query_and_context(messages) else {
        return invalid_request(&InvalidRequestError::MissingUserMessage);
    };

    let input = LoopInput {
        model: model.clone(),
        query_message: split.query_message,
        context: split.context,
        sampling: extract_sampling(&request),
        caller_authorization,
    };

    if wants_stream {
        return stream_rlm_loop(state, input);
    }

    run_loop(&state.model_backend, &state.config.rlm, input)
        .await
        .map_or_else(rlm_loop_error, |answer| {
            Json(rlm_chat_completion_response(&model, &answer)).into_response()
        })
}

async fn passthrough_chat_completions(
    state: AppState,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let request = match serde_json::from_slice::<Value>(&body) {
        Ok(request) => request,
        Err(error) => return invalid_request(&InvalidRequestError::InvalidJson(error)),
    };
    let wants_stream = request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let Ok(caller_authorization) =
        caller_authorization(&headers, state.config.upstream_has_configured_api_key())
    else {
        return upstream_error(
            "upstream request failed: caller authorization header is not valid text",
        );
    };

    forward_to_upstream(&state, request, caller_authorization, wants_stream).await
}

fn has_caller_tools(request: &Value) -> bool {
    request.as_object().is_some_and(|object| {
        object.get("tools").is_some_and(|value| !value.is_null())
            || object
                .get("tool_choice")
                .is_some_and(|value| !value.is_null())
    })
}

async fn forward_to_upstream(
    state: &AppState,
    mut request: Value,
    caller_authorization: Option<SecretString>,
    wants_stream: bool,
) -> Response {
    set_stream(&mut request, false);

    state
        .model_backend
        .complete(ModelRequest {
            body: request,
            caller_authorization,
        })
        .await
        .map_or_else(
            |error| upstream_model_error(&error),
            |response| {
                if wants_stream {
                    stream_response(&response)
                } else {
                    Json(response).into_response()
                }
            },
        )
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
    #[error("missing required field: {field}")]
    MissingField { field: &'static str },
    #[error("rlm mode requires at least one user message")]
    MissingUserMessage,
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

fn invalid_request(error: &InvalidRequestError) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(rlm_or_upstream_body(
            INVALID_REQUEST_ERROR_TYPE,
            &error.to_string(),
        )),
    )
        .into_response()
}

fn upstream_model_error(error: &ModelError) -> Response {
    (StatusCode::BAD_GATEWAY, Json(upstream_error_body(error))).into_response()
}

fn upstream_error(message: &str) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(rlm_or_upstream_body(UPSTREAM_ERROR_TYPE, message)),
    )
        .into_response()
}

fn upstream_error_body(error: &ModelError) -> Value {
    json!({
        "error": {
            "type": UPSTREAM_ERROR_TYPE,
            "message": error.to_string()
        }
    })
}

fn rlm_loop_error(error: RlmError) -> Response {
    let (status, body) = rlm_error_parts(error);

    (status, Json(body)).into_response()
}

fn rlm_or_upstream_body(error_type: &str, message: &str) -> Value {
    json!({
        "error": {
            "type": error_type,
            "message": message
        }
    })
}

fn rlm_error_parts(error: RlmError) -> (StatusCode, Value) {
    match error {
        RlmError::Budget(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            rlm_or_upstream_body(RLM_ERROR_TYPE, &format!("rlm loop budget exhausted: {e}")),
        ),
        RlmError::WallClock { budget } => (
            StatusCode::INTERNAL_SERVER_ERROR,
            rlm_or_upstream_body(
                RLM_ERROR_TYPE,
                &format!("rlm loop exceeded wall clock budget of {budget:?}"),
            ),
        ),
        RlmError::Upstream(e) => (StatusCode::BAD_GATEWAY, upstream_error_body(&e)),
        RlmError::MalformedCompletion { detail } => (
            StatusCode::BAD_GATEWAY,
            rlm_or_upstream_body(
                UPSTREAM_ERROR_TYPE,
                &format!("upstream returned a malformed chat completion: {detail}"),
            ),
        ),
    }
}

#[must_use]
fn rlm_chat_completion_response(model: &str, answer: &str) -> Value {
    json!({
        "id": RLM_COMPLETION_ID,
        "object": "chat.completion",
        "created": unix_timestamp_secs(),
        "model": model,
        "choices": [
            {
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": answer
                },
                "finish_reason": "stop"
            }
        ],
        "usage": {
            "prompt_tokens": 0,
            "completion_tokens": 0,
            "total_tokens": 0
        }
    })
}

fn stream_rlm_loop(state: AppState, input: LoopInput) -> Response {
    let model = input.model.clone();
    let created = unix_timestamp_secs();
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(4);

    tokio::spawn(async move {
        let result = run_loop(&state.model_backend, &state.config.rlm, input).await;
        match result {
            Ok(answer) => {
                if tx
                    .send(Ok(Event::default().data(
                        content_delta_chunk(RLM_COMPLETION_ID, created, &model, &answer)
                            .to_string(),
                    )))
                    .await
                    .is_err()
                {
                    return;
                }

                if tx
                    .send(Ok(Event::default().data(
                        stop_chunk(RLM_COMPLETION_ID, created, &model).to_string(),
                    )))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Err(error) => {
                let (_, error_body) = rlm_error_parts(error);
                if tx
                    .send(Ok(Event::default().data(error_body.to_string())))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }

        let _ = tx.send(Ok(Event::default().data("[DONE]"))).await;
    });

    Sse::new(tokio_stream::wrappers::ReceiverStream::new(rx))
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(10)))
        .into_response()
}

fn stream_response(response: &Value) -> Response {
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
    let assistant_content = response
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
        .map(ToOwned::to_owned);

    let mut events: Vec<Result<Event, Infallible>> = Vec::with_capacity(3);
    if let Some(content) = assistant_content {
        events.push(Ok(Event::default().data(
            content_delta_chunk(&id, created, &model, &content).to_string(),
        )));
    }

    events.push(Ok(
        Event::default().data(stop_chunk(&id, created, &model).to_string())
    ));
    events.push(Ok(Event::default().data("[DONE]")));

    Sse::new(
        stream::iter(events)
            .throttle(Duration::from_millis(100))
            .map(|result| result),
    )
    .into_response()
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn completion_chunk(
    id: &str,
    created: u64,
    model: &str,
    delta: &Value,
    finish_reason: &Value,
) -> Value {
    json!({
        "id": id,
        "object": "chat.completion.chunk",
        "created": created,
        "model": model,
        "choices": [
            {
                "index": 0,
                "delta": delta,
                "finish_reason": finish_reason
            }
        ]
    })
}

fn content_delta_chunk(id: &str, created: u64, model: &str, content: &str) -> Value {
    completion_chunk(
        id,
        created,
        model,
        &json!({ "content": content }),
        &Value::Null,
    )
}

fn stop_chunk(id: &str, created: u64, model: &str) -> Value {
    completion_chunk(id, created, model, &json!({}), &json!("stop"))
}
