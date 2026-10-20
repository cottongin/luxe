//! UI configuration — layered, forward-compatible, never fatal on bad input.
//!
//! Loads `~/.config/luxe/config.json` (global) and `<repo>/.luxe/config.json`
//! (project-local), merges them (project wins), and falls back to defaults on any
//! parse error. Unknown fields are ignored so a newer Luxe's config keeps working
//! with an older build and vice versa.
//!
//! The schema is published via [`Config::schema_json`] for editor autocomplete
//! (JSON Schema, `$schema` keyword). Later phases extend the struct with their own
//! `#[serde(default)]` fields — every new field is opt-in so old configs keep working.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

mod behavior;
mod display;
mod files;
mod integrations;
mod merge;
mod trust;
mod write;
use crate::theme::{ColorValue, ThemeRegistry};
pub use behavior::*;
pub use display::*;
#[cfg(test)]
use files::{ensure_config_file, read_config_file};
use files::{load_global_config, load_project_config};
pub use integrations::*;
#[cfg(test)]
use merge::{merge_color_overrides, merge_disabled};
pub use write::{value_at, ConfigLayers, ConfigWriteError, Layer};

/// The theme a launch lands on when nothing else decides: Luxe's own.
///
/// A default theme is the product's face, so it is the one drawn for it rather than a neutral
/// stand-in. An explicit setting and a matching desktop theme both still win.
pub const DEFAULT_THEME: &str = "luxe";

/// UI preferences, layered: built-in defaults → global config → project config → CLI flags.
///
/// Every field is `#[serde(default)]` so unknown/missing keys don't break loading.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    /// Theme name (resolved by the theme system). `None` = built-in default.
    pub theme: Option<String>,
    /// Per-token color overrides layered on top of the resolved theme. Keys are theme token
    /// names (e.g. `diffAddBg`, `accent`); values are `#rrggbb`, an ANSI index `0-255`, or an
    /// empty string for the terminal default. Unknown/unparseable entries are ignored, so this
    /// is a safe way to tweak individual colors without forking a whole theme.
    pub colors: Option<HashMap<String, String>>,
    /// Show the key hint beside messages waiting for a turn. `None` = not explicitly set (on).
    pub show_key_hints: Option<bool>,
    /// Capture the mouse for wheel scroll + clicks. `None` = not explicitly set (defaults on);
    /// `Some(false)` starts with capture off. Toggle at runtime via `/mouse` / `ToggleMouse`.
    pub mouse: Option<bool>,
    /// The start screen: `false` skips it, `{ "style": "minimal" }` picks the quieter one. `None`
    /// = use default (the launcher).
    pub dashboard: Option<DashboardSetting>,
    /// Animate the dashboard wordmark with a slow shine. `None` = use default (on). Set to
    /// `false` for a still logotype.
    pub banner_animation: Option<bool>,
    /// How much glyph fidelity the TUI asks for. `None` = use default (`auto`).
    pub glyphs: Option<GlyphStyle>,
    /// How inline images are drawn. `None` = use default (`auto`).
    pub images: Option<ImageStyle>,
    /// Send a desktop notification, through the terminal, when something needs the reader while it
    /// is in the background: `true` for every moment, or a list of them. `None` = use default
    /// (disabled).
    pub notifications: Option<Notifications>,
    /// Which sequence a notification is sent as. `None` = use default (`auto`).
    pub notification_method: Option<NotificationMethod>,
    /// Ring the terminal bell when a turn ends or fails, or something waits on the reader. `None` =
    /// use default (silent), because a modern terminal turns a bell into a window-manager attention
    /// request, which pulls focus off whatever the reader walked away to do.
    pub bell: Option<bool>,
    /// Report the agent's state to herdr when Luxe runs in one of its panes. `None` = use default
    /// (on).
    pub herdr: Option<bool>,
    /// Let another front-end on this machine join a running session. `None` = use default (on).
    /// Only this user can connect, and a project file can turn it off but not back on.
    pub joinable: Option<bool>,
    /// Statusline module order and visibility
    pub statusline: StatuslineConfig,
    /// Conversation canvas and tool-detail preferences
    pub conversation: ConversationConfig,
    /// Context Dock visibility preference
    pub dock: DockConfig,
    /// How every surface draws a diff
    pub diff: DiffConfig,
    /// Guided Review preferences
    pub review: ReviewConfig,
    /// Per-tool enablement (deny-list)
    pub tools: ToolsConfig,
    /// Agent-loop behavior
    pub agent: AgentConfig,
    /// What a claimed phase has to clear
    pub gate: GateConfig,
    /// Plan mode's files and model
    pub plan: PlanConfig,
    /// The reviewer `/review` and the loop's gate ask
    pub reviewer: ReviewerConfig,
    /// When a long conversation gets summarized, and what survives it
    pub compaction: CompactionConfig,
    /// Language-server behavior
    pub lsp: LspConfig,
    /// Whether Luxe checks for a newer release
    pub update: UpdateConfig,
    /// How Luxe keeps up with GitHub
    pub github: GithubConfig,
    /// Whether sessions work in worktrees of their own
    pub worktree: WorktreeConfig,
    /// Programs a key hands the terminal to, and gets it back from
    pub launchers: Vec<Launcher>,
    /// Programs whose output the `@` picker lists as a source of its own
    pub mentions: Vec<MentionSource>,
}

impl Config {
    /// Load standard layers, allowing project gate commands only with explicit trust.
    ///
    /// Precedence (highest wins): project `.luxe/config.json` → global
    /// `~/.config/luxe/config.json` → built-in defaults. Any parse error logs a
    /// warning and falls back to the next layer — the app always launches.
    pub fn load_trusted(repo_root: &Path, trust_project: bool) -> Self {
        let global = load_global_config();
        let project = trust::project_config(repo_root, trust_project);

        // A project with no global file beside it still goes through the merge, against the
        // defaults. Taking it whole handed it the global-only keys — on a fresh install, a cloned
        // repository's `lsp.project_servers` decided whether that repository's binaries ran.
        match (global, project) {
            (None, None) => Config::default(),
            (Some(global), None) => global,
            (global, Some(project)) => merge::Layered::layered(global.unwrap_or_default(), project),
        }
    }

    /// Whether Luxe opens on the start screen (default: yes).
    pub fn dashboard_shown(&self) -> bool {
        !matches!(self.dashboard, Some(DashboardSetting::Enabled(false)))
    }

    /// Which start screen (default: the launcher).
    pub fn dashboard_style(&self) -> DashboardStyle {
        match self.dashboard {
            Some(DashboardSetting::Styled { style: DashboardStyle::Minimal }) => {
                DashboardStyle::Minimal
            }
            _ => DashboardStyle::Launcher,
        }
    }

    /// Whether the dashboard wordmark animates (default: yes).
    pub fn banner_animation(&self) -> bool {
        self.banner_animation.unwrap_or(true)
    }

    /// How much glyph fidelity the TUI asks for (default: resolve from what can be drawn).
    pub fn glyphs(&self) -> GlyphStyle {
        self.glyphs.unwrap_or_default()
    }

    /// Which sequence a desktop notification is sent as (default: `auto`).
    pub fn notification_method(&self) -> NotificationMethod {
        match self.notification_method.unwrap_or_default() {
            NotificationMethod::Unknown => NotificationMethod::Auto,
            method => method,
        }
    }

    /// Whether a finished or failed turn, or a prompt waiting on the reader, rings the terminal bell
    /// (default: no).
    pub fn bell(&self) -> bool {
        self.bell.unwrap_or(false)
    }

    /// Whether `event` sends a desktop notification (default: none do).
    pub fn notifies(&self, event: NotificationEvent) -> bool {
        self.notifications.as_ref().is_some_and(|setting| setting.includes(event))
    }

    /// Whether any moment sends a desktop notification (default: no).
    pub fn any_notifications(&self) -> bool {
        self.notifications.as_ref().is_some_and(Notifications::any)
    }

    /// Whether the agent's state is reported to herdr when Luxe runs inside it (default: yes).
    pub fn herdr(&self) -> bool {
        self.herdr.unwrap_or(true)
    }

    /// Whether a running session can be joined from another front-end on this machine (default:
    /// yes). `LUXE_JOINABLE=0` turns it off for one launch.
    pub fn joinable(&self) -> bool {
        let opted_out = std::env::var("LUXE_JOINABLE").is_ok_and(|value| {
            matches!(value.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off")
        });
        !opted_out && self.joinable.unwrap_or(true)
    }

    /// Resolves the active theme name, falling back to the current desktop theme when known.
    ///
    /// An explicit Luxe setting always wins. Without one, the desktop's theme is used when the
    /// registry carries one (Omarchy's, see [`crate::omarchy`]), because matching the desktop beats
    /// showing off, and anything else lands on [`DEFAULT_THEME`].
    pub fn resolved_theme_name(&self, registry: &ThemeRegistry) -> String {
        if let Some(name) = self.theme.as_deref() {
            // The name it is registered under, so a retired `dark` reports as `catppuccin`
            if let Some(name) = registry.canonical(name) {
                return name.to_string();
            }
            tracing::warn!(theme = name, fallback = DEFAULT_THEME, "unknown theme");
            return DEFAULT_THEME.to_string();
        }

        registry.canonical(crate::omarchy::DESKTOP_THEME).unwrap_or(DEFAULT_THEME).to_string()
    }

    /// Resolves the active theme into concrete color tokens, then overlays any per-token
    /// `colors` overrides from config. Unparseable overrides are skipped with a warning so a
    /// typo can never blank the theme.
    pub fn resolved_theme(&self, registry: &ThemeRegistry) -> HashMap<String, ColorValue> {
        let name = self.resolved_theme_name(registry);
        let mut resolved =
            registry.resolve(&name).or_else(|| registry.resolve(DEFAULT_THEME)).unwrap_or_default();
        if let Some(overrides) = self.colors.as_ref() {
            for (token, raw) in overrides {
                match ColorValue::parse(raw) {
                    Ok(value) => {
                        resolved.insert(token.clone(), value);
                    }
                    Err(_) => tracing::warn!(
                        token = token,
                        raw = raw,
                        "ignoring unparseable color override"
                    ),
                }
            }
        }
        resolved
    }

    /// The JSON Schema for the config files: the checked-in `config.schema.json`, which Luxe writes
    /// beside the user's config so an editor can complete and check every key.
    pub fn schema_json() -> &'static str {
        include_str!("config/config.schema.json")
    }
}

/// Serializes the tests that repoint `$XDG_CONFIG_HOME`.
///
/// It is a **process-global**, and `cargo test` runs tests as threads in one process, so two tests
/// setting it race: one writes into the other's temp dir and the failure reads as a config bug. Shared
/// by this module's tests and the keymap's, because two locks would serialize nothing.
#[cfg(test)]
pub(crate) static XDG_CONFIG_HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests;

#[cfg(test)]
mod activity_bar_config_tests {
    use super::*;

    #[test]
    fn the_activity_bar_style_round_trips_and_tolerates_the_unknown() {
        let config: ConversationConfig =
            serde_json::from_str(r#"{"activity_bar":"dots"}"#).expect("parses");
        assert_eq!(config.activity_bar, Some(ActivityBarStyle::Dots));

        // A style from a newer build — or one of the three retired rails — must not fail the
        // whole config file, and must land on a rail every font actually draws.
        for retired in ["progress", "rect", "braille"] {
            let config: ConversationConfig =
                serde_json::from_str(&format!(r#"{{"activity_bar":"{retired}"}}"#))
                    .expect("parses");
            assert_eq!(config.activity_bar, Some(ActivityBarStyle::Unknown), "{retired}");
        }

        let future: ConversationConfig =
            serde_json::from_str(r#"{"activity_bar":"hologram"}"#).expect("parses");
        assert_eq!(future.activity_bar, Some(ActivityBarStyle::Unknown));

        let omitted: ConversationConfig = serde_json::from_str("{}").expect("parses");
        assert_eq!(omitted.activity_bar, None, "omitted stays omitted, so the default applies");
        // Block elements by default: the geometric shapes are missing from enough programming
        // fonts that the substitute's metrics, not the theme, would decide how the bar looks.
        assert_eq!(ActivityBarStyle::default(), ActivityBarStyle::Bars);
    }

    /// Icons are polish, so the key that turns them down has to survive a newer build's spelling too.
    #[test]
    fn the_glyph_style_round_trips_and_tolerates_the_unknown() {
        let config: Config = serde_json::from_str(r#"{"glyphs":"ascii"}"#).expect("parses");
        assert_eq!(config.glyphs, Some(GlyphStyle::Ascii));
        assert_eq!(config.glyphs(), GlyphStyle::Ascii);

        // The published spelling of the wide tier, pinned: it is a config key, so a rename is a
        // breaking change for every file that already carries it.
        let wide: Config = serde_json::from_str(r#"{"glyphs":"nerd_wide"}"#).expect("parses");
        assert_eq!(wide.glyphs, Some(GlyphStyle::NerdWide));

        let future: Config = serde_json::from_str(r#"{"glyphs":"emoji"}"#).expect("parses");
        assert_eq!(future.glyphs, Some(GlyphStyle::Unknown));

        let omitted: Config = serde_json::from_str("{}").expect("parses");
        assert_eq!(omitted.glyphs, None, "omitted stays omitted");
        assert_eq!(omitted.glyphs(), GlyphStyle::Auto, "and auto is the default");

        let schema: serde_json::Value =
            serde_json::from_str(Config::schema_json()).expect("valid schema");
        let styles = &schema["properties"]["glyphs"]["enum"];
        for style in ["auto", "nerd", "unicode", "ascii"] {
            assert!(
                styles.as_array().is_some_and(|values| values.iter().any(|value| value == style)),
                "{style} is missing from the schema: {styles}"
            );
        }
    }

    /// `"dashboard": false` shipped and is in people's files, so the key reads both shapes, and a
    /// style nobody has heard of falls back to the launcher rather than failing the whole file.
    #[test]
    fn the_dashboard_key_takes_a_switch_or_a_style() {
        let read = |text: &str| -> Config { serde_json::from_str(text).expect(text) };
        assert!(read("{}").dashboard_shown());
        assert!(!read(r#"{"dashboard": false}"#).dashboard_shown());
        assert!(read(r#"{"dashboard": true}"#).dashboard_shown());
        let minimal = read(r#"{"dashboard": {"style": "minimal"}}"#);
        assert!(minimal.dashboard_shown());
        assert_eq!(minimal.dashboard_style(), DashboardStyle::Minimal);
        assert_eq!(
            read(r#"{"dashboard": {"style": "carousel"}}"#).dashboard_style(),
            DashboardStyle::Launcher
        );
        assert_eq!(read(r#"{"dashboard": {}}"#).dashboard_style(), DashboardStyle::Launcher);
    }

    /// A retired key is still a key someone has in a file. `banner` chose between the block
    /// logotype and a chamfered one cut from Powerline Extra triangles; the chamfered drawing is
    /// gone, because one mark that is right on every terminal beats two and a question about
    /// which terminal you have. What must not happen is the dashboard refusing to load over it.
    #[test]
    fn a_retired_appearance_key_is_ignored_rather_than_fatal() {
        let config: Config =
            serde_json::from_str(r#"{"banner":"powerline","theme":"nord"}"#).expect("parses");
        assert_eq!(config.theme.as_deref(), Some("nord"), "and the rest of the file still lands");
    }

    /// A source's kind is the one set, or its name folded; it may not take one of Luxe's own tabs',
    /// and it needs a program to run.
    #[test]
    fn a_mention_source_has_a_kind_of_its_own() {
        let source = |name: &str, kind: Option<&str>, command: &str| MentionSource {
            name: name.into(),
            kind: kind.map(str::to_string),
            command: command.into(),
            args: Vec::new(),
            icon: None,
        };
        assert_eq!(source(" Asana tasks! ", None, "a").kind(), "asana-tasks");
        assert_eq!(source("Jira", Some("PROJ"), "a").kind(), "proj");
        assert!(source("Jira", None, "jira-mentions").usable());
        assert!(!source("Files", Some("file"), "x").usable());
        assert!(!source("GitHub", None, "x").usable());
        assert!(!source("Skills", Some("skill"), "x").usable());
        assert!(!source("MCP", None, "x").usable(), "the MCP tab's");
        assert!(source("MCP", None, "x").reserved() && !source("Jira", None, "x").reserved());
        assert!(!source("!!!", None, "x").usable(), "no kind left");
        assert!(!source("Jira", None, "  ").usable(), "nothing to run");
    }

    /// A source's icon is one glyph or it is none: a sentence, a space or a control character in
    /// the config falls back to the default rather than into the tab row.
    #[test]
    fn a_mention_sources_icon_is_one_glyph_or_none() {
        let with = |icon: Option<&str>| MentionSource {
            name: "Jira".into(),
            kind: None,
            command: "jira-mentions".into(),
            args: Vec::new(),
            icon: icon.map(str::to_string),
        };
        assert_eq!(with(Some("\u{f0303}")).icon(), Some("\u{f0303}"));
        assert_eq!(
            with(Some(" \u{2605}\u{fe0f} ")).icon(),
            Some("\u{2605}\u{fe0f}"),
            "and its selector"
        );
        for refused in [None, Some(""), Some("   "), Some("abc"), Some("a b"), Some("\u{1b}[")] {
            assert_eq!(with(refused).icon(), None, "{refused:?}");
        }
    }
}
