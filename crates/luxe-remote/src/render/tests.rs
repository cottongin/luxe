use super::*;

fn user(content: serde_json::Value) -> String {
    let piece: Piece =
        serde_json::from_value(serde_json::json!({ "kind": "user", "content": content }))
            .expect("a user piece as the app sends it");
    render(&PlainText, &piece)
}

/// A user message shows what the reader wrote, escaped, and not what rode along for the model.
///
/// The break this catches: hook context shown as the reader's words, or their words written into
/// the page unescaped.
#[test]
fn a_user_message_shows_only_the_readers_words() {
    let html = user(serde_json::json!([
        { "type": "text", "content": "why <b>this</b>?" },
        { "type": "text", "content": "[hook] Plan check: 10 tool calls" },
    ]));
    assert_eq!(html, "<p class=\"words\">why &lt;b&gt;this&lt;/b&gt;?</p>");
}

/// An image sent first keeps its place, so hook context after it is still hidden.
///
/// The break this catches: skipping the blocks that aren't text instead of keeping them as
/// placeholders, which moves the hook's note to block 0, where it reads as the reader's own.
#[test]
fn an_image_keeps_its_place_in_a_user_message() {
    let html = user(serde_json::json!([
        { "type": "image" },
        { "type": "text", "content": "[hook] Plan check: 10 tool calls" },
    ]));
    assert_eq!(html, "<p class=\"attached\">1 image(s) attached</p>");
}

/// The break this catches: images left without a trace, so a photo sent from the phone vanishes
/// from the conversation, or counted wrong, or their glyph written into the page unescaped.
#[test]
fn images_show_as_the_tuis_attached_line() {
    let piece: Piece = serde_json::from_value(serde_json::json!({
        "kind": "user",
        "content": [{ "type": "text", "content": "what's this?" }, { "type": "image" }, { "type": "image" }],
    }))
    .expect("a user piece as the app sends it");
    assert_eq!(
        render(&Loud, &piece),
        "<p class=\"words\">what&#39;s this?</p><p class=\"attached\">&amp; 2 image(s) attached</p>"
    );
}

/// The break this catches: a file or tree piece renamed, sent to another piece's renderer, or the
/// tree refusing the `null` changes the app sends before anything changed, so the Files tab's
/// request is refused or drawn as something else.
#[test]
fn the_file_pieces_arrive_by_their_kinds() {
    let pieces = serde_json::json!([
        { "kind": "file", "file": { "path": "a.rs", "text": "x\n", "bytes": 2 } },
        { "kind": "file_failed", "error": "wrong_kind" },
        { "kind": "tree", "rows": [{ "kind": "note", "depth": 0, "text": "Loading" }],
          "changes": null },
    ]);
    let pieces: Vec<Piece> = serde_json::from_value(pieces).expect("the pieces the app sends");
    let html: Vec<String> = pieces.iter().map(|piece| render(&PlainText, piece)).collect();
    assert!(html[0].starts_with("<div class=\"file\" "), "{html:?}");
    assert!(html[1].contains("not what it was asked for"), "{html:?}");
    assert!(html[2].starts_with("<div class=\"tree\">"), "{html:?}");
}

/// The break this catches: a GitHub piece renamed or sent to another's renderer, or the
/// conversation refusing a request without `opened_by_you`, so the GitHub tab's request is refused.
#[test]
fn the_github_pieces_arrive_by_their_kinds() {
    let issue = serde_json::json!({ "kind": "issue", "overview": { "number": 41, "title": "Crash",
        "body": "", "url": "https://github.com/o/r/issues/41", "repository": "o/r",
        "author": { "login": "mara", "bot": false }, "state": "open", "updated_at": 0,
        "labels": [], "assignees": [], "milestone": null, "linked": [], "conversation_count": 0 } });
    let pieces = serde_json::json!([
        { "kind": "github_list", "repository": null, "now": 0,
          "section": { "name": "Mine", "items": [], "error": null, "total": 0 } },
        { "kind": "github_header", "overview": issue, "now": 0 },
        { "kind": "github_overview", "overview": issue, "now": 0 },
        { "kind": "github_conversation", "entries": [], "now": 0 },
    ]);
    let pieces: Vec<Piece> = serde_json::from_value(pieces).expect("the pieces the app sends");
    let html: Vec<String> = pieces.iter().map(|piece| render(&PlainText, piece)).collect();
    assert!(html[0].contains("Nothing matches this search"), "{html:?}");
    assert!(html[1].starts_with("<header class=\"gh-head\">"), "{html:?}");
    assert!(html[2].starts_with("<h3 class=\"gh-section\">Status"), "{html:?}");
    assert_eq!(html[3], "<div class=\"gh-conversation\"></div>");
}

/// The break this catches: a review piece renamed, or one refusing a request that leaves out what
/// the app may not have (the walked chapters, a chapter's reading or whether it was walked).
#[test]
fn the_review_pieces_arrive_by_their_kinds() {
    let shown: serde_json::Value =
        serde_json::from_str(include_str!("review/pr-230.json")).expect("the captured review");
    let chapter = &shown["review"]["chapters"][0];
    let pieces = serde_json::json!([
        { "kind": "review_guide", "review": shown["review"], "reading": shown["reading"] },
        { "kind": "review_chapter", "chapter": chapter, "files": [],
          "place": { "number": 1, "total": 3 }, "reading": null },
    ]);
    let pieces: Vec<Piece> = serde_json::from_value(pieces).expect("the pieces the app sends");
    let html: Vec<String> = pieces.iter().map(|piece| render(&PlainText, piece)).collect();
    assert!(html[0].starts_with("<div class=\"rv-guide\">"), "{html:?}");
    assert!(html[1].starts_with("<div class=\"rv-chapter-view\">"), "{html:?}");
}

/// The break this catches: a dock or diff piece renamed, so the app's request for it is refused
/// as a whole and nothing on the screen renders.
#[test]
fn the_dock_and_diff_pieces_arrive_by_their_kinds() {
    let pieces = serde_json::json!([
        { "kind": "plan", "todos": { "current": null, "items": [], "pending": 0, "in_progress": 0,
            "completed": 0, "blocked": 0, "total": 0, "truncated": false } },
        { "kind": "changes", "changes": { "files": [], "total_files": 0, "additions": 0,
            "deletions": 0, "truncated": false } },
        { "kind": "diff", "diff": { "path": "a", "file": null, "untracked": false } },
        { "kind": "diff_failed", "error": "too_large" },
    ]);
    let pieces: Vec<Piece> = serde_json::from_value(pieces).expect("the pieces the app sends");
    let html: Vec<String> = pieces.iter().map(|piece| render(&PlainText, piece)).collect();
    assert!(html[0].contains(">PLAN</span>"), "{html:?}");
    assert!(html[1].contains(">FILES</span>"), "{html:?}");
    assert!(html[2].contains("matches the last commit"), "{html:?}");
    assert!(html[3].contains("too large"), "{html:?}");
}

/// The break this catches: a message with no images given an "attached" line anyway.
#[test]
fn words_alone_say_nothing_is_attached() {
    let html =
        user(serde_json::json!([{ "type": "text", "content": "hi" }, { "type": "tool_result" }]));
    assert_eq!(html, "<p class=\"words\">hi</p>");
}

/// The break this catches: a brief luxe wrote shown whole, as a page of instructions in the
/// reader's colors.
#[test]
fn a_brief_shows_as_one_line() {
    let html = user(serde_json::json!([
        { "type": "text", "content": "Draft a reply to @jon on PR 12.\n\nKeep it short." },
    ]));
    assert_eq!(html, "<p class=\"brief\">Drafted a reply</p>");
}

/// A question from a review is the reader's, so it shows, under where it was asked.
///
/// The break this catches: the brief labeled as one line instead, losing the question the answer
/// below it is answering.
#[test]
fn a_review_question_shows_what_was_asked() {
    let brief = "Guided review feedback\nSubmission: submission:1\nReview: review:2\n\
        Source revision: revision:3\nMode: Question\n\n\
        - Question · Why two passes?\n\
        \x20 Finding ID: finding:2\n\
        \x20 Evidence: src/a.rs:3 (new)\n\
        \x20 Why two passes?\n\
        Do not rewrite unrelated code.";
    let html = user(serde_json::json!([{ "type": "text", "content": brief }]));
    assert_eq!(
        html,
        "<p class=\"words\"><span class=\"chip\">Asked in review \u{b7} src/a.rs:3 (new)</span>\n\
         Why two passes?</p>"
    );
}

/// A renderer that words every status the same, with a mark and words that need escaping.
struct Loud;

impl Renderer for Loud {
    fn markdown(&self, _text: &str, _voice: Voice) -> String {
        String::new()
    }

    fn tool_glyph(&self, _tool: &str) -> String {
        String::new()
    }

    fn status(&self, _facts: &StatusFacts) -> Vec<StatusSegment> {
        let segment = |kind, glyph: Option<&str>, tone| StatusSegment {
            kind,
            glyph: glyph.map(str::to_string),
            label: "<9%>".into(),
            tone,
        };
        vec![
            segment("context", Some("&"), StatusTone::Error),
            segment("cost", None, StatusTone::Plain),
            segment("model", None, StatusTone::Warning),
        ]
    }

    fn image_glyph(&self) -> String {
        "&".into()
    }

    fn mode_mark(&self, mode: &str) -> String {
        if mode == "plan" { "P&" } else { "B" }.into()
    }
}

/// The break this catches: the mode button showing build's mark in plan mode (or none at all),
/// its mark or name written unescaped, or a renderer without marks leaving an empty mark behind.
#[test]
fn a_mode_wears_its_own_mark() {
    let mode = |mode: &str| Piece::ModeName { mode: mode.into() };
    assert_eq!(
        render(&Loud, &mode("plan")),
        "<span class=\"mode-mark\" aria-hidden=\"true\">P&amp;</span> plan"
    );
    assert!(render(&Loud, &mode("<b>")).ends_with(">B</span> &lt;b&gt;"));
    assert_eq!(render(&PlainText, &mode("build")), "build");
}

/// The break this catches: a status segment's words or mark written into the page unescaped, a
/// tone lost so a full window reads as a roomy one, or the app's facts not parsing as sent.
#[test]
fn a_status_renders_each_segment_escaped_with_its_tone() {
    let piece: Piece = serde_json::from_value(serde_json::json!({
        "kind": "status",
        "model": {"provider": "anthropic", "model": "claude-x"},
        "thinking": "high",
        "context": {"used_tokens": 1, "context_window": 2},
    }))
    .expect("a status piece as the app sends it");
    assert_eq!(
        render(&Loud, &piece),
        "<span class=\"seg seg-context tone-error\">\
         <span class=\"seg-glyph\" aria-hidden=\"true\">&amp;</span> &lt;9%&gt;</span>\
         <span class=\"seg seg-cost\">&lt;9%&gt;</span>\
         <span class=\"seg seg-model tone-warning\">&lt;9%&gt;</span>"
    );
}

/// The break this catches: the plain renderer, which a bridge has before the TUI's, showing no
/// model, so a phone can't tell what's answering.
#[test]
fn the_plain_renderer_names_the_model() {
    let facts = StatusFacts {
        model: Some(luxe_core::ModelRef::new("anthropic", "claude-x")),
        ..StatusFacts::default()
    };
    let labels: Vec<_> = PlainText.status(&facts).into_iter().map(|s| s.label).collect();
    assert_eq!(labels, ["claude-x"]);
}

/// The break this catches: an answer rendered as sent, so a draft meant for its card shows in
/// the conversation too.
#[test]
fn an_answer_leaves_out_what_was_written_for_luxe() {
    let piece = Piece::Answer { text: "```luxe-draft\nThanks, merged.\n```".into() };
    let html = render(&PlainText, &piece);
    assert!(!html.contains("merged"), "{html}");
}
