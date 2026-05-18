use std::convert::Infallible;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::stream;
use serde_json::{Value, json};
use tokio_stream::StreamExt as _;

use crate::app::ChatProxyState;
use crate::transform::{lowercase_assistant_output, uppercase_request_message_text};

pub(crate) async fn chat_completions(
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

    let mut builder = state
        .client
        .post(&state.config.upstream_chat_completions_url)
        .json(&request);

    if let Some(api_key) = &state.config.upstream_api_key {
        builder = builder.bearer_auth(api_key);
    } else if let Some(authorization) = headers.get(header::AUTHORIZATION) {
        builder = builder.header(header::AUTHORIZATION, authorization);
    }

    let response = match builder.send().await {
        Ok(response) => response,
        Err(error) => return upstream_error(format!("upstream request failed: {error}")),
    };
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return upstream_error(format!("upstream returned {status}: {body}"));
    }

    let mut response = match response.json::<Value>().await {
        Ok(response) => response,
        Err(error) => return upstream_error(format!("upstream returned invalid JSON: {error}")),
    };

    lowercase_assistant_output(&mut response);

    if wants_stream {
        stream_response(response)
    } else {
        Json(response).into_response()
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

fn stream_response(response: Value) -> Response {
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
    let tokens = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| {
            choices
                .iter()
                .filter_map(|choice| choice.get("message"))
                .find(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        })
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
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
