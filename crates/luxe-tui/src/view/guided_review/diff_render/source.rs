//! Review source rows — highlighting, intraline emphasis, hunk headers, line backgrounds and the
//! active bar.

use crate::app::App;
use crate::guided_review::diff::UnifiedDiffRow;
use crate::view::diff_lines::IntralineEmphasis;
use crate::view::emphasis;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// The full (unclipped) styled source spans for a diff line: syntax-highlighted for context, or the
/// line color with its changed spans marked for a replaced add/deletion. Owned spans so they can be
/// soft-wrapped independently of the borrowed source.
pub(super) fn full_source_spans(
    line: &luxe_core::guided_review::ReviewLine,
    changed: &[std::ops::Range<usize>],
    style: Style,
    file: &luxe_core::guided_review::ReviewFile,
    app: &App,
) -> Vec<Span<'static>> {
    let source = crate::tabstop::display_text(&line.text);
    match line.kind {
        luxe_core::guided_review::ReviewLineKind::Context => {
            highlighted_source_spans(&source, file.language.as_deref(), app)
        }
        _ => emphasis::spans(&source, changed, style, emphasis_style(app, style, line)),
    }
}

/// How a changed span is marked on `line`: a deeper wash of the row's own hue, and bold.
///
/// The wash stands down on a search hit. That row already wears the search highlight to say "this is
/// what you asked for", the match and the change are often the same token, and two backgrounds
/// competing inside one row leave neither legible. The mark still says what changed.
pub(super) fn emphasis_style(
    app: &App,
    base: Style,
    line: &luxe_core::guided_review::ReviewLine,
) -> Style {
    let added = matches!(line.kind, luxe_core::guided_review::ReviewLineKind::Addition);
    let style = app.theme_styles.diff_emphasis(base, added);
    if app.guided_review_workspace.line_matches_search(&crate::tabstop::display_text(&line.text)) {
        Style { bg: None, ..style }
    } else {
        style
    }
}

/// The intra-line marks for one rendered window, mapped from the review's own row space.
pub(super) fn intraline_emphasis(rows: &[UnifiedDiffRow<'_>]) -> IntralineEmphasis {
    let sources: Vec<Option<&luxe_core::guided_review::ReviewLine>> = rows
        .iter()
        .map(|row| match row {
            UnifiedDiffRow::Source(line) => Some(*line),
            _ => None,
        })
        .collect();
    crate::view::diff_lines::intraline_emphasis(&sources)
}

/// Cells at the margin reserved for the bar down the hunk being read.
pub(super) const ACTIVE_BAR_WIDTH: usize = 2;

/// The bar that marks the hunk under the cursor, or the blank that holds its column.
///
/// Spent on every row, selected or not: a marker that only appears under the cursor slides the
/// code sideways as the cursor arrives, which is the thing it exists to make legible.
pub(super) fn active_bar_span(app: &App, active: bool) -> Span<'static> {
    if active {
        Span::styled("\u{2503} ", app.theme_styles.accent)
    } else {
        Span::raw("  ")
    }
}

pub(super) fn highlighted_source_spans(
    text: &str,
    language: Option<&str>,
    app: &App,
) -> Vec<Span<'static>> {
    crate::view::diff_lines::highlighted_source_spans(text, language, &app.theme_styles)
}

/// The `@@ … @@ heading` row that opens a hunk, in either layout.
///
/// When something already vouches for the hunk the header says *what*, in the reviewer's own words
/// — `covered by \`cargo test\``, `formatting only`. Naming it is the difference between a reviewer
/// who can disagree with the harness and one who cannot see that it decided anything.
pub(super) fn hunk_header_line(
    app: &App,
    hunk_id: &luxe_core::guided_review::HunkId,
    old_range: luxe_core::guided_review::LineRange,
    new_range: luxe_core::guided_review::LineRange,
    heading: Option<&str>,
    active: bool,
) -> Line<'static> {
    use luxe_core::guided_review::ItemVerdict;
    let selected = app.guided_review_workspace.position.hunk_id.as_ref() == Some(hunk_id);
    let style = if selected {
        app.theme_styles.accent.patch(app.theme_styles.selected_bg)
    } else {
        app.theme_styles.accent
    };
    let marker = if selected { "› " } else { "  " };
    let mut spans = vec![
        active_bar_span(app, active),
        Span::styled(
            format!(
                "{marker}@@ -{},{} +{},{} @@ {}",
                old_range.start,
                old_range.count,
                new_range.start,
                new_range.count,
                heading.unwrap_or_default()
            ),
            style,
        ),
    ];
    if let ItemVerdict::Accounted(accounted) = app.review.budget.hunk_verdict(hunk_id) {
        spans.push(Span::styled(format!("   {}", accounted.label()), app.theme_styles.dim));
    }
    Line::from(spans)
}

pub(super) fn diff_line_background(
    kind: luxe_core::guided_review::ReviewLineKind,
    app: &App,
) -> Option<Style> {
    crate::view::diff_lines::diff_line_background(kind, &app.theme_styles)
}
