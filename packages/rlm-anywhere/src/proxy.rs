use std::convert::Infallible;
use std::time::Duration;

use async_openai::error::OpenAIError;
use async_openai::traits::RequestOptionsBuilder as _;
use async_openai::types::chat::{CreateChatCompletionRequest, CreateChatCompletionResponse, Role};
use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::stream;
use serde_json::{Value, json};
use tokio_stream::StreamExt as _;

use crate::app::AppState;
use crate::transform::{lowercase_assistant_output, uppercase_request_message_text};

const KNOWN_CHAT_COMPLETION_REQUEST_FIELDS: &[&str] = &[
    "audio",
    "frequency_penalty",
    "function_call",
    "functions",
    "logit_bias",
    "logprobs",
    "max_completion_tokens",
    "max_tokens",
    "messages",
    "metadata",
    "modalities",
    "model",
    "n",
    "parallel_tool_calls",
    "prediction",
    "presence_penalty",
    "prompt_cache_key",
    "reasoning_effort",
    "response_format",
    "safety_identifier",
    "seed",
    "service_tier",
    "stop",
    "store",
    "stream",
    "stream_options",
    "temperature",
    "tool_choice",
    "tools",
    "top_logprobs",
    "top_p",
    "user",
    "verbosity",
    "web_search_options",
];

pub(crate) async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut request = match parse_chat_completion_request(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let wants_stream = request.stream.unwrap_or(false);

    uppercase_request_message_text(&mut request);
    request.stream = Some(false);

    let mut chat = state.client.chat();
    if state.config.upstream_api_key.is_none() {
        if let Some(authorization) = headers.get(header::AUTHORIZATION) {
            let Ok(authorization) = authorization.to_str() else {
                return upstream_error(
                    "upstream request failed: caller authorization header is not valid text"
                        .to_owned(),
                );
            };
            chat = match chat.header(reqwest::header::AUTHORIZATION, authorization) {
                Ok(chat) => chat,
                Err(error) => return upstream_error(format!("upstream request failed: {error}")),
            };
        }
    }

    let mut response = match chat.create(request).await {
        Ok(response) => response,
        Err(error) => return upstream_openai_error(error),
    };

    lowercase_assistant_output(&mut response);

    if wants_stream {
        stream_response(response)
    } else {
        Json(response).into_response()
    }
}

fn parse_chat_completion_request(body: &[u8]) -> Result<CreateChatCompletionRequest, Response> {
    let value = serde_json::from_slice::<Value>(body).map_err(|error| {
        invalid_request(format!("invalid JSON chat completion request: {error}"))
    })?;
    reject_unknown_top_level_fields(&value)?;
    serde_json::from_value(value).map_err(|error| {
        invalid_request(format!("invalid chat completion request schema: {error}"))
    })
}

fn reject_unknown_top_level_fields(value: &Value) -> Result<(), Response> {
    let Some(object) = value.as_object() else {
        return Err(invalid_request(
            "chat completion request must be a JSON object".to_owned(),
        ));
    };
    for field in object.keys() {
        if !KNOWN_CHAT_COMPLETION_REQUEST_FIELDS.contains(&field.as_str()) {
            return Err(invalid_request(format!(
                "unsupported top-level request field: {field}"
            )));
        }
    }
    Ok(())
}

fn invalid_request(message: String) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({
            "error": {
                "type": "invalid_request",
                "message": message
            }
        })),
    )
        .into_response()
}

fn upstream_openai_error(error: OpenAIError) -> Response {
    match error {
        OpenAIError::Reqwest(error) => upstream_error(format!("upstream request failed: {error}")),
        OpenAIError::ApiError(error) => {
            upstream_error(format!("upstream returned API error: {error}"))
        }
        OpenAIError::JSONDeserialize(error, _) => {
            upstream_error(format!("upstream returned invalid JSON: {error}"))
        }
        OpenAIError::StreamError(error) => {
            upstream_error(format!("upstream stream failed: {error}"))
        }
        OpenAIError::InvalidArgument(message) => {
            upstream_error(format!("upstream request failed: {message}"))
        }
        #[cfg(not(target_family = "wasm"))]
        OpenAIError::FileSaveError(error) => {
            upstream_error(format!("upstream file save failed: {error}"))
        }
        #[cfg(not(target_family = "wasm"))]
        OpenAIError::FileReadError(error) => {
            upstream_error(format!("upstream file read failed: {error}"))
        }
    }
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

fn stream_response(response: CreateChatCompletionResponse) -> Response {
    let id = response.id;
    let model = response.model;
    let created = response.created;
    let tokens = response
        .choices
        .iter()
        .filter(|choice| choice.message.role == Role::Assistant)
        .find_map(|choice| choice.message.content.as_deref())
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
