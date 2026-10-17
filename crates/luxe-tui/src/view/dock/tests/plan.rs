//! Plan — the checklist, its order, its wrapping and its window.

use super::*;

pub(super) fn todo(subject: &str, status: TodoSummaryStatus) -> DockPlanItem {
    DockPlanItem { subject: subject.into(), status }
}

pub(super) fn checklist(statuses: &[TodoSummaryStatus]) -> DockPlan {
    let items: Vec<DockPlanItem> = statuses
        .iter()
        .enumerate()
        .map(|(index, status)| todo(&format!("step {index}"), *status))
        .collect();
    DockPlan {
        pending: items.iter().filter(|i| i.status == TodoSummaryStatus::Pending).count(),
        in_progress: items.iter().filter(|i| i.status == TodoSummaryStatus::InProgress).count(),
        completed: items.iter().filter(|i| i.status == TodoSummaryStatus::Completed).count(),
        total: items.len(),
        remaining: None,
        items,
    }
}

/// A plan is a sequence, and ticking a box must not move it.
///
/// The rows used to be regrouped by status on every render, so finishing the third item sent it
/// to the bottom and pulled everything below it up a line — the list rearranged itself under a
/// reader who was working down it.
#[test]
fn the_checklist_keeps_the_order_it_was_written_in() {
    use TodoSummaryStatus::{Completed, InProgress, Pending};
    let app = app();
    let plan = checklist(&[Completed, InProgress, Pending, Completed, Pending]);
    let rows: Vec<String> = plan_lines(&plan, &app)
        .lines
        .iter()
        .map(text)
        .filter(|row| row.contains("step "))
        .collect();
    let subjects: Vec<&str> =
        rows.iter().map(|row| row.trim_end().rsplit_once("step ").expect("a subject").1).collect();
    assert_eq!(subjects, ["0", "1", "2", "3", "4"], "{rows:?}");
}

/// A long subject wraps under its own first word instead of being cut at the rail's edge, and
/// the rows for it come out of a budget that serves the work in hand first.
#[test]
fn a_long_subject_wraps_within_the_plans_budget() {
    use TodoSummaryStatus::{Completed, InProgress, Pending};
    let app = app();
    let long = "Survey the AI Hub codebases (dev/prod, backend/frontend): scale, architecture, \
                    test coverage and every integration that touches billing";
    let mut plan = checklist(&[Completed, InProgress, Pending]);
    for item in &mut plan.items {
        item.subject = long.to_string();
    }
    let rows: Vec<String> = plan_lines(&plan, &app).lines.iter().map(text).collect();
    let body = &rows[3..];
    assert_eq!(body.len(), 2 + 3 + 2, "done and next get two rows, the work in hand three");
    assert!(body[1].starts_with("│  "), "the connector carries down: {:?}", body[1]);
    assert!(body.last().is_some_and(|row| row.starts_with("   ")), "and closes at the end");
    assert!(body.iter().all(|row| row.width() <= DEFAULT_ROW), "{body:?}");
    assert!(body[1].ends_with('…'), "what does not fit is still cut: {:?}", body[1]);

    // A full window has fewer spare rows, and the work in hand is first to them
    let mut busy = checklist(&[Completed; 8]);
    busy.items[7].status = InProgress;
    for item in &mut busy.items {
        item.subject = long.to_string();
    }
    let grants = plan_row_grants(
        &busy.items.iter().collect::<Vec<_>>(),
        &busy.items.iter().map(|item| subject_rows(&item.subject, 35)).collect::<Vec<_>>(),
    );
    assert_eq!(grants[7], 3, "{grants:?}");
    assert_eq!(grants.iter().sum::<usize>(), PLAN_ROWS_MAX + PLAN_WRAP_ROWS, "{grants:?}");

    // And a subject that fits takes the one row it always did
    let short = checklist(&[Pending, Pending]);
    assert_eq!(plan_lines(&short, &app).lines.len(), 3 + 2);
}

/// Done is green *and* struck through; nothing else on the rail is.
#[test]
fn a_finished_row_is_struck_through_and_only_its_words_are() {
    let app = app();
    let done = todo_lines(
        &todo("ship it", TodoSummaryStatus::Completed),
        vec!["ship it".into()],
        true,
        30,
        1,
        &app,
    )
    .remove(0);
    let subject = done.spans.iter().find(|s| s.content.contains("ship it")).expect("subject");
    assert!(subject.style.add_modifier.contains(Modifier::CROSSED_OUT));
    assert_eq!(subject.style.fg, app.theme_styles.success.fg, "still green");

    // The tick itself is not struck: a struck ✓ reads as a mistake rather than a finished row.
    let tick = done.spans.iter().find(|s| s.content.contains('✓')).expect("a tick");
    assert!(!tick.style.add_modifier.contains(Modifier::CROSSED_OUT));

    for open in
        [TodoSummaryStatus::Pending, TodoSummaryStatus::InProgress, TodoSummaryStatus::Blocked]
    {
        let line =
            todo_lines(&todo("ship it", open), vec!["ship it".into()], true, 30, 1, &app).remove(0);
        let subject = line.spans.iter().find(|s| s.content.contains("ship it")).expect("subject");
        assert!(
            !subject.style.add_modifier.contains(Modifier::CROSSED_OUT),
            "{open:?} is not finished"
        );
    }
}

/// The follow position is anchored at the top and slides only to keep the work in hand in view.
#[test]
fn the_plan_window_follows_the_row_in_hand_without_leaving_the_top() {
    use TodoSummaryStatus::{Completed, InProgress, Pending};
    let at = |active: usize, total: usize| {
        let statuses: Vec<TodoSummaryStatus> = (0..total)
            .map(|index| match index.cmp(&active) {
                std::cmp::Ordering::Less => Completed,
                std::cmp::Ordering::Equal => InProgress,
                std::cmp::Ordering::Greater => Pending,
            })
            .collect();
        plan_follow(&checklist(&statuses).items)
    };

    // Everything fits: no window at all.
    assert_eq!(at(3, PLAN_ROWS_MAX), 0);

    // It stays pinned to the top while the active row and its lookahead still fit …
    assert_eq!(at(0, 20), 0);
    assert_eq!(at(PLAN_ROWS_MAX - PLAN_LOOKAHEAD - 1, 20), 0);

    // … then slides one row at a time, keeping the lookahead below the active row.
    assert_eq!(at(PLAN_ROWS_MAX - PLAN_LOOKAHEAD, 20), 1, "one row of history folded away");
    assert_eq!(at(PLAN_ROWS_MAX - PLAN_LOOKAHEAD + 1, 20), 2);

    // And it stops at the end rather than scrolling past it.
    assert_eq!(at(19, 20), 12);

    // A finished plan shows its tail, not its head.
    assert_eq!(plan_follow(&checklist(&[Completed; 20]).items), 12);
}

/// A checklist longer than the rail is a viewport onto the whole thing: the bar says where the
/// window sits, nothing counts what is outside it in words, and the active row survives.
#[test]
fn a_long_checklist_scrolls_under_a_bar_rather_than_counting_its_folds() {
    use TodoSummaryStatus::{Completed, InProgress, Pending};
    let app = app();
    let mut statuses = vec![Completed; 12];
    statuses[11] = InProgress;
    statuses.extend([Pending; 8]);
    let mut plan = checklist(&statuses);
    plan.remaining = Some(2);
    plan.total += 2;

    let section = plan_lines(&plan, &app);
    let rendered: Vec<String> = section.lines.iter().map(text).collect();
    let all = rendered.join("\n");
    assert!(!all.contains("earlier"), "the bar says where the window is: {all}");
    assert!(!all.contains("more"), "and what is below it: {all}");
    assert!(!all.contains("not listed"), "and the rail counts nothing in words: {all}");
    assert!(all.contains("step 11"), "the row in hand is never the one folded away: {all}");
    assert!(all.contains("step 13"), "and the work just after it stays in view: {all}");
    assert_eq!(rendered.iter().filter(|row| row.contains("step ")).count(), PLAN_ROWS_MAX);

    // The lane exists, spans exactly the todo rows, and gives them their two cells back.
    let list = section.list.expect("a checklist that outgrew the rail scrolls");
    assert_eq!(list.total, 20);
    assert_eq!(list.offset, plan_follow(&plan.items));
    assert_eq!(list.rows.1 - list.rows.0, PLAN_ROWS_MAX);
    for row in &rendered[list.rows.0..list.rows.1] {
        assert!(row.contains("step "), "the lane covers only the checklist: {row:?}");
    }

    // A checklist that fits gets no lane, and keeps its full width.
    let short = checklist(&[Completed, InProgress, Pending]);
    assert!(plan_lines(&short, &app).list.is_none());
}
