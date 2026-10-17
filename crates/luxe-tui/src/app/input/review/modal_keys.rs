//! Every key a review window names in its hint, pressed.
//!
//! A hint on screen is a promise, and the tests beside each window mostly send it the message a
//! key would, which proves the message and not the key. These press the keys, and the modifier
//! each guard exists to refuse.

use crate::app::{App, ConvoBlock, Msg};
use crate::fixture::{seeded_app, FixtureScenario};
use crate::guided_review::FinishReviewChoice;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use luxe_core::engine::Action;
use luxe_core::guided_review::{FindingSeverity, FindingState, ReviewOutcome};
use tokio::sync::mpsc;

fn app(scenario: FixtureScenario) -> App {
    seeded_app("nord", scenario).expect("fixture app")
}

/// A fixture app whose engine is listening, for keys whose first act is to ask it something.
fn listened(scenario: FixtureScenario) -> (App, mpsc::Receiver<Action>) {
    let mut app = app(scenario);
    let (engine_tx, engine_rx) = mpsc::channel(16);
    app.engine_tx = engine_tx;
    (app, engine_rx)
}

fn press(app: &mut App, code: KeyCode) {
    app.update(Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

fn press_with(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    app.update(Msg::Key(KeyEvent::new(code, modifiers)));
}

/// What the app has said in the transcript.
fn said(app: &App) -> Vec<String> {
    app.convo
        .iter()
        .filter_map(|item| match item.block() {
            ConvoBlock::System(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// The review, with its finish dialog open on `choice` and the engine's inbox to read.
fn finishing(choice: FinishReviewChoice) -> (App, mpsc::Receiver<Action>) {
    let mut app = app(FixtureScenario::GuidedReviewFindings);
    app.guided_review_workspace.findings_visible = false;
    app.guided_review_workspace.open_finish_review(true);
    app.guided_review_workspace.finish_review_choice = Some(choice);
    let (engine_tx, engine_rx) = mpsc::channel(16);
    app.engine_tx = engine_tx;
    (app, engine_rx)
}

/// The outcome the last review edit asked to record, if one did.
fn recorded(engine_rx: &mut mpsc::Receiver<Action>) -> Option<ReviewOutcome> {
    use luxe_core::engine::ReviewEdit;
    let mut outcome = None;
    while let Ok(action) = engine_rx.try_recv() {
        match action {
            Action::EditReview { edit: ReviewEdit::SetOutcome { outcome: asked }, .. }
            | Action::EditReview { edit: ReviewEdit::Send { outcome: Some(asked), .. }, .. } => {
                outcome = Some(asked);
            }
            _ => {}
        }
    }
    outcome
}

fn set_findings(app: &mut App, edit: impl Fn(&mut luxe_core::guided_review::ReviewFinding)) {
    for finding in &mut app.review.session_mut().expect("session").findings {
        edit(finding);
    }
}

// === Finish ===

#[test]
fn the_finish_dialog_moves_and_closes_on_plain_keys_only() {
    let (mut app, _rx) = finishing(FinishReviewChoice::RequestFixes);
    let choice = |app: &App| app.guided_review_workspace.finish_review_choice;
    press_with(&mut app, KeyCode::Down, KeyModifiers::ALT);
    assert_eq!(choice(&app), Some(FinishReviewChoice::RequestFixes), "alt+↓ is not ↓");
    press(&mut app, KeyCode::Down);
    assert_eq!(choice(&app), Some(FinishReviewChoice::CommentOnly));
    press_with(&mut app, KeyCode::Up, KeyModifiers::ALT);
    assert_eq!(choice(&app), Some(FinishReviewChoice::CommentOnly), "alt+↑ is not ↑");
    press(&mut app, KeyCode::Up);
    assert_eq!(choice(&app), Some(FinishReviewChoice::RequestFixes));
    press_with(&mut app, KeyCode::Esc, KeyModifiers::SHIFT);
    assert!(choice(&app).is_some(), "shift+esc is not Esc");
    press(&mut app, KeyCode::Esc);
    assert!(choice(&app).is_none());
}

#[test]
fn keep_reviewing_closes_and_records_nothing() {
    let (mut app, mut rx) = finishing(FinishReviewChoice::Cancel);
    press(&mut app, KeyCode::Enter);
    assert!(app.guided_review_workspace.finish_review_choice.is_none());
    assert_eq!(recorded(&mut rx), None);
}

#[test]
fn accept_and_commit_records_then_shows_the_commits() {
    let (mut app, mut rx) = finishing(FinishReviewChoice::AcceptAndCommit);
    press(&mut app, KeyCode::Enter);
    assert!(app.guided_review_workspace.commit_preview.is_some());
    assert_eq!(recorded(&mut rx), Some(ReviewOutcome::Accepted));
}

/// Fixes need a comment that asks for work; with one, the send preview opens to record them.
#[test]
fn request_fixes_needs_something_to_fix() {
    let (mut app, _rx) = finishing(FinishReviewChoice::RequestFixes);
    set_findings(&mut app, |finding| finding.severity = FindingSeverity::Note);
    press(&mut app, KeyCode::Enter);
    assert!(app.guided_review_workspace.send_preview.is_none());
    assert!(said(&app).iter().any(|text| text.contains("before requesting fixes")));

    let (mut app, _rx) = finishing(FinishReviewChoice::RequestFixes);
    press(&mut app, KeyCode::Enter);
    let preview = app.guided_review_workspace.send_preview.as_ref().expect("preview");
    assert_eq!(preview.outcome, Some(ReviewOutcome::ChangesRequested));
}

#[test]
fn comment_only_answers_enter_and_not_alt_enter() {
    let (mut app, _rx) = finishing(FinishReviewChoice::CommentOnly);
    press_with(&mut app, KeyCode::Enter, KeyModifiers::ALT);
    assert!(app.guided_review_workspace.send_preview.is_none(), "alt+enter is not Enter");
    press(&mut app, KeyCode::Enter);
    let preview = app.guided_review_workspace.send_preview.as_ref().expect("preview");
    assert_eq!(preview.outcome, Some(ReviewOutcome::Commented));
}

/// Accepting over a blocking comment asks twice; over anything milder it accepts at once. Either
/// way it says so when comments or chapters are still open.
#[test]
fn accepting_confirms_only_over_a_blocking_comment() {
    let (mut app, mut rx) = finishing(FinishReviewChoice::Accept);
    press(&mut app, KeyCode::Enter);
    assert!(app.guided_review_workspace.finish_review_accept_confirmed, "asks first");
    assert_eq!(recorded(&mut rx), None);
    press(&mut app, KeyCode::Enter);
    assert_eq!(recorded(&mut rx), Some(ReviewOutcome::Accepted), "then accepts");

    let (mut app, mut rx) = finishing(FinishReviewChoice::Accept);
    set_findings(&mut app, |finding| finding.severity = FindingSeverity::Issue);
    press(&mut app, KeyCode::Enter);
    assert_eq!(recorded(&mut rx), Some(ReviewOutcome::Accepted), "at once");
    assert!(said(&app).iter().any(|text| text.contains("Recording acceptance")));
}

/// Nothing open and every chapter walked is a clean accept; an unwalked chapter that needs
/// review is worth a word.
#[test]
fn accepting_names_chapters_not_yet_reviewed() {
    let settled = |app: &mut App| {
        set_findings(app, |finding| finding.state = FindingState::Resolved);
    };
    let (mut app, _rx) = finishing(FinishReviewChoice::Accept);
    settled(&mut app);
    let chapters: Vec<_> =
        app.guided_review.as_ref().expect("review").chapters.iter().map(|c| c.id.clone()).collect();
    app.guided_review_workspace.walked_chapters.extend(chapters);
    press(&mut app, KeyCode::Enter);
    assert!(!said(&app).iter().any(|text| text.contains("Recording acceptance")), "clean");

    let (mut app, _rx) = finishing(FinishReviewChoice::Accept);
    settled(&mut app);
    app.guided_review_workspace.walked_chapters.clear();
    press(&mut app, KeyCode::Enter);
    assert!(said(&app).iter().any(|text| text.contains("Recording acceptance")), "unwalked");
}

// === The other windows ===

#[test]
fn the_stack_preview_toggles_draft_opens_and_cancels() {
    let mut app = app(FixtureScenario::GuidedReviewOpenStack);
    let draft = |app: &App| app.guided_review_workspace.stack_opening.as_ref().map(|s| s.draft);
    press_with(&mut app, KeyCode::Char('d'), KeyModifiers::ALT);
    assert_eq!(draft(&app), Some(false), "alt+d is not d");
    press(&mut app, KeyCode::Char('d'));
    assert_eq!(draft(&app), Some(true));
    press(&mut app, KeyCode::Esc);
    assert!(app.guided_review_workspace.stack_opening.is_none());
}

#[test]
fn the_pull_request_preview_edits_scrolls_and_toggles_draft() {
    let mut app = app(FixtureScenario::GuidedReviewOpenPullRequest);
    let opening = |app: &App| app.guided_review_workspace.pull_request_opening.clone().unwrap();
    app.guided_review_workspace.pull_request_opening.as_ref().unwrap().body_room.set(40);
    press(&mut app, KeyCode::Down);
    assert_eq!(opening(&app).body_scroll, 1);
    press(&mut app, KeyCode::PageDown);
    assert_eq!(opening(&app).body_scroll, 11);
    press(&mut app, KeyCode::PageUp);
    assert_eq!(opening(&app).body_scroll, 1);
    press(&mut app, KeyCode::Up);
    assert_eq!(opening(&app).body_scroll, 0);

    let drafted = |app: &App| opening(app).prepared.map(|p| p.pull_request.draft);
    let before = drafted(&app);
    press_with(&mut app, KeyCode::Char('d'), KeyModifiers::ALT);
    assert_eq!(drafted(&app), before, "alt+d is not d");
    press(&mut app, KeyCode::Char('d'));
    assert_ne!(drafted(&app), before);

    let editing = |app: &App| opening(app).editing.map(|(field, _)| field);
    press_with(&mut app, KeyCode::Char('t'), KeyModifiers::ALT);
    assert_eq!(editing(&app), None, "alt+t is not t");
    press(&mut app, KeyCode::Char('t'));
    assert_eq!(editing(&app), Some(crate::guided_review::OpeningField::Title));
    press(&mut app, KeyCode::Esc);
    press_with(&mut app, KeyCode::Char('e'), KeyModifiers::ALT);
    assert_eq!(editing(&app), None, "alt+e is not e");
    press(&mut app, KeyCode::Char('e'));
    assert_eq!(editing(&app), Some(crate::guided_review::OpeningField::Body));
}

#[test]
fn the_answers_preview_moves_toggles_and_replies() {
    let mut app = app(FixtureScenario::GuidedReviewOwnPullRequest);
    let answers = |app: &App| app.guided_review_workspace.thread_answers.clone().unwrap();
    press(&mut app, KeyCode::Down);
    assert_eq!(answers(&app).selected, 1);
    let resolve = answers(&app).rows[1].resolve;
    press(&mut app, KeyCode::Char(' '));
    assert_ne!(answers(&app).rows[1].resolve, resolve);
    press_with(&mut app, KeyCode::Char('c'), KeyModifiers::ALT);
    assert!(answers(&app).replying.is_none(), "alt+c is not c");
    press(&mut app, KeyCode::Char('c'));
    assert!(answers(&app).replying.is_some());
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Esc);
    assert!(app.guided_review_workspace.thread_answers.is_none());
}

#[test]
fn the_post_preview_moves_its_verdict_and_edits_its_summary() {
    let (mut app, _rx) = listened(FixtureScenario::GuidedReviewPullRequestPost);
    let verdict = |app: &App| app.guided_review_workspace.github_post.as_ref().map(|p| p.verdict);
    let start = verdict(&app);
    press(&mut app, KeyCode::Right);
    assert_eq!(verdict(&app), start, "the arrows are not the verdict's");
    press(&mut app, KeyCode::Tab);
    let moved = verdict(&app);
    assert_ne!(moved, start);
    press_with(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT);
    assert_eq!(verdict(&app), start);
    press_with(&mut app, KeyCode::Char('c'), KeyModifiers::ALT);
    assert!(app.guided_review_workspace.finding_draft.is_none(), "alt+c is not c");
    // The fixture's review has a summary already: `e` changes it, and `c` starts no second one
    press(&mut app, KeyCode::Char('c'));
    assert!(app.guided_review_workspace.finding_draft.is_none(), "one summary");
    press(&mut app, KeyCode::Char('e'));
    assert!(app.guided_review_workspace.editing_finding_id.is_some(), "e edits the summary");
    press(&mut app, KeyCode::Esc);
    press_with(&mut app, KeyCode::Char('w'), KeyModifiers::ALT);
    assert!(app.guided_review_workspace.pull_request_fix.is_none(), "alt+w is not w");
    press(&mut app, KeyCode::Char('w'));
    assert!(app.guided_review_workspace.pull_request_fix.is_some(), "w offers the worktree");
}

#[test]
fn esc_closes_the_post_preview_before_it_posts() {
    let (mut app, _rx) = listened(FixtureScenario::GuidedReviewPullRequestPost);
    press(&mut app, KeyCode::Esc);
    assert!(app.guided_review_workspace.github_post.is_none());
}

#[test]
fn enter_posts_from_the_post_preview() {
    let (mut app, _rx) = listened(FixtureScenario::GuidedReviewPullRequestPost);
    press(&mut app, KeyCode::Enter);
    let post = app.guided_review_workspace.github_post.as_ref().expect("still open");
    assert!(post.posting, "enter posts");
    press(&mut app, KeyCode::Esc);
    assert!(app.guided_review_workspace.github_post.is_some(), "and stays until GitHub answers");
}

#[test]
fn the_commit_preview_squashes_and_cancels() {
    let mut app = app(FixtureScenario::GuidedReviewCommit);
    let squash = |app: &App| app.guided_review_workspace.commit_preview.as_ref().map(|c| c.squash);
    press(&mut app, KeyCode::Char('s'));
    assert_eq!(squash(&app), Some(true));
    press(&mut app, KeyCode::Esc);
    assert!(app.guided_review_workspace.commit_preview.is_none());
}

#[test]
fn enter_commits_from_the_commit_preview() {
    let (mut app, _rx) = listened(FixtureScenario::GuidedReviewCommit);
    press(&mut app, KeyCode::Enter);
    let preview = app.guided_review_workspace.commit_preview.as_ref().expect("still open");
    assert!(preview.running, "enter commits");
}

/// Draws the whole app at `width` × `height`, the way a resize and its next frame do.
fn drawn_at(app: &mut App, width: u16, height: u16) {
    use ratatui::{backend::TestBackend, Terminal};
    app.update(Msg::TerminalResized { width, height });
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| crate::view::render(frame, app)).expect("drawn");
}

fn posting(app: &App) -> bool {
    app.guided_review_workspace.github_post.as_ref().expect("still open").posting
}

/// A preview its notice stands in for takes no confirm: Enter there would post a review nobody
/// could see. Esc still closes it, and the guard lifts once the preview draws again.
#[test]
fn a_hidden_preview_takes_only_esc() {
    let (mut app, mut engine) = listened(FixtureScenario::GuidedReviewPullRequestPost);
    drawn_at(&mut app, 40, 6);
    press(&mut app, KeyCode::Enter);
    assert!(!posting(&app), "Enter behind the notice did nothing");
    assert!(engine.try_recv().is_err(), "and asked the engine for nothing");

    drawn_at(&mut app, 120, 40);
    press(&mut app, KeyCode::Enter);
    assert!(posting(&app), "drawn again, Enter posts");

    let (mut app, _engine) = listened(FixtureScenario::GuidedReviewPullRequestPost);
    drawn_at(&mut app, 40, 6);
    press(&mut app, KeyCode::Esc);
    assert!(app.guided_review_workspace.github_post.is_none(), "esc still closes it");
    drawn_at(&mut app, 120, 40);
    let drawn = app.guided_review_workspace.drawn.get();
    assert_eq!(drawn, crate::guided_review::preview_guard::Drawn::ALL, "no guard left behind");
}

/// Below the review's own minimum no preview is drawn at all, so it is guarded there too.
///
/// The break this catches: only the preview's renderer set the guard, and the review's 40 × 5
/// notice returned before it, so a preview drawn at full size and then squeezed kept taking Enter.
#[test]
fn a_preview_below_the_review_minimum_takes_only_esc() {
    let (mut app, mut engine) = listened(FixtureScenario::GuidedReviewPullRequestPost);
    drawn_at(&mut app, 120, 40);
    drawn_at(&mut app, 39, 20);
    press(&mut app, KeyCode::Enter);
    assert!(!posting(&app), "Enter behind the minimum-size notice did nothing");
    assert!(engine.try_recv().is_err());
}

/// Below the review's own minimum nothing is drawn, so no dialog acts on Enter: not the commit
/// preview, and not a composer either. Esc still does.
///
/// The break this catches: the guard listed four previews, so the commit, send, open and finish
/// dialogs took Enter behind the minimum-size notice.
#[test]
fn nothing_below_the_review_minimum_takes_enter() {
    let (mut app, _engine) = listened(FixtureScenario::GuidedReviewCommit);
    drawn_at(&mut app, 39, 20);
    press(&mut app, KeyCode::Enter);
    let preview = app.guided_review_workspace.commit_preview.as_ref().expect("still open");
    assert!(!preview.running, "Enter behind the minimum-size notice committed nothing");
    press(&mut app, KeyCode::Esc);
    assert!(app.guided_review_workspace.commit_preview.is_none(), "esc still closes it");

    let (mut app, _engine) = listened(FixtureScenario::GuidedReviewPullRequestPost);
    drawn_at(&mut app, 120, 40);
    press(&mut app, KeyCode::Char('e'));
    drawn_at(&mut app, 39, 20);
    press(&mut app, KeyCode::Char('x'));
    let draft = app.guided_review_workspace.finding_draft.as_ref().expect("the composer is open");
    assert!(!draft.body().contains('x'), "an undrawn composer took no text");
}

/// Leaving a review that was too small to draw leaves no guard behind: the conversation still
/// takes its keys.
///
/// The break this catches: the guard read the review's last frame in every view, so after esc out
/// of a too-small review every key but esc was dropped, in chat, until the review drew again.
#[test]
fn leaving_a_too_small_review_leaves_the_keys_working() {
    let (mut app, _engine) = listened(FixtureScenario::GuidedReviewCommit);
    drawn_at(&mut app, 39, 20);
    press(&mut app, KeyCode::Esc); // the commit preview
    press(&mut app, KeyCode::Esc); // the review
    assert_eq!(app.current_view, crate::app::ViewMode::Chat, "back in the conversation");
    press(&mut app, KeyCode::Char('a'));
    assert_eq!(app.editor.current_input(), "a", "the composer takes the key");
}

/// With no dialog open, the review's own keys still move it below its minimum: the guard holds
/// back confirms, not navigation.
#[test]
fn a_too_small_review_with_no_dialog_still_navigates() {
    let mut app = app(FixtureScenario::GuidedReviewSplit);
    drawn_at(&mut app, 39, 20);
    let before = app.guided_review_workspace.position.hunk_id.clone();
    press(&mut app, KeyCode::Char('n'));
    assert_ne!(app.guided_review_workspace.position.hunk_id, before, "n moved to the next hunk");
}

/// Every dialog, composer and panel that acts on a key waits below the review's minimum, one
/// scenario each, so none of them can drop out of the guard unnoticed.
#[test]
fn every_open_dialog_waits_below_the_review_minimum() {
    use FixtureScenario::*;
    let guarded = |mut app: App, what: &str| {
        drawn_at(&mut app, 39, 20);
        let workspace = &app.guided_review_workspace;
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        assert!(workspace.hidden_preview_takes(enter), "{what} took Enter unseen");
        assert!(!workspace.hidden_preview_takes(esc), "{what} kept esc");
    };
    for scenario in [
        GuidedReviewCommit,
        GuidedReviewSend,
        GuidedReviewFinish,
        GuidedReviewOpenPullRequest,
        GuidedReviewPullRequestPost,
        GuidedReviewOpenStack,
        GuidedReviewFixPullRequest,
        GuidedReviewOwnPullRequest,
    ] {
        guarded(app(scenario), &format!("{scenario:?}"));
    }
    let mut composing = app(GuidedReviewSplit);
    drawn_at(&mut composing, 120, 40);
    press(&mut composing, KeyCode::Char('c'));
    assert!(composing.guided_review_workspace.finding_draft.is_some(), "the composer opened");
    guarded(composing, "the comment composer");
    let mut picking = app(GuidedReviewSplit);
    picking.guided_review_workspace.toggle_source_picker();
    guarded(picking, "the source picker");
    let mut typing = app(GuidedReviewSplit);
    typing
        .guided_review_workspace
        .begin_source_reference_entry(crate::guided_review::state::ReviewSourceReferenceKind::Base);
    guarded(typing, "the reference prompt");
}

/// The feedback window's `x` deletes the selected comment at once, so below the minimum it must
/// not reach a window nobody can see.
#[test]
fn the_feedback_window_deletes_nothing_below_the_review_minimum() {
    let (mut app, mut engine) = listened(FixtureScenario::GuidedReviewFindings);
    assert!(app.guided_review_workspace.findings_visible, "the feedback window is open");
    drawn_at(&mut app, 39, 20);
    press(&mut app, KeyCode::Char('x'));
    assert!(engine.try_recv().is_err(), "x asked for no deletion unseen");
    drawn_at(&mut app, 120, 40);
    press(&mut app, KeyCode::Char('x'));
    assert!(engine.try_recv().is_ok(), "drawn, x does ask");
}

/// Outside the review its panels and dialogs take no keys, whatever was left open by a way out
/// that does not close them: the conversation gets them instead.
///
/// The break this catches: the review's key layer ran in every view, so with the feedback window
/// left open `x` in the conversation asked to delete a comment, and with a commit preview Enter
/// committed, neither of them on screen.
#[test]
fn outside_the_review_its_panels_take_no_keys() {
    let (mut app, mut engine) = listened(FixtureScenario::GuidedReviewFindings);
    assert!(app.guided_review_workspace.findings_visible, "the feedback window is open");
    app.update(Msg::ExitGuidedReview); // alt+left or the palette, which do not close it first
    while engine.try_recv().is_ok() {} // what leaving records
    press(&mut app, KeyCode::Char('x'));
    assert!(engine.try_recv().is_err(), "x asked for nothing from the conversation");
    assert_eq!(app.editor.current_input(), "x", "the composer took it instead");

    let (mut app, _engine) = listened(FixtureScenario::GuidedReviewCommit);
    app.update(Msg::ExitGuidedReview);
    press(&mut app, KeyCode::Enter);
    let preview = app.guided_review_workspace.commit_preview.as_ref().expect("still open");
    assert!(!preview.running, "Enter in the conversation committed nothing");
}

/// A summary composer opened over a hidden post is drawn on its own and keeps its keys.
#[test]
fn a_summary_composer_over_a_hidden_post_keeps_its_keys() {
    let (mut app, _engine) = listened(FixtureScenario::GuidedReviewPullRequestPost);
    drawn_at(&mut app, 120, 40);
    press(&mut app, KeyCode::Char('e')); // changes the summary the fixture already has
    let draft =
        |app: &App| app.guided_review_workspace.finding_draft.as_ref().map(|d| d.body().to_owned());
    assert!(draft(&app).is_some(), "the composer opened");
    drawn_at(&mut app, 40, 6);
    press(&mut app, KeyCode::Char('x'));
    assert!(draft(&app).is_some_and(|text| text.contains('x')), "the composer took the key");
}

/// A commit preview with no review behind it is not left marked running.
#[test]
fn a_commit_with_no_review_is_not_left_running() {
    let (mut app, _rx) = listened(FixtureScenario::GuidedReviewCommit);
    app.guided_review = None;
    press(&mut app, KeyCode::Enter);
    let preview = app.guided_review_workspace.commit_preview.as_ref().expect("still open");
    assert!(!preview.running, "nothing was asked, so nothing is running");
}

#[test]
fn the_send_preview_scrolls_moves_and_includes() {
    let mut app = app(FixtureScenario::GuidedReviewSend);
    app.guided_review_workspace.feedback_room.set(40);
    press(&mut app, KeyCode::PageDown);
    assert_eq!(app.guided_review_workspace.feedback_scroll, 10);
    press(&mut app, KeyCode::PageUp);
    assert_eq!(app.guided_review_workspace.feedback_scroll, 0);
    let preview = |app: &App| app.guided_review_workspace.send_preview.clone().unwrap();
    press(&mut app, KeyCode::Down);
    assert_eq!(preview(&app).selected, 1);
    press(&mut app, KeyCode::Up);
    assert_eq!(preview(&app).selected, 0);
    press(&mut app, KeyCode::Char(' '));
    assert_eq!(preview(&app).excluded.len(), 1);
}

#[test]
fn a_comment_composer_discards_on_esc_only() {
    let mut app = app(FixtureScenario::GuidedReviewReady);
    press(&mut app, KeyCode::Char('c'));
    assert!(app.guided_review_workspace.finding_draft.is_some());
    press_with(&mut app, KeyCode::Esc, KeyModifiers::SHIFT);
    assert!(app.guided_review_workspace.finding_draft.is_some(), "shift+esc is not Esc");
    press(&mut app, KeyCode::Esc);
    assert!(app.guided_review_workspace.finding_draft.is_none());
}

// === The feedback window ===

/// The window's keys act on the comment under the cursor, and a key it does not use stops there
/// rather than moving something the window covers.
#[test]
fn the_feedback_window_keys_act_on_the_selected_comment() {
    let mut app = app(FixtureScenario::GuidedReviewFindings);
    let (engine_tx, mut engine_rx) = mpsc::channel(16);
    app.engine_tx = engine_tx;
    let selected = |app: &App| app.guided_review_workspace.selected_finding_id.clone();

    app.guided_review_workspace.feedback_room.set(40);
    press(&mut app, KeyCode::PageDown);
    assert_eq!(app.guided_review_workspace.feedback_scroll, 10);
    press(&mut app, KeyCode::PageUp);
    assert_eq!(app.guided_review_workspace.feedback_scroll, 0);

    let order: Vec<_> =
        crate::guided_review::ordered_findings(&app.review.session().expect("session").findings)
            .into_iter()
            .map(|finding| finding.id.clone())
            .collect();
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    assert_eq!(selected(&app).as_ref(), Some(&order[2]), "↓ moves down the list");
    press(&mut app, KeyCode::Up);
    assert_eq!(selected(&app).as_ref(), Some(&order[1]), "and ↑ up it");
    press(&mut app, KeyCode::Up);

    press(&mut app, KeyCode::Char('d'));
    assert!(app.guided_review_workspace.finding_details_visible);
    press(&mut app, KeyCode::Esc);
    assert!(!app.guided_review_workspace.finding_details_visible, "Esc closes the detail first");
    assert!(app.guided_review_workspace.findings_visible);
    press(&mut app, KeyCode::Char('v'));
    assert!(app.guided_review_workspace.finding_fix_visible);
    press(&mut app, KeyCode::Esc);
    assert!(!app.guided_review_workspace.finding_fix_visible);

    press(&mut app, KeyCode::Char('r'));
    assert!(engine_rx.try_recv().is_ok(), "r resolves it");
    press(&mut app, KeyCode::Char('c'));
    let draft = app.guided_review_workspace.finding_draft.as_ref().expect("a reply");
    assert!(draft.reply_to.is_some(), "c replies to it, not a new comment");
    press(&mut app, KeyCode::Esc);
    app.guided_review_workspace.findings_visible = true;

    // Onto the draft, R3, which e edits
    let draft_id = app.review.session().unwrap().findings[2].id.clone();
    app.guided_review_workspace.selected_finding_id = Some(draft_id.clone());
    press(&mut app, KeyCode::Char('e'));
    assert!(app.guided_review_workspace.finding_draft.is_some());
    assert_eq!(app.guided_review_workspace.editing_finding_id.as_ref(), Some(&draft_id));
    press(&mut app, KeyCode::Esc);
    app.guided_review_workspace.findings_visible = true;

    // One stop in, so a step back has somewhere to go
    app.guided_review_workspace.findings_visible = false;
    press(&mut app, KeyCode::Char('n'));
    app.guided_review_workspace.findings_visible = true;
    let position = app.guided_review_workspace.position.clone();
    press(&mut app, KeyCode::Char('n'));
    press_with(&mut app, KeyCode::Char('N'), KeyModifiers::SHIFT);
    assert_eq!(app.guided_review_workspace.position, position, "nothing moves under it");
}

/// Enter goes to the comment under the cursor — that one, not another — and closes the window.
#[test]
fn enter_goes_to_the_selected_comment() {
    let mut app = app(FixtureScenario::GuidedReviewFindings);
    let review = app.guided_review.as_ref().expect("review");
    let file = &review.files[0];
    let hunk = file.hunks.last().expect("a hunk");
    let line = hunk
        .lines
        .iter()
        .find(|line| line.kind == luxe_core::guided_review::ReviewLineKind::Addition)
        .expect("an addition");
    let anchor = luxe_core::guided_review::FindingAnchor::Line {
        file_id: file.id.clone(),
        hunk_id: hunk.id.clone(),
        side: luxe_core::guided_review::DiffSide::New,
        line: line.new_line.expect("a new line"),
        context_hash: line.context_hash(),
    };
    let (hunk_id, target) = (hunk.id.clone(), app.review.session().unwrap().findings[3].id.clone());
    app.review.session_mut().unwrap().findings[3].anchor = anchor;
    app.guided_review_workspace.selected_finding_id = Some(target);
    assert_ne!(app.guided_review_workspace.position.hunk_id.as_ref(), Some(&hunk_id));
    press(&mut app, KeyCode::Enter);
    assert!(!app.guided_review_workspace.findings_visible, "Enter goes to it");
    assert_eq!(app.guided_review_workspace.position.hunk_id.as_ref(), Some(&hunk_id));
}

/// The dialogs that open over the findings fixture, in the order tried.
const OPENED: [&str; 5] = ["finish", "send preview", "post", "pull request", "stack"];

/// Leaving the feedback window by any route leaves nothing of it behind: its details and fix views
/// are drawn over it and answer to its keys, so one left open has no key that closes it.
///
/// The break this catches: three routes — a reply, an edit, and Enter to a comment — and the
/// dialogs that open over the window each cleared the list and left whichever view was up.
#[test]
fn leaving_the_feedback_window_closes_the_views_drawn_over_it() {
    let open_views = |app: &mut App| {
        let workspace = &mut app.guided_review_workspace;
        workspace.findings_visible = true;
        workspace.finding_details_visible = true;
        workspace.finding_fix_visible = true;
    };
    let nothing_left = |app: &App, route: &str| {
        let workspace = &app.guided_review_workspace;
        assert!(!workspace.findings_visible, "{route}: the list");
        assert!(!workspace.finding_details_visible, "{route}: the details");
        assert!(!workspace.finding_fix_visible, "{route}: the fix view");
    };
    // keys: a reply to the question (index 1 is the one that answers `c`), an edit of the draft,
    // and Enter onto a comment
    for (name, key, finding) in [
        ("reply", KeyCode::Char('c'), 1),
        ("edit", KeyCode::Char('e'), 2),
        ("jump", KeyCode::Enter, 3),
    ] {
        let mut app = app(FixtureScenario::GuidedReviewFindings);
        let target = app.review.session().unwrap().findings[finding].id.clone();
        app.guided_review_workspace.selected_finding_id = Some(target);
        open_views(&mut app);
        press(&mut app, key);
        nothing_left(&app, name);
    }
    // dialogs: each opens over the window when the review allows it
    type Dialog = (&'static str, fn(&mut App), fn(&App) -> bool);
    let dialogs: [Dialog; 6] = [
        (
            "finish",
            |app| app.guided_review_workspace.open_finish_review(true),
            |app| app.guided_review_workspace.finish_review_choice.is_some(),
        ),
        (
            "send preview",
            |app| app.update(Msg::GuidedReviewOpenSendPreview),
            |app| app.guided_review_workspace.send_preview.is_some(),
        ),
        (
            "post",
            |app| app.update(Msg::GuidedReviewOpenGithubPost),
            |app| app.guided_review_workspace.github_post.is_some(),
        ),
        (
            "answers",
            |app| app.update(Msg::GuidedReviewOpenThreadAnswers),
            |app| app.guided_review_workspace.thread_answers.is_some(),
        ),
        (
            "pull request",
            |app| app.update(Msg::GuidedReviewOpenPullRequest),
            |app| app.guided_review_workspace.pull_request_opening.is_some(),
        ),
        (
            "stack",
            |app| app.update(Msg::GuidedReviewOpenStack),
            |app| app.guided_review_workspace.stack_opening.is_some(),
        ),
    ];
    let mut opened = Vec::new();
    for (name, open, is_open) in dialogs {
        let (mut app, _engine) = listened(FixtureScenario::GuidedReviewFindings);
        open_views(&mut app);
        open(&mut app);
        if is_open(&app) {
            nothing_left(&app, name);
            opened.push(name);
        }
    }
    // a dialog that stopped opening here is a case that stopped being tested
    assert_eq!(opened, OPENED, "the dialogs that open over this review");

    // your own pull request: its threads are answered on GitHub
    let (mut app, _engine) = listened(FixtureScenario::GuidedReviewOwnPullRequest);
    app.guided_review_workspace.thread_answers = None;
    open_views(&mut app);
    app.update(Msg::GuidedReviewOpenThreadAnswers);
    assert!(app.guided_review_workspace.thread_answers.is_some(), "the answers open");
    nothing_left(&app, "answers");

    // someone else's pull request: the send key offers a worktree instead
    let (mut app, _engine) = listened(FixtureScenario::GuidedReviewPullRequest);
    open_views(&mut app);
    app.update(Msg::GuidedReviewOpenSendPreview);
    assert!(app.guided_review_workspace.pull_request_fix.is_some(), "the offer opens");
    nothing_left(&app, "worktree offer");
}

/// With nothing to send, `u` does nothing at all — it does not reach the send path to say so.
#[test]
fn u_with_nothing_unsent_does_nothing() {
    let mut app = app(FixtureScenario::GuidedReviewFindings);
    set_findings(&mut app, |finding| finding.state = FindingState::Resolved);
    assert_eq!(crate::app::window_sends(&app), None);
    let before = said(&app).len();
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(said(&app).len(), before);
    assert!(app.guided_review_workspace.send_preview.is_none());
}

/// `x` deletes the draft under the cursor.
#[test]
fn x_deletes_a_draft_in_the_window() {
    let mut app = app(FixtureScenario::GuidedReviewFindings);
    let (engine_tx, mut engine_rx) = mpsc::channel(16);
    app.engine_tx = engine_tx;
    let draft_id = app.review.session().unwrap().findings[2].id.clone();
    app.guided_review_workspace.selected_finding_id = Some(draft_id.clone());
    press(&mut app, KeyCode::Char('x'));
    assert!(
        matches!(
            engine_rx.try_recv(),
            Ok(Action::EditReview {
                edit: luxe_core::engine::ReviewEdit::DeleteComment { finding_id },
                ..
            }) if finding_id == draft_id
        ),
        "x deletes it"
    );
}

/// Re-anchoring is `Alt+R` in the window, and only that chord.
#[test]
fn alt_r_reanchors_in_the_window() {
    let mut app = app(FixtureScenario::GuidedReviewFindings);
    let (engine_tx, mut engine_rx) = mpsc::channel(16);
    app.engine_tx = engine_tx;
    let stale = app.review.session().unwrap().findings[4].id.clone();
    app.guided_review_workspace.selected_finding_id = Some(stale);
    press_with(&mut app, KeyCode::Char('z'), KeyModifiers::ALT);
    press_with(&mut app, KeyCode::Char('R'), KeyModifiers::SHIFT);
    press_with(&mut app, KeyCode::Char('r'), KeyModifiers::SHIFT);
    assert!(engine_rx.try_recv().is_err(), "neither alt+z nor shift+r is alt+r");
    press_with(&mut app, KeyCode::Char('r'), KeyModifiers::ALT);
    assert!(engine_rx.try_recv().is_ok(), "alt+r re-anchors");
}
