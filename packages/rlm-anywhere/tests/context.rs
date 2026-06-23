use hegel::TestCase;
use hegel::generators;
use serde_json::{Value, json};

use rlm_anywhere::rlm::{ContextMessage, ContextStore};

/// Generate a chat message with a role drawn from common roles and arbitrary text content.
#[hegel::composite]
fn context_message(tc: TestCase) -> Value {
    let role = tc.draw(generators::sampled_from(vec![
        "user".to_owned(),
        "assistant".to_owned(),
        "system".to_owned(),
    ]));
    let content = tc.draw(generators::text().max_size(32));
    json!({ "role": role, "content": content })
}

// E. `ContextStore::slice` must never panic and must clamp its bounds correctly.
#[hegel::test]
fn slice_never_panics_and_result_matches_clamped_range(tc: TestCase) {
    let n = tc.draw(generators::integers::<usize>().max_value(8));
    let messages: Vec<Value> = (0..n)
        .map(|i| json!({"role": "user", "content": format!("msg {i}")}))
        .collect();
    let store = ContextStore::from_chat_messages(&messages);

    // Draw start/end from the full usize range to exercise boundary and overflow behavior.
    let start = tc.draw(generators::integers::<usize>());
    let end = tc.draw(generators::integers::<usize>());

    let result = store.slice(start, end);

    assert!(
        result.len() <= store.len(),
        "result must not exceed store length"
    );

    let clamped_start = start.min(n);
    let clamped_end = end.min(n);
    let expected_len = if clamped_start > clamped_end {
        0
    } else {
        clamped_end - clamped_start
    };
    assert_eq!(
        result.len(),
        expected_len,
        "length must match independently-clamped range"
    );
}

// F. `ContextStore::grep_indexed` must be sound, complete, and case-insensitive.
#[hegel::test]
fn grep_indexed_is_sound_complete_and_case_insensitive(tc: TestCase) {
    let messages: Vec<Value> = tc.draw(generators::vecs(context_message()));
    let needle: String = tc.draw(generators::text().max_size(8));
    let store = ContextStore::from_chat_messages(&messages);

    // Empty needle is defined to return empty.
    assert!(
        store.grep_indexed("").is_empty(),
        "empty needle must return empty"
    );

    if needle.is_empty() {
        return;
    }

    let needle_lower = needle.to_lowercase();
    let results = store.grep_indexed(&needle);

    // Independent scan: the reference implementation for soundness and completeness.
    let expected_indices: Vec<usize> = (0..store.len())
        .filter(|&i| {
            let msg = store.get(i).expect("index 0..len must exist");
            msg.text().to_lowercase().contains(&needle_lower)
                || msg
                    .role()
                    .is_some_and(|r| r.to_lowercase().contains(&needle_lower))
        })
        .collect();
    let found_indices: Vec<usize> = results.iter().map(|(i, _)| *i).collect();

    assert_eq!(
        found_indices, expected_indices,
        "grep_indexed must be sound and complete"
    );

    // Indices strictly increasing (redundant with sorted, but explicit).
    assert!(
        found_indices.windows(2).all(|w| w[0] < w[1]),
        "indices must be strictly increasing"
    );

    // Case-insensitivity: grep lowercases both sides, so searching for the
    // lowercased needle matches exactly the same messages. We assert against
    // `to_lowercase()`, NOT `to_uppercase()`: Unicode uppercase is lossy
    // (e.g. "ß" -> "SS", ligatures, final sigma), so the uppercase round-trip
    // is not equivalent to the original needle and would fail for valid input.
    let lower_indices: Vec<usize> = store
        .grep_indexed(&needle.to_lowercase())
        .into_iter()
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        found_indices, lower_indices,
        "case: lowercased needle must match same indices"
    );
}

// G. `from_chat_messages` must never panic on any `content` JSON shape.
// Tests all branches of the private `extract_content_text` helper via the public API.
#[hegel::test]
fn from_chat_messages_never_panics_on_arbitrary_content(tc: TestCase) {
    let text = tc.draw(generators::text());
    let shapes: [Value; 6] = [
        json!(text),                               // String
        json!([{ "type": "text", "text": text }]), // array of typed parts
        json!([text]),                             // array of raw strings
        Value::Null,                               // null
        json!(42),                                 // non-string scalar
        json!({}),                                 // no content key at all
    ];
    for shape in &shapes {
        let message = json!({ "role": "user", "content": shape });
        let store = ContextStore::from_chat_messages(&[message]);
        assert_eq!(
            store.len(),
            1,
            "must produce exactly one message for shape: {shape:?}"
        );
    }
}

#[test]
fn builds_context_from_chat_messages() {
    let messages = vec![
        json!({"role": "system", "content": "Be direct."}),
        json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "Find the answer"},
                {"type": "image_url", "image_url": {"url": "https://example.test/image.png"}},
                "extra text"
            ],
            "unknown": true
        }),
    ];

    let store = ContextStore::from_chat_messages(&messages);

    assert_eq!(store.len(), 2);
    assert_eq!(store.get(0).and_then(ContextMessage::role), Some("system"));
    assert_eq!(
        store.get(1).map(ContextMessage::text),
        Some("Find the answer\nextra text")
    );
    assert_eq!(store.get(1).map(ContextMessage::raw), messages.get(1));
}

#[test]
fn slice_clamps_to_available_messages() {
    let messages = vec![
        json!({"role": "user", "content": "one"}),
        json!({"role": "assistant", "content": "two"}),
    ];
    let store = ContextStore::from_chat_messages(&messages);

    assert_eq!(store.slice(0, 99).len(), 2);
    assert!(store.slice(2, 1).is_empty());
}

#[test]
fn grep_matches_role_and_text_case_insensitively() {
    let messages = vec![
        json!({"role": "user", "content": "Need budget details"}),
        json!({"role": "assistant", "content": "Done"}),
    ];
    let store = ContextStore::from_chat_messages(&messages);

    let content_matches = store.grep("BUDGET");
    let role_matches = store.grep("assistant");

    assert_eq!(content_matches.len(), 1);
    assert_eq!(role_matches.len(), 1);
    assert!(store.grep("").is_empty());
}

#[test]
fn grep_indexed_returns_correct_positions() {
    let messages = vec![
        json!({"role": "user", "content": "Need budget details"}),
        json!({"role": "assistant", "content": "Done"}),
        json!({"role": "user", "content": "More budget info"}),
    ];
    let store = ContextStore::from_chat_messages(&messages);

    let matches = store.grep_indexed("budget");
    assert_eq!(matches.len(), 2);
    assert_eq!(matches[0].0, 0);
    assert_eq!(matches[1].0, 2);
    assert!(store.grep_indexed("").is_empty());
}

#[test]
fn describe_returns_counts_and_roles() {
    let messages = vec![
        json!({"role": "user", "content": "abc"}),
        json!({"role": "assistant", "content": null}),
    ];
    let store = ContextStore::from_chat_messages(&messages);
    let summary = store.describe();

    assert_eq!(summary.messages, 2);
    assert_eq!(summary.text_bytes, 3);
    assert_eq!(summary.roles, ["user", "assistant"]);
}
