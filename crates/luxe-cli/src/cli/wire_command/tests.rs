use super::cache::{cache_stability, transitions};
use super::secrets::looks_like_credential;
use super::sections::system_sections;
use super::*;

/// [`cache_stability`] over the transitions of `calls`.
fn stability(out: &mut String, calls: &[Call]) {
    cache_stability(out, calls, &transitions(calls));
}

fn call(id: u64, system: &str, tools: serde_json::Value) -> Call {
    let body = serde_json::json!({
        "model": "m",
        "messages": [{"role": "system", "content": system}, {"role": "user", "content": "hi"}],
        "tools": tools,
    });
    Call::new(id as usize + 1, "test".into(), String::new(), body, None)
}

/// A call with `body` as it was sent.
fn with_body(body: serde_json::Value) -> Call {
    Call::new(1, "test".into(), String::new(), body, None)
}

#[test]
fn system_prompt_is_found_in_every_wire_shape() {
    let chat = call(0, "SYS", serde_json::json!([]));
    assert_eq!(chat.system(), Some("SYS"));

    let responses = with_body(serde_json::json!({"instructions": "SYS", "input": []}));
    assert_eq!(responses.system(), Some("SYS"));

    let anthropic = with_body(serde_json::json!({
        "system": [{"type": "text", "text": "SYS", "cache_control": {"type": "ephemeral"}}],
        "messages": [],
    }));
    assert_eq!(anthropic.system(), Some("SYS"));
}

#[test]
fn tool_entries_read_both_the_nested_and_flat_shapes() {
    let nested = call(
        0,
        "",
        serde_json::json!([{"type": "function", "function": {"name": "read", "description": "d"}}]),
    );
    assert_eq!(nested.tool_entries()[0].0, "read");
    let flat =
        call(0, "", serde_json::json!([{"type": "function", "name": "read", "description": "d"}]));
    assert_eq!(flat.tool_entries()[0].0, "read");
}

/// A deferred definition is on the wire but outside the context: it isn't counted as a tool
/// the model carries, and adding one doesn't move the cached prefix.
#[test]
fn deferred_tools_are_counted_apart_and_never_break_the_prefix() {
    let listed = serde_json::json!({"name": "read", "description": "Read a file."});
    let deferred = |name: &str| serde_json::json!({"name": name, "description": "Rarely needed.", "defer_loading": true});
    let pair = || {
        (
            call(0, "SYS", serde_json::json!([listed.clone(), deferred("debug")])),
            call(
                1,
                "SYS",
                serde_json::json!([listed.clone(), deferred("debug"), deferred("ast_grep")]),
            ),
        )
    };
    let (before, after) = pair();

    let names = |entries: Vec<(String, String, String)>| -> Vec<String> {
        entries.into_iter().map(|(name, _, _)| name).collect()
    };
    assert_eq!(names(after.tool_entries()), vec!["read"]);
    assert_eq!(names(after.deferred_entries()), vec!["debug", "ast_grep"]);
    assert!(after.context_chars() < after.body_chars());

    let mut out = String::new();
    stability(&mut out, &[before, after]);
    assert!(!out.contains("tools CHANGED"), "{out}");

    let mut out = String::new();
    let (before, after) = pair();
    composition(&mut out, &[before, after], 4.0, false);
    assert!(out.contains("(1 tools)"), "{out}");
    assert!(out.contains("deferred tools") && out.contains("(2 tools, outside the context"));
}

/// One turn's messages, as an Anthropic body — with the rolling breakpoint on the last one,
/// the way the adapter really sends it.
fn conversation(messages: serde_json::Value) -> Call {
    let mut body = serde_json::json!({"model": "m", "system": "SYS", "tools": []});
    let mut messages = messages;
    if let Some(last) = messages.as_array_mut().and_then(|list| list.last_mut()) {
        last["cache_control"] = serde_json::json!({"type": "ephemeral"});
    }
    body["messages"] = messages;
    with_body(body)
}

/// The half of the prefix nobody was watching, and the reason a real session spent 71% of its
/// cost on cache writes: the system prompt and the tools were rock solid while the harness
/// quietly re-truncated an old tool result on every single call.
#[test]
fn cache_stability_catches_a_rewritten_conversation() {
    let before = conversation(serde_json::json!([
        {"role": "user", "content": "a full tool result, all 4000 characters of it"},
        {"role": "assistant", "content": "ok"},
    ]));
    let after = conversation(serde_json::json!([
        {"role": "user", "content": "[1000 tokens of earlier tool output elided]"},
        {"role": "assistant", "content": "ok"},
        {"role": "user", "content": "next"},
    ]));

    let mut out = String::new();
    stability(&mut out, &[before, after]);
    assert!(out.contains("conversation REWRITTEN at message 0/2"), "{out}");
    assert!(out.contains("invalidate the cached prefix"), "{out}");
}

/// Appending is the whole point, and the breakpoint marker moving with it is not a change.
#[test]
fn appending_a_turn_and_moving_the_breakpoint_is_stable() {
    let before = conversation(serde_json::json!([{"role": "user", "content": "one"}]));
    let after = conversation(serde_json::json!([
        {"role": "user", "content": "one"},
        {"role": "assistant", "content": "two"},
    ]));

    let mut out = String::new();
    stability(&mut out, &[before, after]);
    assert!(out.contains("stable"), "{out}");
    assert!(!out.contains('▲'), "{out}");
}

#[test]
fn cache_stability_names_the_first_changed_byte() {
    let calls = vec![
        call(0, "## Env\ngit: main (clean)", serde_json::json!([])),
        call(1, "## Env\ngit: main (dirty)", serde_json::json!([])),
    ];
    let mut out = String::new();
    stability(&mut out, &calls);
    assert!(out.contains("system CHANGED"), "{out}");
    assert!(out.contains("invalidate the cached prefix"), "{out}");
    assert!(out.contains("clean") && out.contains("dirty"), "shows the divergence: {out}");
}

#[test]
fn a_stable_prefix_reports_no_warning() {
    let calls =
        vec![call(0, "same", serde_json::json!([])), call(1, "same", serde_json::json!([]))];
    let mut out = String::new();
    stability(&mut out, &calls);
    assert!(out.contains("stable"));
    assert!(!out.contains('▲'), "{out}");
}

#[test]
fn duplication_catches_a_tool_re_listed_in_the_prompt() {
    let calls = vec![call(
        0,
        "## Tools\n\n- **read**: Use 'read' for read",
        serde_json::json!([{"type": "function", "function": {"name": "read", "description": "Read a file"}}]),
    )];
    let mut out = String::new();
    duplication(&mut out, &calls);
    assert!(out.contains("re-listed in the system prompt"), "{out}");
}

#[test]
fn a_bold_label_outside_the_tools_section_is_not_a_duplicate() {
    // `**git**: main (dirty)` is the environment line, and any AGENTS.md may use bold labels.
    let calls = vec![call(
        0,
        "## Environment\n\n**git**: main (dirty)\n\n## Tools\n\nReach for the real tool.",
        serde_json::json!([{"type": "function", "function": {"name": "git", "description": "Git ops"}}]),
    )];
    let mut out = String::new();
    duplication(&mut out, &calls);
    assert!(out.contains("none found"), "{out}");
}

#[test]
fn a_credential_prefix_in_prose_is_not_a_leak_but_a_real_key_is() {
    // Reading luxe's own redact.rs puts every prefix in a tool result; that is not a leak.
    assert!(looks_like_credential("patterns like sk- and ghp_ and AKIA").is_none());
    assert!(looks_like_credential("Authorization: Bearer short").is_none());
    let leak = looks_like_credential("key=sk-abcdefghijklmnopqrstuvwxyz0123")
        .expect("a real-looking key is caught");
    assert!(leak.starts_with("sk-abcd"), "{leak}");
    assert!(!leak.contains("vwxyz"), "the value itself is not echoed: {leak}");
}

#[test]
fn the_verdict_names_prefix_churn_and_overhead() {
    let calls = vec![call(0, "a", serde_json::json!([])), call(1, "b", serde_json::json!([]))];
    let verdict = warnings(&calls, &transitions(&calls)).join("\n");
    assert!(verdict.contains("cacheable prefix changes on 1"), "{verdict}");

    let steady = vec![call(0, "a", serde_json::json!([])), call(1, "a", serde_json::json!([]))];
    assert!(warnings(&steady, &transitions(&steady)).join("\n").contains("no structural problems"));
}

#[test]
fn system_sections_measure_each_heading() {
    let sections = system_sections("## A\nline\n## B\nlonger line here\n");
    let names: Vec<&str> = sections.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, vec!["A", "B"]);
    assert!(sections[1].1 > sections[0].1);
}

#[test]
fn chars_per_token_is_measured_from_reported_usage() {
    let mut call = call(0, "", serde_json::json!([]));
    let chars = call.body_chars();
    call.response = Some(serde_json::json!({"usage": {"prompt_tokens": chars / 4}}));
    assert!((chars_per_token(&[call]) - 4.0).abs() < 0.5);
}
