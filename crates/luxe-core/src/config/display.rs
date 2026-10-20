//! Display settings — how the TUI looks: statusline, start screen, glyphs, images, notifications
//! and the Context Dock.

use serde::{Deserialize, Serialize};

mod conversation;
mod diff;

pub use conversation::*;
pub use diff::*;

/// A setting read leniently: a value this build does not know deserializes to [`Lenient::UNKNOWN`],
/// and resolving it gives the default, with a warning, so one typo never costs the whole file.
pub(crate) trait Lenient: Copy + PartialEq {
    /// What a value this build does not know deserializes to
    const UNKNOWN: Self;
    /// What applies when nobody set it
    const DEFAULT: Self;
    /// [`Self::DEFAULT`] as a config file spells it, for the settings surface and the warning
    const DEFAULT_NAME: &'static str;
    /// The setting, as the warning names it
    const NAME: &'static str;
}

/// The value in force: the one set, or the default when it is unset or unknown.
pub(crate) fn known_or<T: Lenient>(value: Option<T>) -> T {
    match value {
        Some(value) if value == T::UNKNOWN => {
            tracing::warn!("unknown {}; falling back to {}", T::NAME, T::DEFAULT_NAME);
            T::DEFAULT
        }
        Some(value) => value,
        None => T::DEFAULT,
    }
}

/// Configurable modules for either half of the TUI statusline.
///
/// The left half favors workspace/interaction state; the right half favors model/runtime
/// telemetry. A module only renders on the side that knows how to build it, so listing a
/// right-only module on the left (or vice versa) is a harmless no-op.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatuslineModule {
    // === Left half ===
    /// Project name, as the bar's left anchor
    Project,
    /// Git branch with a dirty-worktree indicator
    Branch,
    /// The branch's pull request on GitHub, drawn only when it has one
    Pr,
    /// How far the agent's checklist has got, and what it is pointing at
    Todo,
    /// How many files have changed, and the key that shows them
    Changes,
    /// Compact working-directory path
    Cwd,
    /// Turn activity: spinner, active tool, elapsed time
    Activity,

    // === Right half ===
    /// Estimated turn cost
    Cost,
    /// Prompt and completion token counts
    Tokens,
    /// Context usage and context-window size
    Context,
    /// Active thinking level
    Thinking,
    /// Active model name
    Model,

    /// Unknown module names are ignored for forward compatibility. Retired names land here too, so
    /// an older config keeps loading — `mode` was one, from an experiment in emulating a modal
    /// editor's statusline, and `lsp` another, which only ever read back a setting.
    #[serde(other)]
    Unknown,
}

/// How the statusline draws its modules.
///
/// Shape only — which modules appear and in what order is [`StatuslineConfig::left`] and
/// [`StatuslineConfig::right`], and both styles honor them identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatuslineStyle {
    /// Each module a diagonal-capped parallelogram floating on the bar, colored off one accent
    /// receding into the background (default).
    #[default]
    Slab,
    /// Tinted text on the bar background, with no surface behind it.
    Flat,
    /// Unknown values fall back to slabs.
    #[serde(other)]
    Unknown,
}

impl Lenient for StatuslineStyle {
    const UNKNOWN: Self = Self::Unknown;
    const DEFAULT: Self = Self::Slab;
    const DEFAULT_NAME: &'static str = "slab";
    const NAME: &'static str = "statusline style";
}

/// The start screen's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DashboardStyle {
    /// The prompt, with sessions and commands listed under it (default)
    #[default]
    Launcher,
    /// The prompt alone; the list waits for `↓`
    Minimal,
    /// Unknown values fall back to the launcher.
    #[serde(other)]
    Unknown,
}

/// `dashboard` in `config.json`: a switch, or a style.
///
/// `"dashboard": false` was shipped and is in people's files, so the key takes either shape — the
/// way `permissions.json` takes a bare action or a keyed table — and no file has to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DashboardSetting {
    /// `true` for the default start screen, `false` for none
    Enabled(bool),
    /// `{ "style": "minimal" }`
    Styled {
        /// Which start screen
        #[serde(default)]
        style: DashboardStyle,
    },
}

/// Bottom statusline visibility.
///
/// The statusline and the full-height Context Dock are two forms of the same workspace context.
/// `Auto` shows the statusline as a compact fallback whenever the dock is not sitting beside the
/// conversation, so wide terminals get the rich sidebar and narrower ones keep a glanceable bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatuslineVisibility {
    /// Show the statusline unless the dock is presented side-by-side.
    Auto,
    /// Always show the statusline.
    Shown,
    /// Never show the statusline.
    Hidden,
    /// Unknown values safely fall back to automatic behavior.
    #[serde(other)]
    Unknown,
}

impl Lenient for StatuslineVisibility {
    const UNKNOWN: Self = Self::Unknown;
    const DEFAULT: Self = Self::Auto;
    const DEFAULT_NAME: &'static str = "auto";
    const NAME: &'static str = "statusline visibility";
}

/// User overrides for statusline composition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StatuslineConfig {
    /// Visibility mode; omitted values follow the automatic dock-aware behavior.
    pub visibility: Option<StatuslineVisibility>,
    /// Left-side modules ordered from the outer edge toward the center. `None` keeps the
    /// default composition with Context-Dock-aware de-duplication; an explicit list is
    /// honored verbatim (so `branch`/`cwd` can stay visible even when the dock shows them).
    pub left: Option<Vec<StatuslineModule>>,
    /// Right-side modules ordered from the center toward the outer edge
    pub right: Option<Vec<StatuslineModule>>,
    /// Color the thinking module by level; `None` keeps the neutral default
    pub thinking_colors: Option<bool>,
    /// How modules are drawn; `None` keeps the slabs
    pub style: Option<StatuslineStyle>,
}

impl StatuslineConfig {
    /// Resolves the configured visibility, safely defaulting unknown values to automatic.
    pub fn visibility(&self) -> StatuslineVisibility {
        known_or(self.visibility)
    }

    /// Resolves the configured style, safely defaulting unknown values to slabs.
    pub fn style(&self) -> StatuslineStyle {
        known_or(self.style)
    }
}

/// How much glyph fidelity the TUI asks the terminal for.
///
/// Icons are decoration: the words beside them carry the meaning, so every tier is fully usable and the
/// difference is polish. The nerd tier needs a patched font, which is why it is not simply assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlyphStyle {
    /// Use the Material icons where they can be drawn, plain shapes otherwise (default).
    #[default]
    Auto,
    /// Material Design icons from a Nerd Font.
    Nerd,
    /// The same icons, given a second cell to be drawn in.
    ///
    /// A Nerd Font ships in two cuts of every face. The `Mono` one rescales the icons to a single
    /// cell; the other draws them at their natural size — roughly two cells of ink behind a
    /// one-cell advance — so on that cut an icon paints straight over the space after it and butts
    /// against the word it labels. Nothing can measure which cut is loaded: both report one cell.
    /// This is the lever for anyone whose font is the wide one and who prefers the bigger icons to
    /// swapping the face.
    NerdWide,
    /// Geometric Shapes and Arrows; no private-use codepoints, so no patched font needed.
    Unicode,
    /// Plain ASCII punctuation, for a minimal or non-UTF-8 terminal.
    Ascii,
    /// Unknown values fall back to `auto`.
    #[serde(other)]
    Unknown,
}

impl Lenient for GlyphStyle {
    const UNKNOWN: Self = Self::Unknown;
    const DEFAULT: Self = Self::Auto;
    const DEFAULT_NAME: &'static str = "auto";
    const NAME: &'static str = "glyph style";
}

/// How much image fidelity the TUI asks the terminal for.
///
/// The tiers mirror [`GlyphStyle`], for the same reason: a terminal that cannot do the beautiful
/// thing should lose shine, not information. `pixels` needs the kitty graphics protocol *with*
/// Unicode placeholders, a narrower set than "speaks kitty graphics" (WezTerm draws the
/// placeholders as tofu), so `auto` picks it only for terminals measured to handle it and `pixels`
/// is the override for one `auto` does not know yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageStyle {
    /// Real pixels where the terminal is known to draw them, a line naming the image otherwise.
    ///
    /// Never `blocks`: a screenshot is mostly text, and two scanlines per cell turn text into a
    /// smear that reads as a broken image, so the imitation is opt-in.
    #[default]
    Auto,
    /// Kitty graphics protocol with Unicode placeholders.
    Pixels,
    /// Half-block cells: coarse, but no protocol and nothing to be incompatible with.
    Blocks,
    /// A text line naming the image and its size.
    Off,
    /// Unknown values fall back to `auto`.
    #[serde(other)]
    Unknown,
}

impl Lenient for ImageStyle {
    const UNKNOWN: Self = Self::Unknown;
    const DEFAULT: Self = Self::Auto;
    const DEFAULT_NAME: &'static str = "auto";
    const NAME: &'static str = "image style";
}

/// How a desktop notification reaches the desktop: an escape sequence the terminal turns into one.
///
/// Through the terminal rather than a desktop library, so it needs no D-Bus, works over ssh, and
/// lands wherever the terminal sends its own notifications. Terminals disagree on the sequence and
/// none can be asked, so `auto` picks from the ones known to show it, and the rest is this lever.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationMethod {
    /// The sequence the terminal is known to show, or none for a terminal that is not known.
    #[default]
    Auto,
    /// `OSC 9`, iTerm2's: Ghostty, iTerm2, kitty, WezTerm and Warp show it.
    Osc9,
    /// `OSC 777`, rxvt-unicode's, with a separate title: foot and some VTE builds show it.
    Osc777,
    /// Unknown values fall back to `auto`.
    #[serde(other)]
    Unknown,
}

/// The moments that send a desktop notification: all of them, none, or a list.
///
/// Read leniently, like every other key: a value that is neither a boolean nor a list is treated as
/// unset and a name luxe does not know is skipped, so one typo never takes the whole config with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Notifications {
    /// Every moment, or none
    All(bool),
    /// Only these
    Only(Vec<NotificationEvent>),
}

impl Notifications {
    /// Whether `event` sends a notification under this setting.
    pub fn includes(&self, event: NotificationEvent) -> bool {
        match self {
            Self::All(enabled) => *enabled,
            Self::Only(events) => events.contains(&event),
        }
    }

    /// Whether any moment sends one at all.
    pub fn any(&self) -> bool {
        match self {
            Self::All(enabled) => *enabled,
            Self::Only(events) => !events.is_empty(),
        }
    }
}

impl<'de> Deserialize<'de> for Notifications {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match serde_json::Value::deserialize(deserializer)? {
            serde_json::Value::Bool(enabled) => Self::All(enabled),
            serde_json::Value::Array(names) => Self::Only(
                names
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .filter_map(NotificationEvent::named)
                    .collect(),
            ),
            _ => Self::All(false),
        })
    }
}

/// A moment worth telling a reader who has looked away.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationEvent {
    /// A turn finished, or failed
    Turn,
    /// A tool call is waiting for your approval
    Permission,
    /// The agent asked you a question
    Question,
    /// A Guided Review finished writing its guide
    Review,
}

impl NotificationEvent {
    /// The event a config names, or `None` for a name this build does not know.
    fn named(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "turn" => Some(Self::Turn),
            "permission" => Some(Self::Permission),
            "question" => Some(Self::Question),
            "review" => Some(Self::Review),
            _ => None,
        }
    }
}

/// The look of the activity bar that runs above the composer while a turn is in flight.
///
/// Two shapes and an off switch. The retired `progress`, `rect` and `braille` shapes drew
/// glyphs that only some fonts keep to one cell, and a setting whose result depends on a font
/// substitution nobody can measure is a coin toss with a name. A config still naming one falls
/// back to `bars`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityBarStyle {
    /// `▌▍▎▏` — block-element segments with a gap inside each cell (default).
    ///
    /// The default because block elements are the one glyph family every terminal font draws
    /// itself, at exact cell fractions.
    #[default]
    Bars,
    /// `●•·` — round marks over a dotted rail; softer and lighter than `bars`.
    Dots,
    /// No indicator; the elapsed time and interrupt hint remain.
    Off,
    /// Unknown values fall back to the default.
    #[serde(other)]
    Unknown,
}

impl Lenient for ActivityBarStyle {
    const UNKNOWN: Self = Self::Unknown;
    const DEFAULT: Self = Self::Bars;
    const DEFAULT_NAME: &'static str = "bars";
    const NAME: &'static str = "activity bar";
}

/// Context Dock visibility preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockMode {
    /// Follow the responsive workspace layout.
    Auto,
    /// Always show the Context Dock.
    Shown,
    /// Keep the Context Dock closed.
    Hidden,
    /// Unknown values safely follow the automatic layout.
    #[serde(other)]
    Unknown,
}

impl Lenient for DockMode {
    const UNKNOWN: Self = Self::Unknown;
    const DEFAULT: Self = Self::Auto;
    const DEFAULT_NAME: &'static str = "auto";
    const NAME: &'static str = "dock mode";
}

/// Which side of the terminal the full-height Context Dock occupies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockPosition {
    /// Anchor the dock to the left edge.
    Left,
    /// Anchor the dock to the right edge.
    Right,
    /// Unknown values safely fall back to the left edge.
    #[serde(other)]
    Unknown,
}

impl Lenient for DockPosition {
    const UNKNOWN: Self = Self::Unknown;
    const DEFAULT: Self = Self::Left;
    const DEFAULT_NAME: &'static str = "left";
    const NAME: &'static str = "dock position";
}

/// Persisted Context Dock preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DockConfig {
    /// Visibility mode; omitted values follow the automatic layout.
    pub mode: Option<DockMode>,
    /// Which side the dock occupies; omitted values anchor it to the left.
    pub position: Option<DockPosition>,
}

impl DockConfig {
    /// Resolves the configured mode, safely defaulting unknown values to automatic behavior.
    pub fn mode(&self) -> DockMode {
        known_or(self.mode)
    }

    /// Resolves the configured side, safely defaulting unknown/omitted values to the left.
    pub fn position(&self) -> DockPosition {
        known_or(self.position)
    }
}
