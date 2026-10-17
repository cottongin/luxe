use super::*;

fn roundtrip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(
    name: &str,
    value: &T,
) {
    let json = serde_json::to_string(value).expect("serialize");
    let decoded: T = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(&decoded, value, "{} round-trip mismatch", name);
}

/// The persisted shape of a message, pinned against real `.luxe/sessions/*.jsonl` output.
///
/// These blocks live inside `EntryKind::Message`, so their representation is a compatibility
/// promise: rename a field or retag a variant and every session already on disk stops loading.
/// A round-trip cannot catch that — it changes both sides at once — which is why the five
/// round-trips this replaced proved less than they appeared to.
#[test]
fn the_persisted_message_shape_is_frozen() {
    let stored = r#"{"role":"user","content":[{"type":"text","content":"hello"}]}"#;
    let message: Message = serde_json::from_str(stored).expect("an older user message loads");
    assert_eq!(message.role, Role::User);
    assert_eq!(message.text(), "hello");
    assert_eq!(serde_json::to_string(&message).unwrap(), stored, "and writes back the same");

    let stored =
        r#"{"type":"tool_call","content":{"id":"c1","name":"read","arguments":{"path":"a.rs"}}}"#;
    let block: ContentBlock = serde_json::from_str(stored).expect("an older tool call loads");
    let ContentBlock::ToolCall { id, name, arguments } = &block else {
        panic!("expected a tool call, got {block:?}")
    };
    assert_eq!((id.as_str(), name.as_str()), ("c1", "read"));
    assert_eq!(arguments["path"], "a.rs");
    assert_eq!(serde_json::to_string(&block).unwrap(), stored);

    let stored = r#"{"type":"thinking","content":{"text":"considering"}}"#;
    let block: ContentBlock = serde_json::from_str(stored).expect("older reasoning loads");
    assert!(matches!(&block, ContentBlock::Thinking { text, .. } if text == "considering"));

    // A tool result nests its own blocks, so its shape is two levels of promise.
    let stored = r#"{"type":"tool_result","content":{"call_id":"c1","content":[{"type":"text","content":"ok"}],"is_error":false}}"#;
    let block: ContentBlock = serde_json::from_str(stored).expect("an older tool result loads");
    let ContentBlock::ToolResult { call_id, content, is_error } = &block else {
        panic!("expected a tool result, got {block:?}")
    };
    assert_eq!(call_id, "c1");
    assert!(!is_error);
    assert_eq!(content.len(), 1);
    assert_eq!(serde_json::to_string(&block).unwrap(), stored);
}

#[test]
fn thinking_block_roundtrip() {
    let block = ContentBlock::Thinking {
        text: "let me think".into(),
        signature: Some("sig123".into()),
        id: None,
        encrypted: None,
        opaque: None,
    };
    roundtrip("thinking block", &block);
}

#[test]
fn reasoning_item_block_roundtrip() {
    // OpenAI Responses reasoning: summary text + item id + encrypted content.
    let block = ContentBlock::Thinking {
        text: "summary".into(),
        signature: None,
        id: Some("rs_123".into()),
        encrypted: Some("ENC==".into()),
        opaque: None,
    };
    roundtrip("reasoning item block", &block);
}

/// Copilot's opaque reasoning state, pinned as it is written to a session, and a block from
/// before the field existed still loads without one.
#[test]
fn copilot_reasoning_state_is_pinned() {
    let stored = r#"{"type":"thinking","content":{"text":"","opaque":"CpcBAb4+9vs"}}"#;
    let block: ContentBlock = serde_json::from_str(stored).unwrap();
    assert!(
        matches!(&block, ContentBlock::Thinking { opaque: Some(opaque), .. } if opaque == "CpcBAb4+9vs")
    );
    assert_eq!(serde_json::to_string(&block).unwrap(), stored);
    let older = r#"{"type":"thinking","content":{"text":"weighing it","signature":"sig"}}"#;
    let block: ContentBlock = serde_json::from_str(older).unwrap();
    assert!(matches!(block, ContentBlock::Thinking { opaque: None, .. }));
}

#[test]
fn redacted_thinking_block_roundtrip() {
    let block = ContentBlock::RedactedThinking { data: "ENCRYPTED==".into() };
    roundtrip("redacted thinking block", &block);
}

#[test]
fn tool_call_roundtrip() {
    let block = ContentBlock::ToolCall {
        id: "call_abc".into(),
        name: "read_file".into(),
        arguments: serde_json::json!({"path": "/tmp/x.txt"}),
    };
    roundtrip("tool call", &block);
}

#[test]
fn tool_result_roundtrip() {
    let block = ContentBlock::ToolResult {
        call_id: "call_abc".into(),
        content: vec![ToolOutput::Text("file contents".into())],
        is_error: false,
    };
    roundtrip("tool result", &block);
}

#[test]
fn tool_result_with_image_roundtrip() {
    let block = ContentBlock::ToolResult {
        call_id: "call_img".into(),
        content: vec![ToolOutput::Image {
            data: "base64data==".into(),
            mime: "image/png".into(),
            name: None,
        }],
        is_error: false,
    };
    roundtrip("tool result (image)", &block);
}

#[test]
fn tool_result_multiple_content_roundtrip() {
    let block = ContentBlock::ToolResult {
        call_id: "call_multi".into(),
        content: vec![
            ToolOutput::Text("stdout".into()),
            ToolOutput::Image { data: "img==".into(), mime: "image/jpeg".into(), name: None },
        ],
        is_error: true,
    };
    roundtrip("tool result (multi)", &block);
}

#[test]
fn message_roundtrip() {
    let msg = Message {
        role: Role::Assistant,
        content: vec![
            ContentBlock::Text("I'll help.".into()),
            ContentBlock::ToolCall {
                id: "c1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"cmd": "ls"}),
            },
        ],
    };
    roundtrip("message", &msg);
}

/// A tool result mixes prose and pixels, and each adapter asks for one half at a time: the text
/// joined in order, the images by `(mime, data)` in order, neither seeing the other's parts.
#[test]
fn a_tool_result_splits_into_its_text_and_its_images() {
    let image = |mime: &str, data: &str| ToolOutput::Image {
        data: data.to_string(),
        mime: mime.to_string(),
        name: None,
    };
    let outputs = vec![
        ToolOutput::Text("one".to_string()),
        image("image/png", "AAA"),
        ToolOutput::Text("two".to_string()),
        image("image/jpeg", "BBB"),
    ];
    assert_eq!(ToolOutput::joined_text(&outputs), "one\ntwo");
    let images: Vec<_> = ToolOutput::images(&outputs).collect();
    assert_eq!(images, vec![("image/png", "AAA"), ("image/jpeg", "BBB")]);

    let only_pixels = vec![image("image/png", "AAA")];
    assert_eq!(ToolOutput::joined_text(&only_pixels), "");
    assert_eq!(ToolOutput::images(&[ToolOutput::Text("x".to_string())]).count(), 0);
}
