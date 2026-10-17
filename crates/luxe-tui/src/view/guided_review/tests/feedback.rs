//! Feedback — the drawer, the window, comment cards and the finish dialog.

use super::*;

/// What reviewers already said sits under the line it is about, with the reply beside it, and
/// every thread that cannot sit on a line is listed in the brief — none dropped.
#[test]
fn earlier_threads_sit_on_their_lines_and_the_rest_are_listed() {
    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewPullRequest).expect("fixture app");
    let rows = frame_rows(&mut app, 170, 90);
    let text = rows.join("\n");
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));

    assert!(has("@mara-okafor · unresolved · 1 reply"), "{text}");
    // A card, not a bar: the header rides in the top edge and the frame closes under it
    assert!(has("╭─ @mara-okafor"), "the thread is framed: {text}");
    assert!(rows.iter().any(|row| row.contains("╰─") && row.contains("╯")), "{text}");
    assert!(has("Does anything still construct this from a raw string?"), "{text}");
    assert!(has("@copilot-swe-agent: Only the webhook"), "{text}");

    assert!(has("Discussion on GitHub"), "{text}");
    assert!(has("3 threads · 2 unresolved · 1 comment"), "{text}");
    assert!(!has("on the lines they are about"), "{text}");
    assert!(has("@mara-okafor · src/dispatch/router.rs · outdated"), "{text}");
    assert!(has("This match used to fall through"), "{text}");
    // A longer name wraps the row, and the facts carry on under it
    assert!(has("@jon-avery · src/api/webhook.rs · whole file"), "{text}");
    assert!(has("resolved"), "{text}");
    assert!(has("@jon-avery · in the conversation"), "{text}");

    // The same thread in the unified layout, which draws its rows on its own path
    app.guided_review_workspace.layout = luxe_core::config::DiffLayout::Unified;
    let unified = frame_rows(&mut app, 170, 90);
    assert!(
        unified.iter().any(|row| row.contains("Does anything still construct this")),
        "{}",
        unified.join("\n")
    );
}

#[test]
fn the_feedback_drawer_explains_the_empty_review_state() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    app.guided_review_workspace.findings_visible = true;
    let backend = TestBackend::new(160, 32);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render findings drawer");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("Feedback"), "the drawer names itself: {text}");
    assert!(text.contains("No feedback yet"));
    // Both verbs, because a question and a comment are the same artifact here.
    assert!(text.contains("c comments on it"));
    assert!(text.contains("a asks the agent about it"));
    assert!(text.contains("esc close"));
}

/// The window reads the comment under the cursor in full, as markdown, and a long one scrolls
/// in its own pane while the list stays put. Sending opens the same window with checkboxes.
#[test]
fn the_feedback_window_reads_a_long_comment_in_full() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    let body = format!(
        "## Why this matters\n\n{}\n\nThe **last** line.",
        (1..=60).map(|n| format!("- point {n}")).collect::<Vec<_>>().join("\n")
    );
    let session = app.review.session_mut().expect("session");
    session.findings[0].body = body;
    let first = session.findings[0].id.clone();
    app.guided_review_workspace.selected_finding_id = Some(first);
    app.guided_review_workspace.findings_visible = true;
    let rows = frame_rows(&mut app, 160, 44);
    let text = rows.join("\n");
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("Why this matters") && !has("## Why"), "drawn as markdown: {text}");
    assert!(has("point 1") && !has("The last line."), "{text}");
    assert!(has("pgup/pgdn scroll"), "{text}");
    assert!(has("Confirm the migration") && has("to verify"), "the list stays beside it: {text}");

    app.update(crate::app::Msg::Key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::PageDown,
        crossterm::event::KeyModifiers::NONE,
    )));
    for _ in 0..10 {
        app.guided_review_workspace.scroll_feedback(10);
    }
    let rows = frame_rows(&mut app, 160, 44);
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("The last line."), "{}", rows.join("\n"));
    assert!(has("Confirm the migration"), "the list does not scroll with it");

    // Moving to another comment reads it from the top
    app.update(crate::app::Msg::Key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Down,
        crossterm::event::KeyModifiers::NONE,
    )));
    assert_eq!(app.guided_review_workspace.feedback_scroll, 0);
    // A plain key the window does not use moves nothing behind it
    let before = app.guided_review_workspace.position.clone();
    app.update(crate::app::Msg::Key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char(' '),
        crossterm::event::KeyModifiers::NONE,
    )));
    assert_eq!(app.guided_review_workspace.position, before);

    app.guided_review_workspace.findings_visible = false;
    app.update(crate::app::Msg::GuidedReviewOpenSendPreview);
    let rows = frame_rows(&mut app, 160, 44);
    let text = rows.join("\n");
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("Send to agent") && has("Send these comments to the agent"), "{text}");
    assert!(has("2 of 2 included"), "{text}");
    assert!(has("enter send · space include · ↑/↓ choose"), "{text}");
}

#[test]
fn the_feedback_drawer_orders_actionable_comments_before_finished_ones() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let files = app.guided_review.as_ref().expect("fixture review").files.clone();
    for (title, state, severity) in [
        ("Stale anchor", FindingState::Stale, FindingSeverity::Note),
        ("Resolved concern", FindingState::Resolved, FindingSeverity::Issue),
        ("Draft thought", FindingState::Draft, FindingSeverity::Suggestion),
        ("Verify remediation", FindingState::NeedsVerification, FindingSeverity::Issue),
        ("Open concern", FindingState::Open, FindingSeverity::Issue),
        ("Blocking concern", FindingState::Open, FindingSeverity::Blocking),
    ] {
        app.guided_review_workspace.begin_finding_draft(&files);
        let mut finding = {
            let draft = app.guided_review_workspace.finding_draft.as_mut().expect("draft");
            draft.set_body(format!("{title}. Fixture feedback, kept for visual triage."));
            draft.severity = severity;
            draft.to_review_finding(FindingId::new()).expect("valid finding")
        };
        finding.state = state;
        app.review
            .session_mut()
            .expect("review session")
            .add_finding(finding)
            .expect("add finding");
        app.guided_review_workspace.discard_finding_draft();
    }
    app.guided_review_workspace.findings_visible = true;
    let backend = TestBackend::new(160, 32);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render findings drawer");
    // The list's own columns, the left of the window: the pane beside it reads the selected
    // comment in full, which would put its title on screen a second time.
    let buffer = terminal.backend().buffer().clone();
    let text: String = (0..32u16)
        .map(|y| (5..60u16).map(|x| buffer[(x, y)].symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    let position = |title: &str| text.find(title).expect("a row in the drawer");

    assert!(position("Blocking concern") < position("Open concern"));
    assert!(position("Open concern") < position("Verify remediation"));
    assert!(position("Verify remediation") < position("Draft thought"));
    assert!(position("Draft thought") < position("Resolved concern"));
    assert!(position("Resolved concern") < position("Stale anchor"));
}

/// The drawer's rows are cut from the same slab as every other list, so the one under the
/// cursor has to keep its neighbors' column. The rows arrive laid out, and a paragraph that
/// trims leading whitespace eats the frame off exactly the rows that are *not* selected —
/// which is the sideways shift the frame is spent to prevent.
#[test]
fn findings_drawer_holds_one_column_under_the_cursor() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture");
    app.guided_review_workspace.findings_visible = true;
    let backend = TestBackend::new(160, 32);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render findings drawer");
    let buffer = terminal.backend().buffer().clone();

    // Where each title starts is where its row's text starts
    let column_of = |label: &str| -> Option<u16> {
        (0..buffer.area.height).find_map(|y| {
            let row: String = (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect();
            row.find(label).map(|byte| unicode_width::UnicodeWidthStr::width(&row[..byte]) as u16)
        })
    };
    let columns: Vec<u16> = ["Retry path omits", "Confirm the migration", "Clarify the cancel"]
        .iter()
        .filter_map(|title| column_of(title))
        .collect();
    assert_eq!(columns.len(), 3, "the fixture carries three findings to compare");
    assert!(
        columns.windows(2).all(|pair| pair[0] == pair[1]),
        "the chosen row shares its column with the rest: {columns:?}"
    );
}

/// A comment renders under the line it is about, with what happens to it next.
///
/// It used to be a bare `R1` in a gutter, and reading your own words meant a drawer over the
/// far corner and a second overlay on top of that — which is why "where did my comment go" was
/// a question at all.
#[test]
fn a_comment_renders_under_the_line_it_is_anchored_to() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let files = app.guided_review.as_ref().expect("fixture review").files.clone();
    // Offset zero is the hunk header; the first source row has stable old/new line anchors.
    app.guided_review_workspace.position.line_offset = 1;
    app.guided_review_workspace.begin_finding_draft(&files);
    let finding = {
        let draft = app.guided_review_workspace.finding_draft.as_mut().expect("finding draft");
        draft.set_body("This line needs a compatibility check.");
        draft.to_review_finding(FindingId::new()).expect("valid finding")
    };
    app.review.session_mut().expect("review session").add_finding(finding).expect("add finding");
    app.guided_review_workspace.discard_finding_draft();

    let backend = TestBackend::new(160, 32);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render line marker");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(!text.contains("R1"), "a comment carries no label: {text}");
    assert!(
        text.contains("This line needs a compatibility check."),
        "and says what it says, in the diff, not two overlays away: {text}"
    );
    assert!(
        text.contains("unsent"),
        "and what happens to it next, which a drawer never said: {text}"
    );
}

/// A question the agent is answering spins on its own card, where the reader just wrote it,
/// and stops spinning once its turn is over.
#[test]
fn a_question_being_answered_says_so_on_its_card() {
    use luxe_core::guided_review::{
        FindingSeverity, FindingState, ReviewFeedbackMode, SubmissionDispatch, SubmissionId,
    };
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    let session = app.review.session_mut().expect("review session");
    session.findings[0].severity = FindingSeverity::Question;
    session.findings[0].state = FindingState::Open;
    let asked = session.findings[0].id.clone();
    let submission = SubmissionId::new();
    session
        .queue_submission(
            submission.clone(),
            ReviewFeedbackMode::Question,
            std::slice::from_ref(&asked),
            &[],
            0,
        )
        .expect("queued");
    session.dispatch_submission(&submission).expect("dispatched");
    let card = |app: &crate::app::App| {
        let finding = app.review.session().expect("session").findings[0].clone();
        super::diff_render::comment_lines(app, &[finding], 100)
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let text = card(&app);
    assert!(text.contains("the agent is answering…"), "{text}");

    let session = app.review.session_mut().expect("review session");
    for sent in &mut session.submissions {
        sent.dispatch = SubmissionDispatch::Completed;
    }
    let text = card(&app);
    assert!(!text.contains("answering"), "{text}");
    assert!(text.contains("waiting on the agent"), "{text}");
}

/// A comment the agent answered without fixing, `not fixed` or a question back, is the
/// reader's to read, and its card says so rather than that the agent is still on it.
#[test]
fn a_comment_the_agent_replied_to_says_so_on_its_card() {
    use luxe_core::guided_review::{
        FindingMessage, FindingMessageAuthor, FindingState, ReviewFeedbackMode, SubmissionDispatch,
        SubmissionId,
    };
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    let session = app.review.session_mut().expect("review session");
    session.findings[0].state = FindingState::Open;
    session.findings[0].thread.push(FindingMessage {
        author: FindingMessageAuthor::Agent,
        body: "Agent reported: not fixed. The caller owns that timeout.".into(),
        created_at_ms: 1,
    });
    let sent = SubmissionId::new();
    let ids = [session.findings[0].id.clone()];
    session
        .queue_submission(sent.clone(), ReviewFeedbackMode::RequestFixes, &ids, &[], 0)
        .expect("queued");
    let card = |app: &crate::app::App| {
        let finding = app.review.session().expect("session").findings[0].clone();
        super::diff_render::comment_lines(app, &[finding], 100)
            .iter()
            .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };
    // sent again and still queued: back with the agent, whatever it said last time
    assert!(card(&app).contains("waiting on the agent"), "{}", card(&app));
    app.review.session_mut().expect("session").submissions[0].dispatch =
        SubmissionDispatch::Completed;
    let text = card(&app);
    assert!(text.contains("the agent replied · alt+c reads it"), "{text}");
    assert!(text.contains("agent: Agent reported: not fixed."), "{text}");
}

/// A comment a fix turn ended with no word on says so on its card, with Luxe's reason under
/// it, rather than still claiming the agent is on it.
#[test]
fn a_comment_with_no_word_from_the_agent_says_so_on_its_card() {
    use luxe_core::guided_review::{FindingMessage, FindingMessageAuthor, FindingState};
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    let session = app.review.session_mut().expect("review session");
    session.findings[0].state = FindingState::Open;
    session.findings[0].thread.push(FindingMessage {
        author: FindingMessageAuthor::System,
        body: "No word from the agent on this comment: it wrote no report. Refresh the review \
                   to see the code as it is now."
            .into(),
        created_at_ms: 1,
    });
    let finding = session.findings[0].clone();
    let text = super::diff_render::comment_lines(&app, &[finding], 100)
        .iter()
        .map(|line| line.spans.iter().map(|span| span.content.as_ref()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("no word from the agent · alt+c says why"), "{text}");
    assert!(!text.contains("waiting on the agent"), "{text}");
    assert!(text.contains("luxe: No word from the agent on this comment: it wrote no report."));
}

/// A Suggestion used to be sendable by neither Request fixes nor Comment only, so a review
/// made entirely of suggestions was write-only. Severity is priority now, never routing.
#[test]
fn finish_review_sends_suggestions_like_any_other_comment() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    app.review
        .session_mut()
        .expect("review session")
        .findings
        .retain(|finding| finding.severity == FindingSeverity::Suggestion);
    app.guided_review_workspace.open_finish_review(true);

    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render finish review");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("Sends 1 comment"));
    assert!(!text.contains("No comments to send"));
}

#[test]
fn finish_review_identifies_when_no_feedback_can_be_submitted() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    app.guided_review_workspace.open_finish_review(true);

    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render finish review");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("No comments to send"));
    assert!(text.contains("add one with c."));
}

#[test]
fn header_shows_feedback_snapshot_before_persistence_confirms() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    app.review.edits.insert(
        luxe_core::guided_review::ReviewSessionSaveRequestId::new(),
        crate::app::ReviewEditPurpose::Other,
    );
    app.review.edits.insert(
        luxe_core::guided_review::ReviewSessionSaveRequestId::new(),
        crate::app::ReviewEditPurpose::Send,
    );

    let backend = TestBackend::new(160, 32);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render pending snapshot");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("Saving feedback snapshot"));
    assert!(!text.contains("Saving review"));
}

#[test]
fn finish_review_uses_the_light_theme_surface_for_inactive_choices() {
    use ratatui::{backend::TestBackend, Terminal};

    // No built-in is light any more, so the light palette a reader's own theme file could be
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFinish).expect("fixture app");
    app.theme_styles = crate::fixture::light_theme_styles();
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render finish review");

    // Locate an inactive choice row by its label rather than a fixed cell, so adding a choice
    // does not silently move the sample onto a different line.
    let buffer = terminal.backend().buffer();
    let row = (0..28u16)
        .find(|&y| {
            (0..100u16)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .contains("Comment only")
        })
        .expect("the inactive Comment only row renders");
    let column = (0..100u16)
        .find(|&x| buffer[(x, row)].symbol() == "C")
        .expect("the label starts somewhere on the row");
    let cell = &buffer[(column, row)];
    assert_eq!(cell.bg, app.theme_styles.tool_bg.bg.expect("light tool surface color"));
    assert_eq!(cell.fg, app.theme_styles.text.fg.expect("light text color"));
}

/// The dialog that submits work is the worst place for the list to move. Its options are the
/// same slab every other list is cut from, so they share one column and sit on consecutive
/// rows — a chosen row wide enough to wrap opens a gap under itself and shifts the rest down.
#[test]
fn finish_review_options_share_a_column_and_a_row_each() {
    use ratatui::{backend::TestBackend, Terminal};

    let app = seeded_app("nord", FixtureScenario::GuidedReviewFinish).expect("fixture app");
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render finish review");
    let buffer = terminal.backend().buffer().clone();

    let placement = |label: &str| -> (u16, u16) {
        (0..buffer.area.height)
            .find_map(|y| {
                let row: String = (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect();
                row.find(label)
                    .map(|byte| (unicode_width::UnicodeWidthStr::width(&row[..byte]) as u16, y))
            })
            .unwrap_or_else(|| panic!("the {label} option renders"))
    };
    let options =
        ["Accept ", "Accept and commit", "Request fixes", "Comment only", "Keep reviewing"];
    let placed: Vec<(u16, u16)> = options.iter().map(|label| placement(label)).collect();

    assert!(
        placed.windows(2).all(|pair| pair[0].0 == pair[1].0),
        "one label column, chosen or not: {placed:?}"
    );
    assert!(
        placed.windows(2).all(|pair| pair[1].1 == pair[0].1 + 1),
        "one row each, so nothing shifts as the cursor moves: {placed:?}"
    );
}

#[test]
fn finish_review_explains_pinned_submission_snapshots() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    app.guided_review_workspace.open_finish_review(true);

    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render finish review");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("No comments to send"));
    assert!(text.contains("Request fixes"));
    assert!(text.contains("Request fixes"));
}

#[test]
fn draft_finding_details_only_advertise_draft_mutations() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    let finding = app
        .review
        .session()
        .expect("review session")
        .findings
        .iter()
        .find(|finding| finding.state == FindingState::Draft)
        .expect("draft finding");
    app.guided_review_workspace.selected_finding_id = Some(finding.id.clone());
    app.guided_review_workspace.finding_details_visible = true;

    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render draft evidence");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("e edit · x delete · esc close"));
    assert!(!text.contains("r resolve/reopen"));
}

#[test]
fn needs_verification_requires_explicit_verification_before_resolution() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    let finding = app
        .review
        .session()
        .expect("review session")
        .findings
        .iter()
        .find(|finding| finding.state == FindingState::NeedsVerification)
        .expect("verification finding");
    app.guided_review_workspace.selected_finding_id = Some(finding.id.clone());
    app.guided_review_workspace.finding_details_visible = true;

    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render verification finding");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("r verify and resolve"));
    assert!(!text.contains("r resolve · esc close"));
}

#[test]
fn finding_details_render_the_whole_thread_attributed_to_each_author() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    let finding = app
        .review
        .session_mut()
        .expect("review session")
        .findings
        .first_mut()
        .expect("fixture finding");
    finding.thread.push(luxe_core::guided_review::FindingMessage {
        author: luxe_core::guided_review::FindingMessageAuthor::Agent,
        body: "Agent reported: fixed the retry cap.\nRan focused tests.\nA longer audit trail remains durable.".into(),
        created_at_ms: 42,
    });
    finding.thread.push(luxe_core::guided_review::FindingMessage {
        author: luxe_core::guided_review::FindingMessageAuthor::Reviewer,
        body: "The cap is right, but the caller still ignores the error.".into(),
        created_at_ms: 43,
    });
    app.guided_review_workspace.selected_finding_id = Some(finding.id.clone());
    app.guided_review_workspace.finding_details_visible = true;

    let backend = TestBackend::new(120, 32);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render evidence");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("an agent report is a claim; reviewer verification required"));
    // Both halves of the exchange, attributed and in order.
    assert!(text.contains("Agent:"));
    assert!(text.contains("Agent reported: fixed the retry cap."));
    assert!(text.contains("Ran focused tests."));
    assert!(text.contains("Reviewer:"));
    assert!(text.contains("The cap is right, but the caller still ignores the error."));
    let agent_at = text.find("Agent:").expect("the agent speaks");
    let reviewer_at = text.find("Reviewer:").expect("the reviewer replies");
    assert!(agent_at < reviewer_at, "the thread reads in chronological order");
    // Long bodies stay elided so one message cannot crowd out the rest of the thread.
    assert!(!text.contains("A longer audit trail remains durable."));
}

#[test]
fn stale_finding_drawer_only_advertises_reanchoring() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    let finding = app
        .review
        .session_mut()
        .expect("review session")
        .findings
        .first_mut()
        .expect("fixture finding");
    finding.state = FindingState::Stale;
    app.guided_review_workspace.selected_finding_id = Some(finding.id.clone());
    app.guided_review_workspace.findings_visible = true;

    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render stale drawer");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("alt+r re-anchor and verify"));
    assert!(!text.contains("r resolve/reopen"));
    assert!(!text.contains("x delete draft"));
}

#[test]
fn edit_draft_overlay_preserves_submitted_feedback_immutability() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    let finding = app
        .review
        .session()
        .expect("review session")
        .findings
        .iter()
        .find(|finding| finding.state == FindingState::Draft)
        .expect("draft finding")
        .clone();
    assert!(app.guided_review_workspace.begin_edit_finding_draft(&finding));

    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render edit draft");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("Edit comment"));
    assert!(text.contains("submitted feedback stays immutable"));
}

#[test]
fn stale_finding_details_expose_the_reanchor_shortcut() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    let finding = app
        .review
        .session_mut()
        .expect("review session")
        .findings
        .first_mut()
        .expect("fixture finding");
    finding.state = FindingState::Stale;
    app.guided_review_workspace.selected_finding_id = Some(finding.id.clone());
    app.guided_review_workspace.finding_details_visible = true;

    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render stale details");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("Pinned revision:"));
    assert!(text.contains("alt+r re-anchor and verify"));
}

#[test]
fn submitted_finding_details_name_the_immutable_snapshot_revision() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    let finding_id = app
        .review
        .session()
        .expect("review session")
        .findings
        .first()
        .expect("fixture finding")
        .id
        .clone();
    app.review
        .session_mut()
        .expect("review session")
        .queue_submission(
            luxe_core::guided_review::SubmissionId::new(),
            luxe_core::guided_review::ReviewFeedbackMode::RequestFixes,
            std::slice::from_ref(&finding_id),
            &[],
            0,
        )
        .expect("queue submission");
    app.guided_review_workspace.selected_finding_id = Some(finding_id);
    app.guided_review_workspace.finding_details_visible = true;

    let backend = TestBackend::new(120, 30);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render submitted details");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("Submitted snapshot (queued):"));
    assert!(!text.contains("Pinned revision:"));
}

#[test]
fn the_guide_foot_says_what_the_feedback_wants() {
    use ratatui::{backend::TestBackend, Terminal};

    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let files = app.guided_review.as_ref().expect("fixture review").files.clone();
    app.guided_review_workspace.begin_finding_draft(&files);
    let finding = {
        let draft = app.guided_review_workspace.finding_draft.as_mut().expect("finding draft");
        draft.set_body("The retry path should surface a bounded error.");
        draft.severity = FindingSeverity::Blocking;
        draft.to_review_finding(FindingId::new()).expect("valid finding")
    };
    app.review.session_mut().expect("review session").add_finding(finding).expect("add finding");
    app.guided_review_workspace.discard_finding_draft();

    let backend = TestBackend::new(160, 32);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal.draw(|frame| render(frame, frame.area(), &app)).expect("render findings header");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    // Unsent leads over severity: a count that does not say none of it has left is the wrong
    // fact to put in front of someone. The key that sends it is the footer's.
    assert!(text.contains("FEEDBACK") && text.contains(" 1 · 1 unsent"), "{text}");
    assert!(!text.contains("alt+u"), "{text}");
}
