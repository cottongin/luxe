//! Review footer — the shortcut row under the diff: which keys it offers, fitted to the width, and
//! where each one's click lands.

use crate::app::App;
use crate::guided_review::diff::clip_terminal_cells;
use crate::guided_review::GuidedReviewLoadState;
use crate::mouse::ReviewFooterAction;
use ratatui::{
    layout::Rect,
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

/// A footer shortcut: its rendered label, the action a click dispatches, and how readily it yields
/// its cells. Higher `priority` drops first, so adding a hint can never push `Esc return` off the
/// row — the failure mode the previous hand-tuned width tiers had.
#[derive(Clone)]
pub(super) struct FooterSegment {
    pub(super) label: String,
    action: ReviewFooterAction,
    priority: u8,
}

fn segment(label: impl Into<String>, action: ReviewFooterAction, priority: u8) -> FooterSegment {
    FooterSegment { label: label.into(), action, priority }
}

/// The footer string, plus the clickable segments when the interactive "ready" footer applies.
pub(super) fn resolve_footer(app: &App, width: u16) -> (String, Option<Vec<FooterSegment>>) {
    // The in-diff search prompt replaces the footer while it is open.
    if let Some(query) = app.guided_review_workspace.search.as_ref() {
        let _ = width;
        return (format!("Search: {query}█ · enter/↓ next · ↑ previous · esc close"), None);
    }
    match &app.guided_review_workspace.load_state {
        GuidedReviewLoadState::Loading { .. } => ("esc back to chat".into(), None),
        GuidedReviewLoadState::Failed { error, .. }
            if app.guided_review.is_some() && width >= 90 =>
        {
            // Reserve the recovery actions, then spend only the remaining cells on the cause
            let detail_width = usize::from(width.saturating_sub(64)).min(40);
            let detail = clip_terminal_cells(&error.message, 0, detail_width);
            (
                format!("Refresh failed: {detail} · r retry · s change source · esc back to chat"),
                None,
            )
        }
        GuidedReviewLoadState::Failed { .. } if width < 60 => ("r retry · esc back".into(), None),
        GuidedReviewLoadState::Failed { .. } => {
            ("r retry · s change source · esc back to chat".into(), None)
        }
        GuidedReviewLoadState::Idle | GuidedReviewLoadState::Ready => {
            let mut segments = ready_footer_segments(app, width);
            // At a chapter boundary `n` is a dead key by design (movement is scoped so the
            // narrative always describes the diff), so name the key that does move.
            if let Some((label, action)) = chapter_boundary_hint(app, width) {
                segments.insert(0, segment(label, action, 3));
            }
            let segments = fit_footer_segments(segments, width);
            (footer_segments_text(&segments), Some(segments))
        }
    }
}

/// Joins segment labels the way the footer draws them.
fn footer_segments_text(segments: &[FooterSegment]) -> String {
    segments.iter().map(|segment| segment.label.as_str()).collect::<Vec<_>>().join(SEPARATOR)
}

/// Between two footer shortcuts, as between two keys in a panel's hint.
const SEPARATOR: &str = " · ";

/// Total cells `segments` occupy once joined.
pub(super) fn footer_segments_width(segments: &[FooterSegment]) -> usize {
    let labels: usize =
        segments.iter().map(|segment| UnicodeWidthStr::width(segment.label.as_str())).sum();
    labels + SEPARATOR.width() * segments.len().saturating_sub(1)
}

/// Drops the least important shortcuts until the row fits, so the footer degrades by priority
/// instead of being clipped mid-label (which also desynchronizes the click regions).
pub(super) fn fit_footer_segments(
    mut segments: Vec<FooterSegment>,
    width: u16,
) -> Vec<FooterSegment> {
    while segments.len() > 1 && footer_segments_width(&segments) > usize::from(width) {
        let drop = segments
            .iter()
            .enumerate()
            .max_by_key(|(index, segment)| (segment.priority, *index))
            .map(|(index, _)| index);
        match drop {
            Some(index) => {
                segments.remove(index);
            }
            None => break,
        }
    }
    segments
}

/// The same footer as spans, keys in the accent the way a panel's hint draws them, so the
/// shortcut under the pointer lights up like the button it is.
pub(super) fn footer_spans(app: &App, segments: &[FooterSegment], width: u16) -> Line<'static> {
    let _ = width;
    let styles = &app.theme_styles;
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(segments.len() * 3);
    for (index, segment) in segments.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(SEPARATOR, styles.muted));
        }
        let hovered = app.pointer.hover.as_ref()
            == Some(&crate::mouse::MouseTarget::ReviewFooter(segment.action));
        match hovered {
            true => spans.push(Span::styled(segment.label.clone(), styles.selected_bg)),
            false => spans.extend(hint_spans(&segment.label, styles)),
        }
    }
    Line::from(spans)
}

/// A footer that is not the ready one — a failed refresh, a search — drawn with the same key
/// styling. Its segments are split on the separator, and a segment is only a key and its words
/// when it starts with one, so a sentence among them stays a sentence.
pub(super) fn footer_hint_line(
    text: &str,
    styles: &crate::theme::styles::ThemeStyles,
) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, piece) in text.split(SEPARATOR).enumerate() {
        if index > 0 {
            spans.push(Span::styled(SEPARATOR, styles.muted));
        }
        spans.extend(hint_spans(piece, styles));
    }
    Line::from(spans)
}

/// One shortcut, `Alt+C feedback`: the key in the accent and the words in the body text. A piece
/// that does not open with a key reads as text.
fn hint_spans(label: &str, styles: &crate::theme::styles::ThemeStyles) -> Vec<Span<'static>> {
    match label.split_once(' ') {
        Some((key, words)) if is_key(key) => vec![
            Span::styled(key.to_owned(), styles.accent),
            Span::styled(format!(" {words}"), styles.text),
        ],
        _ => vec![Span::styled(label.to_owned(), styles.text)],
    }
}

/// Whether a label's first word names a key: a chord, a named key, or one character.
pub(super) fn is_key(word: &str) -> bool {
    const NAMED: [&str; 6] = ["space", "enter", "esc", "tab", "pgup", "pgdn"];
    word.contains('+') || NAMED.contains(&word) || word.contains('/') || word.chars().count() == 1
}

/// The interactive "ready" footer as ordered segments; [`fit_footer_segments`] trims it to the
/// width by priority.
pub(super) fn ready_footer_segments(app: &App, width: u16) -> Vec<FooterSegment> {
    use ReviewFooterAction as A;
    // What a reader needs to *work*, and nothing else: past about half a dozen nobody reads any of
    // them, and the ones that matter stop being findable. The layout, the guide, churn and the
    // rest are all still bound and all named by `? help`, the surface built to list them; the
    // churn keys are also in the guide's foot, beside the churn they open. Esc is the one key
    // nobody has to be told.
    //
    // Finishing is where everything that happens *after* reading lives — committing, opening a
    // pull request, posting — and nothing else on screen says it exists. The feedback window
    // earns its cells once there is feedback in it.
    let finish =
        format!("{} finish", chord(app, luxe_core::keymap::UiAction::ReviewFinish, "alt+enter"));
    let feedback =
        format!("{} feedback", chord(app, luxe_core::keymap::UiAction::ReviewFindings, "alt+c"));
    let has_feedback = app.review.session().is_some_and(|session| !session.findings.is_empty());
    let _ = width;
    let mut segments = vec![
        segment("space next", A::HunkNext, 2),
        segment("c comment", A::AddComment, 3),
        segment("a ask agent", A::AskQuestion, 5),
        segment(feedback, A::Feedback, 4),
        segment(finish, A::Finish, 2),
        segment("? help", A::ToggleHelp, 1),
    ];
    if !has_feedback {
        segments.retain(|cell| cell.action != A::Feedback);
    }
    segments
}

/// The first key bound to `action`, as the footer names keys, or `fallback` when nothing is bound.
pub(in crate::view) fn chord(
    app: &App,
    action: luxe_core::keymap::UiAction,
    fallback: &str,
) -> String {
    app.keymap
        .chords_for(action)
        .and_then(|chords| chords.first())
        .map_or_else(|| fallback.to_owned(), ToString::to_string)
}

/// The chapter-boundary hint, when the diff cursor sits on the first or last item the current
/// chapter owns. Scoped movement makes `n`/`N` a deliberate dead key there, so the footer names
/// the key that does move. Forward wins when a one-item chapter is both ends at once.
fn chapter_boundary_hint(app: &App, width: u16) -> Option<(String, ReviewFooterAction)> {
    let workspace = &app.guided_review_workspace;
    if width < 90 {
        return None;
    }
    let review = app.guided_review.as_ref()?;
    let index = review
        .chapters
        .iter()
        .position(|chapter| workspace.position.chapter_id.as_ref() == Some(&chapter.id))?;
    let long = width >= 120;
    if index + 1 < review.chapters.len() && workspace.at_scope_end(review) {
        return Some((
            if long { "] next chapter".into() } else { "] chapter".into() },
            ReviewFooterAction::ChapterNext,
        ));
    }
    if index > 0 && workspace.at_scope_start(review) {
        return Some((
            if long { "[ previous chapter".into() } else { "[ chapter".into() },
            ReviewFooterAction::ChapterPrev,
        ));
    }
    None
}

/// Publishes a click region for each footer segment, matching the left-aligned Paragraph layout.
pub(super) fn publish_footer_regions(app: &App, area: Rect, segments: &[FooterSegment]) {
    let separator_width = SEPARATOR.width() as u16;
    let label_width = |label: &str| UnicodeWidthStr::width(label) as u16;
    let mut x = area.x;
    let mut regions = app.hit.borrow_mut();
    for segment in segments {
        let cells = label_width(&segment.label);
        regions.push(
            Rect::new(x, area.y, cells, 1),
            crate::mouse::MouseTarget::ReviewFooter(segment.action),
        );
        x = x.saturating_add(cells).saturating_add(separator_width);
    }
}
