//! Shades — fades, the slab and gauge ramps, depths and washes, derived from a theme's tokens.

use super::color::{blend_rgb, rgb_color};
use ratatui::style::{Color as RatatuiColor, Modifier, Style};

/// Shades in the activity comet's ramp, its leading cell first.
///
/// The widget stretches however many of these it needs over the comet, so this is a ceiling on the
/// gradient's smoothness rather than a cell count.
pub const ACTIVITY_TRAIL_LEN: usize = 10;

/// Characters the streamed-text fade covers, its settled end first.
///
/// About a word and a half: long enough to read as motion rather than a hard edge, short enough that
/// the text you are actually reading is at full contrast.
pub const STREAM_TRAIL_LEN: usize = 16;

/// How a ramp fades: its curve, what it keeps at the far end, and the lift on its near end.
#[derive(Debug, Clone, Copy)]
pub(super) struct Fade {
    /// Shape of the falloff. Above 1 eases *out* — the near shades hold their color and the fade
    /// accelerates away from them; 1 is a straight line.
    pub(super) falloff: f32,
    /// Color left in the last shade, as a fraction. Zero lands on the base exactly, so the ramp's
    /// far end disappears into it.
    pub(super) floor: f32,
    /// Lift on the first shade, toward white on a dark theme and toward black on a light one — a hot
    /// tip, so the ramp reads as having a direction. Zero leaves it alone.
    pub(super) lift: f32,
}

/// The comet: eases out hard and lands on the rail exactly, so its tail dissolves rather than
/// stopping above the rail as a stub that never quite goes away.
pub(super) const COMET_FADE: Fade = Fade { falloff: 2.0, floor: 0.0, lift: 0.18 };

/// The rail's glow: gentler, because ambient light should reach a good way down the bar, and floored
/// a whisper above the canvas rather than at it so the ends blend instead of cutting off.
pub(super) const RAIL_FADE: Fade = Fade { falloff: 1.6, floor: 0.1, lift: 0.0 };

/// Streamed text arriving: lands on the canvas exactly, so a character starts invisible and resolves
/// as it ages. Eases *in* rather than out — a falloff below one holds the color across most of the
/// ramp and drops away over the last few characters, so only the very newest are hard to read.
pub(super) const STREAM_FADE: Fade = Fade { falloff: 0.7, floor: 0.0, lift: 0.0 };

/// A fill gauge's value: near-linear, floored *well* clear of the rail, and lifted lightly.
///
/// Much shallower than the comet's on purpose. A gauge's *length* is its datum, so the shading is
/// there to point at the frontier rather than to dissolve anything: fade the start toward the rail
/// and you lose where the bar begins, and a hot tip on a bar that never moves reads as glare.
pub(super) const GAUGE_FADE: Fade = Fade { falloff: 1.0, floor: 0.45, lift: 0.12 };

/// How far the cursor line's band leans from the canvas toward the text color.
pub(super) const CURSOR_LINE_WASH: f32 = 0.07;

/// How far a pick's band leans from the canvas toward the accent.
pub(super) const PICK_WASH: f32 = 0.24;

/// A ramp of [`ACTIVITY_TRAIL_LEN`] shades fading `style`'s color into `base`, nearest end first.
pub(super) fn shade_ramp(
    style: Style,
    base: (u8, u8, u8),
    light: bool,
    fade: Fade,
) -> [Style; ACTIVITY_TRAIL_LEN] {
    std::array::from_fn(|index| shade_at(style, base, light, fade, index, ACTIVITY_TRAIL_LEN))
}

/// Shade `index` of a `len`-step ramp fading `style`'s color into `base`.
///
/// Index 0 is the near end and keeps the color; the last index lands on `fade.floor` of it, so a
/// floor of zero disappears into `base` exactly.
///
/// Needs an RGB color to interpolate. A theme that names an ANSI or indexed color instead cannot
/// be blended without guessing the terminal palette, so it degrades to a two-step ramp — the color,
/// then the color dimmed — which still gives the ramp a near end and a far one.
pub(super) fn shade_at(
    style: Style,
    base: (u8, u8, u8),
    light: bool,
    fade: Fade,
    index: usize,
    len: usize,
) -> Style {
    let Some(RatatuiColor::Rgb(r, g, b)) = style.fg else {
        // Nothing to interpolate, so the ramp degrades to color-or-dimmed. A floor past halfway is
        // a ramp that has mostly resolved, and dimming its far end there would be the opposite of
        // what the floor asked for.
        let keep = index == 0 || fade.floor >= 0.5;
        return if keep { style } else { style.add_modifier(Modifier::DIM) };
    };
    let steps = (len.max(2) - 1) as f32;
    let curve = (1.0 - index as f32 / steps).powf(fade.falloff);
    let weight = fade.floor + (1.0 - fade.floor) * curve;
    let mixed = blend_rgb((r, g, b), base, weight);
    let shade = match (index, mixed) {
        (0, RatatuiColor::Rgb(r, g, b)) if fade.lift > 0.0 => {
            let toward = if light { (0, 0, 0) } else { (255, 255, 255) };
            blend_rgb(toward, (r, g, b), fade.lift)
        }
        _ => mixed,
    };
    style.fg(shade)
}

/// How far a slab has receded from the accent toward the canvas behind it.
///
/// One hue, three stops. Depth is a *role*, not a position in the row: assigning it per module
/// keeps the bar's colors still when a module drops out, which a positional ramp would not.
///
/// It was five, and four of them were the same color to a reader. Measured across all twelve
/// shipped themes the receded steps sat 1.14–1.33:1 apart — a hierarchy the code computed, tested
/// and assigned by role, of which the eye could resolve two levels. Three stops at 1.6–2.4:1 is a
/// distinction someone can actually use, and saying three honestly beats saying five and meaning
/// two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Depth {
    /// The accent itself, wearing ink rather than foreground text
    Hero,
    /// One step back — the working surfaces
    Raised,
    /// Two steps back — ambient telemetry, present but not asking for attention
    Recessed,
}

/// Receded steps between the accent and the canvas.
pub const SLAB_DEPTHS: usize = 2;

/// Where those steps sit, as fractions of the canvas mixed into the accent.
///
/// Chosen so both steps clear 1.5:1 against their neighbor in **every** shipped theme, light and
/// dark, rather than for even spacing in blend fraction — which is not even spacing in anything an
/// eye measures, and would leave steps nobody can see.
///
/// Blending stays in sRGB: recomputed in OkLab these land within a couple of points, because both
/// endpoints of a theme ramp share a hue and the path is short. The extra color space would buy
/// arithmetic, not a difference anyone can see.
const SLAB_STOPS: [f32; SLAB_DEPTHS] = [0.40, 0.70];

/// How much of the comment color an idle slab is pressed from: at rest, and under the pointer.
///
/// Enough that the shape reads as a slab beside a lit one, little enough that nobody mistakes it
/// for the chosen one. The second step is the hover, which says "this is the other half of the
/// switch" without borrowing the accent.
const IDLE_STOPS: [f32; 2] = [0.22, 0.32];

/// The surfaces a detached slab can wear, and the two colors that frame them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlabShades {
    /// The hero surface — the accent, undiluted
    pub hero: RatatuiColor,
    /// The receded surfaces, nearest the accent first
    pub receded: [RatatuiColor; SLAB_DEPTHS],
    /// Text on [`Self::hero`]: the canvas color, so the brightest slab reads as cut *out* of the bar
    pub ink: RatatuiColor,
    /// What the caps taper into
    pub canvas: RatatuiColor,
    /// The surfaces of a slab that is *not* chosen — the other face of a switch — at rest and
    /// lifted: the comment color, pressed into the canvas rather than the accent
    pub idle: [RatatuiColor; 2],
}

impl SlabShades {
    /// The ramp for `accent` over `canvas`, falling back to `surface` where there is nothing to mix.
    ///
    /// An ANSI or indexed theme names colors the *terminal* owns, so there are no channels to
    /// interpolate and no honest way to invent four steps. Those themes collapse to two levels — the
    /// accent, and the selection surface they already define — which still gives a slab an edge and
    /// the bar a hero, without a gradient made up on their behalf.
    ///
    /// An idle slab mixes `muted` into the canvas the same way, and in a theme with nothing to mix
    /// takes the selection surface, or no fill at all — its caps and its quieter text still say
    /// which face is not chosen.
    pub(super) fn derive(
        accent: Style,
        muted: Style,
        canvas: Style,
        surface: Style,
        light: bool,
    ) -> Self {
        let hero = accent.fg.unwrap_or(RatatuiColor::Reset);
        let flat = surface.bg.or(accent.fg).unwrap_or(RatatuiColor::Reset);
        // A theme whose canvas is the terminal's own has no color to ink with, so take the end of the
        // range its accent is not at: light themes accent dark, dark themes accent bright.
        let unknown_canvas = if light { RatatuiColor::White } else { RatatuiColor::Black };

        let receded = match (rgb_color(accent.fg), rgb_color(canvas.bg)) {
            // `blend_rgb(a, b, t)` is `a*t + b*(1-t)`, so mixing the canvas *in* at each stop recedes
            // the accent by exactly that fraction.
            (Some(accent), Some(canvas)) => SLAB_STOPS.map(|stop| blend_rgb(canvas, accent, stop)),
            _ => [flat; SLAB_DEPTHS],
        };
        let idle = match (rgb_color(muted.fg), rgb_color(canvas.bg)) {
            (Some(muted), Some(canvas)) => IDLE_STOPS.map(|stop| blend_rgb(muted, canvas, stop)),
            _ => [surface.bg.or(canvas.bg).unwrap_or(RatatuiColor::Reset); 2],
        };
        Self {
            hero,
            receded,
            ink: canvas.bg.unwrap_or(unknown_canvas),
            canvas: canvas.bg.unwrap_or(RatatuiColor::Reset),
            idle,
        }
    }

    /// Every surface the ramp draws a slab on, hero first.
    ///
    /// For callers that need to recognize a slab after the fact rather than draw one — dimming a
    /// surface behind a modal has to tell chrome from evidence, and a slab's fill is the chrome.
    pub fn ramp(&self) -> [RatatuiColor; SLAB_DEPTHS + 1] {
        [self.hero, self.receded[0], self.receded[1]]
    }

    /// The surface at `depth`.
    pub fn at(&self, depth: Depth) -> RatatuiColor {
        match depth {
            Depth::Hero => self.hero,
            Depth::Raised => self.receded[0],
            Depth::Recessed => self.receded[1],
        }
    }
}

/// The two ramps a fill gauge shades along.
#[derive(Debug, Clone)]
pub struct GaugeShades {
    /// The value's shades, its leading edge first — the bar brightens as it fills toward the frontier.
    pub fill: Vec<Style>,
    /// The rail's shades, nearest the frontier first, fading out into the band behind it.
    pub rail: Vec<Style>,
}
