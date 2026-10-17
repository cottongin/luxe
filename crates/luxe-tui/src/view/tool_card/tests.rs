use super::test_support::{context, plain, theme};
use super::*;

fn tool_item(name: &str, label: &str, output: Option<&str>) -> ToolItem {
    let mut activity = luxe_core::ToolActivity::running("session-a", "call-a", name, label, 1);
    activity.state = luxe_core::ToolActivityState::Succeeded;
    activity.output_preview = output.map(str::to_string);
    ToolItem { activity, expanded: false, details_toggled: false, waited_on_user_ms: 0 }
}

/// The `test` tool's reply for one failing Node test, as the tool writes it.
const FAILING_RUN: &str = "$ npm test\n\
        node: FAILED — 3 passed, 1 failed, 0 skipped\n\
        Failures:\n  \
        ✗ dueBy treats an unpadded cutoff as the same day (#12)  (test/filter.test.js:31)\n      \
        AssertionError [ERR_ASSERTION]: Expected values to be strictly deep-equal:\n      \
        + actual - expected\n      \n      \
        [\n        'Ship the docs',\n      +   'Fix the parser'\n      ]\n\n\
        Full output: .luxe/artifacts/test-1.log\n\nExit code: 1";

/// A failing run, opened for the reader because it failed, is its counts and its failure — not
/// the runner's whole reply. Opening it themselves still shows the reply.
#[test]
fn a_failing_test_run_is_its_verdict_and_its_failures() {
    let styles = theme();
    let mut item = tool_item("test", "project tests", Some(FAILING_RUN));
    item.activity.state = luxe_core::ToolActivityState::Failed;
    item.expanded = true;
    let opened_for_them = CardContext { expanded: false, ..context(&styles, 100) };
    let lines: Vec<String> = body(&item, &opened_for_them).lines.iter().map(plain).collect();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert_eq!(lines[0], "3 passed · 1 failed");
    assert!(lines[1].contains("dueBy treats an unpadded cutoff") && lines[1].contains(":31"));
    assert!(lines[2].contains("AssertionError"), "{lines:?}");

    item.details_toggled = true;
    let asked: Vec<String> = body(&item, &opened_for_them).lines.iter().map(plain).collect();
    assert!(asked.iter().any(|line| line.contains("Exit code: 1")), "the reply: {asked:?}");

    // A green run stays a one-row card
    let green = tool_item(
        "test",
        "project tests",
        Some("$ npm test\nnode: ok — 4 passed, 0 failed, 0 skipped"),
    );
    assert!(body(&green, &opened_for_them).lines.is_empty());
}

/// A suite run through `bash` reads the same: a failing one is its verdict and its failures
/// instead of the command, and a passing one keeps showing the command.
#[test]
fn a_failing_suite_through_bash_is_its_verdict_too() {
    let styles = theme();
    let mut item = tool_item("bash", "Run the tests", Some(FAILING_RUN));
    item.activity.card_body = luxe_core::tool_card_body(
        "bash",
        &serde_json::json!({"description": "Run the tests", "command": "npm test"}),
        "/project",
    );
    let closed = CardContext { expanded: false, ..context(&styles, 100) };
    let lines: Vec<String> = body(&item, &closed).lines.iter().map(plain).collect();
    assert_eq!(lines[0], "3 passed · 1 failed", "{lines:?}");

    item.activity.output_preview = Some("node: ok — 4 passed, 0 failed, 0 skipped".into());
    let lines: Vec<String> = body(&item, &closed).lines.iter().map(plain).collect();
    assert_eq!(lines, ["npm test"]);
}

const EDIT_OUTPUT: &str = "Applied 1 edit(s) to 'src/lib.rs'\n\n\
                               \u{20}   ...\n\
                               \u{20}11 fn main() {\n\
                               -12     println!(\"old\");\n\
                               +12     println!(\"new\");\n\
                               \u{20}13 }";

/// The header already names the file; the body should not spend a row saying it again.
#[test]
fn an_edit_body_drops_the_summary_the_header_already_carries() {
    let styles = theme();
    let item = tool_item("edit", "src/lib.rs", Some(EDIT_OUTPUT));
    let lines = body(&item, &context(&styles, 80)).lines;

    assert!(
        !lines.iter().any(|line| plain(line).contains("Applied 1 edit")),
        "{:?}",
        lines.iter().map(plain).collect::<Vec<_>>()
    );
    assert!(lines.iter().any(|line| plain(line).contains("println!(\"new\");")));
}

/// Most cards say everything on their header; a body is the exception, not the rule.
#[test]
fn a_collapsed_card_without_a_payload_has_no_body() {
    let styles = theme();
    for name in ["read", "grep", "glob", "bash"] {
        let item = tool_item(name, "whatever", Some("a\nlot\nof\noutput"));
        let collapsed = body(&item, &CardContext { expanded: false, ..context(&styles, 80) }).lines;
        assert!(collapsed.is_empty(), "{name} grew a body while collapsed");
    }
}

/// Opening a payload-less card shows the raw call and a *bounded* window onto its output.
///
/// Bounded even open, because the toggle opens every card at once — unbounded here turns one
/// keystroke into a wall of file contents.
#[test]
fn an_opened_card_bounds_the_output_it_reveals() {
    let styles = theme();
    let output = (1..=60).map(|n| format!("match {n}")).collect::<Vec<_>>().join("\n");
    let mut item = tool_item("grep", "\"needle\" in src/", Some(&output));
    item.activity.input_preview = Some(r#"{"pattern":"needle","path":"src/"}"#.into());

    let lines = body(&item, &context(&styles, 80)).lines;
    assert_eq!(plain(&lines[0]), r#"arguments: {"pattern":"needle","path":"src/"}"#);
    assert_eq!(plain(&lines[1]), "output: match 1");
    assert_eq!(plain(&lines[2]), "match 2");
    assert_eq!(lines.len(), 1 + OUTPUT_PREVIEW_LINES + 1, "the call, the window, the footer");
    let footer = plain(lines.last().expect("a footer"));
    assert_eq!(footer, "⋯ 36 more lines · 60 total · open the file for the rest");
    assert!(!footer.contains("to expand"), "already open, so it must not offer to open: {footer}");
}

/// The footer accounts for what the budget hid, and names both ways to undo it.
#[test]
fn a_collapsed_body_says_what_it_hid_and_how_to_see_it() {
    let styles = theme();
    let content = (1..=40).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    let mut item = tool_item("write", "notes.txt", None);
    item.activity.card_body = luxe_core::tool_card_body(
        "write",
        &serde_json::json!({"path": "notes.txt", "content": content}),
        "/project",
    );

    let collapsed = body(&item, &CardContext { expanded: false, ..context(&styles, 80) }).lines;
    assert_eq!(collapsed.len(), CARD_PREVIEW_LINES + 1, "ten rows and the footer");
    assert_eq!(
        plain(&collapsed[CARD_PREVIEW_LINES]),
        "⋯ 30 more lines · 40 total · click or ctrl+o to expand"
    );

    // With no chord bound, the mouse is still an answer — and the footer is the click target.
    let chordless =
        body(&item, &CardContext { expanded: false, expand_chord: None, ..context(&styles, 80) });
    assert!(chordless.truncated, "the footer offers the expand, so a click has to reach it");
    assert_eq!(
        plain(&chordless.lines[CARD_PREVIEW_LINES]),
        "⋯ 30 more lines · 40 total · click to expand"
    );

    let expanded = body(&item, &context(&styles, 80)).lines;
    assert_eq!(expanded.len(), 40, "expanded shows everything, footer included in nothing");
    assert!(plain(&expanded[39]).contains("line 40"));
}

/// A search lists the angles it covered, folds past a handful of them, and counts what it hid
/// in the unit the rows actually are.
#[test]
fn a_search_body_lists_its_queries_and_counts_them_as_queries() {
    let styles = theme();
    let queries: Vec<String> = (1..=8).map(|n| format!("query number {n}")).collect();
    let mut item = tool_item("web_search", "query number 1 +7 more", None);
    item.activity.card_body = luxe_core::tool_card_body(
        "web_search",
        &serde_json::json!({ "queries": queries }),
        "/project",
    );

    let collapsed = body(&item, &CardContext { expanded: false, ..context(&styles, 80) });
    assert_eq!(collapsed.lines.len(), CARD_QUERY_PREVIEW_LINES + 1, "five rows and the footer");
    // Numbered, so the list reads as the searches it is rather than as wrapped prose.
    assert_eq!(plain(&collapsed.lines[0]), "1 query number 1");
    assert_eq!(plain(&collapsed.lines[4]), "5 query number 5");
    assert_eq!(
        plain(&collapsed.lines[CARD_QUERY_PREVIEW_LINES]),
        "⋯ 3 more queries · 8 total · click or ctrl+o to expand"
    );
    assert!(collapsed.truncated, "the footer offers the expand, so a click has to reach it");

    let expanded = body(&item, &context(&styles, 80)).lines;
    assert_eq!(expanded.len(), 8, "opening it shows every search");

    // A search the header already carries in full does not repeat itself under it.
    let mut single = tool_item("web_search", "one query", None);
    single.activity.card_body = luxe_core::tool_card_body(
        "web_search",
        &serde_json::json!({"query": "one query"}),
        "/project",
    );
    assert!(body(&single, &context(&styles, 80)).lines.is_empty());
}

/// A card draws ten lines of a body however deep the body is, and it may hold six hundred —
/// which a `write` card now does for as long as the model takes to write the file, rather than
/// the two milliseconds a write takes. Laying out what the window throws away cost **40ms a
/// frame** at that depth, on its own over the frame budget.
///
/// So the layout stops at the window, and the footer still has to be honest about what it
/// stopped short of — which is the half a bounded layout gets wrong if it counts what it drew
/// instead of what it holds.
#[test]
fn a_deep_body_is_laid_out_to_the_window_and_still_counts_honestly() {
    let styles = theme();
    let content = (1..=600).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    let mut item = tool_item("write", "big.txt", None);
    item.activity.card_body = luxe_core::tool_card_body(
        "write",
        &serde_json::json!({"path": "big.txt", "content": content}),
        "/project",
    );

    let collapsed = CardContext { expanded: false, ..context(&styles, 80) };
    let lines = body(&item, &collapsed).lines;
    assert_eq!(lines.len(), CARD_PREVIEW_LINES + 1, "the window, and the footer");
    assert_eq!(
        plain(lines.last().expect("a footer")),
        "⋯ 590 more lines · 600 total · click or ctrl+o to expand",
        "a bounded layout must not report a deep body as one it showed in full",
    );
    // Opened, the reader asked for all of it and gets all of it.
    assert_eq!(body(&item, &context(&styles, 80)).lines.len(), 600);

    // And the bound is on the *lines laid out*, not just the lines kept.
    let ctx = context(&styles, 80);
    assert_eq!(
        content::render(&content, Some("txt"), content::Gutter::LineNumbers, Some(11), &ctx).len(),
        11,
    );
}

/// A file arriving must not shove its own body sideways.
///
/// The gutter is as wide as the largest number it draws. Sized from the payload's *total*
/// instead — which is the obvious reading, and what a settled card could get away with — every
/// hundredth line the model wrote widened the column and shunted the whole body one cell right,
/// over and over, for as long as the write took.
#[test]
fn a_body_filling_in_does_not_shift_under_the_reader() {
    let styles = theme();
    let ctx = CardContext { expanded: false, ..context(&styles, 80) };
    let mut item = tool_item("write", "big.txt", None);

    let leads: Vec<String> = [4usize, 40, 400, 4_000]
        .into_iter()
        .map(|lines| {
            let content = (1..=lines).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
            item.activity.card_body = luxe_core::tool_card_body(
                "write",
                &serde_json::json!({"path": "big.txt", "content": content}),
                "/project",
            );
            let lines = body(&item, &ctx).lines;
            plain(&lines[0]).split_once("line").expect("a first row").0.to_string()
        })
        .collect();

    assert_eq!(leads, [" 1 ", " 1 ", " 1 ", " 1 "], "the first row never moves: {leads:?}");
}

/// When the payload itself was cut short, expanding cannot help and the footer must not pretend.
#[test]
fn a_capped_payload_points_past_the_transcript() {
    let styles = theme();
    let mut item = tool_item("write", "big.txt", None);
    item.activity.card_body = Some(luxe_core::ToolCardBody {
        text: "one\ntwo".into(),
        total_lines: 900,
        collapsed_lines: None,
    });

    let lines = body(&item, &context(&styles, 80)).lines;
    let footer = plain(lines.last().expect("a footer"));
    assert_eq!(footer, "⋯ 898 more lines · 900 total · open the file for the rest");
}

/// A body that fits says nothing about truncation, because nothing was truncated.
#[test]
fn a_short_body_has_no_footer() {
    let styles = theme();
    let mut item = tool_item("write", "a.rs", None);
    item.activity.card_body = luxe_core::tool_card_body(
        "write",
        &serde_json::json!({"path": "a.rs", "content": "fn main() {}\n"}),
        "/project",
    );
    let lines = body(&item, &context(&styles, 80)).lines;
    assert_eq!(lines.len(), 1);
    assert!(!plain(&lines[0]).contains('⋯'));
}

/// A wrapped row costs its visual lines against the budget, or a wide card could not be trusted
/// to stay ten lines tall.
#[test]
fn the_budget_counts_visual_lines_not_rows() {
    let styles = theme();
    let long = (1..=8).map(|n| format!("line {n} {}", "x".repeat(60))).collect::<Vec<_>>();
    let mut item = tool_item("write", "a.txt", None);
    item.activity.card_body = luxe_core::tool_card_body(
        "write",
        &serde_json::json!({"path": "a.txt", "content": long.join("\n")}),
        "/project",
    );

    let lines = body(&item, &CardContext { expanded: false, ..context(&styles, 30) }).lines;
    // Three visual lines per row at this width, so three rows fit and the fourth would breach.
    assert_eq!(lines.len(), 9 + 1, "nine visual lines and the footer, got {}", lines.len());
    assert!(plain(lines.last().expect("a footer")).contains("5 more lines · 8 total"));
}

/// A per-card expand and the global toggle mean the same thing to a body.
#[test]
fn either_expansion_opens_the_card() {
    let styles = theme();
    let content = (1..=40).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    let mut item = tool_item("write", "notes.txt", None);
    item.activity.card_body = luxe_core::tool_card_body(
        "write",
        &serde_json::json!({"path": "notes.txt", "content": content}),
        "/project",
    );
    item.expanded = true;

    let lines = body(&item, &CardContext { expanded: false, ..context(&styles, 80) }).lines;
    assert_eq!(lines.len(), 40, "the card's own toggle still opens it");
}

/// The column a question card's rows put their text in: where its header puts the question.
fn answer_column() -> usize {
    crate::view::blocks::header_subject_column("question")
}

/// One answer row, as the card lays it out — label in the name column, answer under the
/// question. Built from the same measurements the renderer uses, so it holds at every tier
/// rather than only the one the test happens to run at.
fn answer_row(answer: &str) -> String {
    let label = crate::glyphs::icon_label(Icon::Answer.glyph(), "answer");
    format!("{label}{}{answer}", " ".repeat(answer_column() - label.width()))
}

/// The modal is gone by the time anyone scrolls back, so the card is the record of the answer.
#[test]
fn an_answered_question_card_names_what_was_chosen() {
    let styles = theme();
    let answered = QuestionOutcome::Answered(vec!["tokio full".into(), "tracing".into()]);
    let item = tool_item("question", "Which async features?", Some(&answered.result_text()));

    // Never gated on expand: the decision is the whole point of the card.
    let card = body(&item, &CardContext { expanded: false, ..context(&styles, 80) });
    let rows: Vec<String> = card.lines.iter().map(plain).collect();
    assert_eq!(
        rows,
        vec![answer_row("tokio full"), answer_row("tracing")],
        "one labeled row per answer, on the header's columns"
    );
    assert!(card.flush, "the rows carry their own alignment, so the card must not inset them");
    assert_eq!(
        card.lines[0].spans.last().expect("the answer itself").style.fg,
        styles.accent.fg,
        "the reader's own words, in the color a user turn takes"
    );

    // While it is still waiting there is nothing to record — the modal is on screen saying it.
    let mut waiting = tool_item("question", "Which async features?", None);
    waiting.activity.state = luxe_core::ToolActivityState::Running;
    assert!(body(&waiting, &context(&styles, 80)).lines.is_empty());
}

/// The answer row is aligned against a header whose icon comes from the *tool map*, so the two
/// have to vanish together. `Icon::Answer` losing its fallbacks is invisible at the nerd tier
/// and shears the labels apart on every terminal without the Material shapes — which is exactly
/// how this was nearly shipped.
#[test]
fn an_answer_and_its_question_draw_an_icon_at_the_same_tiers() {
    use crate::glyphs::GlyphTier;
    let question = crate::widgets::tool_icon("question");
    for tier in [GlyphTier::Nerd, GlyphTier::NerdWide, GlyphTier::Unicode, GlyphTier::Ascii] {
        assert_eq!(
            question.at(tier).is_empty(),
            Icon::Answer.at(tier).is_empty(),
            "{tier:?}: one of the card's two rows draws an icon and the other does not"
        );
    }
}

/// A long free-text answer folds under the same budget as any other body, and says so — the
/// alternative is one card growing without limit because it happens to be a single row.
#[test]
fn a_long_answer_folds_and_offers_the_expand() {
    let styles = theme();
    let answer = "keep the runtime minimal ".repeat(30);
    let outcome = QuestionOutcome::Answered(vec![answer.trim().to_string()]);
    let item = tool_item("question", "How should we scope it?", Some(&outcome.result_text()));

    let collapsed = body(&item, &CardContext { expanded: false, ..context(&styles, 40) });
    assert!(collapsed.truncated, "the footer offers the expand, so a click has to reach it");
    assert_eq!(collapsed.lines.len(), CARD_PREVIEW_LINES + 1, "ten lines and the footer");
    let footer = plain(collapsed.lines.last().expect("a footer"));
    assert!(footer.contains("more lines") && footer.contains("to expand"), "{footer}");
    assert!(
        footer.starts_with(crate::view::blocks::BODY_INDENT),
        "the footer is card chrome, so it sits where every card's does: {footer:?}"
    );

    let expanded = body(&item, &context(&styles, 40)).lines;
    assert!(expanded.len() > CARD_PREVIEW_LINES, "opening it shows the whole answer");
    // Every row trimmed and rejoined is the label and the answer, whole — the wrap moved the
    // words to new rows without dropping or duplicating any of them.
    let rejoined = expanded.iter().map(plain).collect::<Vec<_>>().join(" ");
    let words = rejoined.split_whitespace().collect::<Vec<_>>().join(" ");
    let label = crate::glyphs::icon_label(Icon::Answer.glyph(), "answer");
    assert_eq!(words, format!("{label} {}", answer.trim()), "the whole answer, in order");
    // Continuations keep the text column rather than stepping back to the margin.
    let second = plain(&expanded[1]);
    assert_eq!(
        second.len() - second.trim_start().len(),
        answer_column(),
        "a wrapped answer stays one block: {second:?}"
    );
}

/// The two endings with no answer in them still have something to say, and a dismissal is not
/// the reader's choice however green the band is.
#[test]
fn an_unanswered_question_card_says_which_ending_it_was() {
    let styles = theme();
    let rendered = |outcome: &QuestionOutcome| {
        let item = tool_item("question", "Which runtime?", Some(&outcome.result_text()));
        body(&item, &context(&styles, 80)).lines.iter().map(plain).collect::<Vec<_>>()
    };

    // No `answer` label on an ending nobody chose: the note sits under the question it is
    // about, and the label column stays empty because there is no answer to put in it.
    let note = |text: &str| format!("{}{text}", " ".repeat(answer_column()));
    assert_eq!(rendered(&QuestionOutcome::Dismissed), vec![note("dismissed without answering")]);
    assert_eq!(rendered(&QuestionOutcome::Empty), vec![note("submitted with no answer")]);

    let unattended =
        QuestionOutcome::Unattended { prompt: "Which runtime?".into(), answer: "tokio".into() };
    assert_eq!(
        rendered(&unattended),
        vec![note("nobody to ask · answered from the default"), answer_row("tokio")],
        "an answer nobody gave must not be credited to the reader"
    );

    // Anything this cannot read is left alone rather than guessed at.
    let item = tool_item("question", "?", Some("Error: 'prompt' argument is required"));
    assert!(body(&item, &context(&styles, 80)).lines.is_empty());
}

/// Context wide enough to wrap used to spend the whole budget, leaving a card that showed
/// everything around an edit except the edit. The window opens on the change instead.
#[test]
fn a_collapsed_edit_opens_on_the_change_rather_than_the_context_above_it() {
    let styles = theme();
    let scenery = "    let value = compute(alpha, beta, gamma, delta, epsilon, zeta, eta);";
    let output = [
        "Applied 1 edit(s) to 'src/lib.rs'".to_string(),
        String::new(),
        "   ...".to_string(),
        format!(" 1 {scenery}"),
        format!(" 2 {scenery}"),
        format!(" 3 {scenery}"),
        "-4     old();".to_string(),
        "+4     new();".to_string(),
        " 5 done();".to_string(),
    ]
    .join("\n");
    let item = tool_item("edit", "src/lib.rs", Some(&output));

    let collapsed = body(&item, &CardContext { expanded: false, ..context(&styles, 30) });
    let rows: Vec<String> = collapsed.lines.iter().map(plain).collect();
    assert_eq!(rows[0].trim(), "⋯", "the window says it did not start at the top: {rows:?}");
    assert!(rows[1].contains("old();"), "the change leads the body: {rows:?}");
    assert!(rows[2].contains("new();"), "{rows:?}");
    assert!(collapsed.truncated, "and a click still reaches the rest: {rows:?}");
    let footer = rows.last().expect("a footer");
    assert!(footer.contains("to expand"), "{footer}");

    // The count covers what the window opened past as well as what it stopped short of.
    let opened: Vec<String> = body(&item, &context(&styles, 30)).lines.iter().map(plain).collect();
    assert_eq!(opened[1].trim(), "1     let value = compute(alp", "{opened:?}");
    assert!(!opened.iter().any(|row| row.contains("to expand")), "{opened:?}");
}

/// A change the budget already reaches leaves the window where it was: the context above an
/// edit is worth showing, and moving the window would cost it for nothing.
#[test]
fn a_change_within_reach_keeps_the_top_of_the_diff() {
    let styles = theme();
    let item = tool_item("edit", "src/lib.rs", Some(EDIT_OUTPUT));
    let collapsed = body(&item, &CardContext { expanded: false, ..context(&styles, 80) });
    let rows: Vec<String> = collapsed.lines.iter().map(plain).collect();

    assert_eq!(rows[0].trim(), "⋯", "the preview's own elision, not the window's");
    assert_eq!(rows[1], " 11 fn main() {", "{rows:?}");
    assert!(!collapsed.truncated, "it all fits, so there is nothing to expand: {rows:?}");
}

/// No body row is ever wider than the card it was laid out for, whatever is in it.
///
/// A card owns its own wrapping, and it has to own all of it: the transcript wraps whatever
/// overflows at a *word* boundary, which drops the leading whitespace and puts the tail back at
/// the left margin with no gutter under it — a side-by-side diff's right column reappearing
/// inside its left one. Swept across widths and bodies, because the two ways to lose the
/// invariant are content the card mismeasured (a tab) and a floor that let a column ask for
/// more cells than it had.
#[test]
fn no_body_row_is_ever_wider_than_the_card() {
    let styles = theme();
    let wide_glyph = "\u{65e5}\u{672c}\u{8a9e}";
    let diff = format!(
        "Applied 1 edit(s) to 'a.svelte'\n\
             \u{20}   ...\n\
             \u{20}415 \t\t\t<GettingStartedChecklist onCloseDrawer={{closeDrawer}} />\n\
             -416 \t\t\t<div class=\"shrink-0 px-2.5 pb-1 max-sm:hidden\">\n\
             -417 \t\t\t\t<button {wide_glyph} onclick={{() => start()}} type=\"button\">\n\
             +416 \t\t\t{{#if tourStore.isEnabled}}\n\
             +417 \t\t\t<!-- Hidden while tourStore.isEnabled is false (see store.svelte.ts). -->"
    );
    let written = "fn main() {\n\tlet value = compute(alpha, beta, gamma, delta);\n}";

    let mut cards: Vec<(&str, ToolItem)> = Vec::new();
    cards.push(("edit", tool_item("edit", "a.svelte", Some(&diff))));
    let mut write = tool_item("write", "a.rs", None);
    write.activity.card_body = luxe_core::tool_card_body(
        "write",
        &serde_json::json!({"path": "a.rs", "content": written}),
        "/project",
    );
    cards.push(("write", write));
    // Gutterless, and long enough to wrap at every width in the sweep: a continuation marker
    // with no gutter to hang under still costs the cells it draws.
    let mut bash = tool_item("bash", "cargo test", None);
    bash.activity.card_body = luxe_core::tool_card_body(
        "bash",
        &serde_json::json!({"command": format!(
            "cat <<'SH'\n\tindented\theredoc line\nSH\ncargo nextest run --workspace {}",
            "--ignore-default-filter ".repeat(12),
        )}),
        "/project",
    );
    cards.push(("bash", bash));
    let mut failed = tool_item("edit", "a.rs", Some("Error: no exact match for\toldText"));
    failed.activity.state = luxe_core::ToolActivityState::Failed;
    cards.push(("rejected edit", failed));
    let answered = QuestionOutcome::Answered(vec!["keep the runtime minimal".into()]);
    cards.push((
        "question",
        tool_item("question", "How should we scope it?", Some(&answered.result_text())),
    ));
    let mut grep = tool_item("grep", "\"needle\" in src/", Some("src/a.rs:1:\tlet needle = 1;"));
    grep.activity.input_preview = Some(r#"{"pattern":"needle"}"#.into());
    cards.push(("grep", grep));

    // Every width from the narrowest card that can still seat a header column up past a
    // full-screen one, so both diff layouts are covered — `auto` crosses over around 125 cells
    // — along with every off-by-one either side of the crossover.
    for width in 14..=232 {
        for (name, item) in &cards {
            for expanded in [false, true] {
                let ctx = CardContext { expanded, ..context(&styles, width) };
                for line in body(item, &ctx).lines {
                    let row = plain(&line);
                    // The footer is prose *about* the card rather than a row of it: it carries
                    // no alignment, so the transcript is welcome to wrap it like any sentence.
                    if row.contains(" total \u{b7} ") {
                        continue;
                    }
                    assert!(
                        row.width() <= usize::from(width),
                        "{name} at {width} (expanded: {expanded}) drew {} cells: {row:?}",
                        row.width()
                    );
                    assert!(!row.contains('\t'), "{name} let a tab reach a cell: {row:?}");
                }
            }
        }
    }
}

/// A rejected edit has no diff, so the card shows why instead of nothing.
#[test]
fn a_failed_edit_shows_the_tool_message_when_opened() {
    let styles = theme();
    let mut item = tool_item("edit", "src/lib.rs", Some("Error: no exact match for oldText"));
    item.activity.state = luxe_core::ToolActivityState::Failed;
    item.activity.summary = Some("no exact match for oldText".into());

    assert!(body(&item, &CardContext { expanded: false, ..context(&styles, 80) }).lines.is_empty());
    let opened = body(&item, &context(&styles, 80)).lines;
    assert_eq!(opened.len(), 0, "the only line repeated the summary and was dropped");

    item.activity.summary = Some("reworded".into());
    let kept = body(&item, &context(&styles, 80)).lines;
    assert_eq!(plain(&kept[0]), "Error: no exact match for oldText");
}
