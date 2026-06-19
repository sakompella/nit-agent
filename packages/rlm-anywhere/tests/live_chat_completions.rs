mod common;

use std::env;
use std::time::Duration;

use reqwest::Client;
use rlm_anywhere::{AppConfig, build_router};
use serde_json::{Value, json};

const LIVE_CHAT_BASE_URL_ENV: &str = "RLM_ANYWHERE_LIVE_CHAT_BASE_URL";
const LIVE_CHAT_MODEL_ENV: &str = "RLM_ANYWHERE_LIVE_CHAT_MODEL";
const LIVE_CHAT_API_KEY_ENV: &str = "RLM_ANYWHERE_LIVE_CHAT_API_KEY";

#[tokio::test]
#[ignore = "requires a real OpenAI-compatible upstream"]
async fn real_upstream_chat_completion_returns_assistant_content() {
    let Some(upstream) = live_upstream() else {
        return;
    };
    let proxy_url = spawn_proxy(&upstream).await;

    let response = client()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": upstream.model,
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

    assert_success_json(response, &upstream.model).await;
}

#[tokio::test]
#[ignore = "requires a real OpenAI-compatible upstream"]
async fn real_upstream_stream_request_returns_fake_sse_done_event() {
    let Some(upstream) = live_upstream() else {
        return;
    };
    let proxy_url = spawn_proxy(&upstream).await;

    let response = client()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": upstream.model,
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
    let events = common::sse_data_events(&body);
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

async fn assert_success_json(response: reqwest::Response, requested_model: &str) {
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.expect("response body should be JSON");

    let response_model = body["model"]
        .as_str()
        .expect("response should include model");
    assert!(
        is_accepted_response_model(requested_model, response_model),
        "response model should match requested model; requested {requested_model:?}, got {response_model:?}, body was {body}"
    );
    assert!(
        body["choices"][0]["message"]["content"]
            .as_str()
            .is_some_and(|content| !content.trim().is_empty()),
        "assistant content should be present; body was {body}"
    );
}

async fn spawn_proxy(upstream: &LiveUpstream) -> String {
    let config = AppConfig::new(
        "127.0.0.1:0"
            .parse()
            .expect("test bind address should parse"),
        &upstream.base_url,
        upstream.api_key.clone(),
    )
    .expect("proxy config should be valid");
    let router = build_router(config).expect("proxy router should build");
    common::spawn_router(router).await
}

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .expect("test HTTP client should build")
}

fn is_accepted_response_model(requested_model: &str, response_model: &str) -> bool {
    response_model == requested_model
        || requested_model
            .strip_prefix("cf/")
            .is_some_and(|canonical_model| response_model == canonical_model)
}

#[derive(Clone, Debug)]
struct LiveUpstream {
    base_url: String,
    model: String,
    api_key: Option<String>,
}

fn live_upstream() -> Option<LiveUpstream> {
    // Ignored live tests intentionally no-op unless a caller opts in with env.
    let base_url = env_value(LIVE_CHAT_BASE_URL_ENV);
    let model = env_value(LIVE_CHAT_MODEL_ENV);

    match (base_url, model) {
        (Some(base_url), Some(model)) => Some(LiveUpstream {
            base_url,
            model,
            api_key: env_value(LIVE_CHAT_API_KEY_ENV),
        }),
        _ => {
            eprintln!(
                "skipping live chat completions test; set {LIVE_CHAT_BASE_URL_ENV} and {LIVE_CHAT_MODEL_ENV}"
            );
            None
        }
    }
}

fn env_value(name: &str) -> Option<String> {
    env::var(name).ok().and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then_some(trimmed.to_owned())
    })
}
