#![expect(
    clippy::result_large_err,
    reason = "figment Jail test closures return figment's native error type"
)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use figment::Jail;
use reqwest::Client;
use rlm_anywhere::{AppConfig, build_router};
use serde_json::{Value, json};
use tokio::net::TcpListener;

#[derive(Clone, Debug)]
struct RecordedRequest {
    authorization: Option<String>,
    content_type: Option<String>,
    body: Value,
}

type RecordedRequestSlot = Arc<Mutex<Option<RecordedRequest>>>;
type FakeJsonUpstreamState = (StatusCode, Value, RecordedRequestSlot);
type FakeRawUpstreamState = (StatusCode, &'static str, RecordedRequestSlot);

#[tokio::test]
async fn typed_request_transform_uppercases_multimodal_text_and_forces_non_stream_upstream() {
    let seen = Arc::new(Mutex::new(None));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy(format!("{upstream_url}/v1/"), None).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [
                {
                    "role": "system",
                    "content": "stay concise"
                },
                {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "hello upstream" },
                        { "type": "image_url", "image_url": { "url": "https://example.test/image.png" } }
                    ]
                },
                {
                    "role": "tool",
                    "tool_call_id": "call_123",
                    "content": "tool output stays mixed CASE"
                }
            ],
            "stream": true,
            "tool_choice": "auto"
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::OK);
    let seen = take_seen(&seen);
    assert_eq!(seen.content_type.as_deref(), Some("application/json"));
    assert_eq!(seen.body["model"], "local-model");
    assert_eq!(seen.body["tool_choice"], "auto");
    assert_eq!(seen.body["messages"][0]["role"], "system");
    assert_eq!(seen.body["messages"][0]["content"], "STAY CONCISE");
    assert_eq!(
        seen.body["messages"][1]["content"][0]["text"],
        "HELLO UPSTREAM"
    );
    assert_eq!(
        seen.body["messages"][1]["content"][1]["image_url"]["url"],
        "https://example.test/image.png"
    );
    assert_eq!(
        seen.body["messages"][2]["content"],
        "tool output stays mixed CASE"
    );
    assert_eq!(seen.body["stream"], false);
}

#[tokio::test]
async fn unknown_top_level_request_field_is_rejected_before_upstream() {
    let seen = Arc::new(Mutex::new(None));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy(format!("{upstream_url}/v1"), None).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{ "role": "user", "content": "hello upstream" }],
            "x_unknown": "reject me"
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.expect("error body should be json");
    assert_eq!(body["error"]["type"], "invalid_request");
    assert!(
        body["error"]["message"]
            .as_str()
            .expect("error message should be a string")
            .contains("x_unknown")
    );
    assert!(
        seen.lock()
            .expect("seen lock should be available")
            .is_none()
    );
}

#[tokio::test]
async fn nested_unknown_request_field_is_rejected_before_upstream() {
    let seen = Arc::new(Mutex::new(None));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy(format!("{upstream_url}/v1"), None).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [
                {
                    "role": "user",
                    "content": "hello upstream",
                    "x_unknown": "reject me"
                }
            ]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.expect("error body should be json");
    assert_eq!(body["error"]["type"], "invalid_request");
    let message = body["error"]["message"]
        .as_str()
        .expect("error message should be a string");
    assert!(message.contains("x_unknown"));
    assert!(
        seen.lock()
            .expect("seen lock should be available")
            .is_none()
    );
}

#[tokio::test]
async fn typed_nested_unknown_request_field_is_rejected_before_upstream() {
    let seen = Arc::new(Mutex::new(None));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy(format!("{upstream_url}/v1"), None).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{ "role": "user", "content": "hello upstream" }],
            "tools": [
                {
                    "type": "function",
                    "function": {
                        "name": "lookup",
                        "description": "Find data",
                        "parameters": { "type": "object" },
                        "x_unknown": "reject me"
                    }
                }
            ]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_unknown_field_rejection(response, &seen, "x_unknown").await;
}

#[tokio::test]
async fn content_part_unknown_request_field_is_rejected_before_upstream() {
    let seen = Arc::new(Mutex::new(None));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy(format!("{upstream_url}/v1"), None).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "hello upstream",
                            "x_unknown": "reject me"
                        }
                    ]
                }
            ]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_unknown_field_rejection(response, &seen, "x_unknown").await;
}

#[tokio::test]
async fn message_tool_call_unknown_request_field_is_rejected_before_upstream() {
    let seen = Arc::new(Mutex::new(None));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy(format!("{upstream_url}/v1"), None).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [
                        {
                            "type": "function",
                            "id": "call_123",
                            "function": {
                                "name": "lookup",
                                "arguments": "{}"
                            },
                            "x_unknown": "reject me"
                        }
                    ]
                }
            ]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_unknown_field_rejection(response, &seen, "x_unknown").await;
}

#[tokio::test]
async fn tool_choice_unknown_request_field_is_rejected_before_upstream() {
    let seen = Arc::new(Mutex::new(None));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy(format!("{upstream_url}/v1"), None).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{ "role": "user", "content": "hello upstream" }],
            "tool_choice": {
                "type": "function",
                "function": { "name": "lookup" },
                "x_unknown": "reject me"
            }
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_unknown_field_rejection(response, &seen, "x_unknown").await;
}

#[tokio::test]
async fn allowed_tools_tool_choice_unknown_request_field_is_rejected_before_upstream() {
    let seen = Arc::new(Mutex::new(None));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy(format!("{upstream_url}/v1"), None).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{ "role": "user", "content": "hello upstream" }],
            "tool_choice": {
                "type": "allowed_tools",
                "allowed_tools": [
                    {
                        "mode": "auto",
                        "tools": [
                            {
                                "type": "function",
                                "function": { "name": "lookup" },
                                "x_unknown": "reject me"
                            }
                        ]
                    }
                ]
            }
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_unknown_field_rejection(response, &seen, "x_unknown").await;
}

#[tokio::test]
async fn response_format_unknown_request_field_is_rejected_before_upstream() {
    let seen = Arc::new(Mutex::new(None));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy(format!("{upstream_url}/v1"), None).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{ "role": "user", "content": "hello upstream" }],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "result",
                    "schema": { "type": "object" }
                },
                "x_unknown": "reject me"
            }
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_unknown_field_rejection(response, &seen, "x_unknown").await;
}

#[tokio::test]
async fn malformed_chat_request_schema_is_rejected_before_upstream() {
    let seen = Arc::new(Mutex::new(None));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy(format!("{upstream_url}/v1"), None).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": "not an array"
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.expect("error body should be json");
    assert_eq!(body["error"]["type"], "invalid_request");
    assert!(
        seen.lock()
            .expect("seen lock should be available")
            .is_none()
    );
}

#[tokio::test]
async fn response_transform_lowercases_assistant_messages_only() {
    let seen = Arc::new(Mutex::new(None));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy(format!("{upstream_url}/v1"), None).await;

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
}

#[tokio::test]
async fn configured_upstream_api_key_is_sent_as_bearer_auth() {
    let seen = Arc::new(Mutex::new(None));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy(
        format!("{upstream_url}/v1"),
        Some("upstream-key".to_owned()),
    )
    .await;

    let response = send_basic_chat_request(&proxy_url, None).await;

    assert_eq!(response.status(), StatusCode::OK);
    let seen = take_seen(&seen);
    assert_eq!(seen.authorization.as_deref(), Some("Bearer upstream-key"));
}

#[tokio::test]
async fn configured_upstream_api_key_wins_over_caller_authorization() {
    let seen = Arc::new(Mutex::new(None));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy(
        format!("{upstream_url}/v1"),
        Some("upstream-key".to_owned()),
    )
    .await;

    let response = send_basic_chat_request(&proxy_url, Some("Bearer caller-key")).await;

    assert_eq!(response.status(), StatusCode::OK);
    let seen = take_seen(&seen);
    assert_eq!(seen.authorization.as_deref(), Some("Bearer upstream-key"));
}

#[tokio::test]
async fn caller_authorization_is_forwarded_without_configured_key() {
    let seen = Arc::new(Mutex::new(None));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy(format!("{upstream_url}/v1"), None).await;

    let response = send_basic_chat_request(&proxy_url, Some("Bearer caller-key")).await;

    assert_eq!(response.status(), StatusCode::OK);
    let seen = take_seen(&seen);
    assert_eq!(seen.authorization.as_deref(), Some("Bearer caller-key"));
}

#[tokio::test]
async fn no_authorization_header_is_sent_when_no_auth_source_exists() {
    let seen = Arc::new(Mutex::new(None));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy(format!("{upstream_url}/v1"), None).await;

    let response = send_basic_chat_request(&proxy_url, None).await;

    assert_eq!(response.status(), StatusCode::OK);
    let seen = take_seen(&seen);
    assert_eq!(seen.authorization, None);
}

#[test]
fn ambient_openai_api_key_is_not_used_by_proxy_auth() {
    Jail::expect_with(|jail| {
        jail.clear_env();
        jail.set_env("OPENAI_API_KEY", "ambient-key");

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should build")
            .block_on(async {
                let seen = Arc::new(Mutex::new(None));
                let upstream_url = spawn_fake_json_upstream(
                    StatusCode::OK,
                    upstream_response(),
                    Arc::clone(&seen),
                )
                .await;
                let proxy_url = spawn_proxy(format!("{upstream_url}/v1"), None).await;

                let response = send_basic_chat_request(&proxy_url, None).await;

                assert_eq!(response.status(), StatusCode::OK);
                let seen = take_seen(&seen);
                assert_eq!(seen.authorization, None);
            });
        Ok(())
    });
}

#[tokio::test]
async fn stream_request_returns_exact_sse_chunks_stop_chunk_and_done() {
    let seen = Arc::new(Mutex::new(None));
    let upstream_url = spawn_fake_json_upstream(StatusCode::OK, upstream_response(), seen).await;
    let proxy_url = spawn_proxy(format!("{upstream_url}/v1"), None).await;

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
    let events = sse_data_events(&body);
    assert_eq!(events.len(), 5);

    let content_chunks = events[..3]
        .iter()
        .map(|event| {
            let chunk: Value = serde_json::from_str(event).expect("SSE chunk should be JSON");
            chunk["choices"][0]["delta"]["content"]
                .as_str()
                .expect("content chunk should contain text")
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(content_chunks, ["hello", "from", "upstream"]);

    let stop_chunk: Value = serde_json::from_str(&events[3]).expect("stop chunk should be JSON");
    assert_eq!(stop_chunk["choices"][0]["delta"], json!({}));
    assert_eq!(stop_chunk["choices"][0]["finish_reason"], "stop");
    assert_eq!(events[4], "[DONE]");
}

#[tokio::test]
async fn upstream_non_success_returns_gateway_error_with_body() {
    let seen = Arc::new(Mutex::new(None));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::BAD_GATEWAY, json!({ "error": "broken" }), seen).await;
    let proxy_url = spawn_proxy(format!("{upstream_url}/v1"), None).await;

    let response = send_basic_chat_request(&proxy_url, None).await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body: Value = response.json().await.expect("error body should be json");
    assert_eq!(body["error"]["type"], "upstream_error");
    let message = body["error"]["message"]
        .as_str()
        .expect("error message should be a string");
    assert!(message.contains("upstream returned API error"));
    assert!(message.contains(r#"{"error":"broken"}"#));
}

#[tokio::test]
async fn upstream_invalid_json_returns_gateway_error() {
    let seen = Arc::new(Mutex::new(None));
    let upstream_url =
        spawn_fake_raw_upstream(StatusCode::OK, "this is not json", Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy(format!("{upstream_url}/v1"), None).await;

    let response = send_basic_chat_request(&proxy_url, None).await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body: Value = response.json().await.expect("error body should be json");
    assert_eq!(body["error"]["type"], "upstream_error");
    assert!(
        body["error"]["message"]
            .as_str()
            .expect("error message should be a string")
            .contains("upstream returned invalid JSON")
    );
}

async fn assert_unknown_field_rejection(
    response: reqwest::Response,
    seen: &RecordedRequestSlot,
    field: &str,
) {
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.expect("error body should be json");
    assert_eq!(body["error"]["type"], "invalid_request");
    let message = body["error"]["message"]
        .as_str()
        .expect("error message should be a string");
    assert!(message.contains(field));
    assert!(
        seen.lock()
            .expect("seen lock should be available")
            .is_none()
    );
}

async fn send_basic_chat_request(
    proxy_url: &str,
    authorization: Option<&str>,
) -> reqwest::Response {
    let mut request = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{ "role": "user", "content": "hello upstream" }]
        }));
    if let Some(authorization) = authorization {
        request = request.header(header::AUTHORIZATION, authorization);
    }
    request.send().await.expect("proxy request should complete")
}

async fn spawn_proxy(upstream_base_url: String, upstream_api_key: Option<String>) -> String {
    let config = AppConfig::new(
        "127.0.0.1:0"
            .parse()
            .expect("test bind address should parse"),
        &upstream_base_url,
        upstream_api_key,
    )
    .expect("proxy config should be valid");
    let router = build_router(config).expect("proxy router should build");
    spawn_router(router).await
}

async fn spawn_fake_json_upstream(
    status: StatusCode,
    response: Value,
    seen: RecordedRequestSlot,
) -> String {
    async fn handler(
        State((status, response, seen)): State<FakeJsonUpstreamState>,
        headers: HeaderMap,
        body: Bytes,
    ) -> (StatusCode, axum::Json<Value>) {
        record_request(&seen, &headers, &body);
        (status, axum::Json(response))
    }

    let router = Router::new()
        .route("/v1/chat/completions", post(handler))
        .with_state((status, response, seen));
    spawn_router(router).await
}

async fn spawn_fake_raw_upstream(
    status: StatusCode,
    response: &'static str,
    seen: RecordedRequestSlot,
) -> String {
    async fn handler(
        State((status, response, seen)): State<FakeRawUpstreamState>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        record_request(&seen, &headers, &body);
        (
            status,
            [(header::CONTENT_TYPE, "application/json")],
            response,
        )
            .into_response()
    }

    let router = Router::new()
        .route("/v1/chat/completions", post(handler))
        .with_state((status, response, seen));
    spawn_router(router).await
}

fn record_request(seen: &RecordedRequestSlot, headers: &HeaderMap, body: &[u8]) {
    let body: Value = serde_json::from_slice(body).expect("upstream request should be json");
    let recorded = RecordedRequest {
        authorization: header_value(headers, header::AUTHORIZATION),
        content_type: header_value(headers, header::CONTENT_TYPE),
        body,
    };
    *seen.lock().expect("seen lock should be available") = Some(recorded);
}

fn header_value(headers: &HeaderMap, name: header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
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

fn take_seen(seen: &RecordedRequestSlot) -> RecordedRequest {
    seen.lock()
        .expect("seen lock should be available")
        .take()
        .expect("upstream should receive request")
}

fn sse_data_events(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(ToOwned::to_owned)
        .collect()
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
