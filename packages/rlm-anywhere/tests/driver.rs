use hegel::TestCase;
use hegel::generators;
use serde_json::{Value, json};

use rlm_anywhere::rlm::ContextMessage;
use rlm_anywhere::rlm::driver::{
    SAMPLING_WHITELIST, extract_sampling, split_query_and_context, truncate_tool_result,
};

/// Generate a chat message with a role that is usually a known role but
/// sometimes an arbitrary string, plus arbitrary textual content.
#[hegel::composite]
fn chat_message(tc: TestCase) -> Value {
    let role: String = tc.draw(hegel::one_of!(
        generators::sampled_from(vec![
            "user".to_owned(),
            "assistant".to_owned(),
            "system".to_owned(),
            "tool".to_owned(),
        ]),
        generators::text().max_size(8),
    ));
    let content = tc.draw(generators::text());
    json!({ "role": role, "content": content })
}

// B. `truncate_tool_result` must never panic on the char-boundary search,
// must return short content unchanged, and must keep a real byte-prefix.
#[hegel::test]
fn truncate_tool_result_is_safe_and_prefix_preserving(tc: TestCase) {
    let content = tc.draw(generators::text());
    // Span below, at, and above the content length.
    let preview_bytes = tc.draw(generators::integers::<usize>().max_value(content.len() + 16));

    let original_len = content.len();
    let result = truncate_tool_result(content.clone(), preview_bytes);

    if original_len <= preview_bytes {
        assert_eq!(
            result, content,
            "content at or below the budget is returned unchanged"
        );
    } else {
        assert!(
            result.contains("\n[truncated "),
            "truncated output carries the marker: {result:?}"
        );
        let (prefix, _) = result
            .split_once("\n[truncated ")
            .unwrap_or((result.as_str(), ""));
        assert!(
            content.as_bytes().starts_with(prefix.as_bytes()),
            "kept prefix must be a byte-prefix of the input"
        );
        assert!(
            prefix.len() <= preview_bytes,
            "kept prefix must respect the byte budget"
        );
    }
}

// C. `split_query_and_context` partitions on the last user message.
#[hegel::test]
fn split_query_and_context_partitions_on_last_user(tc: TestCase) {
    let messages: Vec<Value> = tc.draw(generators::vecs(chat_message()));

    let last_user = messages
        .iter()
        .rposition(|message| message.get("role").and_then(Value::as_str) == Some("user"));

    match split_query_and_context(&messages) {
        None => assert!(
            last_user.is_none(),
            "None is returned only when no user message exists"
        ),
        Some(split) => {
            let Some(idx) = last_user else {
                panic!("a Some split implies at least one user message");
            };
            assert_eq!(
                split.query_message, messages[idx],
                "query message is the last user message"
            );

            // Context is the original list minus exactly the query index,
            // with order preserved.
            let expected: Vec<&Value> = messages
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != idx)
                .map(|(_, message)| message)
                .collect();
            assert_eq!(split.context.len(), expected.len());
            for (position, original) in expected.iter().enumerate() {
                assert_eq!(
                    split.context.get(position).map(ContextMessage::raw),
                    Some(*original),
                    "context preserves original order minus the query"
                );
            }
        }
    }
}

// D. `extract_sampling` forwards only non-null whitelisted keys and never
// leaks forbidden controls such as `logit_bias`.
#[hegel::test]
fn extract_sampling_forwards_only_non_null_whitelist(tc: TestCase) {
    let mut object = serde_json::Map::new();

    for key in SAMPLING_WHITELIST {
        if tc.draw(generators::booleans()) {
            let value = if tc.draw(generators::booleans()) {
                Value::Null
            } else {
                json!(
                    tc.draw(
                        generators::floats::<f64>()
                            .allow_nan(false)
                            .allow_infinity(false)
                    )
                )
            };
            object.insert((*key).to_owned(), value);
        }
    }
    // Forbidden controls that must never reach an internal subcall.
    for key in ["logit_bias", "stop", "n", "stream", "tools"] {
        if tc.draw(generators::booleans()) {
            object.insert(
                key.to_owned(),
                json!(tc.draw(generators::integers::<i32>())),
            );
        }
    }

    let request = Value::Object(object.clone());
    let sampling = extract_sampling(&request);

    for key in sampling.keys() {
        assert!(
            SAMPLING_WHITELIST.contains(&key.as_str()),
            "leaked non-whitelisted key {key}"
        );
    }
    for value in sampling.values() {
        assert!(!value.is_null(), "null sampling value was forwarded");
    }
    // Completeness: every whitelisted non-null input is forwarded verbatim.
    for key in SAMPLING_WHITELIST {
        match object.get(key) {
            Some(value) if !value.is_null() => assert_eq!(sampling.get(key), Some(value)),
            _ => assert!(!sampling.contains_key(key)),
        }
    }
}

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
