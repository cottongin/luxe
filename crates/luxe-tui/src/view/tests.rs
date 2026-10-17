use super::canvas::{PROSE_MAX_WIDTH, WIDE_BLOCK_MAX_WIDTH};
use super::transcript::layout::{image_row_cap, MIN_IMAGE_ROWS};
use super::transcript::reveal::{rendered_chars, revealed_lines};
use super::transcript::selection::line_text;
use super::transcript::RenderedConversationLine;
use super::*;
use crate::app::ConversationItemId;
use crate::{
    app::{ConversationAnchor, Msg},
    fixture::{seeded_app, FixtureScenario},
    markdown::MarkdownLine,
};
use luxe_core::config::ConversationAlignment;
use ratatui::text::{Line, Span};
use ratatui::{
    backend::TestBackend,
    style::{Color, Style},
    Terminal,
};
use std::rc::Rc;

/// Asserts that a frame served from the transcript memo is the same picture as one laid out
/// from scratch.
///
/// The memo is the difference between a keystroke costing nothing and costing the whole
/// session, and the only way it can be wrong is by serving a row that is out of date. So what
/// is checked is never that it was fast — it is that the two frames are identical, cell for
/// cell, after whatever was just done to the app.
pub(super) fn assert_memo_agrees_with_a_cold_layout(app: &mut App, size: (u16, u16), after: &str) {
    assert_memo_agrees_across(app, size, after, |_| {});
}

/// [`assert_memo_agrees_with_a_cold_layout`], with `between` applied to a warmed app.
///
/// The distinction matters for anything that moves without the transcript changing — a
/// spinner on the animation tick, say. Comparing two frames drawn from the same state can
/// never catch that; the memo has to be warmed, the clock moved, and *then* asked.
pub(super) fn assert_memo_agrees_across(
    app: &mut App,
    size: (u16, u16),
    after: &str,
    between: impl FnOnce(&mut App),
) {
    let (width, height) = size;
    let draw = |app: &mut App| {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
        terminal.draw(|frame| render(frame, app)).expect("a frame");
        terminal.backend().buffer().clone()
    };

    // Every frame below has to be drawn at the same instant, or the comparison is not about the
    // memo at all. An unpinned clock reads `Instant::now()` per draw and the turn timer renders
    // to a tenth of a second, so a fixture mid-turn need only straddle one boundary between the
    // warm frame and the cold one to fail as "the memo served a stale frame" — which it had
    // not. A test that wants the clock to move says so in `between`, which runs after this.
    app.status.frame_clock.get_or_insert_with(std::time::Instant::now);

    // The first frame fills the memo; the second is the one that reads from it.
    draw(app);
    between(app);
    let warm = draw(app);
    app.transcript = transcript::TranscriptLayout::default();
    let cold = draw(app);

    if warm != cold {
        let differing = warm
            .content
            .iter()
            .zip(cold.content.iter())
            .position(|(left, right)| left != right)
            .unwrap_or(0);
        let row = differing / usize::from(width);
        let text = |buffer: &ratatui::buffer::Buffer| {
            let start = row * usize::from(width);
            buffer.content[start..start + usize::from(width)]
                .iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
        };
        panic!(
            "the memo served a stale frame after {after}\n  row {row} warm: {:?}\n  row {row} \
                 cold: {:?}",
            text(&warm),
            text(&cold)
        );
    }
}

#[test]
fn guided_review_uses_medium_layout_at_ninety_terminal_columns() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("review fixture");
    let backend = TestBackend::new(90, 24);
    let mut terminal = Terminal::new(backend).expect("test terminal");

    terminal.draw(|frame| render(frame, &mut app)).expect("render top-level review");
    let text: String =
        terminal.backend().buffer().content.iter().map(|cell| cell.symbol()).collect();

    assert!(text.contains("01 / 01"), "the compact header still says where you are: {text}");
    assert!(!text.contains("01/01 · Shared notification model · src/notification.rs"));
}

#[test]
fn conversation_layout_captures_the_first_visible_semantic_line() {
    let item_id = Rc::new(ConversationItemId::Persisted {
        session_id: "session-a".into(),
        entry_id: "entry-1".into(),
        block_index: 0,
    });
    let lines = vec![
        RenderedConversationLine {
            item_index: 0,
            item_id: Rc::clone(&item_id),
            visual_line_within_item: 0,
            line: Rc::new(Line::raw("first")),
            code_copy: None,
            image: None,
            opens_image: None,
            expands: false,
        },
        RenderedConversationLine {
            item_index: 0,
            item_id: Rc::clone(&item_id),
            visual_line_within_item: 1,
            line: Rc::new(Line::raw("wrapped")),
            code_copy: None,
            image: None,
            opens_image: None,
            expands: false,
        },
    ];

    assert_eq!(
        anchors::capture(&lines, 1, 4),
        Some(ConversationAnchor {
            item_id: (*item_id).clone(),
            visual_line_within_item: 1,
            item_index: 0,
            screen_row: 4,
            scroll_offset: 1,
        })
    );
}

#[test]
fn semantic_anchor_restores_its_item_line_to_the_same_screen_row() {
    let item_id = Rc::new(ConversationItemId::Persisted {
        session_id: "session-a".into(),
        entry_id: "entry-1".into(),
        block_index: 0,
    });
    let lines = (0..6)
        .map(|visual_line_within_item| RenderedConversationLine {
            item_index: 0,
            item_id: Rc::clone(&item_id),
            visual_line_within_item,
            line: Rc::new(Line::raw("wrapped")),
            code_copy: None,
            image: None,
            opens_image: None,
            expands: false,
        })
        .collect::<Vec<_>>();
    let anchor = ConversationAnchor {
        item_id: (*item_id).clone(),
        visual_line_within_item: 4,
        item_index: 0,
        screen_row: 3,
        scroll_offset: 3,
    };

    assert_eq!(anchors::restore_offset(&lines, &anchor, 1, 3), Some(2));
}

#[test]
fn semantic_anchor_uses_the_nearest_surviving_item_after_removal() {
    let line = |item_index| RenderedConversationLine {
        item_index,
        item_id: Rc::new(ConversationItemId::Provisional {
            session_id: "session-a".into(),
            turn_id: item_index as u64,
            block_index: 0,
        }),
        visual_line_within_item: 0,
        line: Rc::new(Line::raw("line")),
        code_copy: None,
        image: None,
        opens_image: None,
        expands: false,
    };
    let lines = vec![line(0), line(2), line(4)];
    let anchor = ConversationAnchor {
        item_id: ConversationItemId::Provisional {
            session_id: "session-a".into(),
            turn_id: 3,
            block_index: 0,
        },
        visual_line_within_item: 0,
        item_index: 3,
        screen_row: 0,
        scroll_offset: 1,
    };

    assert_eq!(anchors::restore_offset(&lines, &anchor, 0, 2), Some(1));
}

/// The configured cap is a ceiling, not a size. A full-height window gets the whole 24 rows;
/// a terminal split four ways gets an image that still leaves conversation on screen.
#[test]
fn an_image_never_takes_more_than_half_the_transcript() {
    assert_eq!(image_row_cap(24, 64), 24, "a tall window is governed by the configured cap");
    assert_eq!(image_row_cap(24, 40), 20, "a shorter one by its own height");
    assert_eq!(image_row_cap(24, 20), 10);
    // Below the floor an image stops being a picture, so it stops shrinking instead.
    assert_eq!(image_row_cap(24, 8), MIN_IMAGE_ROWS);
    assert_eq!(image_row_cap(24, 0), MIN_IMAGE_ROWS);
    // An explicitly small cap is still an instruction.
    assert_eq!(image_row_cap(4, 64), 4);
    assert!(image_row_cap(0, 64) >= 1, "and something is always drawable");
}

#[test]
fn left_alignment_anchors_prose_and_code_to_the_gutter() {
    // Default policy: content flows from the left; prose/code share a left edge but keep
    // distinct readable widths, so wide terminals stop wasting the left margin.
    let wide = conversation_canvas_widths(200);
    assert_eq!(wide.available_width, 197);
    assert_eq!((wide.prose_width, wide.prose_x), (120, 1));
    assert_eq!((wide.wide_width, wide.wide_x), (140, 1));

    // Below the caps, content wraps two cells narrower than the band it sits in, so the
    // band keeps a cell of padding on each side of the text.
    let narrow = conversation_canvas_widths(78);
    assert_eq!(narrow.available_width, 75);
    assert_eq!((narrow.prose_width, narrow.prose_x), (73, 1));
    assert_eq!((narrow.wide_width, narrow.wide_x), (73, 1));
}

#[test]
fn center_alignment_splits_the_slack_evenly() {
    let wide = conversation_canvas_widths_with_limits(
        200,
        PROSE_MAX_WIDTH,
        WIDE_BLOCK_MAX_WIDTH,
        ConversationAlignment::Center,
    );
    // Centerd inside the padded content region (195 cells), not the whole band.
    assert_eq!((wide.prose_width, wide.prose_x), (120, 38));
    assert_eq!((wide.wide_width, wide.wide_x), (140, 28));
}

#[test]
fn full_alignment_fills_the_available_width() {
    let wide = conversation_canvas_widths_with_limits(
        200,
        PROSE_MAX_WIDTH,
        WIDE_BLOCK_MAX_WIDTH,
        ConversationAlignment::Full,
    );
    // "Full" still leaves the band's own padding: 197 available, 195 for text.
    assert_eq!((wide.prose_width, wide.prose_x), (195, 1));
    assert_eq!((wide.wide_width, wide.wide_x), (195, 1));
}

#[test]
fn wide_surface_wraps_prose_at_the_configured_canvas_maximum() {
    let canvas = conversation_canvas_widths(200);
    let prose = MarkdownLine::prose(Line::raw("readable ".repeat(30)));
    let wrapped = wrapping::conversation_line(prose, canvas);
    let left_padding = usize::from(canvas.prose_x - SAFE_GUTTER_WIDTH / 2);

    assert!(wrapped.len() > 1);
    assert!(wrapped
        .iter()
        .all(|line| line.width() <= left_padding + usize::from(canvas.prose_width)));
}

/// A slash command lights in the composer as it is typed: the trigger bold in the accent, a
/// word its forms spell out in the second accent, a value in the text's own color.
#[test]
fn the_composer_lights_a_slash_command_and_its_words() {
    let mut app = crate::fixture::seeded_app("nord", FixtureScenario::ChatIdle).expect("fixture");
    app.update(Msg::TerminalResized { width: 100, height: 30 });
    app.editor.insert_text("/diff pr 12");
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
    terminal.draw(|frame| render(frame, &mut app)).expect("frame");
    let buffer = terminal.backend().buffer().clone();
    let (x, y) = (0..30u16)
        .flat_map(|y| (0..89u16).map(move |x| (x, y)))
        .find(|&(x, y)| {
            (0..11u16).map(|i| buffer[(x + i, y)].symbol()).collect::<String>() == "/diff pr 12"
        })
        .expect("the draft is drawn");
    let styles = &app.theme_styles;
    let cell = |offset: u16| &buffer[(x + offset, y)];
    assert!(cell(1).modifier.contains(ratatui::style::Modifier::BOLD), "the trigger is bold");
    assert_eq!(cell(1).fg, styles.accent.fg.expect("an accent"), "in the accent");
    assert_eq!(cell(6).fg, styles.accent_secondary.fg.expect("a second accent"), "`pr`");
    assert_eq!(cell(9).fg, styles.text.fg.unwrap_or(cell(9).fg), "the value is text");
    assert_ne!(cell(9).fg, cell(6).fg);

    app.editor.cancel();
    app.editor.insert_text("/dfif pr");
    terminal.draw(|frame| render(frame, &mut app)).expect("frame");
    let buffer = terminal.backend().buffer().clone();
    assert!(
        !buffer[(x + 1, y)].modifier.contains(ratatui::style::Modifier::BOLD),
        "a typo is plain"
    );
}

/// A table in an answer fits whatever column it lands in, every row the same width. It used to
/// be laid out once at its natural width and then word-wrapped, which is what opening the
/// preview beside the conversation did to it.
#[test]
fn a_table_fits_the_canvas_it_lands_in() {
    let theme = crate::theme::styles::ThemeStyles::from_resolved(
        &luxe_core::theme::ThemeRegistry::new().resolve("nord").expect("nord"),
        "nord",
    );
    let source = "| Item | Status | Notes |\n|---|:-:|---|\n\
                      | Notifications | done | permission, question and review prompts notify |\n\
                      | Peek width | planned | cap the diff at code width so the transcript keeps room |";
    for area in [200u16, 90, 60, 40] {
        let canvas = conversation_canvas_widths(area);
        let left = usize::from(canvas.wide_x.saturating_sub(SAFE_GUTTER_WIDTH / 2));
        let rows: Vec<Line<'static>> = crate::markdown::layout_markdown(source, &theme)
            .into_iter()
            .flat_map(|line| wrapping::conversation_line(line, canvas))
            .collect();
        let grid: Vec<&Line<'static>> = rows
            .iter()
            .filter(|row| row.to_string().trim_start().starts_with(['┌', '│', '├', '└']))
            .collect();
        assert!(grid.len() >= 6, "{area}: still a grid: {rows:#?}");
        let width = grid[0].width();
        assert!(width <= left + usize::from(canvas.wide_width), "{area}: fits");
        assert!(grid.iter().all(|row| row.width() == width), "{area}: aligned {rows:#?}");
    }
}

#[test]
fn styled_wrap_preserves_text_and_cell_width() {
    let line = Line::from(vec![Span::styled("ab界cd", Style::default().fg(Color::Blue))]);
    let wrapped = wrapping::styled_line(line, 4);
    assert_eq!(wrapped.len(), 2);
    assert_eq!(wrapped[0].to_string(), "ab界");
    assert_eq!(wrapped[1].to_string(), "cd");
    assert!(wrapped.iter().all(|line| line.width() <= 4));
    assert!(wrapped
        .iter()
        .flat_map(|line| &line.spans)
        .all(|span| span.style.fg == Some(Color::Blue)));
}

#[test]
fn styled_wrap_prefers_word_boundaries() {
    let wrapped = wrapping::styled_line(Line::from("alpha beta gamma".to_string()), 10);
    assert_eq!(wrapped.iter().map(Line::to_string).collect::<Vec<_>>(), ["alpha beta", "gamma"]);
}

#[test]
fn styled_wrap_splits_embedded_newlines() {
    let wrapped = wrapping::styled_line(Line::from(Span::raw("alpha\nbeta".to_string())), 20);
    assert_eq!(wrapped.iter().map(Line::to_string).collect::<Vec<_>>(), ["alpha", "beta"]);
}

// === Reveal ===

fn theme() -> crate::theme::styles::ThemeStyles {
    let registry = luxe_core::theme::ThemeRegistry::new();
    let resolved = registry.resolve("dark").expect("the dark theme exists");
    crate::theme::styles::ThemeStyles::from_resolved(&resolved, "dark")
}

/// Three wrapped rows of prose, the way the transcript hands them to the reveal.
fn rows() -> Vec<Line<'static>> {
    ["the provider hands you text", "in lumps, so the render loop", "paints it in lumps too"]
        .into_iter()
        .map(|text| Line::from(Span::raw(text.to_string())))
        .collect()
}

/// The property that makes truncating rendered lines safe: every visible row is a prefix of the
/// row it will end up being. Nothing re-flows, nothing moves, text only accumulates.
#[test]
fn a_revealed_row_is_always_a_prefix_of_the_finished_row() {
    let styles = theme();
    let full = rows();
    let total = rendered_chars(&full);
    for shown in 0..=total {
        let partial = revealed_lines(full.clone(), shown, &styles, styles.background, 0.0);
        assert!(partial.len() <= full.len(), "revealing cannot add rows");
        for (index, line) in partial.iter().enumerate() {
            let text = line_text(line);
            let finished = line_text(&full[index]);
            assert!(
                finished.starts_with(&text),
                "row {index} at {shown}/{total} is not a prefix: {text:?} of {finished:?}"
            );
        }
        let visible: usize = partial.iter().map(|line| line_text(line).chars().count()).sum();
        assert_eq!(visible, shown, "exactly the revealed characters are drawn");
    }
}

#[test]
fn the_whole_item_survives_being_fully_revealed() {
    let styles = theme();
    let full = rows();
    let shown = rendered_chars(&full);
    let revealed = revealed_lines(full.clone(), shown, &styles, styles.background, 0.0);
    assert_eq!(
        revealed.iter().map(line_text).collect::<Vec<_>>(),
        full.iter().map(line_text).collect::<Vec<_>>()
    );
}

/// The newest character lands on the background — invisible — and the ones behind it resolve.
#[test]
fn the_newest_characters_fade_up_out_of_the_background() {
    let styles = theme();
    let canvas = styles.background.bg;
    let text = "a sentence long enough to have a settled beginning and a fresh end";
    let lines = vec![Line::from(Span::styled(text.to_string(), styles.text))];
    let shown = text.chars().count();
    let revealed = revealed_lines(lines, shown, &styles, styles.background, 0.0);

    let spans = &revealed[0].spans;
    let newest = spans.last().expect("the tail is drawn");
    assert_eq!(newest.style.fg, canvas, "the character that just arrived is invisible");
    assert_eq!(spans[0].style.fg, styles.text.fg, "and the settled start is at full color");
    assert!(spans.len() > 2, "the tail is a gradient, not a step: {} spans", spans.len());
    // Every character still there, in order — the fade only changes color.
    assert_eq!(line_text(&revealed[0]), text);
}

/// The bug this exists to prevent: a stream that stops mid-sentence must not leave its last
/// word painted on the background, where it is text the reader simply cannot see.
#[test]
fn a_settled_trail_is_drawn_at_full_contrast() {
    let styles = theme();
    let text = "a sentence long enough to have a settled beginning and a fresh end";
    let lines = vec![Line::from(Span::styled(text.to_string(), styles.text))];
    let shown = text.chars().count();

    let revealed = revealed_lines(lines, shown, &styles, styles.background, 1.0);

    assert_eq!(line_text(&revealed[0]), text, "every character is still there");
    for span in &revealed[0].spans {
        assert_eq!(
            span.style.fg, styles.text.fg,
            "a settled trail has no gradient left: {:?} is not the text color",
            span.style.fg
        );
    }
}

/// Halfway through resolving, the tail has lifted off the background but has not yet arrived at
/// the text color — the frames in between are what make it read as a fade rather than a flick.
#[test]
fn a_resolving_trail_lands_between_the_background_and_the_text() {
    let styles = theme();
    let text = "a sentence long enough to have a settled beginning and a fresh end";
    let lines = vec![Line::from(Span::styled(text.to_string(), styles.text))];
    let shown = text.chars().count();

    let newest = |settle| {
        revealed_lines(lines.clone(), shown, &styles, styles.background, settle)[0]
            .spans
            .last()
            .expect("the tail is drawn")
            .style
            .fg
    };

    let midway = newest(0.5);
    assert_ne!(midway, styles.background.bg, "halfway resolved is no longer invisible");
    assert_ne!(midway, styles.text.fg, "...and not yet fully resolved either");
}

#[test]
fn a_fenced_block_fades_into_its_own_background_not_the_canvas() {
    let styles = theme();
    let code = Style::default().fg(Color::Rgb(200, 200, 200)).bg(Color::Rgb(30, 30, 40));
    let lines = vec![Line::from(Span::styled("let x = 1;".to_string(), code))];
    let revealed = revealed_lines(lines, 10, &styles, styles.background, 0.0);
    let newest = revealed[0].spans.last().expect("the tail is drawn");
    assert_eq!(
        newest.style.fg,
        Some(Color::Rgb(30, 30, 40)),
        "a code block's tail dissolves into the block, not into the canvas behind it"
    );
}
