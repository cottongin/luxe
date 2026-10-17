//! Dock telemetry — the bottom-pinned footer: model and session telemetry, and the live activity.

use super::chrome::{section_heading_line, Section};
use super::row_width;
use crate::app::App;
use crate::glyphs::{icon_cells, icon_label, Icon};
use crate::statusline::{compact_number, thinking_label};
use crate::widgets::truncate_end as truncate_cells;
use crate::workspace_dock::DockPresentation;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// Builds the bottom-pinned runtime footer: MODEL telemetry and the live activity line, each
/// separated by a blank so the cluster reads next to the composer.
///
/// Returns the assembled footer plus the line index of each clickable row, present only when
/// telemetry is relocated here, which is when the statusline is hidden.
pub(super) fn footer_lines(
    app: &App,
    show_telemetry: bool,
    presentation: DockPresentation,
) -> (Vec<Line<'static>>, Vec<(usize, crate::mouse::MouseTarget)>) {
    let mut blocks: Vec<Vec<Line<'static>>> = Vec::new();

    let mut telemetry_block = None;
    if show_telemetry {
        let telemetry = telemetry_lines(app);
        if !telemetry.is_empty() {
            telemetry_block = Some(blocks.len());
            blocks.push(telemetry);
        }
    }

    if let Some(activity) = activity_line(app, presentation) {
        blocks.push(vec![activity]);
    }

    // Assemble with a blank between blocks, tracking where the telemetry block lands so the
    // model-name row (the line after its MODEL heading) can be published as a click target.
    //
    // A leading blank keeps the bottom-pinned footer off whatever the scrollable middle ends on: at
    // the wrong height its first heading landed directly under the previous section's last row and
    // the two read as one block.
    let mut lines = vec![Line::default()];
    let mut telemetry_start = None;
    for (index, block) in blocks.into_iter().enumerate() {
        if block.is_empty() {
            continue;
        }
        if lines.len() > 1 {
            lines.push(Line::default());
        }
        if Some(index) == telemetry_block {
            telemetry_start = Some(lines.len());
        }
        lines.extend(block);
    }
    // Nothing but the leading blank means no runtime state to show: drop it rather than reserving a
    // row for an empty footer.
    if lines.len() == 1 {
        lines.clear();
    }
    let rows = telemetry_start
        .map(|start| {
            vec![
                (start + MODEL_ROW, crate::mouse::MouseTarget::DockModelRow),
                (start + MODE_ROW, crate::mouse::MouseTarget::DockModeRow),
            ]
        })
        .unwrap_or_default();
    (lines, rows)
}

/// Where the model and mode rows sit in [`telemetry_lines`], under its heading.
const MODEL_ROW: usize = 1;
const MODE_ROW: usize = 3;

/// Builds the live activity line (spinner + label + elapsed) for the dock footer.
///
/// Only rendered in replacement, where the dock *is* the whole surface and there is no
/// conversation column to anchor the indicator to. Every other layout shows it at the bottom of
/// the conversation, just above the composer (see `view::render`).
fn activity_line(app: &App, presentation: DockPresentation) -> Option<Line<'static>> {
    if presentation != DockPresentation::Replacement {
        return None;
    }
    let (spinner, label) = crate::statusline::activity_parts(app)?;
    let styles = &app.theme_styles;
    let mut spans = vec![
        Span::styled(format!("{spinner} "), styles.accent.patch(styles.tool_bg)),
        Span::styled(label, styles.text.patch(styles.tool_bg)),
    ];
    if let Some(elapsed) = app.status.elapsed(app.frame_now()) {
        spans.push(Span::styled(
            format!(" · {}", crate::statusline::format_elapsed_compact(elapsed)),
            styles.muted.patch(styles.tool_bg),
        ));
    }
    // Follow-ups waiting behind this turn: say how many, so a queued message isn't invisible in
    // the one layout that hides the tray.
    let queued = app.queued_count();
    if queued > 0 {
        spans.push(Span::styled(
            format!(" · ⏸ {queued} queued"),
            styles.warning.patch(styles.tool_bg),
        ));
    }
    Some(Line::from(spans))
}

/// Narrowest the label column in the telemetry blocks gets: its width on the default rail.
///
/// Wide enough to clear the section badge above it, so no value sits under the closing cap of the
/// heading that introduces it, and wide enough for the one label that is not ours: the provider
/// serving the model takes this column, and a provider can be named anything (`foundry-anthropic`
/// is seventeen cells).
///
/// The room comes out of the value column, and the MODEL row spends what is left on a model's
/// name — so this number is a **split**, cell for cell, between two things that both elide. Across
/// the models.dev snapshot each cell returned to the name fits about three percent more display
/// names, while provider ids cluster well under nineteen and only a gateway prefix pushes one past
/// it. Nineteen is where a `foundry-`-prefixed provider still reads in full and the name keeps the
/// rest.
///
/// Every other value is bounded by what it reports — a percentage, a level, a price — or truncates
/// itself, as the token row does ([`token_value`]). `every_telemetry_row_fits_the_rail` holds every
/// end of that.
///
/// All of that is arithmetic about a forty-cell rail. A rail dragged wider has cells to spare, so
/// [`telemetry_label_width`] grows the column from here rather than holding it.
pub(super) const TELEMETRY_LABEL_WIDTH: usize = 19;

/// The label column for a `row`-wide dock whose longest label is `widest` cells.
///
/// The floor on the default rail, so that rail reads as it always has. Wider, the column grows to
/// seat a provider the floor would cut, and the gap before the values grows with the rail: two
/// cells of air read as a pair at forty cells and as a crowd at eighty. Never past two fifths of
/// the row, which is what keeps the model's name the thing a wide rail spends its room on.
pub(super) fn telemetry_label_width(row: usize, widest: usize) -> usize {
    let gap = match row {
        0..48 => 2,
        48..64 => 3,
        _ => 4,
    };
    (widest + gap).min(row * 2 / 5).max(TELEMETRY_LABEL_WIDTH)
}

/// Cells a telemetry value has left in a `row`-wide dock row, after a `column`-wide label.
pub(super) fn telemetry_value_width(row: usize, column: usize) -> usize {
    row.saturating_sub(column)
}

/// What the session spent, in the room the row has: `↑2.3M ↓210k ⟳98.7M`.
///
/// The only telemetry value with nothing bounding it — the rest report a percentage, a level, a
/// price, or truncate themselves — so it is the one that decides how wide the label column beside
/// it may be. Rather than size that column around three figures nobody will ever see at once, the
/// row gives up its **third** datum when all three will not fit: cache reuse is a footnote to the
/// two counts it is measured against, it is already drawn only when there is any, and losing it is
/// cheaper than a row that wraps — which would be two rows where the grid promised one, shifting a
/// block pinned to the bottom of the rail.
pub(super) fn token_value(totals: &luxe_core::usage::UsageTotals, room: usize) -> String {
    let spent = format!(
        "↑{} ↓{}",
        compact_number(totals.prompt_tokens),
        compact_number(totals.completion_tokens)
    );
    if totals.cache_read_tokens == 0 {
        return spent;
    }
    let cached =
        format!(" {}{}", crate::glyphs::Mark::CacheReuse, compact_number(totals.cache_read_tokens));
    match spent.width() + cached.width() <= room {
        true => spent + &cached,
        false => spent,
    }
}

/// The bottom-pinned runtime blocks: MODEL and SESSION.
///
/// Two sections, because they answer two questions. MODEL is *identity and setting* — what is
/// answering, and how hard it is thinking. SESSION is *consumption* — how full the window is, how
/// many tokens went through it, what that cost. The window leads SESSION because it is the only row
/// with a deadline attached: the rest is a receipt, and this is the one about to make something
/// happen. What keeps it from reading as more accounting is its color, which is the datum.
///
/// Every metric row is the same shape: a label naming it, then a value that leads with its own
/// mark. The MODEL block is always here — a session with nothing chosen is a state worth reporting,
/// and its row is the pointer's way into the picker; SESSION waits for a turn to measure, so an
/// idle, pre-turn dock stays short.
pub(super) fn telemetry_lines(app: &App) -> Vec<Line<'static>> {
    let status = &app.status;
    // Who is serving it takes the label column the rest of the block uses, so MODEL reads as one
    // grid with SESSION rather than as a headline with rows under it — and it is the one thing
    // about the model that its name does not say. Until the engine has announced a model there
    // is no provider to name, so the row falls back to saying what it is; leaving the column
    // empty would put the value somewhere no other row's value sits.
    let served_by = if status.provider.is_empty() { "model" } else { status.provider.as_str() };
    let row = row_width(app);
    let column = telemetry_label_width(row, served_by.width());
    let mut lines = model_telemetry(app, served_by, row, column);
    lines.extend(session_telemetry(app, row, column));
    lines
}

/// The MODEL block: who serves it, the model, its thinking level, the mode, and auto-approval.
fn model_telemetry(app: &App, served_by: &str, row: usize, column: usize) -> Vec<Line<'static>> {
    let styles = &app.theme_styles;
    let status = &app.status;
    let mut lines: Vec<Line<'static>> = Vec::new();

    lines.push(section_heading_line(Section::Model, app));
    let room = telemetry_value_width(row, column).saturating_sub(icon_cells(Icon::Model.glyph()));
    // Nothing chooses a model for the reader, so this row says which it is either way — and stays
    // the affordance that opens the picker, which is the whole reason a reader with none needs it.
    let (name, ink) = match status.model.is_empty() {
        true => ("no model".to_string(), styles.warning),
        false => (truncate_cells(&status.model, room), styles.text),
    };
    lines.push(telemetry_affordance(
        served_by,
        icon_label(Icon::Model.glyph(), &name),
        ink,
        column,
        app,
        &crate::mouse::MouseTarget::DockModelRow,
    ));
    lines.push(telemetry_row(
        "thinking",
        icon_label(Icon::Thinking.glyph(), thinking_label(&status.thinking)),
        styles.text,
        column,
        app,
    ));
    // Colored only in plan mode, the way the bar draws it; and a click switches, as the title does
    let mode = status.mode.as_str();
    let mode_ink = match mode {
        luxe_core::mode::BUILD => styles.text,
        _ => crate::modes::accent(mode, styles),
    };
    lines.push(telemetry_affordance(
        "mode",
        icon_label(crate::modes::icon(mode).glyph(), mode),
        mode_ink,
        column,
        app,
        &crate::mouse::MouseTarget::DockModeRow,
    ));
    // The rail carries this only because it is here that the bar is not: telemetry is relocated
    // when the statusline is hidden, and a session approving its own escalations must not be able
    // to hide that by the reader turning a bar off. Drawn only while it is on, like the bar's.
    if app.permission_auto_approve {
        lines.push(telemetry_row(
            "approvals",
            icon_label(Icon::Permission.glyph(), "auto-approve"),
            styles.warning,
            column,
            app,
        ));
    }
    lines
}

/// The SESSION block, once a turn has something to measure: the window, tokens, cost, and what
/// the review and compaction spent of it.
fn session_telemetry(app: &App, row: usize, column: usize) -> Vec<Line<'static>> {
    let styles = &app.theme_styles;
    let status = &app.status;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let totals = &status.session.total;
    let cost = status.session_cost_label();
    let has_tokens = totals.prompt_tokens > 0 || totals.completion_tokens > 0;
    if !(status.context_window > 0 || has_tokens || cost.is_some()) {
        return lines;
    }
    lines.push(Line::default());
    lines.push(section_heading_line(Section::Session, app));
    if status.context_window > 0 {
        // The one row whose value is also a warning, so it takes the severity hue rather than
        // body text. It wears the statusline's context glyph rather than one of its own,
        // because the bar and the rail are two forms of one surface and this is one reading.
        let percent = status.context_pct.clamp(0.0, 100.0);
        lines.push(telemetry_row(
            "context",
            icon_label(
                Icon::Context.glyph(),
                &format!("{percent:.0}% of {}", compact_number(u64::from(status.context_window))),
            ),
            styles.context_fill(percent),
            column,
            app,
        ));
    }
    if has_tokens {
        lines.push(telemetry_row(
            "tokens",
            token_value(totals, telemetry_value_width(row, column)),
            styles.text,
            column,
            app,
        ));
    }
    if let Some(cost) = cost {
        lines.push(telemetry_row("cost", cost, styles.text, column, app));
    }
    lines.extend(source_cost_rows(app, column));
    lines
}

/// What the review and compaction cost, each a dimmed share of the session's total.
fn source_cost_rows(app: &App, column: usize) -> Vec<Line<'static>> {
    let styles = &app.theme_styles;
    let status = &app.status;
    let mut lines: Vec<Line<'static>> = Vec::new();
    // What the review and automatic compaction cost, named: "is the review expensive?" is a fair
    // question that deserves a number rather than a guess.
    // Dimmed, because each is a share of the total above rather than another bill.
    for entry in &status.session.by_source {
        if entry.source == luxe_core::usage::UsageSource::Agent {
            continue;
        }
        // The review's row waits for its spend to settle rather than counting up through it
        let totals = match entry.source {
            luxe_core::usage::UsageSource::Review if review_spending(app) => {
                status.review_held.as_ref()
            }
            _ => Some(&entry.totals),
        };
        if let Some(cost) = totals.and_then(luxe_core::usage::UsageTotals::cost_label) {
            lines.push(telemetry_row(entry.source.label(), cost, styles.dim, column, app));
        }
    }
    lines
}

/// Whether the review is spending right now: its guide being written, or its summary drafted.
fn review_spending(app: &App) -> bool {
    let writing = app.review.queued.as_ref().is_some_and(|queued| {
        matches!(queued.state, crate::app::QueuedReviewState::Writing { .. })
    });
    let drafting = app
        .guided_review_workspace
        .github_post
        .as_ref()
        .is_some_and(|post| post.drafting.is_some());
    writing || drafting
}

/// One `label   value` telemetry row: a muted label column, then the value in `ink`.
///
/// `ink` is body text for a measurement, dim for a share of one above it, and the severity hue for
/// the window, which is the one value that is also a warning.
fn telemetry_row(
    label: &str,
    value: String,
    ink: ratatui::style::Style,
    column: usize,
    app: &App,
) -> Line<'static> {
    Line::from(telemetry_spans(label, value, ink, column, app))
}

/// The same row, clickable — highlighted while the pointer is over it so it reads as actionable.
///
/// The whole row lights up rather than the value alone: the label is half of what was clicked, and
/// a highlight that stopped at the column boundary would read as two rows.
fn telemetry_affordance(
    label: &str,
    value: String,
    ink: ratatui::style::Style,
    column: usize,
    app: &App,
    target: &crate::mouse::MouseTarget,
) -> Line<'static> {
    let styles = &app.theme_styles;
    let spans = telemetry_spans(label, value, ink, column, app);
    if app.pointer.hover.as_ref() != Some(target) {
        return Line::from(spans);
    }
    let hovered = styles.text.patch(styles.tool_bg).patch(styles.selected_bg);
    Line::from(
        spans.into_iter().map(|span| Span::styled(span.content, hovered)).collect::<Vec<_>>(),
    )
}

/// The two spans a telemetry row is made of.
///
/// A label too long for its column is cut rather than allowed to push the value out of it — the
/// grid is the whole point, and a provider can be named anything. Padded in cells rather than
/// characters for the same reason.
fn telemetry_spans(
    label: &str,
    value: String,
    ink: ratatui::style::Style,
    column: usize,
    app: &App,
) -> Vec<Span<'static>> {
    let styles = &app.theme_styles;
    let label = truncate_cells(label, column.saturating_sub(1));
    let pad = " ".repeat(column.saturating_sub(label.width()));
    vec![
        Span::styled(format!("{label}{pad}"), styles.muted.patch(styles.tool_bg)),
        Span::styled(value, ink.patch(styles.tool_bg)),
    ]
}
