use super::*;
use crate::glyphs::Icon;

fn theme() -> ThemeStyles {
    let registry = luxe_core::theme::ThemeRegistry::new();
    let resolved = registry.resolve("nord").expect("nord resolves");
    ThemeStyles::from_resolved(&resolved, "nord")
}

/// The footer is the panel's last two rows inside the frame: a rule across all of it, padding
/// or not, then the keys on the left and the count on the right. The body stops above it.
#[test]
fn a_footer_sits_inside_the_frame_under_its_rule() {
    use ratatui::{backend::TestBackend, widgets::Padding, Terminal};
    let theme = theme();
    let mut terminal = Terminal::new(TestBackend::new(40, 8)).expect("terminal");
    let mut body = Rect::default();
    terminal
        .draw(|frame| {
            let block =
                titled_panel(Icon::Help.glyph(), "Keys", &theme).padding(Padding::horizontal(1));
            body = render_footed(
                frame,
                block,
                frame.area(),
                "enter go · esc back",
                Some("2/9"),
                &theme,
            );
        })
        .expect("draw");
    let buffer = terminal.backend().buffer();
    let row = |y: u16| (0..40).map(|x| buffer[(x, y)].symbol()).collect::<String>();
    assert_eq!(row(5), format!("│{}│", "─".repeat(38)), "the rule meets the frame");
    assert!(row(6).starts_with("│ enter go · esc back"), "{}", row(6));
    assert!(row(6).ends_with("2/9 │"), "{}", row(6));
    assert!(row(7).starts_with('╰'), "the border carries nothing: {}", row(7));
    assert_eq!(body, Rect::new(2, 1, 36, 4), "padded, and above the rule");

    let (body, footer) = split_footer(Rect::new(0, 0, 10, 1));
    assert_eq!((body.height, footer.height), (0, 1), "too short for both: the keys win");
}

/// Keys too many for the row drop from the end, keeping the first and the way out.
#[test]
fn a_hint_too_wide_for_its_row_keeps_the_way_out() {
    let hint = "enter post · e edit · E in your editor · d redraft · esc keep";
    assert_eq!(fit_hint(hint, 80), hint);
    assert_eq!(fit_hint(hint, 40), "enter post · e edit · esc keep");
    assert_eq!(fit_hint(hint, 26), "enter post · esc keep");
    assert_eq!(fit_hint(hint, 5), "enter post", "the first key, however narrow");
    assert_eq!(fit_hint("a one · b two · c three", 13), "a one · b two");
}

/// A count reads as text and a key as a key; a slash alone does not make a count.
#[test]
fn a_count_in_a_hint_is_not_a_key() {
    let theme = theme();
    let style_of = |hint: &str| key_hint(hint, &theme).spans[1].style;
    assert_eq!(style_of("3/12 · enter run"), theme.text);
    for key in ["a/b · x y", "3/ · x y", "/12 · x y", "esc"] {
        assert_eq!(style_of(key), theme.accent, "{key}");
    }
    assert_eq!(key_hint_row("esc close", &theme).alignment, Some(ratatui::layout::Alignment::Left));
}

#[test]
fn title_width_matches_the_rendered_title() {
    let theme = theme();
    for label in ["Files", "Command palette", "Change review source"] {
        assert_eq!(
            panel_title(Icon::Palette.glyph(), label, &theme).width(),
            panel_title_width(Icon::Palette.glyph(), label),
            "{label}: measured width must match the rendered title"
        );
    }
}

#[test]
fn title_accents_the_glyph_and_bolds_the_label() {
    let theme = theme();
    let title = panel_title(Icon::SlashMenu.glyph(), "Commands", &theme);

    assert_eq!(
        title.spans[0].content,
        format!(" {}{}", Icon::SlashMenu.glyph(), crate::glyphs::icon_gap())
    );
    assert_eq!(title.spans[0].style.fg, theme.accent.fg, "glyph carries the accent color");
    assert!(
        !title.spans[0].style.add_modifier.contains(Modifier::BOLD),
        "the glyph is drawn by color, not weight"
    );
    assert_eq!(title.spans[1].content, "Commands");
    assert!(title.spans[1].style.add_modifier.contains(Modifier::BOLD), "the label is always bold");
    assert_eq!(title.spans[1].style.fg, theme.text.fg);
}

#[test]
fn titled_panel_draws_rounded_corners_and_the_accent_border() {
    use ratatui::{backend::TestBackend, layout::Rect, Terminal};

    let theme = theme();
    let mut terminal = Terminal::new(TestBackend::new(24, 4)).expect("terminal");
    terminal
        .draw(|frame| {
            frame.render_widget(
                titled_panel(Icon::FileMenu.glyph(), "Files", &theme),
                Rect::new(0, 0, 24, 4),
            );
        })
        .expect("render");

    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(0, 0)].symbol(), "╭");
    assert_eq!(buffer[(23, 3)].symbol(), "╯");
    let border_fg = theme.border_accent.fg.expect("nord defines a border accent");
    assert_eq!(buffer[(0, 0)].fg, border_fg);
    // Title cells: leading pad, glyph, pad, then the bold label.
    assert_eq!(buffer[(2, 0)].symbol(), Icon::FileMenu.glyph());
    assert_eq!(buffer[(4, 0)].symbol(), "F");
    assert!(buffer[(4, 0)].modifier.contains(Modifier::BOLD));
}

#[test]
fn a_hint_puts_each_key_in_the_accent_and_its_words_in_the_text() {
    let theme = theme();
    let line = key_hint(" enter open · ctrl+enter save · esc ", &theme);
    let spans: Vec<(&str, Style)> =
        line.spans.iter().map(|span| (span.content.as_ref(), span.style)).collect();
    assert_eq!(
        spans,
        [
            (" ", Style::default()),
            ("enter", theme.accent),
            (" open", theme.text),
            (" · ", theme.muted),
            ("ctrl+enter", theme.accent),
            (" save", theme.text),
            (" · ", theme.muted),
            ("esc", theme.accent),
            (" ", Style::default()),
        ]
    );
    let busy = key_hint(" opening… ", &theme);
    assert_eq!((busy.spans[1].content.as_ref(), busy.spans[1].style), ("opening…", theme.text));
}

#[test]
fn quoted_stretches_are_drawn_as_code_without_their_backticks() {
    let theme = theme();
    let drawn = |text: &str| -> Vec<(String, bool)> {
        code_spans(text, theme.text, &theme)
            .iter()
            .map(|span| (span.content.to_string(), span.style == theme.md_code))
            .collect()
    };
    assert_eq!(
        drawn("Checks #42 out on `fix-redirects` into `main`."),
        [
            ("Checks #42 out on ".into(), false),
            ("fix-redirects".into(), true),
            (" into ".into(), false),
            ("main".into(), true),
            (".".into(), false),
        ]
    );
    assert_eq!(drawn("`start`"), [("start".to_string(), true)]);
    assert_eq!(
        drawn("a stray ` tick"),
        [("a stray ".to_string(), false), ("` tick".to_string(), false)]
    );
}
