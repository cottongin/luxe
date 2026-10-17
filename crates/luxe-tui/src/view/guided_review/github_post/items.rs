//! Post items — what the post will carry, where each item lands on GitHub, and the list of them.

use crate::app::App;
use crate::guided_review::diff::clip_terminal_cells_ellipsis;
use luxe_core::guided_review::{BodyReason, DiffSide, FindingId, Placement, PostPlan};
use ratatui::text::{Line, Span};

/// One thing the review will post: the review's own text, or a comment on a line.
pub(super) struct Item {
    /// What the pane heads it with: `Summary`, or where on the diff it lands
    pub(super) place: String,
    /// Where a line comment sits, `None` for the summary
    pub(super) on: Option<OnLines>,
    /// The text GitHub will show, as markdown; empty for a review with no words of its own
    pub(super) body: String,
}

/// The lines a comment sits on.
pub(super) struct OnLines {
    pub(super) path: String,
    pub(super) side: DiffSide,
    pub(super) start: u32,
    pub(super) end: u32,
}

impl OnLines {
    fn of(place: &Placement) -> Option<Self> {
        match place {
            Placement::Line { path, line, side } => {
                Some(Self { path: path.clone(), side: *side, start: *line, end: *line })
            }
            Placement::Lines { path, start, end, side } => {
                Some(Self { path: path.clone(), side: *side, start: *start, end: *end })
            }
            Placement::Body(_) => None,
        }
    }

    /// What the list calls it under its file's name: `line 13`, `lines 12–14 (before)`.
    fn short(&self) -> String {
        let before = if self.side == DiffSide::Old { " (before)" } else { "" };
        if self.start == self.end {
            format!("line {}{before}", self.end)
        } else {
            format!("lines {}–{}{before}", self.start, self.end)
        }
    }
}

/// The review, then each comment on a line, grouped by file the way GitHub lists them: files in
/// the order their first comment was written, and down each file by line.
///
/// A comment placed in the review's body is part of the review's text on GitHub, under a heading
/// that says where it was written, so it is read there rather than listed a second time.
pub(super) fn items(app: &App, plan: &PostPlan) -> Vec<Item> {
    let body_of = |id: &FindingId| {
        app.review
            .session()
            .and_then(|session| session.findings.iter().find(|finding| &finding.id == id))
            .map_or_else(String::new, |finding| finding.body.trim().to_owned())
    };
    let mut lines: Vec<Item> = plan
        .placed
        .iter()
        .filter_map(|placed| {
            let on = OnLines::of(&placed.place)?;
            Some(Item {
                place: place(&placed.place),
                on: Some(on),
                body: body_of(&placed.finding_id),
            })
        })
        .collect();
    let mut files: Vec<String> = Vec::new();
    for item in &lines {
        let path = &item.on.as_ref().expect("a line comment").path;
        if !files.contains(path) {
            files.push(path.clone());
        }
    }
    lines.sort_by_key(|item| {
        let on = item.on.as_ref().expect("a line comment");
        (files.iter().position(|file| file == &on.path), on.start)
    });
    let summary =
        Item { place: "Summary".to_owned(), on: None, body: plan.review.body.trim().to_owned() };
    std::iter::once(summary).chain(lines).collect()
}

/// Where a comment lands, as a reader of GitHub's page would look for it.
fn place(place: &Placement) -> String {
    let before = |side: &DiffSide| if *side == DiffSide::Old { " (before)" } else { "" };
    match place {
        Placement::Line { path, line, side } => format!("{path}:{line}{}", before(side)),
        Placement::Lines { path, start, end, side } => {
            format!("{path}:{start}–{end}{}", before(side))
        }
        Placement::Body(BodyReason::WholeReview) => "on the review".to_owned(),
        Placement::Body(BodyReason::Chapter(title)) => format!("under “{title}”"),
        Placement::Body(BodyReason::OffDiff { path, lines }) => {
            format!("{path}:{lines}, outside GitHub's diff")
        }
    }
}

/// The list's rows: the summary, then each file's name over the comments on it. Each row carries
/// the item it selects, or `None` for a file's name, which selects nothing.
pub(super) fn list_lines(
    app: &App,
    items: &[Item],
    selected: usize,
    width: usize,
) -> Vec<(Option<usize>, Line<'static>)> {
    let styles = &app.theme_styles;
    let room = width.saturating_sub(crate::widgets::SELECTION_FRAME);
    let mut rows = Vec::new();
    let mut file: Option<&str> = None;
    for (index, item) in items.iter().enumerate() {
        let label = match &item.on {
            None => item.place.clone(),
            Some(on) => {
                if file != Some(on.path.as_str()) {
                    file = Some(on.path.as_str());
                    let name = clip_terminal_cells_ellipsis(&on.path, room);
                    // In the column a selected row's label starts in
                    let name = format!("{}{name}", " ".repeat(crate::widgets::SELECTION_INSET));
                    rows.push((None, Line::from(Span::styled(name, styles.muted))));
                }
                format!("  {}", on.short())
            }
        };
        let label = clip_terminal_cells_ellipsis(&label, room);
        rows.push((
            Some(index),
            crate::widgets::selection_row(
                vec![Span::styled(label, styles.text)],
                width,
                index == selected,
                styles,
            ),
        ));
    }
    rows
}
