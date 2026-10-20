//! Theme → ratatui::Style derivation.
//!
//! Maps a resolved `HashMap<String, ColorValue>` (from `ThemeRegistry`) to concrete
//! [`ratatui::style::Style`] values for every theme token. Missing or unparseable
//! tokens fall back to sensible defaults — never panics.

mod color;
mod shades;

use color::{blend_rgb, color_value_to_ratatui, plan_hue, rgb_color, rgb_token, theme_is_dark};
use luxe_core::theme::ColorValue;
use ratatui::style::{Modifier, Style};
use shades::{
    shade_at, shade_ramp, Fade, COMET_FADE, CURSOR_LINE_WASH, GAUGE_FADE, PICK_WASH, RAIL_FADE,
    STREAM_FADE,
};
pub use shades::{
    Depth, GaugeShades, SlabShades, ACTIVITY_TRAIL_LEN, SLAB_DEPTHS, STREAM_TRAIL_LEN,
};
use std::collections::HashMap;
#[cfg(test)]
use {color::is_dark_rgb, ratatui::style::Color as RatatuiColor};

/// Pre-computed [`Style`] for every theme token.
///
/// Constructed from a resolved theme map via [`ThemeStyles::from_resolved`].
#[derive(Debug, Clone, PartialEq)]
pub struct ThemeStyles {
    pub accent: Style,
    pub accent_secondary: Style,
    pub info: Style,
    pub border: Style,
    pub border_accent: Style,
    pub border_muted: Style,
    /// What plan mode wears where the default mode wears `border_accent`
    pub mode_plan: Style,
    pub success: Style,
    pub error: Style,
    pub warning: Style,
    pub muted: Style,
    pub dim: Style,
    pub text: Style,
    /// Shades the activity comet wears, its leading cell first, fading toward the rail behind it.
    pub activity_trail: [Style; ACTIVITY_TRAIL_LEN],
    /// Shades a spent unit wears as the comet's glow falls off it, nearest the comet first.
    ///
    /// The last is the canvas itself but for a whisper, so the rail dissolves with distance instead
    /// of ending in a hard row of ticks across the whole width.
    pub activity_rail: [Style; ACTIVITY_TRAIL_LEN],
    /// Compact paste placeholder foreground and background
    pub paste_indicator: Style,
    /// An atomic token in the composer — a reference, a paste, an image — drawn in the accent on
    /// the paste background so it reads as one thing, not as typed text
    pub chip: Style,
    pub thinking_text: Style,
    pub thinking_off: Style,
    pub thinking_minimal: Style,
    pub thinking_low: Style,
    pub thinking_medium: Style,
    pub thinking_high: Style,
    pub thinking_xhigh: Style,
    pub thinking_ultra: Style,
    pub selected_bg: Style,
    /// Background tint behind added diff lines (Guided Review).
    pub diff_add_bg: Style,
    /// Background tint behind removed diff lines (Guided Review).
    pub diff_del_bg: Style,
    /// Deeper wash marking the changed span inside an added line.
    pub diff_add_emphasis: Style,
    /// Deeper wash marking the changed span inside a removed line.
    pub diff_del_emphasis: Style,
    /// App base background (the main canvas fill).
    pub background: Style,
    /// Distinct raised surface behind user messages.
    pub user_msg_bg: Style,
    /// Distinct raised surface behind tool calls in the transcript.
    pub tool_call_bg: Style,
    /// Tool-call band tinted by outcome: a subtle wash of the theme's success color.
    pub tool_call_success_bg: Style,
    /// Tool-call band tinted by outcome: a subtle wash of the theme's error color.
    pub tool_call_error_bg: Style,
    /// Behind a review comment card: a faint wash of the theme's info color.
    pub card_info_bg: Style,
    /// Behind a review comment card: a faint wash of the theme's error color.
    pub card_error_bg: Style,
    pub tool_bg: Style,
    pub tool_title: Style,
    pub tool_output: Style,
    pub md_heading: Style,
    pub md_link: Style,
    pub md_link_url: Style,
    pub md_code: Style,
    pub md_code_block: Style,
    pub md_code_block_border: Style,
    pub md_quote: Style,
    /// Surfaces the detached slabs are cut from: one accent receding into the canvas.
    pub slab: SlabShades,
    /// Whether the active theme is dark (used for syntax-highlighting theme selection).
    pub dark_mode: bool,
}

impl ThemeStyles {
    /// Build [`ThemeStyles`] from a resolved color map (token → `ColorValue`).
    ///
    /// Missing tokens fall back to sane defaults. Unparseable `ColorValue` variants
    /// also fall back — never panics.
    pub fn from_resolved(resolved: &HashMap<String, ColorValue>, theme_name: &str) -> Self {
        let tokens = Tokens(resolved);
        let (fg, bg, fg_or) =
            (|token| tokens.fg(token), |token| tokens.bg(token), |a, b| tokens.fg_or(a, b));

        let light = !theme_is_dark(resolved, theme_name);
        let accent = fg("accent");
        let accent_secondary = resolved
            .get("accentSecondary")
            .and_then(color_value_to_ratatui)
            .map(|color| Style::new().fg(color))
            .unwrap_or(accent);
        let paste_indicator = fg("text").patch(tokens.bg_or("pasteIndicatorBg", "selectedBg"));
        let chip = accent.patch(tokens.bg_or("pasteIndicatorBg", "selectedBg"));
        // The app canvas, as a local: the slab caps taper into it, so they have to be painted the
        // same color the base fill is.
        let app_canvas = tokens.bg_or("background", "userMsgBg");
        let tint_base =
            rgb_token(resolved, "background").or_else(|| rgb_token(resolved, "userMsgBg"));
        let Bands {
            tool_call_bg,
            tool_call_success_bg,
            tool_call_error_bg,
            card_info_bg,
            card_error_bg,
            diff_add_bg,
            diff_del_bg,
            diff_add_emphasis,
            diff_del_emphasis,
        } = derived_bands(&tokens, tint_base);
        let (activity_trail, activity_rail) =
            derived_activity_shades(&tokens, accent, tint_base, light);

        ThemeStyles {
            accent,
            accent_secondary,
            info: fg("info"),
            border: fg("border"),
            border_accent: fg("borderAccent"),
            border_muted: fg("borderMuted"),
            mode_plan: plan_hue(resolved),
            success: fg("success"),
            error: fg("error"),
            warning: fg("warning"),
            muted: fg("muted"),
            dim: fg("dim"),
            text: fg("text"),
            activity_trail,
            activity_rail,
            paste_indicator,
            chip,
            thinking_text: fg("thinkingText"),
            thinking_off: fg_or("thinkingOff", "muted"),
            thinking_minimal: fg_or("thinkingMinimal", "text"),
            thinking_low: fg_or("thinkingLow", "warning"),
            thinking_medium: fg_or("thinkingMedium", "success"),
            thinking_high: fg_or("thinkingHigh", "accent"),
            thinking_xhigh: fg_or("thinkingXHigh", "accentSecondary"),
            thinking_ultra: fg_or("thinkingUltra", "accentSecondary"),
            selected_bg: bg("selectedBg"),
            // Fall back to the selection tint when a theme predates the diff tokens, so custom
            // themes without them still get a visible (if generic) diff background.
            diff_add_bg,
            diff_del_bg,
            diff_add_emphasis,
            diff_del_emphasis,
            background: app_canvas,
            user_msg_bg: bg("userMsgBg"),
            tool_call_bg,
            tool_call_success_bg,
            tool_call_error_bg,
            card_info_bg,
            card_error_bg,
            tool_bg: bg("toolBg"),
            tool_title: fg("toolTitle"),
            tool_output: fg("toolOutput"),
            md_heading: fg("mdHeading"),
            md_link: fg("mdLink"),
            md_link_url: fg("mdLinkUrl"),
            md_code: fg("mdCode"),
            md_code_block: bg("mdCodeBlock"),
            md_code_block_border: fg("mdCodeBlockBorder"),
            md_quote: fg("mdQuote"),
            slab: SlabShades::derive(accent, fg("muted"), app_canvas, bg("selectedBg"), light),
            dark_mode: !light,
        }
    }

    /// Cursor/marker style for a list row.
    ///
    /// The `❯` (or checkbox, or state glyph) leading a row: accent throughout, picking up the
    /// selection background on the focused row so the highlight runs edge to edge.
    pub fn accent_on(&self, selected: bool) -> Style {
        if selected {
            self.accent.patch(self.selected_bg)
        } else {
            self.accent
        }
    }

    /// Primary label style for a list row.
    ///
    /// The focused row gets the selection background *and* bold weight, so the cursor stays
    /// legible where `selectedBg` is a subtle tint — or where the terminal drops backgrounds
    /// entirely (copy/paste, `TERM` without truecolor, screen readers).
    pub fn label_on(&self, selected: bool) -> Style {
        if selected {
            self.text.add_modifier(Modifier::BOLD).patch(self.selected_bg)
        } else {
            self.text
        }
    }

    /// How full the context window reads at `percent`: body text, then warning, then error.
    ///
    /// One ramp for every surface that prints the number (the rail, the statusline, the phone
    /// app), so a session never reads as "fine" in one place and "filling" in another. Low is body
    /// text rather than a color, because a window with room left is not news; the hue is spent on
    /// the point where the next long turn compacts.
    pub fn context_fill(&self, percent: f32) -> Style {
        match ContextFill::at(percent) {
            ContextFill::Full => self.error,
            ContextFill::Filling => self.warning,
            ContextFill::Roomy => self.text,
        }
    }

    /// A background `amount` of the way from the canvas toward `hue`'s color, or `fallback` where
    /// the theme speaks in terminal palette indices and there is nothing to blend.
    pub fn wash(&self, hue: Style, amount: f32, fallback: Style) -> Style {
        match (rgb_color(hue.fg), rgb_color(self.background.bg)) {
            (Some(hue), Some(canvas)) => {
                Style::default().bg(blend_rgb(hue, canvas, amount.clamp(0.0, 1.0)))
            }
            _ => fallback,
        }
    }

    /// The faint band under the line a cursor stands on: there at a glance, quieter than a pick.
    pub fn cursor_line_bg(&self) -> Style {
        self.wash(self.text, CURSOR_LINE_WASH, self.tool_bg)
    }

    /// The band under picked lines, in the accent's hue, so a pick never reads as the cursor line
    /// or as the plain highlight a hovered row wears.
    pub fn pick_bg(&self) -> Style {
        self.wash(self.accent, PICK_WASH, self.selected_bg)
    }

    /// The ramps a fill gauge shades along, for a value drawn in `fill` on a `band`.
    ///
    /// Derived per call rather than stored: a caller brings its own hue, and the band it sits on is
    /// not always the app canvas — the rail has to fade into whatever is actually behind it or its
    /// far end reads as dark marks instead of disappearing. Every shade comes back wearing `band`,
    /// so a caller cannot forget it.
    pub fn gauge_shades(&self, fill: Style, band: Style) -> GaugeShades {
        let light = !self.dark_mode;
        let canvas = rgb_color(band.bg)
            .or_else(|| rgb_color(self.background.bg))
            .unwrap_or(if light { (255, 255, 255) } else { (0, 0, 0) });
        let rail = rgb_color(self.border_muted.fg).unwrap_or(canvas);
        let shades = |style, base, fade| {
            shade_ramp(style, base, light, fade)
                .into_iter()
                .map(|shade| shade.patch(band))
                .collect()
        };
        GaugeShades {
            fill: shades(fill, rail, GAUGE_FADE),
            rail: shades(self.border_muted, canvas, RAIL_FADE),
        }
    }

    /// The ramp the newest characters of streamed text fade up through, settled end first.
    ///
    /// [`STREAM_TRAIL_LEN`] shades from `style`'s own color down onto whatever is behind the text,
    /// so the character that just arrived starts on the background — invisible — and resolves as the
    /// next ones land. `band` is what the text actually sits on: a fenced code block brings its own
    /// background, while reasoning and prose sit directly on the canvas. Only the blend base is taken
    /// from it; the shades keep `style`'s own background, since whatever paints the band already has.
    ///
    /// `settle` lifts the whole ramp toward the full color, which is what a stalled stream needs:
    /// the fade only means "still arriving" while something is, and at 1 every shade is `style`
    /// itself. It rides in as the ramp's floor because that is exactly what a floor already is —
    /// the color the far end keeps instead of disappearing into `base`.
    pub fn stream_shades(&self, style: Style, band: Style, settle: f32) -> Vec<Style> {
        let light = !self.dark_mode;
        let base = rgb_color(band.bg)
            .or_else(|| rgb_color(self.background.bg))
            .unwrap_or(if light { (255, 255, 255) } else { (0, 0, 0) });
        let fade = Fade { floor: settle.clamp(0.0, 1.0), ..STREAM_FADE };
        (0..STREAM_TRAIL_LEN)
            .map(|index| shade_at(style, base, light, fade, index, STREAM_TRAIL_LEN))
            .collect()
    }

    /// How the changed span of a replaced diff line is marked, over that line's own `base` style.
    ///
    /// Two signals, because they fail differently. The deeper wash of the row's own hue is what makes
    /// the eye land on the token that changed instead of re-reading both lines; bold says the same
    /// on either side and survives a theme or terminal that has no background to give. Which way it
    /// went is the row's own color. Never struck through: a strike draws over the very text a reader
    /// is comparing against its replacement.
    pub fn diff_emphasis(&self, base: Style, added: bool) -> Style {
        let wash = if added { self.diff_add_emphasis } else { self.diff_del_emphasis };
        base.patch(wash).add_modifier(Modifier::BOLD)
    }

    /// De-emphasized text that stays readable on a highlighted row.
    ///
    /// Most bundled themes set `selectedBg` to the same value as `dim`, so `dim`-on-selected
    /// renders as invisible text. Step up to `muted` there and keep `dim` everywhere else.
    pub fn secondary_on(&self, selected: bool) -> Style {
        if selected {
            self.muted.patch(self.selected_bg)
        } else {
            self.dim
        }
    }
}

/// A theme's resolved tokens, read as styles.
struct Tokens<'a>(&'a HashMap<String, ColorValue>);

impl Tokens<'_> {
    fn color(&self, token: &str) -> Option<ratatui::style::Color> {
        self.0.get(token).and_then(color_value_to_ratatui)
    }

    fn fg(&self, token: &str) -> Style {
        self.color(token).map(|c| Style::new().fg(c)).unwrap_or_default()
    }

    fn bg(&self, token: &str) -> Style {
        self.color(token).map(|c| Style::new().bg(c)).unwrap_or_default()
    }

    fn bg_or(&self, token: &str, fallback: &str) -> Style {
        self.0
            .get(token)
            .or_else(|| self.0.get(fallback))
            .and_then(color_value_to_ratatui)
            .map(|c| Style::new().bg(c))
            .unwrap_or_default()
    }

    fn fg_or(&self, token: &str, fallback: &str) -> Style {
        self.0
            .get(token)
            .or_else(|| self.0.get(fallback))
            .and_then(color_value_to_ratatui)
            .map(|c| Style::new().fg(c))
            .unwrap_or_default()
    }
}

/// The backgrounds derived from a theme's hues rather than read from it: tool bands, cards, and
/// diff rows with their emphasis.
struct Bands {
    tool_call_bg: Style,
    tool_call_success_bg: Style,
    tool_call_error_bg: Style,
    card_info_bg: Style,
    card_error_bg: Style,
    diff_add_bg: Style,
    diff_del_bg: Style,
    diff_add_emphasis: Style,
    diff_del_emphasis: Style,
}

/// Tool-call band + its outcome tints, the cards' fainter washes, and the diff rows' tints with the
/// deeper wash that marks what changed inside a replaced line.
///
/// The tints are a low-alpha wash of the theme's success/error color over the base canvas, so a
/// completed/failed call reads at a glance without a loud fill. An explicit
/// `toolCall{Success,Error}Bg` token wins; RGB themes blend; ANSI/indexed themes (no blendable RGB)
/// fall back to the neutral band.
fn derived_bands(tokens: &Tokens<'_>, tint_base: Option<(u8, u8, u8)>) -> Bands {
    let resolved = tokens.0;
    let tool_call_bg = tokens.bg_or("toolCallBg", "userMsgBg");
    let status_bg = |explicit: &str, hue: &str| -> Style {
        if let Some(style) = tokens.color(explicit).map(|c| Style::new().bg(c)) {
            return style;
        }
        match (rgb_token(resolved, hue), tint_base) {
            (Some(hue), Some(base)) => Style::new().bg(blend_rgb(hue, base, TOOL_STATUS_TINT)),
            _ => tool_call_bg,
        }
    };
    // Fainter than a tool band: a card sits among diff rows that are tinted already, and only
    // has to read as one object, not as a verdict.
    let card_bg = |hue: &str| match (rgb_token(resolved, hue), tint_base) {
        (Some(hue), Some(base)) => Style::new().bg(blend_rgb(hue, base, CARD_TINT)),
        _ => tool_call_bg,
    };
    // The emphasis is the row's own hue mixed further into the row tint, so it reads as more of the
    // same signal rather than a new one. An explicit `diff{Add,Del}EmphasisBg` token wins; an
    // ANSI/indexed theme has no blendable RGB, so it keeps the row tint and lets the strike/bold
    // marks carry the emphasis alone.
    let diff_add_bg = tokens.bg_or("diffAddBg", "selectedBg");
    let diff_del_bg = tokens.bg_or("diffDelBg", "selectedBg");
    let emphasis_bg = |explicit: &str, hue: &str, row: Style| -> Style {
        if let Some(style) = tokens.color(explicit).map(|c| Style::new().bg(c)) {
            return style;
        }
        match (rgb_token(resolved, hue), rgb_color(row.bg).or(tint_base)) {
            (Some(hue), Some(base)) => Style::new().bg(blend_rgb(hue, base, DIFF_EMPHASIS_TINT)),
            _ => row,
        }
    };
    Bands {
        tool_call_bg,
        tool_call_success_bg: status_bg("toolCallSuccessBg", "success"),
        tool_call_error_bg: status_bg("toolCallErrorBg", "error"),
        card_info_bg: card_bg("info"),
        card_error_bg: card_bg("error"),
        diff_add_bg,
        diff_del_bg,
        diff_add_emphasis: emphasis_bg("diffAddEmphasisBg", "success", diff_add_bg),
        diff_del_emphasis: emphasis_bg("diffDelEmphasisBg", "error", diff_del_bg),
    }
}

/// The activity comet's trail and the rail it sweeps over.
///
/// The comet fades from the accent into the rail, so its tail dissolves into the chrome instead of
/// stopping at an edge.
fn derived_activity_shades(
    tokens: &Tokens<'_>,
    accent: Style,
    tint_base: Option<(u8, u8, u8)>,
    light: bool,
) -> ([Style; ACTIVITY_TRAIL_LEN], [Style; ACTIVITY_TRAIL_LEN]) {
    let resolved = tokens.0;
    let rail = rgb_token(resolved, "borderMuted")
        .or_else(|| rgb_token(resolved, "dim"))
        .or(tint_base)
        .unwrap_or(if light { (255, 255, 255) } else { (0, 0, 0) });
    let activity_trail = shade_ramp(accent, rail, light, COMET_FADE);
    let canvas = tint_base.unwrap_or(if light { (255, 255, 255) } else { (0, 0, 0) });
    let activity_rail = shade_ramp(tokens.fg("borderMuted"), canvas, light, RAIL_FADE);
    (activity_trail, activity_rail)
}

/// Fraction of the status hue mixed into the canvas for a tool-call band. Deliberately low so the
/// tint reads as a wash, not a fill.
const TOOL_STATUS_TINT: f32 = 0.16;

/// Fraction of a card's hue mixed into the canvas behind a review comment.
const CARD_TINT: f32 = 0.09;

/// Fraction of the line's own hue mixed into its row tint to mark the span that changed.
///
/// Enough that a changed token reads as a block the eye lands on before the reader has parsed a
/// character of it, and no more: the strike and the bold are the primary signal, so a wash deep enough
/// to compete with them takes legibility and gives back nothing. It sits where the span is still
/// obviously located at a glance but the marks read first.
const DIFF_EMPHASIS_TINT: f32 = 0.20;

/// How full a context window reads, on the ramp [`ThemeStyles::context_fill`] colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextFill {
    /// Room left, which is not news
    Roomy,
    /// Past the warning mark
    Filling,
    /// Near the point where the next long turn compacts
    Full,
}

impl ContextFill {
    /// Where `percent` sits on the ramp.
    pub(crate) fn at(percent: f32) -> Self {
        if percent >= CONTEXT_ERROR_PCT {
            Self::Full
        } else if percent >= CONTEXT_WARNING_PCT {
            Self::Filling
        } else {
            Self::Roomy
        }
    }
}

/// Context occupancy at which the reading turns warning-colored.
const CONTEXT_WARNING_PCT: f32 = 75.0;

/// Context occupancy at which it turns error-colored.
const CONTEXT_ERROR_PCT: f32 = 90.0;

#[cfg(test)]
mod tests;
