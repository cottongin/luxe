use super::*;
use serde_json::json;

/// A capture's header names what was captured. The generic fallback lists every argument the
/// call carried, and a model habitually sends this tool's optional ones as empty strings — so a
/// screenshot arrived under `height=800 · mode=url · output= · region= · window=`, above a
/// picture that already showed what it was.
#[test]
fn a_capture_header_names_what_was_captured() {
    let described = |arguments| describe_call("screenshot", &arguments, "/project");
    assert_eq!(
        described(json!({
            "mode": "url", "url": "https://example.com", "width": 1280, "height": 800,
            "output": "", "region": "", "window": "",
        })),
        "url · https://example.com"
    );
    assert_eq!(described(json!({ "mode": "window", "window": "Firefox" })), "window · Firefox");
    assert_eq!(
        described(json!({ "mode": "region", "region": "0,0 800x600" })),
        "region · 0,0 800x600"
    );
    // Nothing to name but the mode, or not even that.
    assert_eq!(described(json!({ "mode": "fullscreen" })), "fullscreen");
    assert_eq!(described(json!({})), "fullscreen", "the tool's own default");
    // A note is the only thing that says what a bare fullscreen grab was *for*.
    assert_eq!(
        described(json!({ "mode": "fullscreen", "note": "the dock at 80 cols" })),
        "fullscreen · the dock at 80 cols"
    );
}

#[test]
fn calls_describe_themselves_by_their_real_subject() {
    assert_eq!(describe_call("read", &json!({"path": "src/main.rs"}), "/project"), "src/main.rs");
    assert_eq!(
        describe_call(
            "read",
            &json!({"path": "src/main.rs", "offset": 40, "limit": 20}),
            "/project"
        ),
        "src/main.rs:40-60"
    );
    assert_eq!(
        describe_call("read", &json!({"paths": ["a.rs", "b.rs"]}), "/project"),
        "a.rs, b.rs"
    );
    assert_eq!(
        describe_call("read", &json!({"paths": ["a", "b", "c", "d", "e"]}), "/project"),
        "a, b, c +2",
        "the first few by name, the rest counted"
    );
    assert_eq!(describe_call("bash", &json!({"command": "cargo test"}), "/project"), "cargo test");
    assert_eq!(
        describe_call("grep", &json!({"pattern": "todo", "path": "src"}), "/project"),
        "\"todo\" in src"
    );
    // Search is the most-read header in a session; the arguments that change *what was
    // searched* have to appear on it, or the card misreports the work.
    assert_eq!(
        describe_call("grep", &json!({"pattern": "todo", "glob": "*.py"}), "/project"),
        "\"todo\" · *.py"
    );
    assert_eq!(
        describe_call("grep", &json!({"pattern": "todo", "path": "src", "all": true}), "/project"),
        "\"todo\" in src · +ignored",
        "a sweep that includes build output must say so"
    );
    assert_eq!(
        describe_call("grep", &json!({"pattern": "x", "glob": "*.rs", "all": true}), "/project"),
        "\"x\" · *.rs · +ignored"
    );
    assert_eq!(
        describe_call("glob", &json!({"pattern": "**/*.py", "all": true}), "/project"),
        "**/*.py · +ignored"
    );
    assert_eq!(
        describe_call("grep", &json!({"pattern": "x", "outputMode": "files"}), "/project"),
        "\"x\" · files only",
        "asking which files match is a different search from asking for the lines"
    );
    assert_eq!(
        describe_call("grep", &json!({"pattern": "x", "output_mode": "count"}), "/project"),
        "\"x\" · counts",
        "the retired spelling still describes the call it made"
    );
    assert_eq!(
        describe_call("grep", &json!({"pattern": "x", "invert": true, "glob": "*.rs"}), "/project"),
        "\"x\" · *.rs · inverted",
        "an inverted search matched the opposite of what the header quotes"
    );
    assert_eq!(
        describe_call(
            "grep",
            &json!({"pattern": "todo", "all": false, "glob": "", "invert": false, "outputMode": "content", "limit": 0}),
            "/project"
        ),
        "\"todo\"",
        "defaults add nothing to the header"
    );
    assert_eq!(
        describe_call("git", &json!({"action": "commit", "args": ["-m", "wip"]}), "/project"),
        "commit -m wip"
    );
    assert_eq!(
        describe_call("edit", &json!({"path": "a.rs", "edits": [{}, {}]}), "/project"),
        "a.rs · 2 edits"
    );
    assert_eq!(
        describe_call("read_symbol", &json!({"file": "a.rs", "symbol": "run"}), "/project"),
        "run in a.rs"
    );
}

/// A commit renders as its subject, and stays one line however long the message is.
///
/// Both halves are the same defect seen from opposite sides. The message never appeared: it is
/// not in `args`, and `args` was all this read — so a correctly-formed commit drew as a bare
/// `git commit`. And when a model put the message in `args` instead, the entire body came
/// through as the header, which is the wall of text in the report that started this.
#[test]
fn a_commit_header_carries_its_subject_and_only_its_subject() {
    let message = "Animate the loading spinner\n\nReplace the four static frames with the \
                       standard ten-frame braille sequence.";

    assert_eq!(
        describe_call("git", &json!({"action": "commit", "message": message}), "/project"),
        "commit · Animate the loading spinner …",
        "the subject, and a mark that there is more"
    );
    assert_eq!(
        describe_call(
            "git",
            &json!({"action": "commit", "message": "tidy up", "args": ["--amend"]}),
            "/project"
        ),
        "commit --amend · tidy up",
        "a flag that changes what the commit does keeps its place"
    );
    assert_eq!(
        describe_call("git", &json!({"action": "commit", "args": [message]}), "/project"),
        "commit Animate the loading spinner …",
        "a message that arrived in args is still cut to one line"
    );
    assert_eq!(describe_call("git", &json!({"action": "status"}), "/project"), "status");
}

#[test]
fn a_skill_card_names_the_skill_without_the_implied_load() {
    // Loading is what a named skill call *is*; saying so twice is noise.
    assert_eq!(
        describe_call("skill", &json!({"action": "load", "name": "slides"}), "/project"),
        "slides"
    );
    // Discovery has no skill to name, so its action is the whole label.
    assert_eq!(describe_call("skill", &json!({"action": "discover"}), "/project"), "discover");
    // Anything else keeps its action, so the card can't imply the wrong one.
    assert_eq!(
        describe_call("skill", &json!({"action": "remove", "name": "slides"}), "/project"),
        "remove · slides"
    );
}

/// The checklist is the only view the user has of the agent's plan, so its card has to name the
/// item — the verb alone (`add`, `update`) says nothing about what is being planned.
#[test]
fn a_todo_card_names_the_item_it_is_about() {
    assert_eq!(
        describe_call("todo", &json!({"action": "add", "subject": "wire the dock"}), "/project"),
        "add · wire the dock"
    );
    // A whole plan lands in one call, so the header leads with it and counts the rest.
    assert_eq!(
        describe_call(
            "todo",
            &json!({"action": "add", "subjects": ["audit the copy", "draft it", "ship it"]}),
            "/project"
        ),
        "add · audit the copy +2 more"
    );
    // The id is the model's handle on the item, not the reader's: it appears nowhere in the
    // checklist they see, so the header spends its slot on where the item is going instead and
    // leaves naming it to the result.
    assert_eq!(
        describe_call(
            "todo",
            &json!({"action": "update", "id": 3, "status": "in_progress"}),
            "/project"
        ),
        "update · in_progress"
    );
    assert_eq!(
        describe_call("todo", &json!({"action": "complete", "id": 7}), "/project"),
        "complete"
    );
    assert_eq!(describe_call("todo", &json!({"action": "update", "id": 3}), "/project"), "update");
    assert_eq!(describe_call("todo", &json!({"action": "list"}), "/project"), "list");
}

/// A model under a strict JSON schema fills every property it was shown, so half the arguments
/// arrive blank. Read naively they turn a label into `add · ` — a separator with nothing after
/// it, which is what a real gpt-5-class session put on every todo card it drew.
#[test]
fn a_blank_argument_is_an_absent_argument() {
    let default_filled = json!({
        "action": "add", "activeForm": "", "blockedBy": [], "id": 0,
        "status": "pending", "subject": "", "subjects": ["audit the copy"]
    });
    assert_eq!(describe_call("todo", &default_filled, "/project"), "add · audit the copy");

    let nothing_to_say = json!({
        "action": "list", "activeForm": "", "blockedBy": [], "id": 0,
        "status": "", "subject": "", "subjects": []
    });
    assert_eq!(describe_call("todo", &nothing_to_say, "/project"), "list");

    // Not a todo problem: every arm reads its arguments the same way.
    assert_eq!(describe_call("task", &json!({"action": "list", "title": ""}), "/project"), "list");
    assert_eq!(
        describe_call("loop_control", &json!({"action": "progress", "reason": ""}), "/project"),
        "progress"
    );
    assert_eq!(
        describe_call("test", &json!({"command": "cargo nextest run", "filter": ""}), "/project"),
        "cargo nextest run"
    );
    for described in [
        describe_call("todo", &nothing_to_say, "/project"),
        describe_call("task", &json!({"action": "list", "title": ""}), "/project"),
        describe_call("loop_control", &json!({"action": "progress", "reason": ""}), "/project"),
    ] {
        assert!(!described.contains('·'), "a dangling separator: {described:?}");
    }
}

#[test]
fn a_tool_with_no_bespoke_shape_reads_as_key_values_not_json() {
    // Without a bespoke shape, a card used to show the raw argument JSON.
    let described =
        describe_call("debug", &json!({"action": "launch", "program": "./app"}), "/project");
    assert_eq!(described, "launch · ./app");

    let unknown = describe_call(
        "some_new_tool",
        &json!({"alpha": "one", "count": 3, "deep": {"x": 1}}),
        "/project",
    );
    assert!(!unknown.contains('{') || unknown.contains("{…}"), "no raw JSON: {unknown}");
    assert!(unknown.contains("alpha=one"), "{unknown}");
    assert!(unknown.contains("count=3"), "{unknown}");
}

#[test]
fn a_multiline_command_collapses_to_one_line() {
    let described =
        describe_call("bash", &json!({"command": "cd /tmp\ncargo build --release"}), "/project");
    assert_eq!(described, "cd /tmp …");
}

/// A search is judged on the angles it covered, so the card has to carry the words.
///
/// A multi-query call has no `query` at all, so it fell through to the generic fallback and
/// drew as `queries=[3]` — a count where the one thing worth reading is the phrasings.
#[test]
fn a_search_header_names_the_query_and_counts_the_rest() {
    let described = |arguments| describe_call("web_search", &arguments, "/project");
    assert_eq!(described(json!({"query": "ratatui line wrapping"})), "ratatui line wrapping");
    assert_eq!(
        described(json!({"queries": ["tokio cancellation", "drop semantics", "kill_on_drop"]})),
        "tokio cancellation +2 more"
    );
    // The single form is folded into the list the tool actually runs, and a repeat of a query
    // already listed is not a second search.
    assert_eq!(
        described(json!({"queries": ["one"], "query": "two"})),
        "one +1 more",
        "both arguments name searches this call will run"
    );
    assert_eq!(described(json!({"queries": ["one"], "query": "one"})), "one");
    // A model under a strict schema sends the argument it did not use as an empty one.
    assert_eq!(described(json!({"queries": [], "query": "solo"})), "solo");
}

/// The card and the search read one list, so a card cannot name a search that never ran.
#[test]
fn the_queries_a_card_lists_are_the_queries_the_call_runs() {
    assert_eq!(
        web_search_queries(&json!({"queries": [" a ", "b", "a", ""], "query": "b"})),
        vec!["a".to_string(), "b".to_string()],
        "trimmed, de-duplicated, in the order they were asked for"
    );
    assert!(web_search_queries(&json!({})).is_empty());
    assert!(web_search_queries(&json!({"queries": "not a list"})).is_empty());
}

/// A `write`'s content is its argument, so the model spends longer writing the call than the
/// tool spends running it. The path arrives first, and naming it is what turns a bare spinner
/// into a card worth looking at.
#[test]
fn a_half_written_call_is_named_from_the_arguments_that_arrived() {
    let streamed = |fragments: &[&str]| {
        let mut buffer = String::new();
        fragments
            .iter()
            .map(|fragment| {
                buffer.push_str(fragment);
                describe_partial_call("write", &buffer, "/project")
            })
            .collect::<Vec<_>>()
    };

    assert_eq!(
        streamed(&["{\"pa", "th\":\"src/fo", "o.rs\",", "\"content\":\"fn ma", "in() {}\"}"]),
        [
            None,
            None,
            Some("src/foo.rs".to_string()),
            Some("src/foo.rs".to_string()),
            Some("src/foo.rs".to_string()),
        ],
        "nothing until the member closes, and never a half-written path",
    );

    // One member is a whole call for most tools, and it has no comma after it to wait for.
    // A `bash` card used to stay nameless until the closing brace because of that.
    assert_eq!(
        describe_partial_call("bash", "{\"command\":\"cargo test\"", "/project"),
        Some("cargo test".into())
    );
    assert_eq!(
        describe_partial_call("bash", "{\"command\":\"cargo test", "/project"),
        None,
        "still arriving"
    );

    // An `edit`'s path is readable at byte thirty and its edits run to tens of kilobytes, so
    // the name must not depend on how much came after it — the long edits are exactly the ones
    // whose name a reader wants while they wait.
    let long_edit = format!(
        "{{\"path\":\"capture-call.sh\",\"edits\":[{{\"oldText\":\"{}",
        "x".repeat(64 * 1024),
    );
    assert_eq!(
        describe_partial_call("edit", &long_edit, "/project"),
        Some("capture-call.sh".into())
    );

    // A member that is not a string closes the same way, and a nested one closes with its
    // bracket rather than waiting for a comma that may never come.
    assert_eq!(
        describe_partial_call("web_search", "{\"queries\":[\"one\",\"two\"]", "/project"),
        Some("one +1 more".to_string()),
    );
    assert_eq!(
        describe_partial_call("todo", "{\"action\":\"add\"", "/project"),
        Some("add".to_string())
    );

    // Every tool's own shape, not just the ones with a path.
    assert_eq!(
        describe_partial_call("bash", "{\"command\":\"cargo test\",\"timeout\":6", "/project"),
        Some("cargo test".to_string())
    );
    assert_eq!(
        describe_partial_call("grep", "{\"pattern\":\"needle\",\"glob\":\"*.r", "/project"),
        Some("\"needle\"".to_string()),
        "the scope is still arriving, so the header names what it can"
    );

    // A nested value's own commas are not the end of a member.
    assert_eq!(
        describe_partial_call(
            "web_search",
            "{\"queries\":[\"one\",\"two\"],\"numResults\":5",
            "/project"
        ),
        Some("one +1 more".to_string())
    );

    // Nor is a comma inside a string, or one behind an escaped quote.
    assert_eq!(
        describe_partial_call(
            "bash",
            "{\"command\":\"echo \\\"a,b\\\"\",\"cwd\":\"/tm",
            "/project"
        ),
        Some("echo \"a,b\"".to_string())
    );

    // Nothing to say, and nothing that pretends otherwise.
    assert_eq!(describe_partial_call("write", "", "/project"), None);
    assert_eq!(describe_partial_call("write", "{\"path\":\"\",", "/project"), None);
    assert_eq!(describe_partial_call("write", "{}", "/project"), None, "a call with no arguments");
    // Not a fragment of anything readable, and emphatically not a panic in core.
    assert_eq!(describe_partial_call("write", "}}}},", "/project"), None);
    assert_eq!(describe_partial_call("write", "\"just a string\"", "/project"), None);
    assert_eq!(
        describe_partial_call("nonesuch", "{\"width\":800,\"height\":600,", "/project"),
        None,
        "a fallback key=value rendering describes the fragment, not the call",
    );

    // The pathological order: the bulky value first, so nothing has closed and there is
    // genuinely nothing to say yet.
    let content_first = format!("{{\"content\":\"{}", "x".repeat(64 * 1024));
    assert_eq!(describe_partial_call("write", &content_first, "/project"), None);
}
