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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use axum::Router;
    use axum::body::Bytes;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::post;
    use reqwest::Client;
    use serde_json::{Value, json};
    use tokio::net::TcpListener;

    use crate::app::{AppConfig, ChatProxyState, build_router};

    type FakeUpstreamState = (StatusCode, Value, Arc<Mutex<Option<Value>>>);

    #[tokio::test]
    async fn non_stream_request_forwards_capitalized_input_and_returns_lowercase_json() {
        let seen = Arc::new(Mutex::new(None));
        let upstream_url =
            spawn_fake_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
        let proxy_url = spawn_proxy(format!("{upstream_url}/v1")).await;

        let response = Client::new()
            .post(format!("{proxy_url}/v1/chat/completions"))
            .json(&json!({
                "model": "local-model",
                "messages": [{ "role": "user", "content": "hello upstream" }]
            }))
            .send()
            .await
            .expect("proxy request should complete");

        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.expect("response body should be json");
        assert_eq!(
            body["choices"][0]["message"]["content"],
            "hello from upstream"
        );

        let seen = seen
            .lock()
            .expect("seen request lock should be available")
            .take()
            .expect("upstream should receive request");
        assert_eq!(seen["messages"][0]["content"], "HELLO UPSTREAM");
        assert_eq!(seen["stream"], false);
    }

    #[tokio::test]
    async fn stream_request_returns_sse_chunks_and_done() {
        let seen = Arc::new(Mutex::new(None));
        let upstream_url = spawn_fake_upstream(StatusCode::OK, upstream_response(), seen).await;
        let proxy_url = spawn_proxy(format!("{upstream_url}/v1")).await;

        let response = Client::new()
            .post(format!("{proxy_url}/v1/chat/completions"))
            .json(&json!({
                "model": "local-model",
                "stream": true,
                "messages": [{ "role": "user", "content": "hello upstream" }]
            }))
            .send()
            .await
            .expect("proxy request should complete");

        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert!(content_type.starts_with("text/event-stream"));

        let body = response
            .text()
            .await
            .expect("stream response body should be readable");
        assert!(body.contains(r#""content":"hello""#));
        assert!(body.contains(r#""content":"from""#));
        assert!(body.contains(r#""content":"upstream""#));
        assert!(body.trim_end().ends_with("data: [DONE]"));
    }

    #[tokio::test]
    async fn upstream_non_success_returns_gateway_error() {
        let seen = Arc::new(Mutex::new(None));
        let upstream_url =
            spawn_fake_upstream(StatusCode::BAD_GATEWAY, json!({ "error": "broken" }), seen).await;
        let proxy_url = spawn_proxy(format!("{upstream_url}/v1")).await;

        let response = Client::new()
            .post(format!("{proxy_url}/v1/chat/completions"))
            .json(&json!({
                "model": "local-model",
                "messages": [{ "role": "user", "content": "hello upstream" }]
            }))
            .send()
            .await
            .expect("proxy request should complete");

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body: Value = response.json().await.expect("error body should be json");
        assert_eq!(body["error"]["type"], "upstream_error");
    }

    async fn spawn_proxy(upstream_base_url: String) -> String {
        let config = AppConfig::new(
            "127.0.0.1:0"
                .parse()
                .expect("test listen addr should parse"),
            upstream_base_url,
            None,
        )
        .expect("proxy config should be valid");
        let state = ChatProxyState::new(config, Client::new());
        spawn_router(build_router(state)).await
    }

    async fn spawn_fake_upstream(
        status: StatusCode,
        response: Value,
        seen: Arc<Mutex<Option<Value>>>,
    ) -> String {
        async fn handler(
            State((status, response, seen)): State<FakeUpstreamState>,
            headers: HeaderMap,
            body: Bytes,
        ) -> (StatusCode, axum::Json<Value>) {
            let request: Value =
                serde_json::from_slice(&body).expect("upstream request should be json");
            assert_eq!(
                headers
                    .get("content-type")
                    .and_then(|value| value.to_str().ok()),
                Some("application/json")
            );
            *seen.lock().expect("seen lock should be available") = Some(request);
            (status, axum::Json(response))
        }

        let router = Router::new()
            .route("/v1/chat/completions", post(handler))
            .with_state((status, response, seen));
        spawn_router(router).await
    }

    async fn spawn_router(router: Router) -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener
            .local_addr()
            .expect("test listener should have addr");
        tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                })
                .await
                .expect("test server should run");
        });
        format!("http://{}:{}", address.ip(), address.port())
    }

    fn upstream_response() -> Value {
        json!({
            "id": "chatcmpl-test",
            "object": "chat.completion",
            "created": 0,
            "model": "local-model",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "HELLO FROM UPSTREAM"
                    },
                    "finish_reason": "stop"
                }
            ]
        })
    }
}
