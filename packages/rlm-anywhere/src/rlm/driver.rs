use std::time::{Duration, Instant};

use secrecy::SecretString;
use serde_json::{Map, Value, json};
use tokio::task::spawn_blocking;
use tokio::time::timeout;

use crate::rlm::sandbox::{Sandbox, SandboxLimits};
use crate::rlm::tools::{ToolInvocation, ToolParseError, parse_tool_call, tool_definitions};
use crate::rlm::{BudgetError, ContextMessage, ContextStore, ContextSummary, Guardrails};
use crate::upstream::{ModelError, ModelRequest, RigModelBackend};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RlmLoopConfig {
    pub max_steps: u64,
    pub max_subcalls: u64,
    pub max_wall: Duration,
    pub tool_result_preview_bytes: usize,
    pub sandbox_limits: SandboxLimits,
}

impl Default for RlmLoopConfig {
    fn default() -> Self {
        Self {
            max_steps: 20,
            max_subcalls: 64,
            max_wall: Duration::from_millis(120_000),
            tool_result_preview_bytes: 8_192,
            sandbox_limits: SandboxLimits::default(),
        }
    }
}

pub(crate) struct LoopInput {
    pub(crate) model: String,
    pub(crate) query_message: Value,
    pub(crate) context: ContextStore,
    pub(crate) sampling: Map<String, Value>,
    pub(crate) caller_authorization: Option<SecretString>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RlmError {
    #[error("rlm loop budget exhausted: {0}")]
    Budget(#[from] BudgetError),
    #[error("rlm loop exceeded wall clock budget of {budget:?}")]
    WallClock { budget: Duration },
    #[error("{0}")]
    Upstream(#[from] ModelError),
    #[error("upstream returned a malformed chat completion: {detail}")]
    MalformedCompletion { detail: String },
}

pub(crate) struct QueryContextSplit {
    pub(crate) query_message: Value,
    pub(crate) context: ContextStore,
}

/// Maximum number of tool calls dispatched from a single assistant message.
/// Upstream responses with more tool calls have their suffix silently dropped
/// to bound pathological fan-out (e.g. thousands of `run_js` calls).
const MAX_TOOL_CALLS_PER_STEP: usize = 32;

pub(crate) const SAMPLING_WHITELIST: [&str; 4] = [
    "temperature",
    "top_p",
    "frequency_penalty",
    "presence_penalty",
];

pub(crate) async fn run_loop(
    backend: &RigModelBackend,
    config: &RlmLoopConfig,
    input: LoopInput,
) -> Result<String, RlmError> {
    let summary = input.context.describe();
    let deadline = Instant::now() + config.max_wall;
    let mut state = LoopState {
        backend,
        config,
        guardrails: Guardrails::new(config.max_steps, config.max_subcalls),
        context: input.context,
        model: input.model,
        sampling: input.sampling,
        caller_authorization: input.caller_authorization,
        messages: vec![controller_system_message(&summary), input.query_message],
        nudged: false,
        deadline,
    };

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(RlmError::WallClock {
                budget: config.max_wall,
            });
        }

        state.guardrails.use_step()?;

        let response = timeout(
            remaining,
            backend.complete(ModelRequest {
                body: build_loop_request_body(&state.model, &state.sampling, &state.messages),
                caller_authorization: state.caller_authorization.clone(),
            }),
        )
        .await
        .map_err(|_| RlmError::WallClock {
            budget: config.max_wall,
        })?
        .map_err(RlmError::Upstream)?;

        let message = response
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .and_then(Value::as_object)
            .cloned()
            .map(Value::Object)
            .ok_or_else(|| RlmError::MalformedCompletion {
                detail: "missing choices[0].message".to_owned(),
            })?;

        let tool_calls = extract_tool_calls(&message)?;
        if tool_calls.is_empty() {
            let text = message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !state.nudged {
                state.nudged = true;
                state.messages.push(message);
                state.messages.push(json!({
                    "role": "user",
                    "content": NUDGE_USER_MESSAGE
                }));
                continue;
            }

            return Ok(text.to_owned());
        }

        state.messages.push(message);

        let dispatched = if tool_calls.len() > MAX_TOOL_CALLS_PER_STEP {
            tracing::warn!(
                received = tool_calls.len(),
                cap = MAX_TOOL_CALLS_PER_STEP,
                "assistant message exceeded tool-call cap; processing prefix"
            );
            &tool_calls[..MAX_TOOL_CALLS_PER_STEP]
        } else {
            &tool_calls[..]
        };

        for call in dispatched {
            // Per-tool wall-clock recheck: bounds time spent across a batch of
            // blocking evals (e.g. run_js) that are not charged via use_subcall.
            if state.deadline.saturating_duration_since(Instant::now()).is_zero() {
                return Err(RlmError::WallClock {
                    budget: config.max_wall,
                });
            }

            match dispatch_tool(&mut state, call).await {
                ToolDispatch::Result(content) => state.messages.push(tool_result_message(
                    &call.id,
                    truncate_tool_result(content, state.config.tool_result_preview_bytes),
                )),
                ToolDispatch::FinalAnswer(content) => return Ok(content),
                ToolDispatch::Fatal(error) => return Err(error),
            }
        }
    }
}

pub(crate) fn split_query_and_context(messages: &[Value]) -> Option<QueryContextSplit> {
    let query_index = messages
        .iter()
        .rposition(|message| message.get("role").and_then(Value::as_str) == Some("user"))?;

    let query_message = messages.get(query_index)?.clone();
    let context_messages = messages
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != query_index)
        .map(|(_, message)| message.clone())
        .collect::<Vec<_>>();

    Some(QueryContextSplit {
        query_message,
        context: ContextStore::from_chat_messages(&context_messages),
    })
}

pub(crate) fn extract_sampling(request: &Value) -> Map<String, Value> {
    SAMPLING_WHITELIST
        .into_iter()
        .filter_map(|key| {
            request
                .get(key)
                .filter(|value| !value.is_null())
                .map(|value| (key.to_owned(), value.clone()))
        })
        .collect()
}

#[must_use]
pub(crate) fn build_loop_request_body(
    model: &str,
    sampling: &Map<String, Value>,
    messages: &[Value],
) -> Value {
    let mut body = Map::new();
    body.insert("model".to_owned(), Value::String(model.to_owned()));
    body.insert("messages".to_owned(), Value::Array(messages.to_vec()));
    body.insert("tools".to_owned(), tool_definitions());
    body.insert("tool_choice".to_owned(), Value::String("auto".to_owned()));
    body.insert("stream".to_owned(), Value::Bool(false));
    body.extend(
        sampling
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    Value::Object(body)
}

#[must_use]
pub(crate) fn build_subcall_body(
    model: &str,
    sampling: &Map<String, Value>,
    prompt: &str,
) -> Value {
    let mut body = Map::new();
    body.insert("model".to_owned(), Value::String(model.to_owned()));
    body.insert(
        "messages".to_owned(),
        Value::Array(vec![json!({
            "role": "user",
            "content": prompt
        })]),
    );
    body.insert("stream".to_owned(), Value::Bool(false));
    body.extend(
        sampling
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    Value::Object(body)
}

#[must_use]
pub(crate) fn controller_system_message(summary: &ContextSummary) -> Value {
    let summary_json =
        serde_json::to_string(summary).unwrap_or_else(|error| format!(r#"{{"error":"{error}"}}"#));

    json!({
        "role": "system",
        "content": format!("{CONTROLLER_SYSTEM_PROMPT}\n\nContext summary: {summary_json}")
    })
}

pub(crate) const CONTROLLER_SYSTEM_PROMPT: &str = "\
You are the controller of a Recursive Language Model (RLM) loop. The caller's \
conversation history has been externalized into a context store you inspect \
with tools instead of reading it all at once.

Rules:
- Use context_describe, context_slice, and context_grep to inspect the \
externalized context. Indices are zero-based; slice(start, end) is half-open.
- Use llm_query to delegate one self-contained subtask to the underlying \
language model. Include all data the subtask needs.
- Use run_js to evaluate a JavaScript function body in a sandbox. The code \
must return a JSON-serializable value. The sandbox is stateless; each call \
starts fresh.
- Tool results may be truncated; narrow your queries instead of requesting \
large ranges.
- When you know the answer, call final_answer exactly once. Its content is \
returned to the caller verbatim.";

pub(crate) const NUDGE_USER_MESSAGE: &str = "Reminder: respond only with tool calls. \
When you have the answer, call final_answer with the complete answer.";

#[must_use]
pub(crate) fn truncate_tool_result(content: String, preview_bytes: usize) -> String {
    if content.len() <= preview_bytes {
        return content;
    }

    let boundary = (0..=preview_bytes)
        .rev()
        .find(|index| content.is_char_boundary(*index))
        .unwrap_or(0);
    let total = content.len();
    let omitted = total.saturating_sub(boundary);
    let (prefix, _) = content.split_at(boundary);
    format!("{prefix}\n[truncated {omitted} of {total} bytes]")
}

#[must_use]
pub(crate) fn tool_result_message(tool_call_id: &str, content: String) -> Value {
    json!({
        "role": "tool",
        "tool_call_id": tool_call_id,
        "content": content
    })
}

#[derive(Clone, Debug)]
pub(crate) struct ParsedToolCall {
    id: String,
    name: String,
    arguments: String,
}

fn extract_tool_calls(message: &Value) -> Result<Vec<ParsedToolCall>, RlmError> {
    let Some(tool_calls) = message.get("tool_calls") else {
        return Ok(Vec::new());
    };
    if tool_calls.is_null() {
        return Ok(Vec::new());
    }
    let Some(tool_calls) = tool_calls.as_array() else {
        return Err(RlmError::MalformedCompletion {
            detail: "tool_calls".to_owned(),
        });
    };
    if tool_calls.is_empty() {
        return Ok(Vec::new());
    }

    tool_calls
        .iter()
        .enumerate()
        .map(|(index, call)| extract_tool_call(index, call))
        .collect()
}

fn extract_tool_call(index: usize, call: &Value) -> Result<ParsedToolCall, RlmError> {
    let base = format!("tool_calls[{index}]");
    let id = string_field(call, "id", &base)?;
    let Some(function) = call.get("function") else {
        return Err(malformed_field(format!("{base}.function")));
    };
    let Some(function) = function.as_object() else {
        return Err(malformed_field(format!("{base}.function")));
    };
    let function = Value::Object(function.clone());
    let name = string_field(&function, "name", &format!("{base}.function"))?;
    let arguments = string_field(&function, "arguments", &format!("{base}.function"))?;

    Ok(ParsedToolCall {
        id,
        name,
        arguments,
    })
}

fn string_field(value: &Value, key: &str, base: &str) -> Result<String, RlmError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| malformed_field(format!("{base}.{key}")))
}

fn malformed_field(detail: String) -> RlmError {
    RlmError::MalformedCompletion { detail }
}

enum ToolDispatch {
    Result(String),
    FinalAnswer(String),
    Fatal(RlmError),
}

async fn dispatch_tool(state: &mut LoopState<'_>, call: &ParsedToolCall) -> ToolDispatch {
    let invocation = match parse_tool_call(&call.name, &call.arguments) {
        Ok(invocation) => invocation,
        Err(ToolParseError::UnknownTool { name }) => {
            return ToolDispatch::Result(tool_error_content(format!("unknown tool: {name}")));
        }
        Err(ToolParseError::InvalidArguments { name, detail }) => {
            return ToolDispatch::Result(tool_error_content(format!(
                "invalid arguments for {name}: {detail}"
            )));
        }
    };

    match invocation {
        ToolInvocation::ContextDescribe => {
            ToolDispatch::Result(json!(state.context.describe()).to_string())
        }
        ToolInvocation::ContextSlice { start, end } => {
            let start = match usize::try_from(start) {
                Ok(value) => value,
                Err(_) => {
                    return ToolDispatch::Result(tool_error_content("index out of range"));
                }
            };
            let end = match usize::try_from(end) {
                Ok(value) => value,
                Err(_) => {
                    return ToolDispatch::Result(tool_error_content("index out of range"));
                }
            };
            ToolDispatch::Result(context_slice_content(&state.context, start, end))
        }
        ToolInvocation::ContextGrep { needle } => {
            ToolDispatch::Result(context_grep_content(&state.context, &needle))
        }
        ToolInvocation::LlmQuery { prompt } => {
            let Err(budget_error) = state.guardrails.use_subcall() else {
                let remaining = state.deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return ToolDispatch::Fatal(RlmError::WallClock {
                        budget: state.config.max_wall,
                    });
                }
                let response = timeout(
                    remaining,
                    state.backend.complete(ModelRequest {
                        body: build_subcall_body(&state.model, &state.sampling, &prompt),
                        caller_authorization: state.caller_authorization.clone(),
                    }),
                )
                .await;
                return match response {
                    Err(_elapsed) => ToolDispatch::Fatal(RlmError::WallClock {
                        budget: state.config.max_wall,
                    }),
                    Ok(Ok(response)) => match subcall_text(response) {
                        Ok(content) => ToolDispatch::Result(content),
                        Err(error) => ToolDispatch::Result(tool_error_content(error)),
                    },
                    Ok(Err(error)) => {
                        ToolDispatch::Result(tool_error_content(format!("subcall failed: {error}")))
                    }
                };
            };
            ToolDispatch::Fatal(RlmError::Budget(budget_error))
        }
        ToolInvocation::RunJs { code } => {
            let limits = state.config.sandbox_limits.clone();
            let join = spawn_blocking(move || Sandbox::new(limits).eval_json(&code)).await;
            match join {
                Ok(Ok(content)) => ToolDispatch::Result(content),
                Ok(Err(error)) => ToolDispatch::Result(tool_error_content(error.to_string())),
                Err(_) => ToolDispatch::Result(tool_error_content("sandbox task failed")),
            }
        }
        ToolInvocation::FinalAnswer { content } => ToolDispatch::FinalAnswer(content),
    }
}

fn context_slice_content(context: &ContextStore, start: usize, end: usize) -> String {
    let items = context
        .slice(start, end)
        .iter()
        .enumerate()
        .map(|(offset, message)| {
            let index = start.saturating_add(offset);
            context_item(index, message)
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&items).unwrap_or_else(|error| tool_error_content(error.to_string()))
}

fn context_grep_content(context: &ContextStore, needle: &str) -> String {
    let messages = context
        .grep_indexed(needle)
        .into_iter()
        .map(|(index, message)| context_item(index, message))
        .collect::<Vec<_>>();
    serde_json::to_string(&messages).unwrap_or_else(|error| tool_error_content(error.to_string()))
}

fn context_item(index: usize, message: &ContextMessage) -> Value {
    json!({
        "index": index,
        "role": message.role(),
        "text": message.text(),
    })
}

fn subcall_text(response: Value) -> Result<String, String> {
    response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"))
        .and_then(Value::as_object)
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| "subcall returned no assistant text".to_owned())
}

fn tool_error_content(message: impl Into<String>) -> String {
    json!({ "error": message.into() }).to_string()
}

struct LoopState<'a> {
    backend: &'a RigModelBackend,
    config: &'a RlmLoopConfig,
    guardrails: Guardrails,
    context: ContextStore,
    model: String,
    sampling: Map<String, Value>,
    caller_authorization: Option<SecretString>,
    messages: Vec<Value>,
    nudged: bool,
    deadline: Instant,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{extract_sampling, split_query_and_context, truncate_tool_result};

    #[test]
    fn split_query_and_context_uses_last_user_message() {
        let messages = vec![
            json!({"role": "system", "content": "Be direct."}),
            json!({"role": "user", "content": "one"}),
            json!({"role": "assistant", "content": "two"}),
            json!({"role": "user", "content": "query"}),
        ];

        let split = split_query_and_context(&messages).expect("split should exist");
        assert_eq!(split.query_message, messages[3]);
        assert_eq!(split.context.len(), 3);
        assert_eq!(
            split.context.get(0).and_then(|message| message.role()),
            Some("system")
        );
        assert_eq!(
            split.context.get(1).and_then(|message| message.role()),
            Some("user")
        );
        assert_eq!(
            split.context.get(2).and_then(|message| message.role()),
            Some("assistant")
        );
    }

    #[test]
    fn split_query_and_context_returns_none_without_user_message() {
        let messages = vec![
            json!({"role": "system", "content": "Be direct."}),
            json!({"role": "assistant", "content": "two"}),
        ];

        assert!(split_query_and_context(&messages).is_none());
    }

    #[test]
    fn extract_sampling_uses_whitelist_and_ignores_nulls() {
        let request = json!({
            "temperature": 0.5,
            "top_p": 0.9,
            "frequency_penalty": null,
            "presence_penalty": -1,
            "logit_bias": { "1": -100 },
            "stop": ["x"]
        });

        let sampling = extract_sampling(&request);
        assert_eq!(sampling.get("temperature"), Some(&json!(0.5)));
        assert_eq!(sampling.get("top_p"), Some(&json!(0.9)));
        assert_eq!(sampling.get("presence_penalty"), Some(&json!(-1)));
        assert!(!sampling.contains_key("frequency_penalty"));
        assert!(!sampling.contains_key("logit_bias"));
        assert!(!sampling.contains_key("stop"));
    }

    #[test]
    fn truncate_tool_result_preserves_short_content_and_utf8_boundaries() {
        assert_eq!(
            truncate_tool_result("short".to_owned(), 32),
            "short".to_owned()
        );

        let content = "é".repeat(20);
        let truncated = truncate_tool_result(content.clone(), 7);
        assert!(truncated.is_char_boundary(truncated.len()));
        assert!(truncated.contains("[truncated "));
        assert!(truncated.starts_with("é"));
    }
}
