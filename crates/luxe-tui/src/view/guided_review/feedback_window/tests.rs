use crate::app::{App, Msg};
use crate::fixture::{seeded_app, FixtureScenario};
use luxe_core::guided_review::{FindingId, FindingSeverity, ReviewFinding};
use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};

fn draw(app: &mut App, width: u16, height: u16) -> Buffer {
    app.update(Msg::TerminalResized { width, height });
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| crate::view::render(frame, app)).expect("render");
    terminal.backend().buffer().clone()
}

fn row(buffer: &Buffer, y: u16) -> String {
    (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect()
}

/// The row the window's top border is on, and its footer rule's.
fn frame_rows(buffer: &Buffer, title: &str) -> (u16, u16) {
    let top = (0..buffer.area.height)
        .find(|y| {
            let row = row(buffer, *y);
            row.contains(title) && row.contains(['╭', '┌'])
        })
        .expect("top");
    // The footer's rule, the row above its keys: where what the window says ends
    let keys = (top + 2..buffer.area.height)
        .find(|y| row(buffer, *y).contains("esc ") && row(buffer, y - 1).contains("────"))
        .expect("the footer");
    (top, keys - 1)
}

fn feedback(app: &mut App) -> &mut Vec<ReviewFinding> {
    &mut app.review.session_mut().expect("session").findings
}

fn open(scenario: FixtureScenario) -> App {
    let mut app = seeded_app("nord", scenario).expect("fixture app");
    app.guided_review_workspace.findings_visible = true;
    app
}

/// Centered both ways, the list two fifths of the window, the divider right after it, and as
/// tall as the longer of the list and the comment with a floor of eight rows.
#[test]
fn the_window_is_centered_and_split_two_fifths_to_three() {
    let mut app = open(FixtureScenario::GuidedReviewFindings);
    let buffer = draw(&mut app, 100, 40);
    let (top, bottom) = frame_rows(&buffer, "Feedback");
    assert_eq!(bottom - top + 1, 8 + 2, "{}", row(&buffer, top));
    let corners = row(&buffer, top);
    let left = corners.chars().position(|c| matches!(c, '╭' | '┌')).expect("left");
    let right = corners.chars().position(|c| matches!(c, '╮' | '┐')).expect("right");
    // 94 wide: three columns either side of a 100-column terminal
    assert_eq!((left, right), (3, 96), "{corners}");
    // Frame, pad, a 37-column list, then the divider's middle column
    assert_eq!(buffer[(3 + 2 + 37 + 1, top + 1)].symbol(), "│");
    // The review's inner area sits under a row of chrome and over the footer
    let above = usize::from(top);
    let below = usize::from(buffer.area.height - 1 - bottom);
    assert!(above.abs_diff(below) <= 3, "{above} vs {below}");

    let mut app = open(FixtureScenario::GuidedReviewReady);
    let buffer = draw(&mut app, 100, 40);
    let (top, bottom) = frame_rows(&buffer, "Feedback");
    assert_eq!(bottom - top + 1, 7, "the empty window is its message and a frame");
    let corners = row(&buffer, top);
    let left = corners.chars().position(|c| matches!(c, '╭' | '┌')).expect("left");
    assert_eq!(left, (100 - 56) / 2, "{corners}");
    assert!(usize::from(top).abs_diff(usize::from(40 - 1 - bottom)) <= 3);
}

/// The cursor row is marked, a list longer than the window scrolls to keep it on screen, and
/// each drawn row is the click target for its own comment.
#[test]
fn the_list_follows_the_cursor_and_clicks_land_on_their_rows() {
    let mut app = open(FixtureScenario::GuidedReviewFindings);
    let template = feedback(&mut app)[0].clone();
    for n in 0..30 {
        let mut extra = template.clone();
        extra.id = FindingId::new();
        extra.label = format!("R{}", n + 10);
        extra.title = format!("Extra comment {n}");
        feedback(&mut app).push(extra);
    }
    let findings = crate::guided_review::ordered_findings(feedback(&mut app));
    let last = findings.last().expect("last").id.clone();
    // The first whole, with the gap before its state so `0` is not the start of `09`; the last
    // by its start, since a long title is cut to fit its row
    let first_title = format!("{}  ", findings[0].title);
    let last_title = findings.last().unwrap().title.chars().take(12).collect::<String>();
    app.guided_review_workspace.selected_finding_id = Some(last.clone());
    let buffer = draw(&mut app, 120, 24);
    let text = (0..24).map(|y| row(&buffer, y)).collect::<Vec<_>>().join("\n");
    // Inside the window: the guide behind it keeps a cursor of its own
    let inside = |row: String| row.split_once('│').map(|(_, rest)| rest.to_owned());
    let marked: Vec<String> =
        (0..24).filter_map(|y| inside(row(&buffer, y))).filter(|row| row.contains('›')).collect();
    assert_eq!(marked.len(), 1, "{text}");
    assert!(marked[0].contains(&last_title), "{text}");
    // In the list's columns, since the guide behind the window lists the feedback too
    let list = |y: u16| (0..50).map(|x| buffer[(x, y)].symbol()).collect::<Vec<_>>();
    let listed = (0..24).map(|y| list(y).concat()).collect::<Vec<_>>().join("\n");
    assert!(!listed.contains(&first_title), "scrolled past the first: {listed}");
    let (x, y) = (0..24)
        .find_map(|y| {
            let cells = list(y);
            (0..cells.len() - last_title.len())
                .find(|&x| cells[x..x + last_title.len()].concat() == last_title)
                .map(|x| (u16::try_from(x).unwrap(), y))
        })
        .expect("drawn in the list");
    let (_, rule) = frame_rows(&buffer, "Feedback");
    assert_eq!(y, rule - 1, "the last row of the window's list, over the footer");
    let hit = app.hit.borrow().at(x, y).cloned();
    assert_eq!(hit, Some(crate::mouse::MouseTarget::ReviewFindingRow(last)));
}

/// A long comment scrolls to its last line, which lands on the pane's last row rather than
/// short of it or past it, and its text never runs into the scrollbar's column.
#[test]
fn a_long_comment_scrolls_exactly_to_its_end() {
    let mut app = open(FixtureScenario::GuidedReviewFindings);
    // The pane's text is 50 columns wide here: 94 less the frame and pad (3), the list (37),
    // the divider (3) and the scrollbar (1). Each line is one column wider, so it wraps once
    // exactly at the true width, and a pane counted even a column wider scrolls short.
    let line = |n: usize| format!("{n:02} {}abcdef", "abcde ".repeat(7));
    assert_eq!(line(0).len(), 51);
    let body = (0..40).map(line).collect::<Vec<_>>().join("\n\n");
    feedback(&mut app)[0].body = format!("{body}\n\nTHE END");
    let first = feedback(&mut app)[0].id.clone();
    app.guided_review_workspace.selected_finding_id = Some(first);
    let buffer = draw(&mut app, 100, 40);
    assert!((0..40).any(|y| row(&buffer, y).contains("pgup/pgdn scroll")));
    app.guided_review_workspace.scroll_feedback(10_000);
    let buffer = draw(&mut app, 100, 40);
    let (_, bottom) = frame_rows(&buffer, "Feedback");
    assert!(row(&buffer, bottom - 1).contains("THE END"), "{}", row(&buffer, bottom - 1));
    // The pane's last column is the scrollbar's, never a letter: the right border is at 96
    let lane = 96 - 1;
    for y in bottom.saturating_sub(20)..bottom {
        let cell = buffer[(lane, y)].symbol();
        assert!(cell.chars().all(|c| !c.is_alphanumeric()), "{cell:?} at row {y}");
    }

    // A short one says nothing about scrolling
    let mut app = open(FixtureScenario::GuidedReviewFindings);
    let buffer = draw(&mut app, 100, 40);
    assert!(!(0..40).any(|y| row(&buffer, y).contains("pgup/pgdn")));
}

/// The reading pane names what the comment is — a question, a reviewer's, or yours — with its
/// label when it has one, and shows a proposed fix only when there is one.
#[test]
fn the_reading_pane_says_what_the_comment_is() {
    let mut app = open(FixtureScenario::GuidedReviewFindings);
    let first = feedback(&mut app)[0].id.clone();
    app.guided_review_workspace.selected_finding_id = Some(first);
    let text = |app: &mut App| {
        let buffer = draw(app, 120, 40);
        (0..40).map(|y| row(&buffer, y)).collect::<Vec<_>>().join("\n")
    };
    let label = feedback(&mut app)[0].label.clone();
    let shown = text(&mut app);
    assert!(shown.contains("Comment · open"), "{shown}");
    assert!(!shown.contains(&format!("{label} ")), "no label is drawn: {shown}");
    feedback(&mut app)[0].severity = FindingSeverity::Question;
    assert!(text(&mut app).contains("Question · open"));
    feedback(&mut app)[0].severity = FindingSeverity::Issue;
    feedback(&mut app)[0].github_thread = Some(luxe_core::guided_review::GitHubThread {
        id: "PRRT_1".into(),
        url: String::new(),
        author: "mara".into(),
        can_reply: true,
        can_resolve: true,
        reply_url: None,
        resolved: false,
        conversation: false,
    });
    let shown = text(&mut app);
    assert!(shown.contains("Reviewer comment") && shown.contains("from mara on GitHub"));

    feedback(&mut app)[0].proposed_fix = Some("   ".into());
    assert!(!text(&mut app).contains("Proposed fix"));
    feedback(&mut app)[0].proposed_fix = Some("Bound the retry.".into());
    let shown = text(&mut app);
    assert!(shown.contains("Proposed fix") && shown.contains("Bound the retry."));
}

/// Sending shows the same window with checkboxes: a question says so, an excluded row dims,
/// a long title is cut before the row's end, and the header counts what goes.
#[test]
fn sending_lists_each_comment_with_a_checkbox() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture");
    let unsent = crate::app::input::unsent_finding_ids(&app);
    let long = "A title long enough that it cannot fit beside its mark and label ".repeat(2);
    for finding in feedback(&mut app).iter_mut() {
        if finding.id == unsent[0] {
            finding.severity = FindingSeverity::Question;
            finding.title = long.trim().to_owned();
        }
    }
    app.update(Msg::GuidedReviewOpenSendPreview);
    let buffer = draw(&mut app, 120, 40);
    let text = (0..40).map(|y| row(&buffer, y)).collect::<Vec<_>>().join("\n");
    let question = (0..40)
        .map(|y| row(&buffer, y))
        .find(|row| row.contains("A title long enough"))
        .expect("the question's row");
    // Cut to exactly the room there is: the ellipsis meets the state's own gap
    assert!(question.contains("…  question  ▌"), "and the row fills to its edge: {question}");
    assert!(text.contains(&format!("{} of {} included", unsent.len(), unsent.len())));
    app.update(Msg::GuidedReviewToggleSendPreviewRow);
    let buffer = draw(&mut app, 120, 40);
    let text = (0..40).map(|y| row(&buffer, y)).collect::<Vec<_>>().join("\n");
    assert!(text.contains(&format!("{} of {} included", unsent.len() - 1, unsent.len())));
}

/// Opened with nothing selected, the window selects the first comment, since it draws the
/// cursor there and its keys act on the selection; a title's markdown reads as plain text.
#[test]
fn opening_selects_the_first_comment_and_titles_read_plain() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewFindings).expect("fixture");
    app.guided_review_workspace.selected_finding_id = None;
    app.guided_review_workspace.findings_visible = false;
    feedback(&mut app)[0].title = "Pin **actions/setup-dotnet** to a `SHA`".into();
    app.update(Msg::GuidedReviewToggleFindings);
    let first = crate::guided_review::ordered_findings(feedback(&mut app))[0].id.clone();
    assert_eq!(app.guided_review_workspace.selected_finding_id, Some(first));
    let buffer = draw(&mut app, 140, 40);
    let text = (0..40).map(|y| row(&buffer, y)).collect::<Vec<_>>().join("\n");
    assert!(text.contains("Pin actions/setup-dotnet to a SHA"), "{text}");
    assert!(!text.contains("**") && text.contains("enter go to it"), "{text}");
    // Closing and opening again keeps where the reader was
    let second = crate::guided_review::ordered_findings(feedback(&mut app))[1].id.clone();
    app.guided_review_workspace.selected_finding_id = Some(second.clone());
    app.update(Msg::GuidedReviewToggleFindings);
    app.update(Msg::GuidedReviewToggleFindings);
    assert_eq!(app.guided_review_workspace.selected_finding_id, Some(second));
}
