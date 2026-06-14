use serde::Deserialize;
use serde_json::{Value, json};

pub(crate) const TOOL_CONTEXT_DESCRIBE: &str = "context_describe";
pub(crate) const TOOL_CONTEXT_SLICE: &str = "context_slice";
pub(crate) const TOOL_CONTEXT_GREP: &str = "context_grep";
pub(crate) const TOOL_LLM_QUERY: &str = "llm_query";
pub(crate) const TOOL_RUN_JS: &str = "run_js";
pub(crate) const TOOL_FINAL_ANSWER: &str = "final_answer";

#[must_use]
pub(crate) fn tool_definitions() -> Value {
    json!([
        {
            "type": "function",
            "function": {
                "name": TOOL_CONTEXT_DESCRIBE,
                "description": "Summarize the externalized context: message count, total text bytes, and the role of each message.",
                "parameters": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": TOOL_CONTEXT_SLICE,
                "description": "Return context messages in the half-open index range [start, end). Indices are zero-based and clamped to the available range.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "start": { "type": "integer", "minimum": 0 },
                        "end": { "type": "integer", "minimum": 0 }
                    },
                    "required": ["start", "end"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": TOOL_CONTEXT_GREP,
                "description": "Case-insensitively search context message text and roles for a substring. Returns matching messages with their indices.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "needle": { "type": "string" }
                    },
                    "required": ["needle"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": TOOL_LLM_QUERY,
                "description": "Ask the underlying language model one self-contained question. Include all data the question needs; it cannot see the context store.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "prompt": { "type": "string" }
                    },
                    "required": ["prompt"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": TOOL_RUN_JS,
                "description": "Evaluate a JavaScript function body in a stateless sandbox and return its JSON-serialized result. The code must end with a return statement.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "code": { "type": "string" }
                    },
                    "required": ["code"],
                    "additionalProperties": false
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": TOOL_FINAL_ANSWER,
                "description": "Finish the task. The content is returned to the caller verbatim as the assistant answer.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "content": { "type": "string" }
                    },
                    "required": ["content"],
                    "additionalProperties": false
                }
            }
        }
    ])
}

pub(crate) enum ToolInvocation {
    ContextDescribe,
    ContextSlice { start: u64, end: u64 },
    ContextGrep { needle: String },
    LlmQuery { prompt: String },
    RunJs { code: String },
    FinalAnswer { content: String },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ToolParseError {
    #[error("unknown tool: {name}")]
    UnknownTool { name: String },
    #[error("invalid arguments for {name}: {detail}")]
    InvalidArguments { name: &'static str, detail: String },
}

#[derive(Deserialize)]
struct ContextSliceArguments {
    start: u64,
    end: u64,
}

#[derive(Deserialize)]
struct ContextGrepArguments {
    needle: String,
}

#[derive(Deserialize)]
struct LlmQueryArguments {
    prompt: String,
}

#[derive(Deserialize)]
struct RunJsArguments {
    code: String,
}

#[derive(Deserialize)]
struct FinalAnswerArguments {
    content: String,
}

pub(crate) fn parse_tool_call(
    name: &str,
    arguments: &str,
) -> Result<ToolInvocation, ToolParseError> {
    match name {
        TOOL_CONTEXT_DESCRIBE => Ok(ToolInvocation::ContextDescribe),
        TOOL_CONTEXT_SLICE => parse_arguments::<ContextSliceArguments>(TOOL_CONTEXT_SLICE, arguments)
            .map(|arguments| ToolInvocation::ContextSlice {
                start: arguments.start,
                end: arguments.end,
            }),
        TOOL_CONTEXT_GREP => parse_arguments::<ContextGrepArguments>(TOOL_CONTEXT_GREP, arguments)
            .map(|arguments| ToolInvocation::ContextGrep {
                needle: arguments.needle,
            }),
        TOOL_LLM_QUERY => parse_arguments::<LlmQueryArguments>(TOOL_LLM_QUERY, arguments)
            .map(|arguments| ToolInvocation::LlmQuery {
                prompt: arguments.prompt,
            }),
        TOOL_RUN_JS => parse_arguments::<RunJsArguments>(TOOL_RUN_JS, arguments)
            .map(|arguments| ToolInvocation::RunJs {
                code: arguments.code,
            }),
        TOOL_FINAL_ANSWER => parse_arguments::<FinalAnswerArguments>(TOOL_FINAL_ANSWER, arguments)
            .map(|arguments| ToolInvocation::FinalAnswer {
                content: arguments.content,
            }),
        _ => Err(ToolParseError::UnknownTool {
            name: name.to_owned(),
        }),
    }
}

fn parse_arguments<T>(name: &'static str, arguments: &str) -> Result<T, ToolParseError>
where
    for<'de> T: serde::Deserialize<'de>,
{
    serde_json::from_str(arguments).map_err(|error| ToolParseError::InvalidArguments {
        name,
        detail: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        TOOL_CONTEXT_DESCRIBE, TOOL_CONTEXT_GREP, TOOL_CONTEXT_SLICE, TOOL_FINAL_ANSWER,
        TOOL_LLM_QUERY, TOOL_RUN_JS, ToolInvocation, ToolParseError, parse_tool_call,
    };

    #[test]
    fn parses_each_tool_call() {
        assert!(matches!(
            parse_tool_call(TOOL_CONTEXT_DESCRIBE, "{}").expect("context_describe should parse"),
            ToolInvocation::ContextDescribe
        ));
        assert!(matches!(
            parse_tool_call(TOOL_CONTEXT_SLICE, r#"{"start":0,"end":3}"#)
                .expect("context_slice should parse"),
            ToolInvocation::ContextSlice { start: 0, end: 3 }
        ));
        assert!(matches!(
            parse_tool_call(TOOL_CONTEXT_GREP, r#"{"needle":"alpha"}"#)
                .expect("context_grep should parse"),
            ToolInvocation::ContextGrep { needle } if needle == "alpha"
        ));
        assert!(matches!(
            parse_tool_call(TOOL_LLM_QUERY, r#"{"prompt":"hello"}"#)
                .expect("llm_query should parse"),
            ToolInvocation::LlmQuery { prompt } if prompt == "hello"
        ));
        assert!(matches!(
            parse_tool_call(TOOL_RUN_JS, r#"{"code":"return 1;"}"#)
                .expect("run_js should parse"),
            ToolInvocation::RunJs { code } if code == "return 1;"
        ));
        assert!(matches!(
            parse_tool_call(TOOL_FINAL_ANSWER, r#"{"content":"done"}"#)
                .expect("final_answer should parse"),
            ToolInvocation::FinalAnswer { content } if content == "done"
        ));
    }

    #[test]
    fn rejects_unknown_tool_and_bad_arguments() {
        assert!(matches!(
            parse_tool_call("not-a-tool", "{}"),
            Err(ToolParseError::UnknownTool { name }) if name == "not-a-tool"
        ));
        assert!(matches!(
            parse_tool_call(TOOL_CONTEXT_SLICE, r#"{"start":"zero"}"#),
            Err(ToolParseError::InvalidArguments { name, detail })
                if name == TOOL_CONTEXT_SLICE && !detail.is_empty()
        ));
    }

    #[test]
    fn tolerates_extra_unknown_argument_keys() {
        assert!(matches!(
            parse_tool_call(
                TOOL_CONTEXT_SLICE,
                &json!({"start": 1, "end": 2, "x_unknown": true}).to_string()
            )
            .expect("extra keys should be ignored"),
            ToolInvocation::ContextSlice { start: 1, end: 2 }
        ));
        assert!(matches!(
            parse_tool_call(
                TOOL_FINAL_ANSWER,
                &json!({"content": "ok", "x_unknown": true}).to_string()
            )
            .expect("extra keys should be ignored"),
            ToolInvocation::FinalAnswer { content } if content == "ok"
        ));
    }
}
