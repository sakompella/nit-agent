use serde_json::json;

use rlm_anywhere::rlm::driver::{extract_sampling, split_query_and_context, truncate_tool_result};

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
