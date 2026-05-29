use std::time::Duration;

use axum::Router;
use reqwest::Client;
use rlm_anywhere::{AppConfig, build_router};
use serde_json::{Value, json};
use tokio::net::TcpListener;

const REAL_UPSTREAM_BASE_URL: &str = "http://localhost:20128/v1";
const REAL_MODEL: &str = "cf/@cf/moonshotai/kimi-k2.6";
const CANONICAL_RESPONSE_MODEL: &str = "@cf/moonshotai/kimi-k2.6";

#[tokio::test]
#[ignore = "requires a real OpenAI-compatible upstream at http://localhost:20128/v1"]
async fn real_upstream_chat_completion_returns_assistant_content() {
    let proxy_url = spawn_proxy().await;

    let response = client()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": REAL_MODEL,
            "messages": [
                {
                    "role": "user",
                    "content": "Say ok."
                }
            ]
        }))
        .send()
        .await
        .expect("real upstream proxy request should complete");

    assert_success_json(response).await;
}

#[tokio::test]
#[ignore = "requires a real OpenAI-compatible upstream at http://localhost:20128/v1"]
async fn real_upstream_stream_request_returns_fake_sse_done_event() {
    let proxy_url = spawn_proxy().await;

    let response = client()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": REAL_MODEL,
            "stream": true,
            "messages": [
                {
                    "role": "user",
                    "content": "Say ok."
                }
            ]
        }))
        .send()
        .await
        .expect("real upstream streaming proxy request should complete");

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(content_type.starts_with("text/event-stream"));

    let body = response
        .text()
        .await
        .expect("stream response body should be readable");
    let events = sse_data_events(&body);
    assert!(
        events.iter().any(|event| event == "[DONE]"),
        "stream should end with [DONE]; body was {body:?}"
    );
    assert!(
        events.iter().any(|event| {
            let Ok(chunk) = serde_json::from_str::<Value>(event) else {
                return false;
            };
            chunk["choices"][0]["delta"]["content"].is_string()
        }),
        "stream should include at least one content chunk; body was {body:?}"
    );
}

async fn assert_success_json(response: reqwest::Response) {
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.expect("response body should be JSON");

    let model = body["model"]
        .as_str()
        .expect("response model should be a string");
    assert!(
        model == REAL_MODEL || model == CANONICAL_RESPONSE_MODEL,
        "response model should match requested or canonical upstream model; body was {body}"
    );
    assert!(
        body["choices"][0]["message"]["content"]
            .as_str()
            .is_some_and(|content| !content.trim().is_empty()),
        "assistant content should be present; body was {body}"
    );
}

async fn spawn_proxy() -> String {
    let config = AppConfig::new(
        "127.0.0.1:0"
            .parse()
            .expect("test bind address should parse"),
        REAL_UPSTREAM_BASE_URL,
        None,
    )
    .expect("proxy config should be valid");
    let router = build_router(config).expect("proxy router should build");
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
                tokio::time::sleep(Duration::from_secs(10)).await;
            })
            .await
            .expect("test server should run");
    });
    format!("http://{}:{}", address.ip(), address.port())
}

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .expect("test HTTP client should build")
}

fn sse_data_events(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(ToOwned::to_owned)
        .collect()
}
