use super::*;

#[test]
fn test_mcp_server_config_stdio() {
    let config = McpServerConfig {
        id: "test-server".to_string(),
        name: "Test Server".to_string(),
        transport: McpTransport::Stdio {
            command: "node".to_string(),
            args: vec!["server.js".to_string()],
            cwd: None,
            env: Default::default(),
        },
        enabled: true,
        ..Default::default()
    };
    assert_eq!(config.id, "test-server");
    assert!(config.enabled);
}

#[test]
fn test_mcp_server_config_http() {
    let config = McpServerConfig {
        id: "remote-server".to_string(),
        name: "Remote Server".to_string(),
        transport: McpTransport::HttpSse { url: "http://localhost:3000/mcp".to_string() },
        enabled: true,
        ..Default::default()
    };
    assert_eq!(config.id, "remote-server");
}

#[test]
fn a_plain_name_is_short_and_has_nothing_that_could_carry_a_sentence() {
    for name in ["asana", "my-server.v2", "get_tasks", "A1", &"x".repeat(64)] {
        assert!(is_plain_name(name), "{name:?}");
    }
    let long = "x".repeat(65);
    for name in ["", "two words", "asana/get", "é", "tab\there", "line\nbreak", "@[x]", &long] {
        assert!(!is_plain_name(name), "{name:?}");
    }
}

#[test]
fn a_tool_call_result_carries_its_joined_text() {
    let ok = parse_tool_call_result(
        &serde_json::json!({ "content": [{"type": "text", "text": "hello"}] }),
    );
    assert!(ok.success, "a missing isError means success");
    assert_eq!(ok.output.as_deref(), Some("hello"));

    let failed = parse_tool_call_result(
        &serde_json::json!({ "content": [{"type": "text", "text": "boom"}], "isError": true }),
    );
    assert!(!failed.success);
    assert_eq!(failed.error.as_deref(), Some("boom"));
}

/// A block this surface cannot print is named at the end rather than dropped without a word:
/// the model should know there was an image, even if it cannot see it.
#[test]
fn a_block_that_cannot_be_shown_is_named_not_dropped() {
    let result = parse_tool_call_result(&serde_json::json!({ "content": [
        {"type": "text", "text": "first"},
        {"type": "image", "data": "…"},
        {"type": "resource", "resource": {"uri": "file:///a", "text": "…"}},
        {"type": "image", "data": "…"},
        {"type": "text", "text": "second"},
    ]}));
    assert_eq!(
        result.output.as_deref(),
        Some("first\nsecond\n(2 images and 1 embedded resource not shown)")
    );
    let only = parse_tool_call_result(&serde_json::json!({ "content": [
        {"type": "audio", "data": "…"}, {"type": "resource_link", "uri": "x"}, {"type": "mystery"},
    ]}));
    assert_eq!(
        only.output.as_deref(),
        Some("(1 audio clip, 1 resource link and 1 block of another kind not shown)")
    );
}

/// A JSON answer loses its indentation and nothing else; prose is untouched; an error is
/// shaped the same way as a success.
#[test]
fn a_json_answer_is_compacted_and_prose_is_not() {
    let pretty =
        "{\n  \"data\": [\n    { \"gid\": \"1209\", \"name\": \"Draft  the plan\" }\n  ]\n}";
    let result = parse_tool_call_result(&serde_json::json!({ "content": [
        {"type": "text", "text": pretty},
        {"type": "text", "text": "Found 1 task:\n  - Draft  the plan"},
    ]}));
    assert_eq!(
        result.output.as_deref(),
        Some("{\"data\":[{\"gid\":\"1209\",\"name\":\"Draft  the plan\"}]}\nFound 1 task:\n  - Draft  the plan")
    );
    let failed = parse_tool_call_result(&serde_json::json!({
        "content": [{"type": "text", "text": "{ \"error\": \"not found\" }"}], "isError": true
    }));
    assert_eq!(failed.error.as_deref(), Some("{\"error\":\"not found\"}"));
}

/// No answer puts more than the shared ceiling into the conversation, and one that would says
/// how much it held and how to see the rest.
#[test]
fn a_result_past_the_ceiling_is_capped_and_says_so() {
    let huge = "x".repeat(crate::output_limit::RESULT_MAX_CHARS + 5_000);
    let result =
        parse_tool_call_result(&serde_json::json!({ "content": [{"type": "text", "text": huge}] }));
    let output = result.output.expect("a success");
    assert!(output.starts_with(&"x".repeat(crate::output_limit::RESULT_MAX_CHARS)));
    assert!(output.contains("5000 more characters not shown"), "{}", &output[output.len() - 200..]);
    assert!(output.contains("Narrow the call"));
    assert!(output.chars().count() < crate::output_limit::RESULT_MAX_CHARS + 300);
}

#[test]
fn a_tool_list_is_attributed_to_its_server_and_skips_nameless_entries() {
    let tools = parse_tool_list(
        &serde_json::json!({ "tools": [
            {"name": "search", "description": "Find things", "inputSchema": {"type": "object"}},
            {"description": "no name, no tool"},
            {"name": "bare"},
        ]}),
        "docs",
    )
    .expect("a tools array parses");

    assert_eq!(tools.len(), 2, "the nameless entry is skipped: {tools:?}");
    assert_eq!(tools[0].server_id, "docs");
    assert_eq!(tools[0].name, "search");
    assert_eq!(tools[1].description, "", "a missing description reads empty, not absent");
    assert!(tools[1].input_schema.is_null());
}

#[test]
fn a_reply_with_no_tools_array_is_a_protocol_error_not_an_empty_list() {
    assert!(parse_tool_list(&serde_json::json!({}), "docs").is_none());
    assert!(parse_tool_list(&serde_json::json!({ "tools": 7 }), "docs").is_none());
}

#[test]
fn test_mcp_tool_serialization() {
    let tool = McpTool {
        server_id: "test".to_string(),
        name: "test_tool".to_string(),
        description: "A test tool".to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "input": {"type": "string"}
            }
        }),
    };
    let json = serde_json::to_string(&tool).unwrap();
    assert!(json.contains("test_tool"));
    assert!(json.contains("A test tool"));
}

#[test]
fn test_mcp_server_state() {
    let state =
        McpServerState::Connected { name: "Test".to_string(), version: Some("1.0.0".to_string()) };
    match state {
        McpServerState::Connected { name, version } => {
            assert_eq!(name, "Test");
            assert_eq!(version, Some("1.0.0".to_string()));
        }
        _ => panic!("Expected Connected state"),
    }
}

#[test]
fn expand_env_expands_dollar_brace_and_env_colon() {
    std::env::set_var("LUXE_EXPAND_TEST_A", "hello");
    assert_eq!(expand_env("prefix_${LUXE_EXPAND_TEST_A}_suffix"), "prefix_hello_suffix");
    assert_eq!(expand_env("$env:LUXE_EXPAND_TEST_A!"), "hello!");
    std::env::remove_var("LUXE_EXPAND_TEST_A");
}

#[test]
fn expand_env_unknown_var_is_empty() {
    assert_eq!(expand_env("${LUXE_EXPAND_DEFINITELY_UNSET_XYZ}x"), "x");
}

/// Both markers in one value expand, in order, and what a variable held is not read again.
///
/// The break this catches: the expander looked for `${` first and fell back to `$env:` only when
/// there was none, so `$env:A ${B}` left `$env:A` as written.
#[test]
fn expand_env_takes_mixed_markers_in_order() {
    std::env::set_var("LUXE_EXPAND_TEST_M1", "one");
    std::env::set_var("LUXE_EXPAND_TEST_M2", "${LUXE_EXPAND_TEST_M1}");
    assert_eq!(expand_env("$env:LUXE_EXPAND_TEST_M1 ${LUXE_EXPAND_TEST_M1}"), "one one");
    assert_eq!(expand_env("${LUXE_EXPAND_TEST_M2}"), "${LUXE_EXPAND_TEST_M1}", "not re-read");
    std::env::remove_var("LUXE_EXPAND_TEST_M1");
    std::env::remove_var("LUXE_EXPAND_TEST_M2");
}

/// A bare `$NAME` expands too where asked, and one that names nothing is left as written: a key
/// may hold a `$` of its own.
#[test]
fn a_bare_name_expands_only_when_asked_and_set() {
    std::env::set_var("LUXE_EXPAND_TEST_B", "bare");
    assert_eq!(expand_env_with_bare("$LUXE_EXPAND_TEST_B/${LUXE_EXPAND_TEST_B}"), "bare/bare");
    assert_eq!(expand_env("$LUXE_EXPAND_TEST_B"), "$LUXE_EXPAND_TEST_B");
    assert_eq!(expand_env_with_bare("pa$sword_UNSET_XYZ"), "pa$sword_UNSET_XYZ");
    std::env::remove_var("LUXE_EXPAND_TEST_B");
}
