use super::*;
use crate::activity::describe::MAX_DESCRIPTION_CHARS;
use crate::activity::{describe_call, CARD_BODY_MAX_LINES};
use serde_json::json;

/// The other half of naming a half-written call: watching its payload arrive.
///
/// A `write`'s content is the member that closes *last*, so the "complete members only" rule
/// that names the call would never show a byte of the file. This reads into the member still
/// streaming — and has to stop before a half-written escape, or the decode fails outright and
/// drops a frame of the very thing the reader is watching.
#[test]
fn a_payload_still_being_written_is_shown_as_far_as_it_is_whole() {
    let write = |content: &str| {
        partial_card_body(
            "write",
            &format!("{{\"path\":\"a.rs\",\"content\":\"{content}"),
            "/project",
        )
    };

    // The count the reader watches climb is the whole point.
    let counts: Vec<Option<usize>> = ["fn main", "fn main() {}\\n", "fn main() {}\\nlet x"]
        .iter()
        .map(|content| write(content).map(|body| body.total_lines))
        .collect();
    assert_eq!(counts, [Some(1), Some(1), Some(2)]);
    assert_eq!(
        write("fn main() {}\\nlet x").map(|body| body.text),
        Some("fn main() {}\nlet x".into())
    );

    // A `\` or a `\u00` the model has not finished writing is held back rather than failing
    // the decode and blanking the card for a frame.
    assert_eq!(write("one\\ntwo\\").map(|body| body.text), Some("one\ntwo".into()));
    assert_eq!(write("one\\ntwo\\u00").map(|body| body.text), Some("one\ntwo".into()));
    assert_eq!(
        write("one\\ntwo\\u0021").map(|body| body.text),
        Some("one\ntwo!".into()),
        "and let through the moment it is whole"
    );

    // A quote, a brace and a comma inside the payload are content, not structure.
    assert_eq!(
        write(r#"say \"hi\", {then} go"#).map(|body| body.text),
        Some("say \"hi\", {then} go".into())
    );

    // The payload closing hands over to the settled body, which has to agree with it.
    let closed = "{\"path\":\"a.rs\",\"content\":\"one\\ntwo\\n\"}";
    assert_eq!(
        partial_card_body("write", closed, "/project"),
        tool_card_body("write", &json!({"path": "a.rs", "content": "one\ntwo\n"}), "/project"),
    );

    // Nothing to show, and nothing invented.
    assert_eq!(
        partial_card_body("write", "{\"path\":\"a.rs\",", "/project"),
        None,
        "no payload yet"
    );
    assert_eq!(partial_card_body("read", "{\"path\":\"a.rs\",\"offset\":1", "/project"), None);
    // A one-line command is already on the header in full, mid-stream as much as after.
    assert_eq!(partial_card_body("bash", "{\"command\":\"cargo te", "/project"), None);
    assert_eq!(
        partial_card_body("bash", "{\"command\":\"cargo test\\ncargo clippy", "/project")
            .map(|body| body.total_lines),
        Some(2),
    );
}

/// A card body is the *payload*, and only where the payload is the point.
/// A described command is named by what it does, and the command itself becomes the body,
/// folded to its first line, however short it is: the reader glances at the purpose and can
/// still see exactly what ran.
#[test]
fn a_described_bash_call_is_named_by_its_description() {
    let call = json!({"description": "Run the forms tests", "command": "cd tests && python runtests.py forms"});
    assert_eq!(describe_call("bash", &call, "/project"), "Run the forms tests");
    let body = tool_card_body("bash", &call, "/project").expect("the command is the body");
    assert_eq!(body.text, "cd tests && python runtests.py forms");
    assert_eq!(body.collapsed_lines, Some(1));

    // A blank description is no description
    let blank = json!({"description": "  ", "command": "cargo test"});
    assert_eq!(describe_call("bash", &blank, "/project"), "cargo test");
    assert!(tool_card_body("bash", &blank, "/project").is_none());

    let long = json!({"description": "word ".repeat(40), "command": "true"});
    let label = describe_call("bash", &long, "/project");
    assert!(label.chars().count() <= MAX_DESCRIPTION_CHARS + 1 && label.ends_with('…'), "{label}");
}

#[test]
fn only_tools_whose_payload_is_the_point_carry_a_card_body() {
    let write =
        tool_card_body("write", &json!({"path": "a.rs", "content": "fn main() {}\n"}), "/project")
            .expect("a write carries its content");
    assert_eq!(write.text, "fn main() {}", "the trailing newline is not a line of content");
    assert_eq!(write.total_lines, 1);
    assert!(!write.is_truncated());

    // A one-line command is already on the header in full; saying it twice is noise.
    assert!(tool_card_body("bash", &json!({"command": "cargo build"}), "/project").is_none());
    let heredoc =
        tool_card_body("bash", &json!({"command": "python3 - <<'PY'\nprint(1)\nPY"}), "/project")
            .expect("a multi-line command is hidden behind the header's ellipsis");
    assert_eq!(heredoc.total_lines, 3);

    // A search with several angles lists them; one query is already on the header in full.
    let searched = tool_card_body(
        "web_search",
        &json!({"queries": ["tokio cancellation", "drop semantics"]}),
        "/project",
    )
    .expect("several queries are the payload");
    assert_eq!(searched.text, "tokio cancellation\ndrop semantics");
    assert_eq!(searched.total_lines, 2);
    assert!(tool_card_body("web_search", &json!({"query": "solo"}), "/project").is_none());
    assert!(tool_card_body("web_search", &json!({}), "/project").is_none());

    // Everything else stays a one-line card until the reader asks for more.
    for tool in ["read", "grep", "glob", "edit", "git"] {
        assert!(
            tool_card_body(
                tool,
                &json!({"path": "a.rs", "content": "x", "command": "a\nb"}),
                "/project"
            )
            .is_none(),
            "{tool} should not grow a body"
        );
    }
    // An empty or whitespace-only write has nothing to show.
    assert!(
        tool_card_body("write", &json!({"path": "a.rs", "content": "\n\n"}), "/project").is_none()
    );
}

/// The footer has to be able to say what was cut, so the count survives the cut.
#[test]
fn a_capped_body_still_knows_how_long_it_really_was() {
    let content =
        (1..=CARD_BODY_MAX_LINES + 40).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    let body = tool_card_body("write", &json!({"path": "a.rs", "content": content}), "/project")
        .expect("a long write still has a body");

    assert_eq!(body.text.lines().count(), CARD_BODY_MAX_LINES);
    assert_eq!(body.total_lines, CARD_BODY_MAX_LINES + 40);
    assert!(body.is_truncated());
}

/// A written file can carry a key; the body must never be the thing that leaks it.
#[test]
fn a_card_body_is_redacted_before_it_leaves_core() {
    let content = "const KEY: &str = \"sk-abc123def456ghi789jkl012mno345\";";
    let body = tool_card_body("write", &json!({"path": "a.rs", "content": content}), "/project")
        .expect("a write carries its content");
    assert!(!body.text.contains("sk-abc123"), "secret survived: {}", body.text);
    assert!(body.text.contains("[REDACTED_API_KEY]"), "{}", body.text);
}

/// End to end through the label and the body, against the root the caller says the command ran
/// in, and only that root: the labels read no process state, so a replayed card reads the same
/// wherever the front-end happens to run.
#[test]
fn the_card_drops_the_prefix_for_the_directory_the_command_ran_in() {
    let root = Path::new("/home/dev/project");
    let command = "cd /home/dev/project && cargo test";
    assert_eq!(describe_call("bash", &json!({ "command": command }), root), "cargo test");
    assert_eq!(
        describe_call("bash", &json!({ "command": command }), Path::new("/home/dev/other")),
        command,
        "a cd into anywhere else is a real move"
    );

    let script = "cd /home/dev/project && python3 - <<'PY'\nprint(1)\nPY";
    let body = tool_card_body("bash", &json!({ "command": script }), root)
        .expect("a multi-line command still has a body");
    assert!(body.text.starts_with("python3 - <<'PY'"), "{}", body.text);

    // The prefix is the only thing that spanned lines here, so there is no body left to draw.
    let one_liner = "cd /home/dev/project &&\ncargo test";
    assert!(tool_card_body("bash", &json!({ "command": one_liner }), root).is_none());
}
