use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use reqwest::Client;
use rlm_anywhere::{AppConfig, build_router};
use serde_json::{Value, json};
use tokio::net::TcpListener;

type FakeUpstreamState = (StatusCode, Value, Arc<Mutex<Option<Value>>>);

#[tokio::test]
async fn non_stream_request_forwards_capitalized_input_and_returns_lowercase_json() {
    let seen = Arc::new(Mutex::new(None));
    let upstream_url =
        spawn_fake_upstream(StatusCode::OK, upstream_response(), Arc::clone(&seen)).await;
    let proxy_url = spawn_proxy(format!("{upstream_url}/v1/")).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [
                {
                    "role": "system",
                    "content": "stay concise",
                    "metadata": { "label": "do-not-change" }
                },
                {
                    "role": "user",
                    "content": [
                        { "type": "text", "text": "hello upstream" },
                        { "type": "image_url", "image_url": { "url": "https://example.test/image.png" } }
                    ]
                }
            ],
            "tool_choice": "auto",
            "x_unknown": "preserve me"
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
    assert_eq!(body["choices"][1]["message"]["content"], "DO NOT LOWERCASE");

    let seen = seen
        .lock()
        .expect("seen request lock should be available")
        .take()
        .expect("upstream should receive request");
    assert_eq!(seen["model"], "local-model");
    assert_eq!(seen["tool_choice"], "auto");
    assert_eq!(seen["x_unknown"], "preserve me");
    assert_eq!(seen["messages"][0]["role"], "system");
    assert_eq!(seen["messages"][0]["content"], "STAY CONCISE");
    assert_eq!(seen["messages"][0]["metadata"]["label"], "do-not-change");
    assert_eq!(seen["messages"][1]["content"][0]["text"], "HELLO UPSTREAM");
    assert_eq!(
        seen["messages"][1]["content"][1]["image_url"]["url"],
        "https://example.test/image.png"
    );
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
        &upstream_base_url,
        None,
    )
    .expect("proxy config should be valid");
    spawn_router(build_router(config, Client::new())).await
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
            },
            {
                "index": 1,
                "message": {
                    "role": "tool",
                    "content": "DO NOT LOWERCASE"
                }
            }
        ]
    })
}
