use super::*;

const CANVAS: Color = Color::Rgb(0x12, 0x10, 0x0e);
const ACCENT: Color = Color::Rgb(0xdd, 0xc0, 0x84);

fn ink() -> WordmarkInk {
    WordmarkInk { accent: ACCENT, canvas: Some(CANVAS), dark: true }
}

fn row_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|span| span.content.as_ref()).collect()
}

/// Every color the mark can wear, at one tick.
fn colors(size: Wordmark, tick: u64) -> Vec<Color> {
    size.lines(ink(), Some(tick))
        .iter()
        .flat_map(|line| line.spans.clone())
        .filter(|span| span.content.trim() != "")
        .filter_map(|span| span.style.fg)
        .collect()
}

#[test]
fn every_art_row_is_the_same_width() {
    for size in [Wordmark::Full, Wordmark::Small, Wordmark::Line] {
        for row in size.rows() {
            assert_eq!(
                row.chars().count() as u16,
                size.width(),
                "ragged rows shift a centerd logotype line by line: {row:?}"
            );
        }
    }
}

/// One cell per glyph, or the art shears. The half blocks are single-width everywhere, but a
/// future edit that reached for something wider would push the rest of its row over.
#[test]
fn every_glyph_measures_one_cell() {
    use unicode_width::UnicodeWidthStr;
    for size in [Wordmark::Full, Wordmark::Small, Wordmark::Line] {
        for row in size.rows() {
            assert_eq!(
                UnicodeWidthStr::width(*row),
                row.chars().count(),
                "a glyph in {row:?} does not measure one column"
            );
        }
    }
}

/// The portability promise, asserted where it can fail: no private-use codepoint may reach the
/// screen, because the whole point of this drawing is that it needs no patched font.
#[test]
fn the_art_needs_no_font_anyone_has_to_install() {
    for size in [Wordmark::Full, Wordmark::Small] {
        for glyph in size.rows().iter().flat_map(|row| row.chars()) {
            assert!(
                glyph == ' ' || art::BLOCKS.contains(&glyph),
                "U+{:04X} is outside the three blocks every terminal draws itself",
                glyph as u32
            );
        }
    }
}

#[test]
fn fit_takes_the_largest_size_the_space_allows() {
    let (full_w, full_h) = (Wordmark::Full.width(), Wordmark::Full.height());
    let (small_w, small_h) = (Wordmark::Small.width(), Wordmark::Small.height());

    assert_eq!(Wordmark::fit(full_w, full_h), Wordmark::Full);
    assert_eq!(Wordmark::fit(full_w - 1, full_h), Wordmark::Small, "a column short");
    assert_eq!(Wordmark::fit(full_w, full_h - 1), Wordmark::Small, "a row short");
    assert_eq!(Wordmark::fit(small_w, small_h), Wordmark::Small);
    assert_eq!(Wordmark::fit(small_w - 1, small_h), Wordmark::Line);
    assert_eq!(Wordmark::fit(small_w, small_h - 1), Wordmark::Line);
}

#[test]
fn the_line_size_is_the_wordmark_itself() {
    let lines = Wordmark::Line.lines(ink(), None);
    assert_eq!(row_text(&lines[0]), "luxe.");
}

// === The drift ===

/// The same point in the drift as `tick`, at a moment the shine is resting.
///
/// The shine is the one light that legitimately takes a cell off the canvas→accent line — it
/// lifts toward white — so a drift assertion sampled mid-pass fails on the wrong thing. Which
/// sample ticks land mid-pass falls out of [`DRIFT_TICKS`], and that is a knob, so the rest is
/// derived rather than left to hold by luck.
///
/// Whole drift periods, so the sample keeps its place in the wave and moves only through the
/// shine's cycle. One of them always rests, because [`DRIFT_TICKS`] is not a multiple of
/// [`CYCLE_TICKS`] — asserted at the top of this file, which is what bounds the search.
fn while_resting(tick: u64) -> u64 {
    (0..CYCLE_TICKS)
        .map(|period| tick + period * DRIFT_TICKS)
        .find(|tick| !shine_active(*tick))
        .expect("a drift that is not in step with the shine reaches its rest")
}

/// One hue. The mark is not a legend of the palette, so every cell it draws has to be the
/// accent shaded somewhere between itself and the canvas — never a second color.
#[test]
fn every_cell_is_the_accent_shaded_toward_the_canvas() {
    let (Color::Rgb(ar, ag, ab), Color::Rgb(cr, cg, cb)) = (ACCENT, CANVAS) else {
        unreachable!("the fixture is RGB")
    };
    for tick in [0, STILL_TICK, DRIFT_TICKS / 3, DRIFT_TICKS - 1].map(while_resting) {
        for color in colors(Wordmark::Full, tick) {
            let Color::Rgb(r, g, b) = color else { panic!("{color:?} is not blendable") };
            // Solve for the blend fraction on one channel with range, then hold the others to
            // it: a color off that line is a hue the mark has no business wearing.
            let span = f32::from(ar) - f32::from(cr);
            let step = (f32::from(r) - f32::from(cr)) / span;
            for (value, end, start) in [(g, ag, cg), (b, ab, cb)] {
                let expected = f32::from(start) + (f32::from(end) - f32::from(start)) * step;
                assert!(
                    (f32::from(value) - expected).abs() <= 2.0,
                    "{color:?} is off the canvas→accent line at tick {tick}"
                );
            }
        }
    }
}

/// A letter is one flat tone. The drift is positioned by ink group rather than by raw column,
/// which is what stops a gradient running through the middle of the `x`.
#[test]
fn a_letter_wears_one_tone() {
    let groups = Wordmark::Full.groups();
    let mut per_group: std::collections::BTreeMap<usize, std::collections::BTreeSet<u32>> =
        Default::default();
    for line in Wordmark::Full.lines(ink(), None) {
        let mut column = 0usize;
        for span in &line.spans {
            for glyph in span.content.chars() {
                if glyph != ' ' {
                    if let Some(Color::Rgb(r, g, b)) = span.style.fg {
                        let key = groups.get(column).copied().unwrap_or(0);
                        per_group
                            .entry(key)
                            .or_default()
                            .insert((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b));
                    }
                }
                column += 1;
            }
        }
    }
    assert_eq!(per_group.len(), 5, "four letters and a full stop");
    for (group, tones) in &per_group {
        assert_eq!(tones.len(), 1, "group {group} wears {} tones", tones.len());
    }
}

/// The drift actually moves: the same cell is a different shade a few seconds later.
#[test]
fn the_drift_travels() {
    let column = 3; // inside the `l`
    let shade = |tick| Wordmark::Full.drift(column, tick);
    assert!(
        (shade(0) - shade(DRIFT_TICKS / 4)).abs() > 0.05,
        "the reflection has not moved in under two seconds"
    );
    assert!(
        (shade(0) - shade(DRIFT_TICKS)).abs() < 1e-6,
        "and it closes on itself, so the wrap has no seam"
    );
}

/// A still is a clean gradient: dimmest at the `l`, the accent untouched at the full stop, and
/// every step in between going one way. Anything else reads as a pattern rather than as light.
#[test]
fn a_still_runs_dim_to_bright_across_the_mark() {
    let size = Wordmark::Full;
    let shades: Vec<f32> =
        (0..size.group_count()).map(|g| size.group_shade(g, STILL_TICK)).collect();
    assert!(
        shades.windows(2).all(|pair| pair[1] < pair[0]),
        "the still is not monotonic: {shades:?}"
    );
    assert!(shades[0] > 0.5, "the far end is genuinely cut back");
    assert!(
        shades[size.group_count() - 1] < 0.01,
        "and the near end is the accent itself, not an approximation of it"
    );
}

/// Neighboring groups differ by a shade, not a step: the ramp is the illusion, and a coarse
/// one reads as stripes rather than as a surface.
#[test]
fn the_drift_steps_gently_between_letters() {
    let groups = Wordmark::Full.groups();
    let first_column = |group: usize| {
        (0..Wordmark::Full.width()).find(|c| groups[usize::from(*c)] == group).expect("a column")
    };
    for tick in [0, STILL_TICK, DRIFT_TICKS / 3] {
        let shades: Vec<f32> =
            (0..5).map(|g| Wordmark::Full.drift(first_column(g), tick)).collect();
        let biggest =
            shades.windows(2).map(|pair| (pair[1] - pair[0]).abs()).fold(0.0_f32, f32::max);
        assert!(biggest < 0.30, "letters jump by {biggest} at tick {tick}, which will band");
    }
}

/// A still is a frame of the same animation, not a separate rendering path.
#[test]
fn a_still_is_the_frame_at_the_still_tick() {
    let still = Wordmark::Full.lines(ink(), None);
    let held = Wordmark::Full.lines(ink(), Some(STILL_TICK));
    assert_eq!(format!("{still:?}"), format!("{held:?}"));
}

// === The shine ===

/// The pass runs, then the logotype rests.
#[test]
fn a_pass_is_followed_by_a_long_rest() {
    assert!(shine_active(0));
    assert!(shine_active(SWEEP_TICKS - 1));
    assert!(!shine_active(SWEEP_TICKS));
    assert!(!shine_active(CYCLE_TICKS - 1));
    assert!(shine_active(CYCLE_TICKS), "and then it comes back around");
}

/// Between passes the mark still owes the screen frames, but far fewer: the drift changes
/// color rather than position, so it does not need the shine's rate.
#[test]
fn the_drift_asks_for_a_fraction_of_the_shines_frames() {
    let resting: Vec<u64> = (SWEEP_TICKS..CYCLE_TICKS).collect();
    let due = resting.iter().filter(|tick| banner_frame_due(**tick)).count();
    let share = due as f32 / resting.len() as f32;
    assert!(share <= 0.5, "the rest costs {share} of the frames a pass does");
    assert!(due > 0, "and the drift never stops");
    for tick in 0..SWEEP_TICKS {
        assert!(banner_frame_due(tick), "every frame of a pass is drawn");
    }
}

/// Mid-pass every lit cell must differ from its neighbor by a hair, not a step.
#[test]
fn the_band_ramps_gradually_across_the_letters() {
    let pass = Pass::at(Wordmark::Full, ink(), SWEEP_TICKS / 2).expect("mid-pass");
    let lifts: Vec<f32> = (0..Wordmark::Full.width()).map(|col| pass.lift(col, 3)).collect();
    assert!(lifts.iter().any(|lift| *lift > 0.5), "something is brightly lit mid-pass");
    assert!(lifts.iter().any(|lift| *lift < 0.05), "and something is untouched");

    let biggest_step =
        lifts.windows(2).map(|pair| (pair[1] - pair[0]).abs()).fold(0.0_f32, f32::max);
    assert!(biggest_step < 0.08, "neighboring cells jump by {biggest_step}, which will band");
}

/// The lean is what gives the pass its depth; without it the band is a vertical wipe.
#[test]
fn the_band_leans_across_the_rows() {
    let pass = Pass::at(Wordmark::Full, ink(), SWEEP_TICKS / 2).expect("mid-pass");
    let brightest = |row: u16| {
        (0..Wordmark::Full.width())
            .max_by(|a, b| pass.lift(*a, row).total_cmp(&pass.lift(*b, row)))
            .expect("a widest cell")
    };
    assert!(
        brightest(0) > brightest(Wordmark::Full.height() - 1),
        "the top of the band runs ahead of the bottom"
    );
}

#[test]
fn a_light_theme_is_lifted_more_gently_than_a_dark_one() {
    let light = WordmarkInk { dark: false, ..ink() };
    let dark = Pass::at(Wordmark::Full, ink(), SWEEP_TICKS / 2).expect("mid-pass");
    let light = Pass::at(Wordmark::Full, light, SWEEP_TICKS / 2).expect("mid-pass");
    let cell = (20, 3);
    assert!(
        light.lift(cell.0, cell.1) < dark.lift(cell.0, cell.1),
        "a light theme's letters wash out if pushed as hard as a dark theme's"
    );
}

/// The shine sits on top of the drift rather than replacing it: mid-pass a cell is lighter than
/// the same cell at rest, and both are still the mark's one hue.
#[test]
fn the_shine_lifts_whatever_the_drift_left() {
    let lit = Wordmark::Full.lines(ink(), Some(SWEEP_TICKS / 2));
    let rested = Wordmark::Full.lines(ink(), Some(SWEEP_TICKS + 10));
    let brightness = |line: &Line<'_>| {
        line.spans
            .iter()
            .filter_map(|span| match span.style.fg {
                Some(Color::Rgb(r, g, b)) => Some(u32::from(r) + u32::from(g) + u32::from(b)),
                _ => None,
            })
            .max()
            .unwrap_or(0)
    };
    assert!(
        brightness(&lit[3]) > brightness(&rested[3]),
        "a pass has to be visible over the drift"
    );
}

/// An ANSI/indexed palette has nothing to blend, and inventing an RGB approximation of a
/// terminal's own color would be a worse answer than leaving the mark alone.
#[test]
fn an_unblendable_palette_renders_the_mark_untouched() {
    let ink = WordmarkInk { accent: Color::Blue, canvas: None, dark: true };
    for tick in [0, SWEEP_TICKS / 2, STILL_TICK] {
        for line in Wordmark::Full.lines(ink, Some(tick)) {
            for span in &line.spans {
                if let Some(color) = span.style.fg {
                    assert_eq!(color, Color::Blue, "an indexed palette was approximated");
                }
            }
        }
    }
}

/// The clock the surfaces share: it advances with the UI tick, holds the still when the
/// reader turned the light off, and keeps its place either way.
#[test]
fn the_clock_runs_only_while_the_light_is_on() {
    let mut clock = BannerClock::default();
    assert_eq!(clock.frame(), Some(0));
    clock.advance();
    clock.advance();
    assert_eq!(clock.frame(), Some(2), "a tick is a frame");

    // Turning it off holds the frame it is on rather than resetting it — and a still is
    // drawn at [`STILL_TICK`] regardless of where the clock happened to stop.
    let mut stopped = clock.with_animation(false);
    assert_eq!(stopped.frame(), None);
    stopped.advance();
    assert_eq!(stopped.with_animation(true).frame(), Some(2), "a stopped clock does not run");
    assert!(!stopped.animating());
}

/// Rest is the common case, so it must not cost a span per cell.
#[test]
fn resting_rows_collapse_into_a_few_spans() {
    let lines = Wordmark::Full.lines(ink(), None);
    for line in &lines {
        assert!(line.spans.len() <= 20, "a resting row is {} spans", line.spans.len());
    }
    assert_eq!(row_text(&lines[0]).chars().count(), Wordmark::Full.width() as usize);
}
