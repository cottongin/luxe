use super::*;
use crate::glyphs::{set_tier, GlyphTier};

const SURFACE: Color = Color::Rgb(0xdd, 0xc0, 0x84);
const CANVAS: Color = Color::Rgb(0x12, 0x10, 0x0e);

fn text(spans: &[Span<'_>]) -> String {
    spans.iter().map(|span| span.content.as_ref()).collect()
}

fn cells(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|span| UnicodeWidthStr::width(span.content.as_ref())).sum()
}

fn nord() -> ThemeStyles {
    let registry = luxe_core::theme::ThemeRegistry::new();
    let resolved = registry.resolve("nord").expect("nord resolves");
    ThemeStyles::from_resolved(&resolved, "nord")
}

fn channels(span: &Span<'_>) -> (u8, u8, u8) {
    match span.style.bg {
        Some(Color::Rgb(red, green, blue)) => (red, green, blue),
        other => panic!("a ramp step is painted in RGB, not {other:?}"),
    }
}

/// A ramp is three cells, every one strictly between its two ends and each a further step from
/// where it started: a single cell between two colors reads as a seam, not a gradient.
#[test]
fn a_ramp_steps_evenly_from_one_color_to_the_next() {
    let steps = ramp(SURFACE, CANVAS, FADE_CELLS);
    assert_eq!(steps.len(), 3, "never fewer than three");
    assert!(steps.iter().all(|span| span.content == " "), "painted, not drawn");
    let reds: Vec<u8> = steps.iter().map(|span| channels(span).0).collect();
    let (from, to) = (0xdd, 0x12);
    assert!(reds.windows(2).all(|pair| pair[0] > pair[1]), "each nearer the canvas: {reds:?}");
    assert!(reds.iter().all(|red| *red < from && *red > to), "reaching neither end: {reds:?}");
    // A quarter, a half and three quarters of the way
    assert_eq!(reds, [170, 120, 69]);
}

/// A fade runs the ramp toward the canvas on a trailing edge and out of it on a leading one.
#[test]
fn a_fade_runs_toward_the_canvas_on_the_side_it_ends() {
    let out = fade(SURFACE, CANVAS, FADE_CELLS, Fade::Out);
    let into = fade(SURFACE, CANVAS, FADE_CELLS, Fade::In);
    let reds = |spans: &[Span<'_>]| spans.iter().map(|span| channels(span).0).collect::<Vec<_>>();
    let mut reversed = reds(&into);
    reversed.reverse();
    assert_eq!(reds(&out), reversed, "the same steps, mirrored");
    assert!(reds(&out)[0] > reds(&out)[2], "a trailing edge starts nearest the slab");
}

/// A theme that names terminal colors has nothing to mix, so its edge is the cap it always had
/// and a change of depth is a hard edge, rather than a gradient made up on its behalf.
#[test]
fn a_theme_without_rgb_keeps_its_hard_edges() {
    let faded = fade(Color::Yellow, Color::Black, FADE_CELLS, Fade::Out);
    assert_eq!(text(&faded), Cap::Close.glyph(), "the cap, not three painted cells");
    let stepped = ramp(Color::Yellow, Color::Blue, FADE_CELLS);
    assert_eq!(stepped.len(), 1);
    assert_eq!(stepped[0].style.bg, Some(Color::Blue));
}

/// A heading stays flush where it labels its column and fades out after its word.
#[test]
fn a_badge_is_flush_where_it_starts_and_fades_where_it_ends() {
    let theme = nord();
    let spans = badge("MODEL", Depth::Raised, theme.slab.canvas, &theme);
    let surface = theme.slab.at(Depth::Raised);
    assert_eq!(spans[0].style.bg, Some(surface), "no cap at the leading edge");
    assert_eq!(cells(&spans), 2 + "MODEL".len() + 1 + FADE_CELLS);
    let tail = &spans[spans.len() - FADE_CELLS..];
    assert!(tail.iter().all(|span| span.content == " " && span.style.bg != Some(surface)));
}

/// The cell each line's leading and trailing cap sits on.
fn edges(line: &Line<'_>) -> (usize, usize) {
    let text: String = line.spans.iter().map(|span| span.content.as_ref()).collect();
    let open = text.find(Cap::Open.glyph()).expect("every line carries an opening cap");
    let close = text.rfind(Cap::Close.glyph()).expect("and closes it");
    (UnicodeWidthStr::width(&text[..open]), UnicodeWidthStr::width(&text[..close]))
}

/// Cells of clear slab between each cap and `needle` — how far the words stand off the shape.
fn margins(spans: &[Span<'_>], needle: &str) -> (usize, usize) {
    let text = text(spans);
    let open = text.find(Cap::Open.glyph()).expect("an opening cap");
    let close = text.rfind(Cap::Close.glyph()).expect("a closing cap");
    let at = text.find(needle).expect("the content is on the row");
    (
        UnicodeWidthStr::width(&text[open + Cap::Open.glyph().len()..at]),
        UnicodeWidthStr::width(&text[at + needle.len()..close]),
    )
}

/// One entry per cell, so a test can name the column it means rather than index the spans.
fn painted(line: &Line<'_>) -> Vec<(String, Style)> {
    line.spans
        .iter()
        .flat_map(|span| span.content.chars().map(move |glyph| (glyph.to_string(), span.style)))
        .collect()
}

/// The caps are one drawing now, so the tier cannot reach them at all. That is the whole point
/// of the range they moved to — a bar whose width or shape follows the font is the thing the
/// Powerline triangles kept threatening to be.
#[test]
fn a_slab_is_the_same_shape_at_every_tier() {
    let body = || vec![Span::raw("main")];
    let mut drawn = Vec::new();
    for at in [GlyphTier::Nerd, GlyphTier::NerdWide, GlyphTier::Unicode, GlyphTier::Ascii] {
        set_tier(at);
        let spans = slab(body(), SURFACE, CANVAS, Style::new());
        drawn.push((cells(&spans), text(&spans)));
    }
    set_tier(GlyphTier::Nerd);
    assert!(drawn.windows(2).all(|pair| pair[0] == pair[1]), "{drawn:?}");
    assert_eq!(drawn[0].0, "main".len() + SLAB_FRAME, "four frame cells around a four-cell body");
}

/// Nothing the slab draws may need a patched font, and after the retreat nothing it draws is
/// even newer than Unicode 1.1 — the range every terminal has synthesized since before any of
/// this was a question.
#[test]
fn the_caps_are_block_elements_a_bare_font_already_has() {
    for cap in [Cap::Open, Cap::Close] {
        let glyph = cap.glyph();
        assert_eq!(glyph.chars().count(), 1, "{cap:?} is not one glyph");
        assert_eq!(UnicodeWidthStr::width(glyph), 1, "{cap:?} is not one cell");
        let cp = u32::from(glyph.chars().next().expect("a glyph"));
        assert!(
            (0x2580..=0x259F).contains(&cp),
            "{cap:?} draws U+{cp:04X}, outside Block Elements"
        );
    }
}

/// A cap is the slab's color in the *foreground* over the canvas — half the cell inked, half
/// not. Painting it as a background fills the whole cell and squares the end off, which is the
/// one mistake here that looks almost right.
#[test]
fn a_cap_paints_half_a_cell_over_the_canvas() {
    let open = Cap::Open.span(SURFACE, CANVAS);
    assert_eq!(open.content.as_ref(), "\u{2590}", "the opening cap fills the right half");
    assert_eq!(open.style.fg, Some(SURFACE));
    assert_eq!(open.style.bg, Some(CANVAS));

    let close = Cap::Close.span(SURFACE, CANVAS);
    assert_eq!(close.content.as_ref(), "\u{258c}", "the closing cap fills the left half");
    assert_eq!(close.style.fg, Some(SURFACE));
    assert_eq!(close.style.bg, Some(CANVAS));
}

/// Each cap inks the half facing the body. Swapped, the slab grows a cell at each end while
/// appearing to float away from its own content.
#[test]
fn the_caps_face_inward() {
    assert_ne!(Cap::Open.glyph(), Cap::Close.glyph());
    assert_eq!(Cap::Open.glyph(), "\u{2590}", "right half block, inking toward the body");
    assert_eq!(Cap::Close.glyph(), "\u{258c}", "left half block, inking back toward it");
}

/// A selected row fills its width exactly — one cell short and the canvas shows through as a
/// notch, one over and the row wraps.
#[test]
fn a_selected_row_fills_its_width_and_a_plain_one_keeps_its_column() {
    let content = || vec![Span::raw("\u{276f} "), Span::raw("session")];

    let selected = row(content(), 30, 0, true, SURFACE, CANVAS);
    assert_eq!(cells(&selected.spans), 30);
    assert!(text(&selected.spans).starts_with(Cap::Open.glyph()));
    assert!(text(&selected.spans).ends_with(Cap::Close.glyph()));

    let plain = row(content(), 30, 0, false, SURFACE, CANVAS);
    assert_eq!(
        text(&plain.spans),
        "  \u{276f} session",
        "plain cells standing in for the cap and the pad inside it"
    );
}

/// The point of the pad: a chosen row is the bar's shape at full width, so its words stand off
/// the cap by the cell the bar's do rather than starting on it.
#[test]
fn a_chosen_row_stands_off_the_cap_the_way_the_bar_does() {
    let bar = slab([Span::raw("main")], SURFACE, CANVAS, Style::new());
    // Sized so nothing is left to fill, which is the only case where both margins are the
    // padding alone.
    let chosen =
        row(vec![Span::raw("main")], "main".len() + SELECTION_FRAME, 0, true, SURFACE, CANVAS);

    assert_eq!(margins(&bar, "main"), (1, 1), "the bar keeps a cell either side of its word");
    assert_eq!(margins(&chosen.spans, "main"), margins(&bar, "main"));
}

/// And that cell is *filled*, not canvas showing through: the highlight has to reach the cap, or
/// the padding reads as a gap in the shape instead of room inside it.
#[test]
fn the_pad_inside_each_cap_carries_the_slab_color() {
    let width = 30;
    let chosen = row(vec![Span::raw("session")], width, 0, true, SURFACE, CANVAS);
    let columns = painted(&chosen);
    assert_eq!(columns.len(), width);

    for column in [1, width - 2] {
        let (glyph, style) = &columns[column];
        assert_eq!(glyph, " ", "column {column} is padding");
        assert_eq!(style.bg, Some(SURFACE), "column {column} is inked");
    }
}

/// A block's lines are cut with the same frame as a single row — a difference between them
/// would leave a tall entry's text on the cap while a short one's stood clear.
#[test]
fn every_line_of_a_block_keeps_the_same_padding() {
    let theme = nord();
    let block = selection_block(
        vec![vec![Span::raw("label")], vec![Span::raw("detail")]],
        40,
        true,
        &theme,
    );
    for (line, needle) in block.iter().zip(["label", "detail"]) {
        assert_eq!(margins(&line.spans, needle).0, 1, "{needle} stands off the cap");
    }
}

/// The label column has to land on the same cell whether or not the row is selected, or the list
/// shifts sideways under the cursor.
#[test]
fn selection_does_not_move_the_label() {
    let content = || vec![Span::raw("  "), Span::raw("luxe")];
    // In cells, not bytes: a three-byte cap standing in for a one-byte space would read as a
    // shift that never happened.
    let column = |line: &Line<'_>| {
        let text: String = line.spans.iter().map(|span| span.content.as_ref()).collect();
        let byte = text.find("luxe").expect("the label is on the row");
        UnicodeWidthStr::width(&text[..byte])
    };
    let selected = row(content(), 20, 0, true, SURFACE, CANVAS);
    let plain = row(content(), 20, 0, false, SURFACE, CANVAS);
    assert_eq!(column(&selected), column(&plain));
}

/// A tall row is one shape: every line the same width, capped at the same two columns. It used
/// to have to *lean* — the caps were halves of one diagonal, so a block that capped every line
/// at the same column restarted the slope on each one and read as a stack of separate rows.
/// Square ends stack without that, and this is what proves the stagger is gone rather than
/// merely unused.
#[test]
fn a_block_stacks_square() {
    let theme = nord();
    let block = selection_block(
        vec![vec![Span::raw("label")], vec![Span::raw("detail")], vec![Span::raw("more")]],
        40,
        true,
        &theme,
    );

    let (opening, closing): (Vec<usize>, Vec<usize>) = block.iter().map(edges).unzip();
    assert!(opening.iter().all(|at| *at == opening[0]), "one leading column: {opening:?}");
    assert!(closing.iter().all(|at| *at == closing[0]), "one trailing column: {closing:?}");
    assert!(block.iter().all(|line| cells(&line.spans) == 40), "every line spans the full width");
}

/// The frame is spent whether or not the row is chosen — otherwise the text slides sideways as
/// the cursor lands on it, which is the exact jitter the frame cells exist to prevent.
#[test]
fn a_block_holds_its_column_whether_or_not_it_is_chosen() {
    let theme = nord();
    let rows = || vec![vec![Span::raw("label")], vec![Span::raw("detail")]];
    let column_of = |lines: Vec<Line<'static>>| -> Vec<usize> {
        lines
            .iter()
            .map(|line| {
                let text: String = line.spans.iter().map(|span| span.content.as_ref()).collect();
                let at = text.find("label").or_else(|| text.find("detail")).expect("content");
                UnicodeWidthStr::width(&text[..at])
            })
            .collect()
    };
    assert_eq!(
        column_of(selection_block(rows(), 40, true, &theme)),
        column_of(selection_block(rows(), 40, false, &theme)),
    );
}

/// A row too narrow to cut keeps its content rather than losing cells to a frame that will not
/// fit — the pickers get very narrow on a small terminal.
#[test]
fn a_row_narrower_than_its_frame_is_left_alone() {
    let line = row(vec![Span::raw("ab")], SELECTION_FRAME, 0, true, SURFACE, CANVAS);
    assert_eq!(text(&line.spans), "  ab");
}
