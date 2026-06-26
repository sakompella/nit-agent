#![expect(
    clippy::result_large_err,
    reason = "figment Jail test closures return figment's native error type"
)]

mod common;

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use figment::Jail;
use reqwest::Client;
use rlm_anywhere::{AppConfig, RequestMode, UpstreamProvider, build_router};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
struct RecordedRequest {
    authorization: Option<String>,
    content_type: Option<String>,
    body: Value,
}

/// Holds the last recorded upstream request and a count of all requests seen.
#[derive(Default)]
struct RecordedSlot {
    last: Option<RecordedRequest>,
    count: usize,
}

type RecordedRequestSlot = Arc<Mutex<RecordedSlot>>;
type FakeJsonUpstreamState = (StatusCode, Value, RecordedRequestSlot);
type FakeRawUpstreamState = (StatusCode, &'static str, RecordedRequestSlot);

#[tokio::test]
async fn tool_bearing_request_bypasses_loop_and_forwards_unchanged() {
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
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
            "stream": false,
            "tool_choice": "auto"
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.expect("response body should be json");
    assert_eq!(
        body["choices"][0]["message"]["content"],
        "HELLO FROM UPSTREAM"
    );
    // Spec §13.2 B1: exactly one upstream request must be made (no stray extra calls).
    assert_eq!(
        request_count(&seen),
        1,
        "expected exactly one upstream request"
    );
    let seen = take_seen(&seen);
    assert_eq!(seen.content_type.as_deref(), Some("application/json"));
    assert_eq!(seen.body["model"], "local-model");
    assert_eq!(seen.body["tool_choice"], "auto");
    assert_eq!(seen.body["messages"][0]["role"], "system");
    assert_eq!(seen.body["messages"][0]["content"], "stay concise");
    assert_eq!(
        seen.body["messages"][1]["content"][0]["text"],
        "hello upstream"
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
async fn allowed_tools_tool_choice_request_is_forwarded() {
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
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
                                "function": { "name": "lookup" }
                            }
                        ]
                    }
                ]
            }
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::OK);
    let seen = take_seen(&seen);
    assert_eq!(seen.body["tool_choice"]["type"], "allowed_tools");
    assert_eq!(
        seen.body["tool_choice"]["allowed_tools"][0]["tools"][0]["function"]["name"],
        "lookup"
    );
    assert_eq!(seen.body["stream"], false);
}

#[tokio::test]
async fn tool_choice_only_request_bypasses_loop() {
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
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
                "function": { "name": "lookup" }
            }
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::OK);
    let seen = take_seen(&seen);
    assert_eq!(seen.body["tool_choice"]["type"], "function");
    assert_eq!(seen.body["stream"], false);
}

#[tokio::test]
async fn passthrough_runs_zero_loop_machinery() {
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url =
        spawn_proxy_with_mode(format!("{upstream_url}/v1"), None, RequestMode::Passthrough).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [
                {
                    "role": "user",
                    "content": "hello upstream",
                    "x_provider_message": "keep me"
                }
            ],
            "x_provider_top_level": { "route": "custom" }
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.expect("response body should be json");
    assert_eq!(
        body["choices"][0]["message"]["content"],
        "HELLO FROM UPSTREAM"
    );

    // Spec §13.2 B3: passthrough must make exactly one upstream request (no loop subcalls).
    assert_eq!(
        request_count(&seen),
        1,
        "expected exactly one upstream request"
    );
    let seen = take_seen(&seen);
    assert_eq!(
        seen.body,
        json!({
            "model": "local-model",
            "messages": [
                {
                    "role": "user",
                    "content": "hello upstream",
                    "x_provider_message": "keep me"
                }
            ],
            "x_provider_top_level": { "route": "custom" },
            "stream": false
        })
    );
    assert_eq!(seen.body["stream"], false);
}

#[tokio::test]
async fn passthrough_stream_forwards_stream_true_and_pipes_sse_verbatim() {
    const UPSTREAM_SSE: &str = "data: {\"id\":\"chatcmpl-x\",\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: {\"id\":\"chatcmpl-x\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";

    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
    let upstream_url = spawn_fake_sse_upstream(UPSTREAM_SSE, Arc::clone(&seen)).await;
    let proxy_url =
        spawn_proxy_with_mode(format!("{upstream_url}/v1"), None, RequestMode::Passthrough).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{ "role": "user", "content": "hello upstream" }],
            "stream": true,
            "x_provider_top_level": "forward me"
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );

    let body = response.text().await.expect("sse body should be text");
    // The upstream SSE bytes are piped through unmodified.
    assert_eq!(body, UPSTREAM_SSE);
    // The exact data events (including the verbatim final [DONE]) survive.
    let events = common::sse_data_events(&body);
    assert_eq!(events.len(), 3);
    assert_eq!(events[2], "[DONE]");

    let seen = take_seen(&seen);
    assert_eq!(seen.body["x_provider_top_level"], "forward me");
    // stream:true must be forwarded upstream, not rewritten to false.
    assert_eq!(seen.body["stream"], true);
}

#[tokio::test]
async fn allowed_tools_tool_choice_forwards_raw_tool_values() {
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
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
                                "x_provider_extension": {
                                    "routing": "custom"
                                }
                            }
                        ]
                    }
                ]
            }
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::OK);
    let seen = take_seen(&seen);
    assert_eq!(
        seen.body["tool_choice"]["allowed_tools"][0]["tools"][0]["x_provider_extension"],
        json!({ "routing": "custom" })
    );
    assert_eq!(seen.body["stream"], false);
}

#[tokio::test]
async fn unknown_top_level_request_field_is_rejected_before_upstream() {
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
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
            .last
            .is_none()
    );
}

#[tokio::test]
async fn nested_unknown_request_field_is_rejected_before_upstream() {
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
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
            .last
            .is_none()
    );
}

#[tokio::test]
async fn typed_nested_unknown_request_field_is_rejected_before_upstream() {
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
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
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
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
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
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
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
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
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
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
                        "x_unknown": "reject me",
                        "tools": [
                            {
                                "type": "function",
                                "function": { "name": "lookup" }
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
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
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
async fn supported_response_format_is_forwarded() {
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url =
        spawn_proxy_with_mode(format!("{upstream_url}/v1"), None, RequestMode::Passthrough).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{ "role": "user", "content": "hello upstream" }],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "result",
                    "strict": true,
                    "schema": { "type": "object" }
                }
            }
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::OK);
    let seen = take_seen(&seen);
    assert_eq!(seen.body["response_format"]["type"], "json_schema");
    assert_eq!(
        seen.body["response_format"]["json_schema"]["name"],
        "result"
    );
    assert_eq!(
        seen.body["response_format"]["json_schema"]["schema"]["type"],
        "object"
    );
}

#[tokio::test]
async fn malformed_chat_request_schema_is_rejected_before_upstream() {
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
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
            .last
            .is_none()
    );
}

#[tokio::test]
async fn configured_upstream_api_key_is_sent_as_bearer_auth() {
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy_with_mode(
        format!("{upstream_url}/v1"),
        Some("upstream-key".to_owned()),
        RequestMode::Passthrough,
    )
    .await;

    let response = send_basic_chat_request(&proxy_url, None).await;

    assert_eq!(response.status(), StatusCode::OK);
    let seen = take_seen(&seen);
    assert_eq!(seen.authorization.as_deref(), Some("Bearer upstream-key"));
}

#[tokio::test]
async fn configured_upstream_api_key_wins_over_caller_authorization() {
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy_with_mode(
        format!("{upstream_url}/v1"),
        Some("upstream-key".to_owned()),
        RequestMode::Passthrough,
    )
    .await;

    let response = send_basic_chat_request(&proxy_url, Some("Bearer caller-key")).await;

    assert_eq!(response.status(), StatusCode::OK);
    let seen = take_seen(&seen);
    assert_eq!(seen.authorization.as_deref(), Some("Bearer upstream-key"));
}

#[tokio::test]
async fn caller_authorization_is_forwarded_without_configured_key() {
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url =
        spawn_proxy_with_mode(format!("{upstream_url}/v1"), None, RequestMode::Passthrough).await;

    let response = send_basic_chat_request(&proxy_url, Some("Bearer caller-key")).await;

    assert_eq!(response.status(), StatusCode::OK);
    let seen = take_seen(&seen);
    assert_eq!(seen.authorization.as_deref(), Some("Bearer caller-key"));
}

#[tokio::test]
async fn no_authorization_header_is_sent_when_no_auth_source_exists() {
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url =
        spawn_proxy_with_mode(format!("{upstream_url}/v1"), None, RequestMode::Passthrough).await;

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
                let seen = Arc::new(Mutex::new(RecordedSlot::default()));
                let upstream_url = spawn_fake_json_upstream(
                    StatusCode::OK,
                    upstream_response(),
                    Arc::clone(&seen),
                )
                .await;
                let proxy_url = spawn_proxy_with_mode(
                    format!("{upstream_url}/v1"),
                    None,
                    RequestMode::Passthrough,
                )
                .await;

                let response = send_basic_chat_request(&proxy_url, None).await;

                assert_eq!(response.status(), StatusCode::OK);
                let seen = take_seen(&seen);
                assert_eq!(seen.authorization, None);
            });
        Ok(())
    });
}

#[tokio::test]
async fn request_body_over_limit_returns_payload_too_large() {
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy_with_body_limit(format!("{upstream_url}/v1"), 256).await;

    // A body well above the 256-byte limit.
    let big_content = "x".repeat(4096);
    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{ "role": "user", "content": big_content }]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    // The oversized request must be rejected before any upstream call.
    assert_eq!(
        request_count(&seen),
        0,
        "no upstream request should be made"
    );
}

#[tokio::test]
async fn request_body_under_limit_is_accepted() {
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy_with_body_limit(format!("{upstream_url}/v1"), 65_536).await;

    let response = send_basic_chat_request(&proxy_url, None).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        request_count(&seen),
        1,
        "one upstream request should be made"
    );
}

#[tokio::test]
async fn upstream_timeout_returns_gateway_error_not_panic() {
    let upstream_url = spawn_slow_upstream().await;
    let proxy_url = spawn_proxy_with_timeout(
        format!("{upstream_url}/v1"),
        std::time::Duration::from_millis(50),
    )
    .await;

    let response = send_basic_chat_request(&proxy_url, None).await;

    // The reqwest per-call timeout surfaces as ModelError::Request -> 502.
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body: Value = response.json().await.expect("error body should be json");
    assert_eq!(body["error"]["type"], "upstream_error");
}

#[tokio::test]
async fn upstream_non_success_returns_gateway_error_with_body() {
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
    let upstream_url =
        spawn_fake_json_upstream(StatusCode::BAD_GATEWAY, json!({ "error": "broken" }), seen).await;
    let proxy_url =
        spawn_proxy_with_mode(format!("{upstream_url}/v1"), None, RequestMode::Passthrough).await;

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
    let seen = Arc::new(Mutex::new(RecordedSlot::default()));
    let upstream_url =
        spawn_fake_raw_upstream(StatusCode::OK, "this is not json", Arc::clone(&seen)).await;
    let proxy_url =
        spawn_proxy_with_mode(format!("{upstream_url}/v1"), None, RequestMode::Passthrough).await;

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
    assert!(!message.contains("expected one of"));
    assert!(!message.contains(" at line "));
    assert!(!message.contains(" column "));
    assert!(
        seen.lock()
            .expect("seen lock should be available")
            .last
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
    spawn_proxy_with_mode(upstream_base_url, upstream_api_key, RequestMode::Rlm).await
}

async fn spawn_proxy_with_mode(
    upstream_base_url: String,
    upstream_api_key: Option<String>,
    mode: RequestMode,
) -> String {
    let config = AppConfig::new_with_provider(
        "127.0.0.1:0"
            .parse()
            .expect("test bind address should parse"),
        mode,
        UpstreamProvider::OpenAiCompatible,
        &upstream_base_url,
        upstream_api_key,
        std::time::Duration::from_secs(30),
        0,
    )
    .expect("proxy config should be valid");
    let router = build_router(config).expect("proxy router should build");
    common::spawn_router(router).await
}

async fn spawn_proxy_with_body_limit(
    upstream_base_url: String,
    max_request_body_bytes: usize,
) -> String {
    let config = AppConfig::new_with_provider(
        "127.0.0.1:0"
            .parse()
            .expect("test bind address should parse"),
        RequestMode::Passthrough,
        UpstreamProvider::OpenAiCompatible,
        &upstream_base_url,
        None,
        std::time::Duration::from_secs(30),
        0,
    )
    .expect("proxy config should be valid")
    .with_max_request_body_bytes(max_request_body_bytes);
    let router = build_router(config).expect("proxy router should build");
    common::spawn_router(router).await
}

async fn spawn_proxy_with_timeout(
    upstream_base_url: String,
    timeout: std::time::Duration,
) -> String {
    let config = AppConfig::new_with_provider(
        "127.0.0.1:0"
            .parse()
            .expect("test bind address should parse"),
        RequestMode::Passthrough,
        UpstreamProvider::OpenAiCompatible,
        &upstream_base_url,
        None,
        timeout,
        0,
    )
    .expect("proxy config should be valid");
    let router = build_router(config).expect("proxy router should build");
    common::spawn_router(router).await
}

/// Upstream that sleeps past any reasonable per-call timeout before replying,
/// so the proxy's upstream HTTP timeout fires first.
async fn spawn_slow_upstream() -> String {
    async fn handler() -> Response {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        (StatusCode::OK, axum::Json(upstream_response())).into_response()
    }

    let router = Router::new().route("/v1/chat/completions", post(handler));
    common::spawn_router(router).await
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
    common::spawn_router(router).await
}

/// Upstream that records the request and replies with a raw `text/event-stream`
/// body, used to verify true streaming passthrough.
async fn spawn_fake_sse_upstream(response: &'static str, seen: RecordedRequestSlot) -> String {
    async fn handler(
        State((response, seen)): State<(&'static str, RecordedRequestSlot)>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        record_request(&seen, &headers, &body);
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/event-stream")],
            response,
        )
            .into_response()
    }

    let router = Router::new()
        .route("/v1/chat/completions", post(handler))
        .with_state((response, seen));
    common::spawn_router(router).await
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
    common::spawn_router(router).await
}

/// Upstream that blocks each request until `release` is notified, so a request
/// can be held in-flight while a second request is sent.
async fn spawn_gated_upstream(release: Arc<tokio::sync::Notify>) -> String {
    async fn handler(State(release): State<Arc<tokio::sync::Notify>>) -> Response {
        release.notified().await;
        (StatusCode::OK, axum::Json(upstream_response())).into_response()
    }

    let router = Router::new()
        .route("/v1/chat/completions", post(handler))
        .with_state(release);
    common::spawn_router(router).await
}

async fn spawn_proxy_with_concurrency_limit(
    upstream_base_url: String,
    max_concurrent_requests: usize,
) -> String {
    let config = AppConfig::new_with_provider(
        "127.0.0.1:0"
            .parse()
            .expect("test bind address should parse"),
        RequestMode::Passthrough,
        UpstreamProvider::OpenAiCompatible,
        &upstream_base_url,
        None,
        std::time::Duration::from_secs(30),
        0,
    )
    .expect("proxy config should be valid")
    .with_max_concurrent_requests(max_concurrent_requests);
    let router = build_router(config).expect("proxy router should build");
    common::spawn_router(router).await
}

#[tokio::test]
async fn concurrency_limit_sheds_excess_requests_with_503() {
    let release = Arc::new(tokio::sync::Notify::new());
    let upstream_url = spawn_gated_upstream(Arc::clone(&release)).await;
    let proxy_url = spawn_proxy_with_concurrency_limit(format!("{upstream_url}/v1"), 1).await;

    // Request 1 occupies the single concurrency permit (blocked in upstream).
    let url = format!("{proxy_url}/v1/chat/completions");
    let first = tokio::spawn({
        let url = url.clone();
        async move {
            Client::new()
                .post(url)
                .json(&json!({
                    "model": "local-model",
                    "messages": [{ "role": "user", "content": "first" }]
                }))
                .send()
                .await
        }
    });

    // Give request 1 time to be accepted and reach the gated upstream.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Request 2 should be shed immediately with 503 overloaded.
    let second = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        Client::new()
            .post(&url)
            .json(&json!({
                "model": "local-model",
                "messages": [{ "role": "user", "content": "second" }]
            }))
            .send(),
    )
    .await
    .expect("second request should not hang")
    .expect("second request should complete");

    assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = second.json().await.expect("503 body should be JSON");
    assert_eq!(body["error"]["type"], "overloaded");

    // Release request 1 so it can finish cleanly.
    release.notify_waiters();
    let first = tokio::time::timeout(std::time::Duration::from_secs(5), first)
        .await
        .expect("first request should finish after release")
        .expect("first task should join")
        .expect("first request should complete");
    assert_eq!(first.status(), StatusCode::OK);
}

fn record_request(seen: &RecordedRequestSlot, headers: &HeaderMap, body: &[u8]) {
    let body: Value = serde_json::from_slice(body).expect("upstream request should be json");
    let recorded = RecordedRequest {
        authorization: header_value(headers, header::AUTHORIZATION),
        content_type: header_value(headers, header::CONTENT_TYPE),
        body,
    };
    let mut slot = seen.lock().expect("seen lock should be available");
    slot.last = Some(recorded);
    slot.count += 1;
}

fn request_count(seen: &RecordedRequestSlot) -> usize {
    seen.lock().expect("seen lock should be available").count
}

fn header_value(headers: &HeaderMap, name: header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

fn take_seen(seen: &RecordedRequestSlot) -> RecordedRequest {
    seen.lock()
        .expect("seen lock should be available")
        .last
        .take()
        .expect("upstream should receive request")
}

fn upstream_response() -> Value {
    upstream_response_with_content("HELLO FROM UPSTREAM")
}

fn upstream_response_with_content(content: &str) -> Value {
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
                    "content": content
                },
                "finish_reason": "stop"
            }
        ]
    })
}
