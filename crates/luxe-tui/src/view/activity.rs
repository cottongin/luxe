//! Activity — the row above the composer: the mode button, then the turn's activity bar, a copy
//! toast, or a hint.

use crate::app::App;
use ratatui::{
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// Cells the activity bar occupies.
///
/// Wide enough for the comet to have somewhere to travel, narrow enough to leave the elapsed time
/// and the interrupt hint on the same row in a modest terminal. Packed, this is also the unit count:
/// twenty-four cells give the gradient enough steps to read as a flow rather than as three lit
/// blocks shuffling along.
const ACTIVITY_BAR_WIDTH: usize = 24;

/// Renders the live activity indicator above the composer during a turn.
///
/// This is the conversation's own footer, not a sidebar statistic: it reads as the last thing on
/// screen before the prompt, so the eye finds "still working" exactly where it left the output. The
/// sweeping bar carries that on its own — the word "working" beside a spinner said nothing the
/// motion didn't — so only the two informative bits follow it: how long the turn has been running,
/// and the chord that stops it. Running tool cards keep their own spinner and name.
///
/// When idle it carries one thing: an invitation to review what has piled up unreviewed. Quiet by
/// design — one line here and one in the dock, never a modal, never an auto-opened review — because
/// the reviewable unit is the increment and the only way that stays small is if the count is always
/// in view.
///
/// The mode button always leads the row, so everything else here starts to its right.
pub(super) fn render_composer_activity(f: &mut Frame, area: Rect, app: &App) {
    let badge = super::input::render_mode_badge(f, area, app);
    let area = Rect { x: area.x + badge, width: area.width - badge, ..area };
    if crate::statusline::activity_parts(app).is_none() {
        if let Some(line) =
            unreviewed_invitation(app).or_else(|| first_steps(app, usize::from(area.width)))
        {
            f.render_widget(Paragraph::new(line), area);
        }
        return;
    }
    let styles = &app.theme_styles;
    let mut spans = vec![Span::raw(" ")];
    spans.extend(activity_bar_spans(app, ACTIVITY_BAR_WIDTH.min(usize::from(area.width) / 3)));
    if let Some(elapsed) = app.status.elapsed(app.frame_now()) {
        spans.push(Span::styled(
            format!("  {}", crate::statusline::format_elapsed_compact(elapsed)),
            styles.muted,
        ));
    }
    if let Some((attempt, attempts)) = app.status.retrying {
        spans.push(Span::styled(format!(" · retrying ({attempt}/{attempts})"), styles.warning));
    }
    if let Some(chord) = interrupt_chord(app) {
        spans.push(Span::styled(format!(" · {chord} to interrupt"), styles.muted));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// `▪ Copied to clipboard`, right-aligned in the row above the prompt, for a few seconds.
///
/// A bold accent chip rather than plain text: it has to catch an eye that is looking somewhere
/// else, and it shares its row with the activity bar, so it needs an edge of its own. Clipped
/// rather than dropped on a narrow terminal — a partial confirmation still confirms.
pub(super) fn render_toast(f: &mut Frame, area: Rect, app: &App) {
    let Some(message) = app.status.toast().filter(|_| area.width > 0) else {
        return;
    };
    let styles = &app.theme_styles;
    let label = format!(" {message} ");
    let width =
        u16::try_from(crate::selection::cell_width(&label)).unwrap_or(area.width).min(area.width);
    let toast = Rect { x: area.right() - width, width, ..area };
    let style = styles.accent_on(true).add_modifier(Modifier::BOLD);
    f.render_widget(Paragraph::new(Line::from(Span::styled(label, style))), toast);
}

/// `12 changes since your last review · Ctrl+G` — or nothing when there is nothing waiting.
///
/// Counted at file granularity from the same record the dock reads, so the two never disagree. Shown
/// only when a review has actually happened before: on a first pass "everything is unreviewed" is
/// true and useless.
fn unreviewed_invitation(app: &App) -> Option<Line<'static>> {
    let styles = &app.theme_styles;
    let changes = app.workspace.as_ref().map(|snapshot| &snapshot.changes)?;
    if changes.total_files == 0 {
        return None;
    }
    let seen = app.review.seen.as_ref().filter(|seen| !seen.is_empty())?;
    let unreviewed = changes
        .files
        .iter()
        .filter(|file| {
            !seen.covers_file(
                &file.path.to_string_lossy(),
                file.additions.unwrap_or_default(),
                file.deletions.unwrap_or_default(),
            )
        })
        .count();
    if unreviewed == 0 {
        return None;
    }
    let files = if unreviewed == 1 { "file" } else { "files" };
    Some(Line::from(vec![
        Span::raw(" "),
        Span::styled(format!("{unreviewed} {files} changed since your last review"), styles.muted),
        Span::styled("  ·  ctrl+g to review", styles.accent),
    ]))
}

/// `shift+tab for plan mode · / or ctrl+k for commands · @ to reference files, skills, and more ·
/// /help for keys`, in a conversation that has not started.
///
/// Nothing else on screen says the composer has doors: the palette, the `/` list and the `@`
/// picker are all invisible until you know the key. So the idle row names them until the first
/// message, and goes the moment there is one; a resumed session never shows it, and nor does
/// anything with a reply or a tool card in it. It lives here
/// rather than in the transcript because it is about the composer, not the conversation, and
/// because nothing in the transcript can be un-said: a line there would outlive its use and ride
/// along in every copy.
///
/// The mode key and the palette's are read from the keymap, and each is left out when nothing is
/// bound. The mode hint names the mode it switches *to*, so it reads `build mode` while planning. On
/// a narrow terminal the tail goes first, so the mode and the commands are the last things it says.
/// A toast has the row to itself while it lasts.
fn first_steps(app: &App, width: usize) -> Option<Line<'static>> {
    // A toast shares the row, and is the more urgent thing to read while it lasts
    // Fresh means nothing in the conversation but notices, like the one `--issue` leaves
    let fresh =
        app.convo.iter().all(|item| matches!(item.block(), crate::app::ConvoBlock::System(_)));
    if !fresh || !app.pending_submissions.is_empty() || app.status.toast().is_some() {
        return None;
    }
    let styles = &app.theme_styles;
    let key = |text: &str| Span::styled(text.to_owned(), styles.accent);
    let words = |text: &str| Span::styled(text.to_owned(), styles.muted);
    let chord = |action| {
        app.keymap.chords_for(action).and_then(|chords| chords.first()).map(ToString::to_string)
    };
    let palette = chord(luxe_core::keymap::UiAction::OpenPalette);
    let mode = app
        .mode_switch_key()
        .map(|(chord, next)| vec![key(&chord), words(&format!(" for {next} mode"))]);
    let mut commands = vec![key("/")];
    if let Some(palette) = palette {
        commands.extend([words(" or "), key(&palette)]);
    }
    commands.push(words(" for commands"));
    let segments = mode.into_iter().chain([
        commands,
        // "and more" rather than a list: the picker's tabs grow with MCP servers and sources of your
        // own, and a hint that names three of them undersells the rest
        vec![key("@"), words(" to reference files, skills, and more")],
        vec![key("/help"), words(" for keys")],
    ]);
    let separator = || words("  ·  ");
    let width_of = |spans: &[Span]| spans.iter().map(|span| span.width()).sum::<usize>();
    // The leading pad the review invitation starts with, so the two sit on the same column
    let mut spans = vec![Span::raw(" ")];
    for (index, segment) in segments.into_iter().enumerate() {
        let joint = usize::from(index > 0) * separator().width();
        if width_of(&spans) + joint + width_of(&segment) > width {
            break;
        }
        if index > 0 {
            spans.push(separator());
        }
        spans.extend(segment);
    }
    (spans.len() > 1).then(|| Line::from(spans))
}

/// One row of the sweeping activity bar, as spans that can share a line with text.
///
/// The comet sweeps in shades of the theme accent, or of plan's hue while the session plans, and the
/// rail it travels is lit by the comet rather than drawn at a fixed weight, so the indicator belongs
/// to the chrome and the motion is the only thing that draws the eye.
fn activity_bar_spans(app: &App, width: usize) -> Vec<Span<'static>> {
    use crate::widgets::ActivityGlyphs;
    use luxe_core::config::ActivityBarStyle;

    if width < 3 {
        return Vec::new();
    }
    let glyphs = match app.config.conversation.activity_bar.unwrap_or_default() {
        ActivityBarStyle::Bars | ActivityBarStyle::Unknown => ActivityGlyphs::BARS,
        ActivityBarStyle::Dots => ActivityGlyphs::DOTS,
        ActivityBarStyle::Off => return Vec::new(),
    };
    let styles = &app.theme_styles;
    crate::widgets::activity_bar(
        app.status.spinner_frame as u64,
        width,
        glyphs,
        crate::modes::activity_trail(&app.status.mode, styles),
        &styles.activity_rail,
    )
}

/// The key the user would actually press to stop the turn, read from the live key map so a
/// rebound cancel doesn't leave the hint lying.
fn interrupt_chord(app: &App) -> Option<String> {
    app.keymap
        .chords_for(luxe_core::keymap::UiAction::Cancel)
        .and_then(|chords| chords.first())
        .map(ToString::to_string)
}
