use serde_json::json;

use rlm_anywhere::rlm::tools::{
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
