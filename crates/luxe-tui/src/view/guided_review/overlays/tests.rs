use super::*;
use crate::fixture::{seeded_app, FixtureScenario};
use crate::guided_review::FinishReviewChoice;
use ratatui::{backend::TestBackend, Terminal};
use ratatui::{layout::Rect, Frame};

/// The rows `draw` puts on a `width` × `height` screen.
fn drawn(width: u16, height: u16, draw: impl FnOnce(&mut Frame)) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(draw).expect("draw");
    let buffer = terminal.backend().buffer();
    (0..height).map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect::<String>()).collect()
}

/// Where a card's top-left corner is, and how big it is, from its rounded corners.
fn card(rows: &[String]) -> (u16, u16, u16, u16) {
    let top = rows.iter().position(|row| row.contains('╭')).expect("a top edge");
    let bottom = rows.iter().rposition(|row| row.contains('╰')).expect("a bottom edge");
    let chars: Vec<char> = rows[top].chars().collect();
    let left = chars.iter().position(|c| *c == '╭').expect("left corner");
    let right = chars.iter().position(|c| *c == '╮').expect("right corner");
    (left as u16, top as u16, (right - left + 1) as u16, (bottom - top + 1) as u16)
}

/// The finish dialog is as tall as what it says, centered, with one row an option however
/// narrow, and the cursor on the choice it was given.
#[test]
fn the_finish_dialog_hugs_its_options_and_centers() {
    let app = seeded_app("nord", FixtureScenario::GuidedReviewFinish).expect("fixture app");
    for (width, height) in [(120u16, 40u16), (44, 31)] {
        let area = Rect::new(0, 0, width, height);
        let rows = drawn(width, height, |frame| {
            render_finish_review(frame, area, &app, FinishReviewChoice::RequestFixes)
        });
        let text = rows.join("\n");
        let (left, top, card_width, card_height) = card(&rows);
        assert_eq!(left, (width - card_width) / 2, "centered across: {text}");
        assert_eq!(top, (height - card_height) / 2, "and down: {text}");
        let labels = ["Accept ", "Accept and commit", "Request fixes", "Comment only", "Keep"];
        let at: Vec<usize> = labels
            .iter()
            .map(|label| rows.iter().position(|row| row.contains(label)).expect(label))
            .collect();
        assert!(at.windows(2).all(|pair| pair[1] == pair[0] + 1), "a row each: {text}");
        // The last option, then the footer's rule and keys, then the frame
        assert_eq!(usize::from(top + card_height), at[4] + 4, "nothing under the last: {text}");
        assert!(rows[at[4] + 1].contains("────") && rows[at[4] + 2].contains("enter finish"));
        assert!(text.contains("› Request fixes") && !text.contains("› Accept"), "{text}");
    }
    let finish = |app: &App| {
        drawn(120, 40, |frame| {
            let area = Rect::new(0, 0, 120, 40);
            render_finish_review(frame, area, app, FinishReviewChoice::Accept)
        })
        .join("\n")
    };
    let text = finish(&app);
    assert!(text.contains("Sends 2 comments, 1 of them again."), "{text}");
    assert!(text.contains("Accept — mark it reviewed; the agent isn't told"), "in full: {text}");
    // With nothing the agent already has, nothing goes again
    let mut fresh = app;
    for finding in &mut fresh.review.session_mut().expect("session").findings {
        if finding.state == luxe_core::guided_review::FindingState::Open {
            finding.state = luxe_core::guided_review::FindingState::Draft;
        }
    }
    let text = finish(&fresh);
    assert!(text.contains("Sends 2 comments."), "{text}");
}

/// On a screen smaller than their floor the finish and details dialogs stay inside it, and
/// where not even a few rows fit they say so rather than draw a card with its bottom missing.
///
/// The break this catches: both were sized with a floor and no ceiling, so below it the card
/// was wider or taller than the screen and ratatui clipped the border and the footer away.
#[test]
fn the_finish_and_details_dialogs_stay_inside_a_small_screen() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    let first = app.review.session().expect("session").findings[0].id.clone();
    app.guided_review_workspace.selected_finding_id = Some(first);
    for (width, height) in [(30u16, 14u16), (44, 9), (60, 5), (36, 20)] {
        let area = Rect::new(0, 0, width, height);
        let finish = drawn(width, height, |frame| {
            render_finish_review(frame, area, &app, FinishReviewChoice::Accept)
        });
        let details = drawn(width, height, |frame| render_finding_details(frame, area, &app));
        for (name, rows) in [("finish", finish), ("details", details)] {
            let text = rows.join("\n");
            if text.contains("Make the window bigger") {
                assert!(name == "finish" || height < 6, "{name} {width}x{height} had room: {text}");
                continue;
            }
            if name == "finish" {
                // every choice is on screen, or none of the card is: a cut-off list hides
                // options the keys still act on
                for label in
                    ["Accept ", "Accept and commit", "Request fixes", "Comment only", "Keep"]
                {
                    assert!(text.contains(label), "finish {width}x{height} lost {label}: {text}");
                }
            }
            let (left, top, card_width, card_height) = card(&rows);
            assert!(left + card_width <= width, "{name} {width}x{height} too wide: {text}");
            assert!(top + card_height <= height, "{name} {width}x{height} too tall: {text}");
            let last = &rows[usize::from(top + card_height - 1)];
            assert!(last.contains('╯'), "{name} {width}x{height} lost its bottom: {text}");
            // the keys sit in a row of the card, under the rule, not in its border
            let keys = rows
                .iter()
                .find(|row| row.contains("esc cancel") || row.contains("esc close"))
                .unwrap_or_else(|| panic!("{name} {width}x{height} lost its keys: {text}"));
            assert!(keys.trim_start().starts_with('│'), "{name} {width}x{height}: {text}");
        }
    }
}

/// Behind the notice that stands in for the finish dialog, nothing but esc acts: Enter would
/// finish a review on a choice the reader cannot see.
#[test]
fn the_finish_dialog_behind_its_notice_takes_only_esc() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFinish).expect("fixture app");
    app.guided_review_workspace.finish_review_choice = Some(FinishReviewChoice::Accept);
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

    let area = Rect::new(0, 0, 44, 9);
    let notice =
        drawn(44, 9, |frame| render_finish_review(frame, area, &app, FinishReviewChoice::Accept));
    assert!(notice.join("\n").contains("Make the window bigger"), "{notice:?}");
    assert!(app.guided_review_workspace.hidden_preview_takes(key(KeyCode::Enter)));
    assert!(!app.guided_review_workspace.hidden_preview_takes(key(KeyCode::Esc)));

    let area = Rect::new(0, 0, 60, 30);
    drawn(60, 30, |frame| render_finish_review(frame, area, &app, FinishReviewChoice::Accept));
    assert!(!app.guided_review_workspace.hidden_preview_takes(key(KeyCode::Enter)));
}

/// The finish dialog is drawn the moment its whole card fits and not before, with a row clear
/// above and below, so a dialog that fits is never hidden and one that does not is never cut.
#[test]
fn the_finish_dialog_is_drawn_exactly_when_its_card_fits() {
    let app = seeded_app("nord", FixtureScenario::GuidedReviewFinish).expect("fixture app");
    let at = |height: u16| {
        let area = Rect::new(0, 0, 60, height);
        drawn(60, height, |frame| {
            render_finish_review(frame, area, &app, FinishReviewChoice::Accept)
        })
    };
    let (_, _, _, card_height) = card(&at(40));
    let roomy = at(card_height + 2);
    assert!(!roomy.join("\n").contains("Make the window bigger"), "{roomy:?}");
    assert_eq!(card(&roomy).3, card_height, "the whole card, a row clear above and below");
    let short = at(card_height + 1);
    assert!(short.join("\n").contains("Make the window bigger"), "{short:?}");
}

/// Behind the notice that stands in for a finding's details, nothing but esc acts: the feedback
/// window's `x` would delete a comment the reader cannot see.
#[test]
fn a_findings_details_behind_their_notice_take_only_esc() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture app");
    let first = app.review.session().expect("session").findings[0].id.clone();
    let workspace = &mut app.guided_review_workspace;
    workspace.selected_finding_id = Some(first);
    workspace.findings_visible = true;
    workspace.finding_details_visible = true;
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);

    let area = Rect::new(0, 0, 60, 5);
    let rows = drawn(60, 5, |frame| render_finding_details(frame, area, &app));
    assert!(rows.join("\n").contains("Make the window bigger"), "{rows:?}");
    assert!(app.guided_review_workspace.hidden_preview_takes(key(KeyCode::Char('x'))));
    assert!(!app.guided_review_workspace.hidden_preview_takes(key(KeyCode::Esc)));

    let area = Rect::new(0, 0, 80, 30);
    drawn(80, 30, |frame| render_finding_details(frame, area, &app));
    assert!(!app.guided_review_workspace.hidden_preview_takes(key(KeyCode::Char('x'))));
}

/// Help sits in two columns where they fit and one where they do not, centered, with a blank
/// row between groups and none above the first.
#[test]
fn help_takes_two_columns_where_they_fit() {
    let app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let rows_at = |width: u16, height: u16| {
        drawn(width, height, |frame| render_help(frame, Rect::new(0, 0, width, height), &app))
    };
    let row_of = |rows: &[String], word: &str| {
        rows.iter().position(|row| row.contains(word)).unwrap_or_else(|| panic!("{word}"))
    };
    let two = rows_at(97, 30);
    let (left, top, width, height) = card(&two);
    assert_eq!((left, top), ((97 - width) / 2, (30 - height) / 2), "centered");
    assert_eq!(row_of(&two, "Move"), usize::from(top) + 1, "the first group leads");
    assert_eq!(row_of(&two, "Feedback"), row_of(&two, "Move"), "side by side at 97");
    let read = row_of(&two, "Read");
    assert!(two[read - 1].chars().skip(usize::from(left) + 2).take(40).all(|c| c == ' '));
    let one = rows_at(96, 44);
    assert!(row_of(&one, "Feedback") > row_of(&one, "Move"), "stacked at 96");
    let roomy = rows_at(130, 30);
    let (left, top, width, height) = card(&roomy);
    assert_eq!((left, top), ((130 - width) / 2, (30 - height) / 2), "centered with room");
}

/// The finish line counts what it sends and says how much of it the agent has already had,
/// because the guide's count of unsent leaves those out.
#[test]
fn finishing_says_what_it_sends_and_what_it_sends_again() {
    assert_eq!(finish_carries(0, 0), "No comments to send — add one with c.");
    assert_eq!(finish_carries(1, 0), "Sends 1 comment.");
    assert_eq!(finish_carries(3, 0), "Sends 3 comments.");
    assert_eq!(finish_carries(1, 1), "Sends 1 comment again.");
    assert_eq!(finish_carries(2, 1), "Sends 2 comments, 1 of them again.");
}
