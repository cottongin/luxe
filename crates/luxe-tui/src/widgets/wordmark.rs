//! Wordmark — the luxe logotype, and the light that moves over it.
//!
//! The letters are drawn with three glyphs — the two half blocks and the full block — which every
//! terminal with built-in box drawing synthesizes against the cell box rather than scaling a font
//! outline into it. That is the whole reason the mark looks the same for a reader with a patched
//! Nerd Font and one with the plainest monospace their distribution shipped. See [`art`].
//!
//! Three sizes, picked by [`Wordmark::fit`] from the space on offer: the full 44×7 logotype, a 32×5
//! reduction of the same drawing, and the wordmark set as text for what is left.
//!
//! # The light
//!
//! Two effects, because metal has two. A broad **drift** travels across the letters over about
//! seven seconds, shading each column between the theme's accent and its canvas — that is the
//! diffuse reflection, and it is what makes the mark look like a material rather than a fill. A
//! narrow **shine** crosses in 1.6 seconds and rests for five, blending toward white — the specular
//! highlight, and the one the eye actually catches.
//!
//! Both are pure functions of the tick, so any frame can be reproduced by asking for it, and a still
//! is just the frame at [`STILL_TICK`].
//!
//! Themes whose colors aren't RGB (an ANSI/indexed palette) have nothing to blend. They render the
//! wordmark in the flat accent and skip both rather than approximate them.

mod art;

use art::{column_groups, FULL_ART, LINE_ART, SMALL_ART};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::sync::LazyLock;

/// Ticks for one full pass of the drift (~7s at the 33ms animation tick).
///
/// Slow on purpose. This is the reflection moving, not an indicator: fast enough to read as motion
/// if you look at it, slow enough that you never catch it starting. Three things have to hold when
/// it is retuned, and the assertions below hold all three.
const DRIFT_TICKS: u64 = 212;

/// How far the drift cuts a column back toward the canvas at its dimmest.
///
/// Public because the website animates the same swing in CSS, and the two ends of it have to be the
/// two colors the terminal actually reaches — a second copy of this number is one that drifts.
pub const DRIFT_DEPTH: f32 = 0.55;

/// How much of the drift's wave is visible across the mark at once.
///
/// Half a period, which is the only fraction that makes every frame read as *one* light: the wave
/// runs from a trough to a crest across the letters and never folds back, so the mark is never
/// symmetrical about its own middle the way a full period would make it.
const DRIFT_SPREAD: f32 = 0.5;

/// The frame a still is taken at.
///
/// Half way through a drift, where the wave sits trough-to-crest across the mark: dimmest at the
/// `l`, the accent untouched at the full stop. A clean gradient, and the drawing at its most
/// legible — which is what a reader with `banner_animation = false` should get, and what the
/// website's generated logotype freezes.
///
/// A whole drift period is added on top. The drift cannot tell — its phase repeats — but the shine
/// runs on its own unrelated cycle, so the multiple is the knob for landing the frame in the
/// shine's *rest*. A still caught mid-sweep is a logotype frozen half-lit, which is not a still, it
/// is a bug that looks like a gradient. The assertion below is what re-checks it.
pub const STILL_TICK: u64 = DRIFT_TICKS + DRIFT_TICKS / 2;

/// Ticks between drift frames while no shine is running.
///
/// A frame is mostly escape sequences for the ~300 cells the gradient touches, so this knob sets
/// both the CPU cost and the bytes a reader at the end of an SSH session receives. Four is where
/// the drift reads as continuous (about 14 fps against 30 at every tick); slower saves little,
/// because the *shine* runs at full rate for 1.6 s of every 6.6 s whatever the drift does.
/// `banner_animation = false` turns both off outright.
const DRIFT_FRAME: u64 = 4;

/// Ticks the highlight takes to cross the logotype (~1.6s at the 33ms animation tick).
const SWEEP_TICKS: u64 = 48;

/// Ticks the logotype rests between passes (~5s).
const REST_TICKS: u64 = 152;

/// One pass plus its rest.
const CYCLE_TICKS: u64 = SWEEP_TICKS + REST_TICKS;

// The rest is most of the cycle, and has to stay that way: the shine is a punctuation mark on an
// otherwise still screen, and a short rest turns it into a strobe an arm's length from the eye.
const _: () = assert!(REST_TICKS > SWEEP_TICKS * 3);

// A still has to be a *resting* frame. Retuning either period can slide `STILL_TICK` into a sweep,
// which freezes the mark half-lit; that is a one-character change nobody would think to re-check, so
// the compiler re-checks it. The drift's own two go the same way: an odd period puts `STILL_TICK`
// half a tick off the midpoint, and a period that is not a whole number of frames leaves a short one
// where the wave wraps.
const _: () = assert!(STILL_TICK % CYCLE_TICKS >= SWEEP_TICKS);
const _: () = assert!(DRIFT_TICKS.is_multiple_of(2) && DRIFT_TICKS.is_multiple_of(DRIFT_FRAME));

// The two lights have to stay out of step. A drift that divides into the shine's cycle lands every
// pass at the same point in the wave, and what should read as two independent lights becomes one
// loop repeating: 200 would realign every 6.6 s, where 212 takes near six minutes.
const _: () = assert!(!DRIFT_TICKS.is_multiple_of(CYCLE_TICKS));

/// Cells the band leans right per row, upward — a `/` slant at the terminal's ~1:2 cell aspect.
///
/// Public because the website draws the same band, in this same grid: the mark's SVG is the art
/// scaled by one cell, so the lean carries over as it stands rather than as an angle recomputed
/// from it — and an angle recomputed is an angle that comes out mirrored.
pub const LEAN: f32 = 1.0;

/// Half-width of the highlight as a fraction of the sweep axis.
pub const BAND: f32 = 0.30;

/// How far the highlight blends toward white at its center, on a dark theme and on a light one.
///
/// Held well short of white: these letters are large solid shapes, and a band that takes one all the
/// way to white doesn't look lit, it looks blank — the logotype loses the color it is identified by
/// for a third of the pass. A light theme's letters are saturated mid-tones on near-white and wash
/// out sooner still, so they are pushed less far again.
pub const PEAK_DARK: f32 = 0.62;
pub const PEAK_LIGHT: f32 = 0.30;

/// The logotype, in the sizes it comes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wordmark {
    /// The full 54×8 logotype.
    Full,
    /// A 38×6 reduction for a terminal too narrow or too short for [`Self::Full`].
    Small,
    /// The wordmark set as text, for what's left.
    Line,
}

/// What the logotype is drawn in.
///
/// One hue, not four. A mark is not a legend, so it spends the theme's accent and nothing else — a
/// palette's error red has no business inside the product's name. Everything else about how it
/// looks is light: the drift shades that accent toward the canvas, the shine lifts it toward white.
#[derive(Debug, Clone, Copy)]
pub struct WordmarkInk {
    /// The hue the whole mark is drawn in.
    pub accent: Color,
    /// The canvas behind the logotype, when the theme names one.
    ///
    /// What the drift shades toward, which is what makes one rule right twice: on a dark theme the
    /// letters climb out of the background like brass catching light, and on a light one the same
    /// rule reads as ink laid down. A theme that leaves the background to the terminal has nothing
    /// to shade toward and gets the accent flat, rather than a guess.
    pub canvas: Option<Color>,
    /// Whether the active theme is dark, which sets how hard the highlight may push.
    pub dark: bool,
}

impl WordmarkInk {
    /// The ink a theme draws the logotype in.
    ///
    /// One place, because two surfaces show the mark now, and a second copy of this rule is how
    /// they start disagreeing about what the product's name looks like.
    pub fn from_theme(theme: &crate::theme::styles::ThemeStyles) -> Self {
        Self {
            accent: theme.accent.fg.unwrap_or(Color::Reset),
            canvas: theme.background.bg,
            dark: theme.dark_mode,
        }
    }
}

/// What the highlight blends toward. A lit edge is lighter than the thing it crosses, on ink or on
/// metal, so this is white in both modes and only [`PEAK_DARK`]/[`PEAK_LIGHT`] differ.
const HIGHLIGHT: Color = Color::Rgb(0xff, 0xff, 0xff);

impl Wordmark {
    /// The largest size that fits `width` × `height`.
    pub fn fit(width: u16, height: u16) -> Self {
        for size in [Self::Full, Self::Small] {
            if width >= size.width() && height >= size.height() {
                return size;
            }
        }
        Self::Line
    }

    /// Cells across.
    pub fn width(self) -> u16 {
        self.rows()[0].chars().count() as u16
    }

    /// Rows tall.
    pub fn height(self) -> u16 {
        self.rows().len() as u16
    }

    /// The logotype's rows, shaded for `tick` — or held at [`STILL_TICK`] when `tick` is `None`.
    ///
    /// A still is a frame of the same animation rather than a separate code path, so what a reader
    /// sees with `banner_animation = false` is a moment of what everyone else sees moving.
    pub fn lines(self, ink: WordmarkInk, tick: Option<u64>) -> Vec<Line<'static>> {
        let (tick, pass) = match tick {
            Some(tick) => (tick, Pass::at(self, ink, tick)),
            None => (STILL_TICK, None),
        };
        self.rows()
            .iter()
            .enumerate()
            .map(|(row, text)| self.row_line(text, row as u16, ink, tick, pass.as_ref()))
            .collect()
    }

    /// The logotype's rows, one string per terminal row.
    pub fn rows(self) -> &'static [&'static str] {
        match self {
            Self::Full => &FULL_ART,
            Self::Small => &SMALL_ART,
            Self::Line => &LINE_ART,
        }
    }

    /// Distance along the sweep axis of the last cell, which is how far the band has to travel.
    fn span(self) -> f32 {
        f32::from(self.width() - 1) + LEAN * f32::from(self.height() - 1)
    }

    fn row_line(
        self,
        text: &str,
        row: u16,
        ink: WordmarkInk,
        tick: u64,
        pass: Option<&Pass>,
    ) -> Line<'static> {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut run = String::new();
        let mut run_style = Style::default();

        for (column, glyph) in text.chars().enumerate() {
            let (glyph, style) = self.cell(glyph, column as u16, row, ink, tick, pass);

            // Merge equal-styled neighbors: at rest a whole letter is one span, and a row of the
            // full logotype goes from 54 spans to a handful.
            if !run.is_empty() && style != run_style {
                spans.push(Span::styled(std::mem::take(&mut run), run_style));
            }
            run_style = style;
            run.push(glyph);
        }
        if !run.is_empty() {
            spans.push(Span::styled(run, run_style));
        }
        Line::from(spans)
    }

    /// The glyph and style one cell of the art is drawn with.
    ///
    /// Two lights, in the order a surface receives them: the drift shades the accent toward the
    /// canvas (diffuse), then the shine lifts whatever is left toward white (specular).
    fn cell(
        self,
        glyph: char,
        column: u16,
        row: u16,
        ink: WordmarkInk,
        tick: u64,
        pass: Option<&Pass>,
    ) -> (char, Style) {
        if glyph == ' ' {
            return (glyph, Style::default());
        }

        // Diffuse first: the accent cut back toward the canvas by however far the drift has moved.
        let base = match ink.canvas {
            Some(canvas) => blend_toward(ink.accent, canvas, self.drift(column, tick)),
            None => ink.accent,
        };
        // Then specular, on top, toward white.
        let lit = pass.map(|pass| pass.lift(column, row)).unwrap_or(0.0);
        let color = blend_toward(base, HIGHLIGHT, lit);

        if matches!(self, Self::Line) {
            // Real text at the one-line size, where bold is worth something.
            return (glyph, Style::default().fg(color).add_modifier(Modifier::BOLD));
        }
        (glyph, Style::default().fg(color))
    }

    /// How far this column is cut back toward the canvas at `tick`.
    ///
    /// A raised cosine, so the wave closes on itself and there is no seam where it wraps. The
    /// group, not the raw column, sets the position — which is what keeps a letter one flat tone
    /// instead of a gradient running through the middle of it, and keeps the full stop reading as
    /// punctuation rather than as the tail of the `e`.
    fn drift(self, column: u16, tick: u64) -> f32 {
        let group = self.groups().get(usize::from(column)).copied().unwrap_or(0);
        self.group_shade(group, tick)
    }

    /// How far ink group `group` is cut back toward the canvas at `tick`.
    ///
    /// The same number `Self::drift` uses, reachable by group so the generators that produce the
    /// website's logotype and color tokens compute the ramp from this rather than from a copy of
    /// it. `tick` of [`STILL_TICK`] is the frame a still is taken at.
    pub fn group_shade(self, group: usize, tick: u64) -> f32 {
        let last = self.groups().last().copied().unwrap_or(0).max(1) as f32;
        let phase = (tick % DRIFT_TICKS) as f32 / DRIFT_TICKS as f32;
        let turn = (group as f32 / last * DRIFT_SPREAD - phase) * std::f32::consts::TAU;
        DRIFT_DEPTH * (1.0 - turn.cos()) / 2.0
    }

    /// How many groups of ink this size's art is divided into.
    pub fn group_count(self) -> usize {
        self.groups().last().copied().unwrap_or(0) + 1
    }

    /// Which ink group owns each column of this size's art.
    pub fn groups(self) -> &'static [usize] {
        static TABLES: LazyLock<[Vec<usize>; 3]> = LazyLock::new(|| {
            [column_groups(&FULL_ART), column_groups(&SMALL_ART), column_groups(&LINE_ART)]
        });
        let size = match self {
            Self::Full => 0,
            Self::Small => 1,
            Self::Line => 2,
        };
        &TABLES[size]
    }
}

/// Whether the logotype owes the screen another frame at `tick`.
///
/// True through a shine pass, and on the drift's own slower cadence in between. The drift changes
/// *color* rather than position — there is no edge for the eye to track — so it does not need the
/// shine's frame rate, and a dashboard nobody is touching should not repaint thirty times a second
/// all day.
pub fn banner_frame_due(tick: u64) -> bool {
    shine_active(tick) || tick.is_multiple_of(DRIFT_FRAME)
}

/// The clock every surface that draws the logotype reads.
///
/// One owner, on the app, because two surfaces show the mark and each answering "what frame is it"
/// for itself would let walking from the dashboard to setup stop the light dead. A clock that
/// belongs to the session instead means the mark is the same mark wherever it appears, and
/// `banner_animation = false` turns it off in one place.
#[derive(Debug, Clone, Copy)]
pub struct BannerClock {
    tick: u64,
    animate: bool,
}

impl Default for BannerClock {
    fn default() -> Self {
        Self { tick: 0, animate: true }
    }
}

impl BannerClock {
    /// A clock the reader's `banner_animation` preference has been applied to.
    pub fn new(animate: bool) -> Self {
        Self { tick: 0, animate }
    }

    /// Start at a chosen frame — for a fixture or a test that wants a named moment.
    pub fn with_tick(mut self, tick: u64) -> Self {
        self.tick = tick;
        self
    }

    /// Turn the light on or off, leaving the frame where it is.
    pub fn with_animation(mut self, animate: bool) -> Self {
        self.animate = animate;
        self
    }

    /// Advance by one UI tick.
    pub fn advance(&mut self) {
        if self.animate {
            self.tick = self.tick.wrapping_add(1);
        }
    }

    /// The frame to draw, or `None` to hold the still.
    pub fn frame(self) -> Option<u64> {
        self.animate.then_some(self.tick)
    }

    /// Whether the mark owes the screen another frame.
    ///
    /// False through the five-second rest between passes, which is what keeps a surface nobody is
    /// touching from repainting thirty times a second all day.
    pub fn animating(self) -> bool {
        self.animate && banner_frame_due(self.tick)
    }
}

/// Draw the logotype centered in `area`, at the frame `clock` is on.
///
/// Every surface that shows the mark calls this, so it is one drawing lit one way — a surface
/// holding its own still would put the light out when you walked to it from the dashboard.
pub fn render_wordmark(
    f: &mut Frame,
    area: Rect,
    theme: &crate::theme::styles::ThemeStyles,
    size: Wordmark,
    clock: BannerClock,
) {
    let lines = size.lines(WordmarkInk::from_theme(theme), clock.frame());
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
}

/// The ticks a shine pass occupies, and the ticks one whole cycle does.
///
/// Exposed so a caller that wants to reason about the rest between passes asks rather than
/// re-derives: two copies of a timing constant is one that drifts.
pub const SHINE_SWEEP_TICKS: u64 = SWEEP_TICKS;
/// One pass plus its rest, in ticks.
pub const SHINE_CYCLE_TICKS: u64 = CYCLE_TICKS;

fn shine_active(tick: u64) -> bool {
    tick % CYCLE_TICKS < SWEEP_TICKS
}

/// One tick of one pass: where the highlight band sits, and how hard it pushes.
struct Pass {
    center: f32,
    half: f32,
    peak: f32,
    lean: f32,
}

impl Pass {
    /// The pass in flight at `tick`, or `None` while the logotype rests.
    fn at(size: Wordmark, ink: WordmarkInk, tick: u64) -> Option<Self> {
        let phase = tick % CYCLE_TICKS;
        if phase >= SWEEP_TICKS {
            return None;
        }
        let span = size.span();
        let half = (span * BAND).max(1.0);
        let travel = span + 2.0 * half;
        let progress = phase as f32 / SWEEP_TICKS as f32;

        Some(Self {
            // The band starts and ends clear of the letters, so a pass arrives and leaves rather
            // than appearing half-drawn across them.
            center: -half + progress * travel,
            half,
            peak: if ink.dark { PEAK_DARK } else { PEAK_LIGHT },
            lean: LEAN,
        })
    }

    /// How far this cell is lifted toward the highlight.
    fn lift(&self, column: u16, row: u16) -> f32 {
        let axis = f32::from(column) + self.lean * f32::from(row);
        let distance = ((axis - self.center).abs() / self.half).min(1.0);
        smoothstep(1.0 - distance) * self.peak
    }
}

/// Smooth 0→1 with zero slope at both ends, so a band edge has no seam.
///
/// The shape of the highlight, and the whole reason it reads as light rather than as a stripe: a
/// linear falloff leaves a crease along the ridge and a hard shoulder at each edge. Public so the
/// website samples this curve rather than approximating it.
pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Alpha-blend `base` `amount` of the way toward `target`.
///
/// An ANSI/indexed palette has no channels to mix, so an unblendable color comes back untouched
/// rather than as an invented RGB approximation of the terminal's own.
fn blend_toward(base: Color, target: Color, amount: f32) -> Color {
    let (Color::Rgb(red, green, blue), Color::Rgb(tr, tg, tb)) = (base, target) else {
        return base;
    };
    if amount <= 0.0 {
        return base;
    }
    let amount = amount.clamp(0.0, 1.0);
    let lift = |channel: u8, toward: u8| {
        (f32::from(channel) + (f32::from(toward) - f32::from(channel)) * amount)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color::Rgb(lift(red, tr), lift(green, tg), lift(blue, tb))
}

#[cfg(test)]
mod tests;
