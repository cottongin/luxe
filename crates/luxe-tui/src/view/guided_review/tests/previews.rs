//! Previews — the dialogs that confirm before anything leaves: post, answers, pull request, fix and stack.

use super::*;

/// Rows of the modal titled `title`: its top border's, and its footer rule's, which is where
/// what the modal says ends. The keys sit under the rule, on the last row inside the frame.
pub(super) fn modal_rows(rows: &[String], title: &str) -> (usize, usize) {
    let top = rows
        .iter()
        .position(|row| row.contains(title) && row.contains(['╭', '┌']))
        .expect("top border");
    let keys = (top + 2..rows.len().saturating_sub(1))
        .find(|&y| {
            rows[y].contains("esc")
                && rows[y - 1].contains("────")
                && rows[y + 1].contains(['╰', '└'])
        })
        .expect("the footer");
    (top, keys - 1)
}

/// The pull request preview is exactly as tall as what it holds: the heading under its top
/// border and the description down to the footer's rule, with nothing pinned under it. It
/// drafts and fails in a small box, a title being written wraps, and the scrollbar keeps its
/// own column.
#[test]
fn the_pull_request_preview_holds_exactly_what_it_shows() {
    use crate::guided_review::OpeningField;
    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewOpenPullRequest).expect("fixture app");
    let rows = frame_rows(&mut app, 150, 44);
    let (top, bottom) = modal_rows(&rows, "Open pull request");
    assert!(rows[top + 1].contains("Into main from"), "{}", rows.join("\n"));
    let closes = rows.iter().position(|row| row.contains("Closes #123")).expect("body");
    assert_eq!(closes, bottom - 1, "{}", rows.join("\n"));
    assert!(!rows[bottom].contains("scroll"), "a short description does not scroll");

    // A title being written wraps onto a second row rather than running off
    let opening = app.guided_review_workspace.pull_request_opening.as_mut().expect("preview");
    let mut editor = crate::editor::Editor::new();
    editor.insert_text(&"A title that goes on ".repeat(8));
    opening.editing = Some((OpeningField::Title, editor));
    let rows = frame_rows(&mut app, 150, 44);
    let label = rows.iter().position(|row| row.contains("│ Title")).expect("title label");
    assert!(rows[label + 1].contains("A title that goes on"), "{}", rows.join("\n"));
    assert!(rows[label + 2].contains("A title that goes on"), "wrapped: {}", rows.join("\n"));
    assert!(rows[label + 4].contains("Description"), "{}", rows.join("\n"));

    // Every body line one column wider than the text, so each wraps and none may reach the
    // scrollbar's column: 120 wide at 15, less the frame and pad, less that column
    let opening = app.guided_review_workspace.pull_request_opening.as_mut().expect("preview");
    opening.editing = None;
    let prepared = opening.prepared.as_mut().expect("prepared");
    // Alternately exactly as wide as the text, which ends at the lane's edge, and one wider,
    // which must wrap rather than spill into the lane
    let line = |n: usize| format!("{n:03} {}", "x".repeat(112 + n % 2));
    assert_eq!((line(0).len(), line(1).len()), (116, 117));
    prepared.pull_request.body = (0..60).map(line).collect::<Vec<_>>().join("\n\n");
    let rows = frame_rows(&mut app, 150, 44);
    let (top, bottom) = modal_rows(&rows, "Open pull request");
    let corners: Vec<char> = rows[top].chars().collect();
    let (left, right) = (
        corners.iter().position(|c| matches!(c, '╭' | '┌')).unwrap(),
        corners.iter().position(|c| matches!(c, '╮' | '┐')).unwrap(),
    );
    assert_eq!(right - left + 1, 120, "{}", rows[top]);
    for row in &rows[top + 7..bottom - 3] {
        let lane = row.chars().nth(right - 1).unwrap_or(' ');
        assert!(!lane.is_alphanumeric(), "{row}");
        let text = row.chars().nth(right - 2).unwrap_or(' ');
        assert!(text == 'x' || text == ' ', "the text reaches the lane's edge: {row}");
    }
    assert!(rows[top + 7..bottom].iter().any(|row| row.chars().nth(right - 2) == Some('x')));
    // The wider lines wrapped: a row begins with their remainder, not with a line number. A
    // pane counted a column wider would fit them whole, with the last letter under the bar.
    let wrapped = rows[top + 7..bottom].iter().any(|row| row.chars().nth(left + 2) == Some('x'));
    assert!(wrapped, "{}", rows.join("\n"));

    // Drafting, and failing, in a box sized to the words
    let opening = app.guided_review_workspace.pull_request_opening.as_mut().expect("preview");
    opening.prepared = None;
    opening.error = Some("GitHub would not say. ".repeat(12));
    let rows = frame_rows(&mut app, 150, 44);
    let (top, bottom) = modal_rows(&rows, "Open pull request");
    let wrapped = rows[top + 1..bottom].iter().filter(|row| row.contains("GitHub")).count();
    assert!(wrapped >= 2, "{}", rows.join("\n"));
    assert_eq!(bottom - top + 1, (wrapped + 3).max(5), "{}", rows.join("\n"));
}

/// The post preview lists what GitHub will get — the review, then each comment on a line —
/// and shows the whole text of the one under the cursor, before anything is sent.
#[test]
fn the_post_preview_shows_what_each_comment_says_and_where() {
    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewPullRequestPost).expect("fixture app");
    let rows = frame_rows(&mut app, 160, 46);
    let text = rows.join("\n");
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));

    assert!(has("Post this review to acme/notify#42"), "{text}");
    // The chosen verdict is the cut tab, with its icon and no arrow
    assert!(has("Request changes") && !has("› Request changes"), "{text}");
    // The review's own text, whole, in the pane: both paragraphs, not a title
    assert!(has("An unknown category now fails the whole webhook"), "{text}");
    assert!(has("Can we log and drop those for one release"), "{text}");
    // Under its file, the way GitHub groups a review's comments
    let listed = rows.windows(2).any(|pair| {
        pair[0].contains("│   src/notification.rs ") && pair[1].contains("│     line 13 ")
    });
    assert!(listed, "the line comment is listed: {text}");
    assert!(has("Summary"), "{text}");
    assert!(!has("The review's own comment"), "the pane explains nothing the list shows: {text}");
    // The keys on the frame sit under a rule of their own
    let keys = rows.iter().position(|row| row.contains("enter post")).expect("the keys");
    assert!(rows[keys - 1].contains("──────────"), "{text}");
    // The question went to the agent and was answered; it is neither listed nor explained
    assert_eq!(crate::app::github_post_item_count(&app), 2, "the review and one line comment");
    assert!(!has("not posted"), "{text}");
    // The card says what posts it, and carries no agent label: GitHub has no `R2`
    assert!(has("alt+enter to post"), "{text}");
    assert!(!has("R2"), "{text}");
    assert!(
        has("FEEDBACK") && has(" 3 · 2 unposted"),
        "a question is not waiting to be posted: {text}"
    );
    assert!(has("○ notification.rs:13  Worth a doc comment here"), "the foot lists it: {text}");
    assert!(
        has("enter post · ↑/↓ comment · tab verdict · d draft summary · e edit summary"),
        "{text}"
    );
    assert!(!has("pushed after"), "{text}");

    // Down moves to the line comment, and the pane follows with its place and its words
    app.update(crate::app::Msg::GuidedReviewMoveGithubPostItem(true));
    let rows = frame_rows(&mut app, 160, 46);
    let text = rows.join("\n");
    assert_eq!(
        rows.iter().filter(|row| row.contains("src/notification.rs:13")).count(),
        1,
        "heading the pane: {text}"
    );
    // Under the diff it sits on, ending at its own line, marked
    let own = rows.iter().find(|row| row.contains("▎13 +")).expect("its line, marked");
    assert!(own.contains("pub category: Category"), "{text}");
    assert!(rows.iter().any(|row| row.contains("13 -") && row.contains("String")), "{text}");
    assert!(!rows.iter().any(|row| row.contains("Can we log and drop")), "{text}");
    assert!(rows.iter().any(|row| row.contains("Worth a doc comment here")), "{text}");

    let post = app.guided_review_workspace.github_post.as_mut().expect("post preview");
    let prepared = post.prepared.as_mut().expect("prepared");
    prepared.head_moved = true;
    prepared.own_pull_request = true;
    post.verdict = luxe_core::github::ReviewEvent::Comment;
    let rows = frame_rows(&mut app, 160, 46);
    let text = rows.join("\n");
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("Your pull request · comment only"), "{text}");
    assert!(has("Pushed since you read it"), "{text}");
    assert!(
        has("enter post · ↑/↓ comment · d draft summary · e edit summary · w fix in a worktree · esc cancel"),
        "a lone verdict offers no choice: {text}"
    );

    // Once everything is posted, the preview says so rather than asking for a comment
    let posted = luxe_core::guided_review::PostedComment { review_id: 9, url: String::new() };
    for finding in &mut app.review.session_mut().expect("session").findings {
        if finding.severity != luxe_core::guided_review::FindingSeverity::Question {
            finding.posted = Some(posted.clone());
        }
    }
    let rows = frame_rows(&mut app, 160, 46);
    let text = rows.join("\n");
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("Already on GitHub · nothing new to post"), "{text}");
    assert!(has("2 comments already on GitHub"), "{text}");
}

/// A narrow terminal keeps the keys that post and leave, dropping the rest to fit the frame.
#[test]
fn a_narrow_post_preview_keeps_its_keys_inside_the_frame() {
    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewPullRequestPost).expect("fixture app");
    let rows = frame_rows(&mut app, 72, 30);
    let text = rows.join("\n");
    let hint = rows.iter().find(|row| row.contains("enter post")).expect("the keys");
    assert!(hint.contains("esc cancel"), "{text}");
    assert!(!hint.contains("w fix in a worktree"), "the least-needed key goes: {text}");
    assert!(hint.trim_end().ends_with('│'), "inside the frame, on a row of its own: {text}");
}

/// While it reads the pull request there is nothing to retry, so the preview offers only a
/// way out; once reading failed, Enter asks again. Titles read as plain text.
#[test]
fn the_post_preview_offers_a_retry_only_after_a_failure() {
    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewPullRequestPost).expect("fixture app");
    for finding in &mut app.review.session_mut().expect("session").findings {
        finding.title = format!("Pin **{}**", finding.title);
    }
    let rows = frame_rows(&mut app, 160, 46);
    let text = rows.join("\n");
    assert!(text.contains("Pin ") && !text.contains("**"), "{text}");
    let post = app.guided_review_workspace.github_post.as_mut().expect("preview");
    post.prepared = None;
    let rows = frame_rows(&mut app, 160, 46);
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("Reading the pull request…") && has("esc cancel"));
    assert!(!has("enter retry"), "{}", rows.join("\n"));
    app.guided_review_workspace.github_post.as_mut().expect("preview").error =
        Some("GitHub did not answer.".into());
    let rows = frame_rows(&mut app, 160, 46);
    assert!(rows.iter().any(|row| row.contains("enter retry · esc cancel")));
}

/// The answers preview says whether the fix is on GitHub, what each thread will get — a
/// resolve, a reply in the reader's words — and where it stands locally. The threads it lists
/// are feedback now, so the brief counts them there instead of listing them twice.
#[test]
fn the_answers_preview_says_what_each_thread_gets() {
    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewOwnPullRequest).expect("fixture app");
    let rows = frame_rows(&mut app, 160, 60);
    let text = rows.join("\n");
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));

    assert!(has("Answer these threads on acme/notify#42"), "{text}");
    assert!(has("Resolving closes the thread; leave it open for the reviewer"), "{text}");
    assert!(has("Your fix is not on GitHub yet: push this branch first."), "{text}");
    assert!(has("✓ resolve") && has("verified"), "{text}");
    assert!(has("reply: Only the webhook built it from a string"), "{text}");
    let replies = rows.iter().filter(|row| row.contains("reply: ")).count();
    assert_eq!(replies, 1, "a thread with nothing written shows no reply: {text}");
    assert!(has("○ leave open") && has("open · mara"), "{text}");
    assert!(
        has("› ✓ resolve") && has("  ○ leave open"),
        "the cursor marks the selected thread: {text}"
    );
    assert!(!has("R1") && !has("R2"), "the agent's names for threads stay off GitHub's: {text}");
    assert!(has("enter post · space resolve · c reply"), "{text}");

    // A fix the agent made and the reader has not looked at yet
    for finding in &mut app.review.session_mut().expect("session").findings {
        if finding.title.starts_with("jon-avery:") {
            finding.state = luxe_core::guided_review::FindingState::NeedsVerification;
        }
    }
    let answers = app.guided_review_workspace.thread_answers.as_mut().expect("preview");
    answers.selected = 1;
    let mut editor = crate::editor::Editor::new();
    editor.insert_text("On it");
    answers.replying = Some(editor);
    let rows = frame_rows(&mut app, 160, 60);
    let text = rows.join("\n");
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("Reply to @mara-okafor · type below") && has("On it"), "{text}");
    assert!(has("reply: Only the webhook"), "another thread's reply stays listed: {text}");
    assert_eq!(rows.iter().filter(|row| row.contains("type below")).count(), 1);
    assert!(has("to verify · jon-avery:"), "{text}");
    assert!(has("ctrl+enter save · esc cancel"), "{text}");
    assert!(has("3 in your feedback"), "{text}");
    assert!(has("○ conversation") && has("jon-avery: Could the changelog"), "{text}");
    assert!(has("pull request #42 · notify"), "{text}");
}

/// The pull request preview shows where it goes, what happens first, and the draft; each field
/// edits in place, and the states before and after a draft say what they are.
#[test]
fn the_pull_request_preview_shows_the_draft_and_what_happens_first() {
    use crate::guided_review::OpeningField;
    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewOpenPullRequest).expect("fixture app");
    let rows = frame_rows(&mut app, 150, 44);
    let text = rows.join("\n");
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("Open pull request"), "{text}");
    assert!(has("Into main from 123-category-enum · commits and pushes first"), "{text}");
    assert!(!has("ready for review"), "a pull request is ready unless it is a draft: {text}");
    assert!(has("Title") && has("Refactor notification model"), "{text}");
    // The description is drawn as markdown: its heading is a heading, not three hashes
    assert!(has("Description") && has("Shared notification model"), "{text}");
    assert!(!has("### Shared"), "{text}");
    assert!(has("Closes #123"), "{text}");
    assert!(
        has("enter open · t title · e description · E in your editor · d draft · esc cancel"),
        "{text}"
    );
    // Centered: as far from the left edge as from the right
    let top: Vec<char> =
        rows.iter().find(|row| row.contains("Open pull request")).expect("top").chars().collect();
    let left = top.iter().position(|c| matches!(c, '╭' | '┌')).expect("left corner");
    // rows come back trimmed, so the right margin counts from the frame's width
    let right = 150 - 1 - top.iter().rposition(|c| matches!(c, '╮' | '┐')).expect("corner");
    assert!(left > 10 && left.abs_diff(right) <= 1, "{left} vs {right}: {text}");
    // As tall as the draft
    let top_row = rows.iter().position(|row| row.contains("Open pull request")).expect("top");
    let bottom_row = rows.iter().position(|row| row.contains("esc cancel ")).expect("bottom");
    assert!(rows[bottom_row - 1].contains("────"), "the keys under their rule: {text}");
    assert!(rows[bottom_row - 2].contains("Closes #123"), "{text}");
    let below = rows.len() - 1 - bottom_row;
    assert!(top_row > 5 && top_row.abs_diff(below) <= 3, "{top_row} vs {below}: {text}");

    // Editing shows the terminal's own cursor in the field, where typing lands
    let opening = app.guided_review_workspace.pull_request_opening.as_mut().expect("preview");
    let editor = crate::editor::Editor::with_text("Name the categories", "Name the".len());
    opening.editing = Some((OpeningField::Title, editor));
    let (rows, _, before) = frame_with_cursor(&mut app, 150, 44);
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("Name the categories"), "{}", rows.join("\n"));
    assert!(has("enter save · esc cancel"), "{}", rows.join("\n"));
    assert!(before.ends_with("Name the"), "{before:?}");

    let opening = app.guided_review_workspace.pull_request_opening.as_mut().expect("preview");
    let mut editor = crate::editor::Editor::new();
    editor.insert_text("First line\nSecond line");
    opening.editing = Some((OpeningField::Body, editor));
    let (rows, _, before) = frame_with_cursor(&mut app, 150, 44);
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("First line") && has("Second line"), "{}", rows.join("\n"));
    assert!(before.ends_with("Second line"), "{before:?}");
    assert!(has("ctrl+enter save · esc cancel"), "{}", rows.join("\n"));

    // A long description scrolls, and says so
    let opening = app.guided_review_workspace.pull_request_opening.as_mut().expect("preview");
    opening.editing = None;
    let prepared = opening.prepared.as_mut().expect("prepared");
    prepared.pull_request.body =
        (1..=80).map(|n| format!("- item {n}")).collect::<Vec<_>>().join("\n");
    let rows = frame_rows(&mut app, 150, 44);
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("item 1") && !has("item 80"), "{}", rows.join("\n"));
    assert!(has("↑/↓ scroll"), "{}", rows.join("\n"));
    assert!(has("Into main from 123-category-enum · commits and"), "{}", rows.join("\n"));
    app.update(crate::app::Msg::GuidedReviewScrollPullRequest(1000));
    let rows = frame_rows(&mut app, 150, 44);
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("item 80") && !has("item 1 "), "{}", rows.join("\n"));
    app.update(crate::app::Msg::GuidedReviewScrollPullRequest(-1));
    let rows = frame_rows(&mut app, 150, 44);
    assert!(!rows.iter().any(|row| row.contains("item 80")), "one row up moves at once");

    let opening = app.guided_review_workspace.pull_request_opening.as_mut().expect("preview");
    let prepared = opening.prepared.as_mut().expect("prepared");
    prepared.pull_request.body = "  ".into();
    prepared.commits.clear();
    prepared.uncommitted = 0;
    prepared.publication = luxe_core::github::Publication::Pushed;
    let rows = frame_rows(&mut app, 150, 44);
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("(no description)") && !has("first"), "nothing to do first: {}", rows.join("\n"));
    assert!(!has("left out"), "{}", rows.join("\n"));
    assert!(!has("scroll"), "{}", rows.join("\n"));

    let opening = app.guided_review_workspace.pull_request_opening.as_mut().expect("preview");
    opening.prepared = None;
    let rows = frame_rows(&mut app, 150, 44);
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("Drafting it from the review…") && has("esc cancel"), "{}", rows.join("\n"));
    assert!(!has("Enter try again"), "{}", rows.join("\n"));

    let opening = app.guided_review_workspace.pull_request_opening.as_mut().expect("preview");
    opening.error = Some("You're on `main`, the branch it would merge into.".into());
    let rows = frame_rows(&mut app, 150, 44);
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("You're on main,") && !has("Drafting"), "{}", rows.join("\n"));
    assert!(has("enter try again · esc cancel"), "{}", rows.join("\n"));
}

/// The offer to fix a pull request says where it goes and what goes with it, while GitHub is
/// asked says so, and a refusal says why and offers nothing to confirm.
#[test]
fn the_fix_offer_says_where_it_goes_or_why_not() {
    let mut app =
        seeded_app("nord", FixtureScenario::GuidedReviewFixPullRequest).expect("fixture app");
    let rows = frame_rows(&mut app, 150, 44);
    let text = rows.join("\n");
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("Fix in a worktree") && has("Checks #42 out on fix-redirects"), "{text}");
    assert!(has("with your 3 comments for the agent to address."), "{text}");
    assert!(has("enter check out · esc cancel"), "{text}");
    let top = rows.iter().position(|row| row.contains("Fix in a worktree")).expect("top");
    let bottom = rows.iter().position(|row| row.contains("esc cancel ")).expect("bottom");
    assert!(rows[bottom - 2].contains("come back to this session at any time"), "tight: {text}");
    assert!(top > 5 && top.abs_diff(rows.len() - 1 - bottom) <= 3, "centered: {text}");
    let corner = rows[top].chars().position(|c| matches!(c, '╭' | '┌')).expect("corner");
    let right = 150
        - 1
        - rows[top]
            .chars()
            .collect::<Vec<_>>()
            .iter()
            .rposition(|c| matches!(c, '╮' | '┐'))
            .unwrap();
    assert!(corner > 10 && corner.abs_diff(right) <= 1, "{corner} vs {right}");

    for (comments, said) in [(1, "with your comment for the agent to address."), (0, "│ session.")]
    {
        app.guided_review_workspace.pull_request_fix.as_mut().unwrap().comments = comments;
        let rows = frame_rows(&mut app, 150, 44);
        assert!(rows.iter().any(|row| row.contains(said)), "{}", rows.join("\n"));
    }

    let fix = app.guided_review_workspace.pull_request_fix.as_mut().unwrap();
    fix.branch = None;
    let rows = frame_rows(&mut app, 150, 44);
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("Asking GitHub whether a fix could reach #42") && has("esc close"));

    let fix = app.guided_review_workspace.pull_request_fix.as_mut().unwrap();
    fix.refusal = Some("The author did not allow edits by maintainers.".into());
    let rows = frame_rows(&mut app, 150, 44);
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("did not allow edits") && has("esc close") && !has("Enter check out"));
    assert!(!has("Asking GitHub"), "{}", rows.join("\n"));
}

/// The stack preview lists every pull request with its branch, says what happens first, and
/// says drafts, drafting, opening and a failure as each comes.
#[test]
fn the_stack_preview_lists_each_pull_request_and_what_happens_first() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewOpenStack).expect("fixture app");
    let rows = frame_rows(&mut app, 150, 44);
    let text = rows.join("\n");
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(
        has("3 pull requests into main, each based on the one below · commits and pushes first"),
        "{text}"
    );
    assert!(!has("ready for review"), "{text}");
    assert!(has("1. 123-category-enum-part-1 — Shared notification model"), "{text}");
    assert!(has("3. 123-category-enum — Reject unknown categories"), "{text}");
    assert!(has("enter open · d draft · esc cancel"), "{text}");
    let top = rows.iter().position(|row| row.contains("Open as a stack")).expect("top");
    let bottom = rows.iter().position(|row| row.contains("esc cancel ")).expect("bottom");
    assert!(rows[bottom - 2].contains("Reject unknown categories"), "tight: {text}");
    assert!(top > 5 && top.abs_diff(rows.len() - 1 - bottom) <= 3, "centered: {text}");
    let corner = rows[top].chars().position(|c| matches!(c, '╭' | '┌')).expect("corner");
    let edge: Vec<char> = rows[top].chars().collect();
    let right = 150 - 1 - edge.iter().rposition(|c| matches!(c, '╮' | '┐')).unwrap();
    assert!(corner > 10 && corner.abs_diff(right) <= 1, "{corner} vs {right}");

    let stack = app.guided_review_workspace.stack_opening.as_mut().unwrap();
    stack.draft = true;
    let rows = frame_rows(&mut app, 150, 44);
    assert!(rows.iter().any(|row| row.contains("each based on the one below · as drafts")));

    let stack = app.guided_review_workspace.stack_opening.as_mut().unwrap();
    stack.opening = true;
    let rows = frame_rows(&mut app, 150, 44);
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("Opening the stack…") && has("opening…"), "{}", rows.join("\n"));

    let stack = app.guided_review_workspace.stack_opening.as_mut().unwrap();
    stack.opening = false;
    stack.layers = None;
    let rows = frame_rows(&mut app, 150, 44);
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("Drafting a pull request per chapter") && has("esc close"));

    let stack = app.guided_review_workspace.stack_opening.as_mut().unwrap();
    stack.error = Some("One chapter is one pull request.".into());
    let rows = frame_rows(&mut app, 150, 44);
    let has = |phrase: &str| rows.iter().any(|row| row.contains(phrase));
    assert!(has("One chapter is one pull request.") && !has("Drafting"));
}
