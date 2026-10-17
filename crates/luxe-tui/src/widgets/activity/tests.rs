use super::*;
use ratatui::style::{Color, Modifier};
use unicode_width::UnicodeWidthStr;

/// A five-shade stand-in for a theme's derived accent ramp.
fn trail() -> Vec<Style> {
    [Color::Rgb(200, 220, 255), Color::Rgb(160, 190, 240), Color::Rgb(120, 150, 200)]
        .into_iter()
        .chain([Color::Rgb(90, 110, 150), Color::Rgb(70, 80, 105)])
        .map(|color| Style::default().fg(color))
        .collect()
}

/// A stand-in for the rail's glow ramp, nearest the comet first. Its head is the same color the
/// comet's tail lands on, exactly as the derived ramps meet in production.
fn rail() -> Vec<Style> {
    [Color::Rgb(70, 80, 105), Color::Rgb(56, 62, 80), Color::Rgb(44, 47, 60)]
        .into_iter()
        .chain([Color::Rgb(36, 38, 48), Color::Rgb(32, 33, 42)])
        .map(|color| Style::default().fg(color))
        .collect()
}

/// The dimmest rail shade: what a unit the glow cannot reach wears.
fn unlit() -> Style {
    *rail().last().expect("the ramp has shades")
}

fn text(spans: &[Span<'static>]) -> String {
    spans.iter().map(|span| span.content.as_ref()).collect()
}

fn bar(frame: u64, width: usize) -> Vec<Span<'static>> {
    activity_bar(frame, width, ActivityGlyphs::BARS, &trail(), &rail())
}

/// Every pair the config can pick.
const STYLES: [ActivityGlyphs; 2] = [ActivityGlyphs::BARS, ActivityGlyphs::DOTS];

/// Cells wearing a comet shade bright enough to tell from the rail.
///
/// The comet's last shade *is* the rail's first by design — that is how the tail disappears — so
/// only the shades above it count as lit.
fn lit(spans: &[Span<'static>]) -> usize {
    let trail = trail();
    let bright = &trail[..trail.len() - 1];
    spans
        .iter()
        .filter(|span| bright.contains(&span.style))
        .map(|span| span.content.chars().count())
        .sum()
}

/// The rail is `width` cells of one glyph each — the whole point of dropping the separator.
/// The default pair must come from the block elements (`U+2580`–`U+259F`).
///
/// Not fussiness: those are the only glyphs a terminal font is obliged to draw itself, at exact
/// cell fractions. A shape the font lacks is substituted from elsewhere at that font's metrics —
/// `▰` arrives from Noto Sans Mono at double width — and bleeds into its neighbors.
#[test]
fn the_default_units_are_block_elements() {
    for glyph in ActivityGlyphs::BARS.lit.iter().copied().chain([ActivityGlyphs::BARS.track]) {
        assert!(
            ('\u{2580}'..='\u{259f}').contains(&glyph),
            "{glyph:?} is not a block element, so a font substitution decides how it looks"
        );
    }
}

#[test]
fn the_bar_fills_its_width_exactly() {
    for glyphs in STYLES {
        for width in 2..64usize {
            for frame in [0u64, 1, 7, 30, 97, 1_000] {
                let rendered =
                    text(&activity_bar(frame, width, glyphs, &trail(), &rail())).to_string();
                assert_eq!(
                    rendered.width(),
                    width,
                    "{glyphs:?} at width {width} frame {frame}: {rendered:?}"
                );
            }
        }
    }
}

/// Packed means packed: no separator anywhere, in any style, at any frame.
#[test]
fn units_are_packed_without_gaps() {
    for glyphs in STYLES {
        for frame in 0..200u64 {
            let rendered = text(&activity_bar(frame, 24, glyphs, &trail(), &rail()));
            assert!(!rendered.contains(' '), "{glyphs:?} frame {frame}: {rendered:?}");
        }
    }
}

/// A bar with room for one unit still renders it rather than panicking on a clamp whose floor
/// exceeds its ceiling.
#[test]
fn a_one_unit_bar_still_renders() {
    for width in 1..=3usize {
        assert!(!bar(0, width).is_empty(), "width {width} renders something");
    }
}

#[test]
fn a_bar_with_no_room_renders_nothing() {
    assert!(bar(0, 0).is_empty());
    assert!(activity_bar(0, 24, ActivityGlyphs::DOTS, &[], &rail()).is_empty());
    assert!(activity_bar(0, 24, ActivityGlyphs::DOTS, &trail(), &[]).is_empty());
}

/// The comet crosses, clears the rail, waits, and comes back the way it came.
#[test]
fn the_comet_sweeps_out_one_end_and_returns_through_it() {
    let (count, comet) = (24usize, comet_span(24, trail().len()));
    let leg = leg_frames((count + comet - 1) as u64);

    // Leaving: the head enters at cell 0 and walks off the far end.
    assert_eq!(sweep(0, count, comet), Sweep::Moving { head: 0, forward: true });
    let last_out = sweep(leg - 1, count, comet);
    let Sweep::Moving { head, forward } = last_out else { panic!("{last_out:?} is still moving") };
    assert!(forward && head >= count as isize, "the head is past the far edge: {head}");

    // Returning: back in through the end it left by, and out through the one it came in at.
    let back = sweep(leg + HOLD_FRAMES, count, comet);
    assert_eq!(back, Sweep::Moving { head: count as isize - 1, forward: false });
    let last_back = sweep(2 * leg + HOLD_FRAMES - 1, count, comet);
    let Sweep::Moving { head, forward } = last_back else { panic!("{last_back:?} moves") };
    assert!(!forward && head < 0, "and off the near edge: {head}");

    // And the cycle closes, so the motion is a loop of two legs rather than a drift.
    let cycle = 2 * (leg + HOLD_FRAMES);
    assert_eq!(text(&bar(0, count)), text(&bar(cycle, count)));
    assert_ne!(text(&bar(0, count)), text(&bar(leg, count)), "the legs are not the same frame");
}

/// The pause is empty rail, not a comet sitting still: the trail drains off the end first.
#[test]
fn the_rail_empties_completely_between_legs() {
    let (count, comet) = (24usize, comet_span(24, trail().len()));
    let leg = leg_frames((count + comet - 1) as u64);
    for offset in 0..HOLD_FRAMES {
        for start in [leg, 2 * leg + HOLD_FRAMES] {
            let spans = bar(start + offset, count);
            assert_eq!(lit(&spans), 0, "frame {} still lit: {:?}", start + offset, text(&spans));
            assert!(text(&spans).chars().all(|glyph| glyph == ActivityGlyphs::BARS.track));
            // And the glow has gone with it: every unit sits at the ramp's far end, so the row
            // is the canvas but for a whisper rather than a drawn-out strip of ticks.
            assert_eq!(spans.len(), 1, "one span, one shade: {spans:?}");
            assert_eq!(spans[0].style, unlit(), "the rail is at its dimmest");
        }
    }
}

/// Over one cycle the rail is lit for most of it and empty for a beat — the comet is the
/// indicator, so it cannot be missing for long.
#[test]
fn the_rail_is_lit_far_more_often_than_not() {
    let (count, comet) = (24usize, comet_span(24, trail().len()));
    let leg = leg_frames((count + comet - 1) as u64);
    let cycle = 2 * (leg + HOLD_FRAMES);
    let dark: Vec<u64> = (0..cycle).filter(|&frame| lit(&bar(frame, count)) == 0).collect();

    // The holds are dark by construction; the frames either side of them are the ones where all
    // that is left on the rail is the comet's last shade, which is the rail's own color.
    for hold in [leg, 2 * leg + HOLD_FRAMES] {
        assert!(
            (hold..hold + HOLD_FRAMES).all(|frame| dark.contains(&frame)),
            "the hold at {hold} is dark: {dark:?}"
        );
    }
    assert!(
        (dark.len() as f32) < cycle as f32 * 0.2,
        "the bar is dark for {} of {cycle} frames",
        dark.len()
    );
}

/// The comet never spills past the rail's ends and never swamps it.
#[test]
fn the_comet_is_bounded_by_the_rail() {
    for width in 2..40usize {
        let comet = comet_span(width, trail().len());
        assert!(comet <= width && comet >= 1, "width {width} comet {comet}");
        for frame in 0..240u64 {
            let count = lit(&bar(frame, width));
            assert!(count <= comet, "width {width} frame {frame}: {count} lit > {comet}");
        }
    }
}

/// The gradient is the polish: the leading cell wears the brightest shade, each cell behind it
/// a dimmer one, and the last of the comet the dimmest the palette has.
#[test]
fn the_comet_wears_the_palette_head_to_tail() {
    let trail = trail();
    let comet = comet_span(24, trail.len());
    assert!(comet >= 3, "the default bar has room for a gradient, not just a head and a tail");

    let shade = |behind| trail[ramp_step(comet, behind, trail.len())];
    let shades: Vec<Style> = (0..comet).map(shade).collect();
    assert_eq!(shades[0], trail[0], "the head is the brightest shade");
    assert_eq!(shades[comet - 1], trail[trail.len() - 1], "the tail is the dimmest");
    for pair in shades.windows(2) {
        assert_ne!(pair[0], pair[1], "each cell of the comet is its own shade: {shades:?}");
    }

    // A comet squeezed onto a narrow rail still spans the whole palette.
    let short = comet_span(6, trail.len());
    assert_eq!(ramp_step(short, 0, trail.len()), 0);
    assert_eq!(ramp_step(short, short - 1, trail.len()), trail.len() - 1);
}

/// The comet thins as it dims, and its last cell is the rail's own glyph — so the far tail is
/// gone in shape as well as in color rather than lingering as a wide bar in a rail-ish shade.
#[test]
fn the_comet_tapers_into_the_rail() {
    let glyphs = ActivityGlyphs::BARS;
    let comet = comet_span(24, trail().len());
    let steps: Vec<usize> =
        (0..comet).map(|behind| ramp_step(comet, behind, glyphs.lit.len())).collect();

    assert!(steps.windows(2).all(|pair| pair[0] <= pair[1]), "only ever thins: {steps:?}");
    assert_eq!(glyphs.lit[steps[0]], glyphs.lit[0], "the head is the heaviest glyph");
    assert_eq!(glyphs.lit[steps[comet - 1]], glyphs.track, "and the tail is the rail itself");
    assert!(steps.contains(&1) && steps.contains(&2), "every weight gets used: {steps:?}");

    // A one-weight ramp has nothing to thin, so every cell stays on the only glyph there is
    // and the comet fades by color alone. No shipped style is one weight any more — the three
    // that were are retired — so the arithmetic is checked rather than a stand-in style.
    assert!((0..comet).all(|behind| ramp_step(comet, behind, 1) == 0));
}

/// Both ramps are applied verbatim, so a theme owns the whole look.
#[test]
fn styles_are_applied_verbatim() {
    let trail =
        vec![Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD), Style::default()];
    let rail = vec![Style::default().fg(Color::Rgb(1, 2, 3)), Style::default().fg(Color::Black)];
    let spans = activity_bar(0, 16, ActivityGlyphs::DOTS, &trail, &rail);
    assert_eq!(spans.first().map(|span| span.style), Some(trail[0]), "the head leads");
    assert!(spans.iter().any(|span| span.style == rail[0]), "the rail is lit near the comet");
    assert!(spans.iter().any(|span| span.style == rail[1]), "and dark away from it");
}

/// Adjacent cells that share a style share a span — a still rail is one span, not twenty-four.
#[test]
fn runs_of_one_style_collapse_into_one_span() {
    let (count, comet) = (24usize, comet_span(24, trail().len()));
    let leg = leg_frames((count + comet - 1) as u64);
    let spans = bar(leg, count);
    assert_eq!(spans.len(), 1, "the empty rail is a single span: {spans:?}");
    assert_eq!(spans[0].content.chars().count(), count);
}
