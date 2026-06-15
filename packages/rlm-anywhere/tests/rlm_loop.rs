use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use reqwest::Client;
use rlm_anywhere::rlm::RlmLoopConfig;
use rlm_anywhere::{AppConfig, RequestMode, UpstreamProvider, build_router};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct RecordedRequest {
    authorization: Option<String>,
    body: Value,
}

// ---------------------------------------------------------------------------
// Scripted upstream harness
// ---------------------------------------------------------------------------

struct ScriptedUpstream {
    responses: VecDeque<Value>,
    seen: Vec<RecordedRequest>,
}

type ScriptHandle = Arc<Mutex<ScriptedUpstream>>;
type ScriptedState = Arc<Mutex<ScriptedUpstream>>;

async fn spawn_scripted_upstream(responses: Vec<Value>) -> (String, ScriptHandle) {
    let handle: ScriptHandle = Arc::new(Mutex::new(ScriptedUpstream {
        responses: VecDeque::from(responses),
        seen: Vec::new(),
    }));

    async fn handler(
        State(state): State<ScriptedState>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let body_value: Value =
            serde_json::from_slice(&body).expect("scripted upstream body should be JSON");

        let authorization = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(ToOwned::to_owned);

        let mut guard = state
            .lock()
            .expect("scripted upstream lock should be available");
        guard.seen.push(RecordedRequest {
            authorization,
            body: body_value,
        });

        match guard.responses.pop_front() {
            Some(response) => (StatusCode::OK, axum::Json(response)).into_response(),
            None => (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(json!({"error": "script exhausted"})),
            )
                .into_response(),
        }
    }

    let router = Router::new()
        .route("/v1/chat/completions", post(handler))
        .with_state(Arc::clone(&handle));
    let base_url = spawn_router(router).await;
    (base_url, handle)
}

/// Spawns the proxy in RLM mode with the given loop config.
async fn spawn_rlm_proxy(upstream_base_url: String, rlm: RlmLoopConfig) -> String {
    let config = AppConfig::new_with_provider(
        "127.0.0.1:0"
            .parse()
            .expect("test bind address should parse"),
        RequestMode::Rlm,
        UpstreamProvider::OpenAiCompatible,
        &upstream_base_url,
        None,
    )
    .expect("rlm proxy config should be valid")
    .with_rlm(rlm);
    let router = build_router(config).expect("rlm proxy router should build");
    spawn_router(router).await
}

/// Same as `spawn_rlm_proxy` but with a configured upstream API key.
async fn spawn_rlm_proxy_with_key(
    upstream_base_url: String,
    rlm: RlmLoopConfig,
    api_key: Option<String>,
) -> String {
    let config = AppConfig::new_with_provider(
        "127.0.0.1:0"
            .parse()
            .expect("test bind address should parse"),
        RequestMode::Rlm,
        UpstreamProvider::OpenAiCompatible,
        &upstream_base_url,
        api_key,
    )
    .expect("rlm proxy config should be valid")
    .with_rlm(rlm);
    let router = build_router(config).expect("rlm proxy router should build");
    spawn_router(router).await
}

/// Default test budget — enough headroom for multi-step tests.
fn default_rlm() -> RlmLoopConfig {
    RlmLoopConfig {
        max_steps: 5,
        max_subcalls: 5,
        max_wall: Duration::from_secs(30),
        tool_result_preview_bytes: 8_192,
        ..Default::default()
    }
}

async fn spawn_router(router: Router) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test listener should bind");
    let address = listener
        .local_addr()
        .expect("test listener should have a local address");
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

// ---------------------------------------------------------------------------
// Response builders
// ---------------------------------------------------------------------------

/// Build a full chat.completion response whose choices[0].message carries tool calls.
/// `calls` is a slice of `(id, name, args_as_Value)`. `args` is serialized to a JSON string.
fn tool_call_response(calls: &[(&str, &str, Value)]) -> Value {
    let tool_calls: Vec<Value> = calls
        .iter()
        .map(|(id, name, args)| {
            json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": args.to_string()
                }
            })
        })
        .collect();

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
                    "content": null,
                    "tool_calls": tool_calls
                },
                "finish_reason": "tool_calls"
            }
        ]
    })
}

/// Build a plain text chat.completion response.
fn text_response(content: &str) -> Value {
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

fn take_seen(handle: &ScriptHandle) -> Vec<RecordedRequest> {
    handle
        .lock()
        .expect("script handle lock should be available")
        .seen
        .clone()
}

fn sse_data_events(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(ToOwned::to_owned)
        .collect()
}

// ---------------------------------------------------------------------------
// L1: final_answer_on_first_step_returns_chat_completion
// ---------------------------------------------------------------------------

#[tokio::test]
async fn final_answer_on_first_step_returns_chat_completion() {
    let (upstream_url, handle) = spawn_scripted_upstream(vec![tool_call_response(&[(
        "call_1",
        "final_answer",
        json!({"content": "4"}),
    )])])
    .await;
    let proxy_url = spawn_rlm_proxy(format!("{upstream_url}/v1"), default_rlm()).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{"role": "user", "content": "what is 2+2?"}]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.expect("response should be JSON");
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["model"], "local-model");
    assert_eq!(body["choices"][0]["message"]["role"], "assistant");
    assert_eq!(body["choices"][0]["message"]["content"], "4");
    assert_eq!(body["choices"][0]["finish_reason"], "stop");
    assert_eq!(body["usage"]["total_tokens"], 0);

    let seen = take_seen(&handle);
    assert_eq!(seen.len(), 1);
    let req = &seen[0];

    // System message with context summary prefix
    assert_eq!(
        req.body["messages"][0]["role"], "system",
        "first loop message should be the controller system message"
    );
    let system_content = req.body["messages"][0]["content"]
        .as_str()
        .expect("system message content should be a string");
    assert!(
        system_content.contains("Context summary: "),
        "system message should contain context summary prefix: {system_content}"
    );
    assert!(
        system_content.contains("\"messages\":0"),
        "context summary should show 0 context messages: {system_content}"
    );

    // messages[1] is the raw user message
    assert_eq!(req.body["messages"][1]["role"], "user");
    assert_eq!(req.body["messages"][1]["content"], "what is 2+2?");

    // Tools array has exactly 6 entries with the correct names
    let tools = req.body["tools"]
        .as_array()
        .expect("loop request should include tools array");
    assert_eq!(tools.len(), 6, "loop request should have exactly 6 tools");
    let tool_names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();
    for expected in &[
        "context_describe",
        "context_slice",
        "context_grep",
        "llm_query",
        "run_js",
        "final_answer",
    ] {
        assert!(
            tool_names.contains(expected),
            "tools should include {expected}: {tool_names:?}"
        );
    }

    assert_eq!(req.body["tool_choice"], "auto");
    assert_eq!(req.body["stream"], false);
}

// ---------------------------------------------------------------------------
// L2: context_grep_roundtrip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn context_grep_roundtrip() {
    let step1 = tool_call_response(&[("call_g", "context_grep", json!({"needle": "code word"}))]);
    let step2 = tool_call_response(&[("call_f", "final_answer", json!({"content": "petrichor"}))]);
    let (upstream_url, handle) = spawn_scripted_upstream(vec![step1, step2]).await;
    let proxy_url = spawn_rlm_proxy(format!("{upstream_url}/v1"), default_rlm()).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [
                {"role": "system", "content": "Be direct."},
                {"role": "user", "content": "the code word is petrichor"},
                {"role": "user", "content": "what is the code word?"}
            ]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.expect("response should be JSON");
    assert_eq!(body["choices"][0]["message"]["content"], "petrichor");

    let seen = take_seen(&handle);
    assert_eq!(seen.len(), 2);

    // Request #2 messages end with the verbatim assistant tool-call message,
    // then a tool message for call_g whose content contains "petrichor" and "index"
    let req2_messages = seen[1].body["messages"]
        .as_array()
        .expect("request #2 should have messages");
    let last_two = &req2_messages[req2_messages.len().saturating_sub(2)..];

    // Second-to-last: the assistant tool-call message
    assert_eq!(last_two[0]["role"], "assistant");
    assert!(!last_two[0]["tool_calls"].is_null());

    // Last: the tool result message
    assert_eq!(last_two[1]["role"], "tool");
    assert_eq!(last_two[1]["tool_call_id"], "call_g");
    let tool_content = last_two[1]["content"]
        .as_str()
        .expect("tool result content should be a string");
    assert!(
        tool_content.contains("petrichor"),
        "tool content should contain 'petrichor': {tool_content}"
    );
    assert!(
        tool_content.contains("index"),
        "tool content should contain 'index': {tool_content}"
    );
}

// ---------------------------------------------------------------------------
// L3: context_slice_clamps_and_reports_indices
// ---------------------------------------------------------------------------

#[tokio::test]
async fn context_slice_clamps_and_reports_indices() {
    let step1 = tool_call_response(&[("call_s", "context_slice", json!({"start": 0, "end": 99}))]);
    let step2 = tool_call_response(&[("call_f", "final_answer", json!({"content": "done"}))]);
    let (upstream_url, handle) = spawn_scripted_upstream(vec![step1, step2]).await;
    let proxy_url = spawn_rlm_proxy(format!("{upstream_url}/v1"), default_rlm()).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [
                {"role": "system", "content": "context msg 0"},
                {"role": "assistant", "content": "context msg 1"},
                {"role": "user", "content": "what do we have?"}
            ]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::OK);

    let seen = take_seen(&handle);
    assert_eq!(seen.len(), 2);

    // Find the tool result message for call_s in request #2
    let req2_messages = seen[1].body["messages"]
        .as_array()
        .expect("request #2 should have messages");
    let tool_result = req2_messages
        .iter()
        .find(|msg| msg["role"] == "tool" && msg["tool_call_id"] == "call_s")
        .expect("should find tool result for call_s");

    let content_str = tool_result["content"]
        .as_str()
        .expect("tool result content should be a string");
    let items: Vec<Value> =
        serde_json::from_str(content_str).expect("tool result should be a JSON array");

    // 2 context messages (system + assistant), clamped from 0..99
    assert_eq!(items.len(), 2, "should have 2 context items");
    for item in &items {
        assert!(item.get("index").is_some(), "item should have 'index'");
        assert!(item.get("role").is_some(), "item should have 'role'");
        assert!(item.get("text").is_some(), "item should have 'text'");
    }
}

// ---------------------------------------------------------------------------
// L4: run_js_success_and_eval_error_are_tool_results
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_js_success_and_eval_error_are_tool_results() {
    let step1 = tool_call_response(&[("call_js1", "run_js", json!({"code": "return 6*7;"}))]);
    let step2 = tool_call_response(&[(
        "call_js2",
        "run_js",
        json!({"code": "throw new Error('boom');"}),
    )]);
    let step3 = tool_call_response(&[("call_f", "final_answer", json!({"content": "done"}))]);
    let (upstream_url, handle) = spawn_scripted_upstream(vec![step1, step2, step3]).await;
    let proxy_url = spawn_rlm_proxy(format!("{upstream_url}/v1"), default_rlm()).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{"role": "user", "content": "compute"}]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.expect("response should be JSON");
    assert_eq!(body["choices"][0]["message"]["content"], "done");

    let seen = take_seen(&handle);
    assert_eq!(seen.len(), 3, "should have exactly 3 upstream requests");

    // Request #2: tool result for call_js1 should contain 42
    let req2_messages = seen[1].body["messages"]
        .as_array()
        .expect("request #2 should have messages");
    let tool_result_js1 = req2_messages
        .iter()
        .find(|msg| msg["role"] == "tool" && msg["tool_call_id"] == "call_js1")
        .expect("should find tool result for call_js1");
    let content_js1 = tool_result_js1["content"]
        .as_str()
        .expect("js1 tool content should be a string");
    // 42 can appear either as the bare JSON number or quoted string
    assert!(
        content_js1 == "42" || content_js1.contains("42"),
        "js1 tool content should contain 42: {content_js1}"
    );

    // Request #3: tool result for call_js2 should have an error containing "boom"
    let req3_messages = seen[2].body["messages"]
        .as_array()
        .expect("request #3 should have messages");
    let tool_result_js2 = req3_messages
        .iter()
        .find(|msg| msg["role"] == "tool" && msg["tool_call_id"] == "call_js2")
        .expect("should find tool result for call_js2");
    let content_js2 = tool_result_js2["content"]
        .as_str()
        .expect("js2 tool content should be a string");
    let parsed_js2: Value =
        serde_json::from_str(content_js2).expect("js2 tool content should be JSON");
    let error_msg = parsed_js2["error"]
        .as_str()
        .expect("js2 tool content should have 'error' field");
    assert!(
        error_msg.contains("boom"),
        "js2 error should mention 'boom': {error_msg}"
    );
}

// ---------------------------------------------------------------------------
// L5: llm_query_subcall_whitelist_and_strip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn llm_query_subcall_whitelist_and_strip() {
    let step1 = tool_call_response(&[(
        "call_lq",
        "llm_query",
        json!({"prompt": "summarize: alpha"}),
    )]);
    // subcall reply (no tools key)
    let subcall_reply = text_response("ALPHA SUMMARY");
    let step2 = tool_call_response(&[("call_f", "final_answer", json!({"content": "done"}))]);
    let (upstream_url, handle) = spawn_scripted_upstream(vec![step1, subcall_reply, step2]).await;
    let proxy_url = spawn_rlm_proxy(format!("{upstream_url}/v1"), default_rlm()).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{"role": "user", "content": "summarize alpha"}],
            "temperature": 0.5,
            "top_p": 0.9,
            "logit_bias": {"50256": -100},
            "user": "caller-1"
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::OK);

    let seen = take_seen(&handle);
    assert_eq!(
        seen.len(),
        3,
        "should have 3 upstream requests: loop#1, subcall, loop#2"
    );

    // Request #2 is the subcall: identified by absence of "tools" key
    let subcall_req = &seen[1];
    assert!(
        subcall_req.body.get("tools").is_none(),
        "subcall should not have 'tools'"
    );
    assert_eq!(subcall_req.body["model"], "local-model");
    assert_eq!(
        subcall_req.body["messages"],
        json!([{"role": "user", "content": "summarize: alpha"}])
    );
    assert_eq!(subcall_req.body["temperature"], 0.5);
    assert_eq!(subcall_req.body["top_p"], 0.9);
    assert_eq!(subcall_req.body["stream"], false);

    // Fields that must NOT be in the subcall body
    for forbidden in &[
        "tools",
        "tool_choice",
        "logit_bias",
        "response_format",
        "stop",
        "logprobs",
        "n",
        "metadata",
        "user",
        "safety_identifier",
    ] {
        assert!(
            subcall_req.body.get(*forbidden).is_none(),
            "subcall should not contain '{forbidden}'"
        );
    }

    // Loop requests (#1 and #3) carry temperature but not logit_bias
    for (i, req) in [&seen[0], &seen[2]].iter().enumerate() {
        assert_eq!(
            req.body["temperature"],
            0.5,
            "loop request #{} should carry temperature=0.5",
            i + 1
        );
        assert!(
            req.body.get("logit_bias").is_none(),
            "loop request #{} should not carry logit_bias",
            i + 1
        );
    }

    // Request #3 (second loop step) should have the subcall result as a tool message
    let req3_messages = seen[2].body["messages"]
        .as_array()
        .expect("request #3 should have messages");
    let subcall_tool_msg = req3_messages
        .iter()
        .find(|msg| msg["role"] == "tool" && msg["tool_call_id"] == "call_lq")
        .expect("should find tool result for call_lq");
    assert_eq!(subcall_tool_msg["content"], "ALPHA SUMMARY");
}

// ---------------------------------------------------------------------------
// L6: nudge_then_text_is_final_answer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn nudge_then_text_is_final_answer() {
    let step1 = text_response("thinking out loud");
    let step2 = text_response("the answer is 42");
    let (upstream_url, handle) = spawn_scripted_upstream(vec![step1, step2]).await;
    let proxy_url = spawn_rlm_proxy(format!("{upstream_url}/v1"), default_rlm()).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{"role": "user", "content": "answer me"}]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.expect("response should be JSON");
    assert_eq!(body["choices"][0]["message"]["content"], "the answer is 42");

    let seen = take_seen(&handle);
    assert_eq!(seen.len(), 2, "should have exactly 2 upstream requests");

    // Request #2 messages should end with the verbatim assistant text message
    // followed by a user nudge message
    let req2_messages = seen[1].body["messages"]
        .as_array()
        .expect("request #2 should have messages");
    let len = req2_messages.len();
    assert!(len >= 2, "request #2 should have at least 2 messages");

    // Second-to-last: the assistant text message from step1
    assert_eq!(req2_messages[len - 2]["role"], "assistant");
    assert_eq!(req2_messages[len - 2]["content"], "thinking out loud");

    // Last: the nudge user message
    assert_eq!(req2_messages[len - 1]["role"], "user");
    let nudge_content = req2_messages[len - 1]["content"]
        .as_str()
        .expect("nudge message content should be a string");
    assert!(
        nudge_content.contains("Reminder:") || nudge_content.contains("final_answer"),
        "nudge message should contain 'Reminder:' or 'final_answer': {nudge_content}"
    );
}

// ---------------------------------------------------------------------------
// L7: loop_and_subcall_share_caller_authorization
// ---------------------------------------------------------------------------

#[tokio::test]
async fn loop_and_subcall_share_caller_authorization() {
    let step1 = tool_call_response(&[(
        "call_lq",
        "llm_query",
        json!({"prompt": "summarize: alpha"}),
    )]);
    let subcall_reply = text_response("ALPHA SUMMARY");
    let step2 = tool_call_response(&[("call_f", "final_answer", json!({"content": "done"}))]);
    let (upstream_url, handle) = spawn_scripted_upstream(vec![step1, subcall_reply, step2]).await;

    // No configured upstream API key so caller's auth header is forwarded
    let proxy_url =
        spawn_rlm_proxy_with_key(format!("{upstream_url}/v1"), default_rlm(), None).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .header(header::AUTHORIZATION, "Bearer caller-key")
        .json(&json!({
            "model": "local-model",
            "messages": [{"role": "user", "content": "summarize alpha"}]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::OK);

    let seen = take_seen(&handle);
    assert_eq!(seen.len(), 3, "should have 3 upstream requests");

    for (i, req) in seen.iter().enumerate() {
        assert_eq!(
            req.authorization.as_deref(),
            Some("Bearer caller-key"),
            "request #{} should carry caller authorization",
            i + 1
        );
    }
}

// ---------------------------------------------------------------------------
// L8: step_budget_exhaustion_is_500_rlm_error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn step_budget_exhaustion_is_500_rlm_error() {
    // context_describe does not terminate the loop, so a second step would be needed
    let step1 = tool_call_response(&[("call_d", "context_describe", json!({}))]);
    let (upstream_url, handle) = spawn_scripted_upstream(vec![step1]).await;
    let rlm = RlmLoopConfig {
        max_steps: 1,
        ..default_rlm()
    };
    let proxy_url = spawn_rlm_proxy(format!("{upstream_url}/v1"), rlm).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{"role": "user", "content": "describe context"}]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body: Value = response.json().await.expect("error body should be JSON");
    assert_eq!(body["error"]["type"], "rlm_error");
    let message = body["error"]["message"]
        .as_str()
        .expect("error message should be a string");
    assert!(
        message.contains("steps") || message.contains("budget"),
        "error message should mention 'steps' or 'budget': {message}"
    );

    let seen = take_seen(&handle);
    assert_eq!(seen.len(), 1, "should have exactly 1 upstream request");
}

// ---------------------------------------------------------------------------
// L9: subcall_budget_exhaustion_is_500_rlm_error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subcall_budget_exhaustion_is_500_rlm_error() {
    let step1 = tool_call_response(&[(
        "call_lq",
        "llm_query",
        json!({"prompt": "summarize: alpha"}),
    )]);
    let (upstream_url, handle) = spawn_scripted_upstream(vec![step1]).await;
    let rlm = RlmLoopConfig {
        max_subcalls: 0,
        ..default_rlm()
    };
    let proxy_url = spawn_rlm_proxy(format!("{upstream_url}/v1"), rlm).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{"role": "user", "content": "summarize alpha"}]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body: Value = response.json().await.expect("error body should be JSON");
    assert_eq!(body["error"]["type"], "rlm_error");
    let message = body["error"]["message"]
        .as_str()
        .expect("error message should be a string");
    assert!(
        message.contains("subcalls") || message.contains("budget"),
        "error message should mention 'subcalls' or 'budget': {message}"
    );

    let seen = take_seen(&handle);
    assert_eq!(seen.len(), 1, "should have exactly 1 upstream request");
}

// ---------------------------------------------------------------------------
// L10: wall_clock_exhaustion_is_500_rlm_error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wall_clock_exhaustion_is_500_rlm_error() {
    let (upstream_url, handle) = spawn_scripted_upstream(vec![]).await;
    let rlm = RlmLoopConfig {
        max_wall: Duration::ZERO,
        ..default_rlm()
    };
    let proxy_url = spawn_rlm_proxy(format!("{upstream_url}/v1"), rlm).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{"role": "user", "content": "quick answer"}]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body: Value = response.json().await.expect("error body should be JSON");
    assert_eq!(body["error"]["type"], "rlm_error");
    let message = body["error"]["message"]
        .as_str()
        .expect("error message should be a string");
    assert!(
        message.contains("wall") || message.contains("clock"),
        "error message should mention 'wall' or 'clock': {message}"
    );

    let seen = take_seen(&handle);
    assert_eq!(seen.len(), 0, "should have zero upstream requests");
}

// ---------------------------------------------------------------------------
// L11: query_context_split_edge_cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn query_context_single_user_message_shows_empty_context() {
    let step1 = tool_call_response(&[("call_f", "final_answer", json!({"content": "done"}))]);
    let (upstream_url, handle) = spawn_scripted_upstream(vec![step1]).await;
    let proxy_url = spawn_rlm_proxy(format!("{upstream_url}/v1"), default_rlm()).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{"role": "user", "content": "single user message"}]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::OK);

    let seen = take_seen(&handle);
    let system_content = seen[0].body["messages"][0]["content"]
        .as_str()
        .expect("system message content should be a string");
    assert!(
        system_content.contains("\"messages\":0"),
        "context summary should show 0 context messages for single-message input: {system_content}"
    );
}

#[tokio::test]
async fn query_context_no_user_message_is_400() {
    let (upstream_url, handle) = spawn_scripted_upstream(vec![]).await;
    let proxy_url = spawn_rlm_proxy(format!("{upstream_url}/v1"), default_rlm()).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [
                {"role": "system", "content": "be direct"},
                {"role": "assistant", "content": "understood"}
            ]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.expect("error body should be JSON");
    assert_eq!(body["error"]["type"], "invalid_request");
    let message = body["error"]["message"]
        .as_str()
        .expect("error message should be a string");
    assert!(
        message.contains("user message"),
        "error message should mention 'user message': {message}"
    );

    let seen = take_seen(&handle);
    assert_eq!(seen.len(), 0, "should have zero upstream requests");
}

// ---------------------------------------------------------------------------
// L12: tool_results_are_truncated_to_preview_size
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tool_results_are_truncated_to_preview_size() {
    let long_content = "x".repeat(500);
    let step1 = tool_call_response(&[("call_s", "context_slice", json!({"start": 0, "end": 1}))]);
    let step2 = tool_call_response(&[("call_f", "final_answer", json!({"content": "done"}))]);
    let (upstream_url, handle) = spawn_scripted_upstream(vec![step1, step2]).await;
    let rlm = RlmLoopConfig {
        tool_result_preview_bytes: 32,
        ..default_rlm()
    };
    let proxy_url = spawn_rlm_proxy(format!("{upstream_url}/v1"), rlm).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [
                {"role": "user", "content": long_content},
                {"role": "user", "content": "summarize"}
            ]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::OK);

    let seen = take_seen(&handle);
    assert_eq!(seen.len(), 2);

    let req2_messages = seen[1].body["messages"]
        .as_array()
        .expect("request #2 should have messages");
    let tool_result = req2_messages
        .iter()
        .find(|msg| msg["role"] == "tool" && msg["tool_call_id"] == "call_s")
        .expect("should find tool result for call_s");
    let content = tool_result["content"]
        .as_str()
        .expect("tool result content should be a string");

    assert!(
        content.len() < 150,
        "truncated tool result should be < 150 bytes but got {}: {content}",
        content.len()
    );
    assert!(
        content.contains("[truncated "),
        "truncated tool result should contain '[truncated ': {content}"
    );
}

// ---------------------------------------------------------------------------
// L13: unknown_tool_and_bad_arguments_are_tool_errors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_tool_and_bad_arguments_are_tool_errors() {
    // Both calls in one step
    let step1 = tool_call_response(&[
        ("call_u", "made_up_tool", json!({})),
        ("call_b", "context_slice", json!({"start": "zero"})), // wrong type
    ]);
    let step2 = tool_call_response(&[("call_f", "final_answer", json!({"content": "done"}))]);
    let (upstream_url, handle) = spawn_scripted_upstream(vec![step1, step2]).await;
    let proxy_url = spawn_rlm_proxy(format!("{upstream_url}/v1"), default_rlm()).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{"role": "user", "content": "test errors"}]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.expect("response should be JSON");
    assert_eq!(body["choices"][0]["message"]["content"], "done");

    let seen = take_seen(&handle);
    assert_eq!(seen.len(), 2);

    let req2_messages = seen[1].body["messages"]
        .as_array()
        .expect("request #2 should have messages");

    let tool_msgs: Vec<&Value> = req2_messages
        .iter()
        .filter(|msg| msg["role"] == "tool")
        .collect();
    assert_eq!(
        tool_msgs.len(),
        2,
        "should have 2 tool result messages in request #2"
    );

    let unknown_tool_msg = req2_messages
        .iter()
        .find(|msg| msg["role"] == "tool" && msg["tool_call_id"] == "call_u")
        .expect("should find tool result for call_u");
    let unknown_content = unknown_tool_msg["content"]
        .as_str()
        .expect("unknown tool content should be a string");
    assert!(
        unknown_content.contains("unknown tool") || unknown_content.contains("made_up_tool"),
        "unknown tool message should mention 'unknown tool' or 'made_up_tool': {unknown_content}"
    );

    let bad_args_msg = req2_messages
        .iter()
        .find(|msg| msg["role"] == "tool" && msg["tool_call_id"] == "call_b")
        .expect("should find tool result for call_b");
    let bad_args_content = bad_args_msg["content"]
        .as_str()
        .expect("bad args content should be a string");
    assert!(
        bad_args_content.contains("invalid arguments")
            || bad_args_content.contains("context_slice"),
        "bad args message should mention 'invalid arguments' or 'context_slice': {bad_args_content}"
    );
}

// ---------------------------------------------------------------------------
// L14: assistant_message_truncated_to_cap_before_push
// ---------------------------------------------------------------------------

#[tokio::test]
async fn assistant_message_truncated_to_cap_before_push() {
    // Build a response with 33 llm_query tool calls (one over the 32-call cap).
    let over_cap_calls: Vec<(&str, &str, Value)> = (0..=32_usize)
        .map(|i| {
            // The lifetime of the owned strings needs to outlast the vec, so we
            // build them as leaked &'static str via Box::leak. This is fine in a
            // test context.
            let id: &'static str = Box::leak(format!("call_{i}").into_boxed_str());
            (id, "llm_query", json!({"prompt": "q"}))
        })
        .collect();

    // First upstream call: assistant message with 33 tool calls.
    let step1 = tool_call_response(&over_cap_calls);

    // 32 subcall replies (only the first 32 calls are dispatched).
    let mut responses = vec![step1];
    for _ in 0..32 {
        responses.push(text_response("sub"));
    }

    // Second loop step: final_answer.
    responses.push(tool_call_response(&[(
        "call_final",
        "final_answer",
        json!({"content": "done"}),
    )]));

    let (upstream_url, handle) = spawn_scripted_upstream(responses).await;
    let rlm = RlmLoopConfig {
        max_steps: 5,
        max_subcalls: 64,
        max_wall: Duration::from_secs(30),
        ..Default::default()
    };
    let proxy_url = spawn_rlm_proxy(format!("{upstream_url}/v1"), rlm).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{"role": "user", "content": "run many queries"}]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "proxy should succeed, not return 400/500 from orphaned tool-call IDs"
    );
    let body: Value = response.json().await.expect("response should be JSON");
    assert_eq!(body["choices"][0]["message"]["content"], "done");

    // On the second loop request (request #34: loop#1 + 32 subcalls + loop#2),
    // the assistant message in history should carry exactly 32 tool_calls, and
    // there should be exactly 32 tool result messages following it.
    let seen = take_seen(&handle);
    // requests: 1 loop + 32 subcalls + 1 loop = 34 total
    assert_eq!(seen.len(), 34, "should have exactly 34 upstream requests");

    let last_loop_req = &seen[33];
    let messages = last_loop_req.body["messages"]
        .as_array()
        .expect("last loop request should have messages");

    let assistant_msg = messages
        .iter()
        .find(|msg| msg["role"] == "assistant" && !msg["tool_calls"].is_null())
        .expect("second loop request should have an assistant tool-call message in history");

    let pushed_tool_calls = assistant_msg["tool_calls"]
        .as_array()
        .expect("assistant tool_calls should be an array");
    assert_eq!(
        pushed_tool_calls.len(),
        32,
        "assistant message pushed to history should have exactly 32 tool_calls (cap), not 33"
    );

    let tool_result_count = messages.iter().filter(|msg| msg["role"] == "tool").count();
    assert_eq!(
        tool_result_count, 32,
        "there should be exactly 32 tool result messages, matching the 32 tool_calls in history"
    );
}

// ---------------------------------------------------------------------------
// E1: loop_upstream_failure_is_502
// ---------------------------------------------------------------------------

#[tokio::test]
async fn loop_upstream_failure_is_502() {
    // Use a raw handler that always returns 502
    async fn broken_handler() -> Response {
        (
            StatusCode::BAD_GATEWAY,
            axum::Json(json!({"error": "broken"})),
        )
            .into_response()
    }

    let router = Router::new().route("/v1/chat/completions", post(broken_handler));
    let upstream_url = spawn_router(router).await;
    let proxy_url = spawn_rlm_proxy(format!("{upstream_url}/v1"), default_rlm()).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{"role": "user", "content": "test"}]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body: Value = response.json().await.expect("error body should be JSON");
    assert_eq!(body["error"]["type"], "upstream_error");
    let message = body["error"]["message"]
        .as_str()
        .expect("error message should be a string");
    assert!(
        message.contains("upstream"),
        "error message should contain 'upstream': {message}"
    );
}

// ---------------------------------------------------------------------------
// E2: malformed_completion_is_502
// ---------------------------------------------------------------------------

#[tokio::test]
async fn malformed_completion_is_502() {
    // choices is an empty array — no choices[0].message
    let malformed = json!({"id": "x", "object": "chat.completion", "choices": []});
    let (upstream_url, _handle) = spawn_scripted_upstream(vec![malformed]).await;
    let proxy_url = spawn_rlm_proxy(format!("{upstream_url}/v1"), default_rlm()).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "messages": [{"role": "user", "content": "test"}]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body: Value = response.json().await.expect("error body should be JSON");
    let message = body["error"]["message"]
        .as_str()
        .expect("error message should be a string");
    assert!(
        message.contains("malformed"),
        "error message should contain 'malformed': {message}"
    );
}

// ---------------------------------------------------------------------------
// S1: stream_loop_emits_intact_delta_stop_and_done
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stream_loop_emits_intact_delta_stop_and_done() {
    let answer = "HELLO  FROM\nLOOP";
    let step1 = tool_call_response(&[("call_1", "final_answer", json!({"content": answer}))]);
    let (upstream_url, _handle) = spawn_scripted_upstream(vec![step1]).await;
    let proxy_url = spawn_rlm_proxy(format!("{upstream_url}/v1"), default_rlm()).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "stream": true,
            "messages": [{"role": "user", "content": "stream test"}]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        content_type.starts_with("text/event-stream"),
        "content-type should start with text/event-stream: {content_type}"
    );

    let body = response.text().await.expect("SSE body should be readable");
    let events = sse_data_events(&body);
    assert_eq!(events.len(), 3, "should have exactly 3 SSE data events");

    // event[0]: content delta
    let chunk0: Value = serde_json::from_str(&events[0]).expect("event[0] should be JSON");
    let delta_content = chunk0["choices"][0]["delta"]["content"]
        .as_str()
        .expect("event[0] delta should have content");
    assert_eq!(
        delta_content, answer,
        "delta content should equal answer byte-for-byte"
    );

    // event[1]: stop chunk
    let chunk1: Value = serde_json::from_str(&events[1]).expect("event[1] should be JSON");
    assert_eq!(chunk1["choices"][0]["finish_reason"], "stop");
    assert_eq!(chunk1["choices"][0]["delta"], json!({}));

    // event[2]: [DONE]
    assert_eq!(events[2], "[DONE]");
}

// ---------------------------------------------------------------------------
// S2: stream_loop_error_is_sse_error_event_then_done
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stream_loop_error_is_sse_error_event_then_done() {
    let (upstream_url, _handle) = spawn_scripted_upstream(vec![]).await;
    let rlm = RlmLoopConfig {
        max_wall: Duration::ZERO,
        ..default_rlm()
    };
    let proxy_url = spawn_rlm_proxy(format!("{upstream_url}/v1"), rlm).await;

    let response = Client::new()
        .post(format!("{proxy_url}/v1/chat/completions"))
        .json(&json!({
            "model": "local-model",
            "stream": true,
            "messages": [{"role": "user", "content": "quick"}]
        }))
        .send()
        .await
        .expect("proxy request should complete");

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(content_type.starts_with("text/event-stream"));

    let body = response.text().await.expect("SSE body should be readable");
    let events = sse_data_events(&body);
    assert!(!events.is_empty(), "should have at least one SSE event");

    // First data event should contain the error
    let first_event: Value =
        serde_json::from_str(&events[0]).expect("first SSE event should be JSON");
    assert_eq!(
        first_event["error"]["type"], "rlm_error",
        "first SSE event should be an rlm_error"
    );

    // Last event should be [DONE]
    assert_eq!(
        events.last().expect("should have at least one event"),
        "[DONE]"
    );
}

// ---------------------------------------------------------------------------
// S3: stream_headers_arrive_before_loop_completes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stream_headers_arrive_before_loop_completes() {
    // Gated upstream: does not reply until we fire the gate
    let (gate_tx, gate_rx) = oneshot::channel::<()>();
    let gate_rx = Arc::new(tokio::sync::Mutex::new(Some(gate_rx)));

    async fn gated_handler(
        State(gate): State<Arc<tokio::sync::Mutex<Option<oneshot::Receiver<()>>>>>,
    ) -> Response {
        // Wait for the gate to be fired (or sender dropped)
        let receiver = gate.lock().await.take();
        if let Some(rx) = receiver {
            let _ = rx.await;
        }
        (
            StatusCode::OK,
            axum::Json(json!({
                "id": "chatcmpl-gated",
                "object": "chat.completion",
                "created": 0,
                "model": "local-model",
                "choices": [
                    {
                        "index": 0,
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [
                                {
                                    "id": "call_g",
                                    "type": "function",
                                    "function": {
                                        "name": "final_answer",
                                        "arguments": "{\"content\":\"gated answer\"}"
                                    }
                                }
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }
                ]
            })),
        )
            .into_response()
    }

    let router = Router::new()
        .route("/v1/chat/completions", post(gated_handler))
        .with_state(gate_rx);
    let upstream_url = spawn_router(router).await;
    let proxy_url = spawn_rlm_proxy(format!("{upstream_url}/v1"), default_rlm()).await;

    // Send the stream request — headers should arrive before the gate fires
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        Client::new()
            .post(format!("{proxy_url}/v1/chat/completions"))
            .json(&json!({
                "model": "local-model",
                "stream": true,
                "messages": [{"role": "user", "content": "gated stream test"}]
            }))
            .send(),
    )
    .await
    .expect("request headers should arrive within 5 seconds")
    .expect("proxy request should complete");

    // At this point headers have arrived; gate is still closed
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        content_type.starts_with("text/event-stream"),
        "content-type should be text/event-stream while gate is closed: {content_type}"
    );

    // Fire the gate so the loop can complete
    gate_tx.send(()).expect("gate channel should be open");

    // Body should complete with [DONE]
    let body = tokio::time::timeout(Duration::from_secs(5), response.text())
        .await
        .expect("SSE body should arrive within 5 seconds after gate fires")
        .expect("SSE body should be readable");
    assert!(body.contains("[DONE]"), "SSE body should contain [DONE]");
}
