//! Settings — the curated part of `config.json` a settings surface offers as controls.
//!
//! Not every key. A list of everything the schema knows is a reference, and the file already is
//! one; what a person wants from a settings page is the dozen or so choices they actually make,
//! in an order that makes sense, with the rest one keystroke away in their editor. So this is a
//! hand-kept table of paths, labels and controls, and the words under each come from the schema
//! (`Config::schema_json`) so there is one description to keep right. A test walks the table
//! against the schema: every path exists there, and every choice is one the schema allows.
//!
//! Pure data and pure functions. Reading the files and writing them is
//! [`crate::config::ConfigLayers`] and [`crate::config::Config::set_global`]; the surface
//! that draws all this lives in the TUI.

use std::sync::LazyLock;

use serde_json::Value;

use crate::cache_warming::CacheWarming;
use crate::config::{
    ActivityBarStyle, ConfigLayers, ConversationAlignment, ConversationGrowth, DiffLayout,
    DockMode, DockPosition, GlyphStyle, ImageStyle, Layer, Lenient, StatuslineStyle,
    StatuslineVisibility, StreamReveal, ThinkingDisplay, ToolDetailsPreference, ToolRuns,
};

/// One page of the settings surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Page {
    Appearance,
    Conversation,
    Layout,
    Tools,
    Agent,
}

impl Page {
    /// Every page, in the order the surface lists them.
    pub const ALL: [Self; 5] =
        [Self::Appearance, Self::Conversation, Self::Layout, Self::Tools, Self::Agent];

    /// The page's name in the list down the left.
    pub fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Conversation => "Conversation",
            Self::Layout => "Dock & statusline",
            Self::Tools => "Tools",
            Self::Agent => "Agent",
        }
    }
}

/// How a setting is changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// On or off
    Toggle {
        /// What applies when nobody set it
        default: bool,
    },
    /// One of a fixed list
    Choice {
        /// Every value, in the order a cycle steps through them
        options: &'static [&'static str],
        /// What applies when nobody set it
        default: &'static str,
    },
    /// The theme, which has a picker of its own with a live preview
    Theme,
    /// The start screen: `launcher`, `minimal` or `off`, written as the shapes `dashboard` takes
    StartScreen,
}

/// The start screen's choices, in the order a cycle steps through them.
const START_SCREENS: [&str; 3] = ["launcher", "minimal", "off"];

/// When a changed value is felt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applies {
    /// The moment it is written
    Now,
    /// Once Luxe starts again: read when the engine or the terminal is set up
    NextLaunch,
}

impl Applies {
    /// How the surface says it.
    pub fn note(self) -> &'static str {
        match self {
            Self::Now => "Takes effect now",
            Self::NextLaunch => "Takes effect the next time Luxe starts",
        }
    }
}

/// One setting the surface offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setting {
    /// Where it lives in `config.json`
    pub path: &'static [&'static str],
    /// What the surface calls it
    pub label: &'static str,
    /// Which page it is on
    pub page: Page,
    /// How it is changed
    pub control: Control,
    /// When the change is felt
    pub applies: Applies,
}

const fn toggle(
    path: &'static [&'static str],
    label: &'static str,
    page: Page,
    default: bool,
    applies: Applies,
) -> Setting {
    Setting { path, label, page, control: Control::Toggle { default }, applies }
}

const fn choice(
    path: &'static [&'static str],
    label: &'static str,
    page: Page,
    options: &'static [&'static str],
    default: &'static str,
    applies: Applies,
) -> Setting {
    Setting { path, label, page, control: Control::Choice { options, default }, applies }
}

use Applies::{NextLaunch, Now};
use Page::{Agent, Appearance, Conversation, Layout, Tools};

/// Every setting, page by page, in the order each page lists them.
pub const SETTINGS: &[Setting] = &[
    Setting {
        path: &["theme"],
        label: "Theme",
        page: Appearance,
        control: Control::Theme,
        applies: Now,
    },
    Setting {
        path: &["dashboard"],
        label: "Start screen",
        page: Appearance,
        control: Control::StartScreen,
        applies: NextLaunch,
    },
    choice(
        &["glyphs"],
        "Icons",
        Appearance,
        &["auto", "nerd", "nerd_wide", "unicode", "ascii"],
        GlyphStyle::DEFAULT_NAME,
        Now,
    ),
    choice(
        &["images"],
        "Inline images",
        Appearance,
        &["auto", "pixels", "blocks", "off"],
        ImageStyle::DEFAULT_NAME,
        NextLaunch,
    ),
    toggle(&["banner_animation"], "Animate the wordmark", Appearance, true, Now),
    choice(
        &["conversation", "activity_bar"],
        "Activity bar",
        Appearance,
        &["bars", "dots", "off"],
        ActivityBarStyle::DEFAULT_NAME,
        Now,
    ),
    choice(
        &["statusline", "style"],
        "Statusline style",
        Appearance,
        &["slab", "flat"],
        StatuslineStyle::DEFAULT_NAME,
        Now,
    ),
    toggle(&["statusline", "thinking_colors"], "Color the thinking level", Appearance, false, Now),
    toggle(&["show_key_hints"], "Key hints on waiting messages", Appearance, true, Now),
    choice(
        &["conversation", "alignment"],
        "Alignment",
        Conversation,
        &["full", "left", "center"],
        ConversationAlignment::DEFAULT_NAME,
        Now,
    ),
    choice(
        &["conversation", "growth"],
        "Grows from",
        Conversation,
        &["bottom", "top"],
        ConversationGrowth::DEFAULT_NAME,
        Now,
    ),
    choice(
        &["conversation", "thinking"],
        "Reasoning",
        Conversation,
        &["shown", "collapsed", "hidden"],
        ThinkingDisplay::DEFAULT_NAME,
        Now,
    ),
    choice(
        &["conversation", "tool_details"],
        "Tool details open",
        Conversation,
        &["errors", "all", "collapsed"],
        ToolDetailsPreference::DEFAULT_NAME,
        Now,
    ),
    choice(
        &["conversation", "tool_runs"],
        "Runs of lookups",
        Conversation,
        &["fold", "cards"],
        ToolRuns::DEFAULT_NAME,
        Now,
    ),
    choice(
        &["conversation", "stream_reveal"],
        "Streaming text",
        Conversation,
        &["smooth", "off"],
        StreamReveal::DEFAULT_NAME,
        Now,
    ),
    choice(
        &["diff", "layout"],
        "Diff layout",
        Conversation,
        &["auto", "unified", "split"],
        DiffLayout::DEFAULT_NAME,
        Now,
    ),
    toggle(&["diff", "word_wrap"], "Wrap long diff lines", Conversation, false, Now),
    choice(
        &["dock", "mode"],
        "Context Dock",
        Layout,
        &["auto", "shown", "hidden"],
        DockMode::DEFAULT_NAME,
        Now,
    ),
    choice(
        &["dock", "position"],
        "Dock side",
        Layout,
        &["left", "right"],
        DockPosition::DEFAULT_NAME,
        Now,
    ),
    choice(
        &["statusline", "visibility"],
        "Statusline",
        Layout,
        &["auto", "shown", "hidden"],
        StatuslineVisibility::DEFAULT_NAME,
        Now,
    ),
    toggle(&["mouse"], "Mouse", Layout, true, Now),
    toggle(&["tools", "format_on_save"], "Format on save", Tools, true, NextLaunch),
    toggle(&["tools", "clone_repositories"], "Clone GitHub repositories", Tools, true, NextLaunch),
    choice(
        &["agent", "default_mode"],
        "Sessions start in",
        Agent,
        &["build", "plan"],
        crate::mode::BUILD,
        NextLaunch,
    ),
    toggle(&["agent", "remember_model"], "Remember the last model", Agent, true, NextLaunch),
    choice(
        &["agent", "cache_warming"],
        "Keep the cache warm",
        Agent,
        &["streaming", "idle", "off"],
        CacheWarming::DEFAULT_NAME,
        NextLaunch,
    ),
    toggle(&["compaction", "auto"], "Compact automatically", Agent, true, NextLaunch),
    toggle(&["compaction", "prune"], "Elide stale tool output", Agent, true, NextLaunch),
    toggle(&["worktree", "always"], "A worktree for every session", Agent, false, NextLaunch),
    toggle(&["bell"], "Ring the bell", Agent, false, Now),
    toggle(&["herdr"], "Report to herdr", Agent, true, NextLaunch),
    toggle(&["update", "check"], "Check for updates", Agent, true, NextLaunch),
];

/// The settings on `page`, in order.
pub fn on(page: Page) -> impl Iterator<Item = &'static Setting> {
    SETTINGS.iter().filter(move |setting| setting.page == page)
}

/// The setting at `path`.
pub fn find(path: &[&str]) -> Option<&'static Setting> {
    SETTINGS.iter().find(|setting| setting.path == path)
}

impl Setting {
    /// The dotted key a person would search the file for: `conversation.growth`.
    pub fn key(&self) -> String {
        self.path.join(".")
    }

    /// What the schema says about it.
    pub fn description(&self) -> Option<&'static str> {
        schema_node(self.path)?.get("description")?.as_str()
    }

    /// The value in force and the layer it came from, the built-in default when nobody set it.
    ///
    /// A value that is the wrong type for the control (a hand-edited `"mouse": "yes"`) reads as the
    /// default, which is what the loader does with it too.
    pub fn current(&self, layers: &ConfigLayers) -> (Layer, Value) {
        let (layer, value) = layers.lookup(self.path);
        if self.control == Control::StartScreen {
            let screen = match value {
                Some(Value::Bool(false)) => "off",
                Some(Value::Object(map)) if map.get("style") == Some(&"minimal".into()) => {
                    "minimal"
                }
                _ => "launcher",
            };
            let layer = if value.is_some() { layer } else { Layer::Default };
            return (layer, Value::String(screen.into()));
        }
        let fits = |value: &Value| match self.control {
            Control::Toggle { .. } => value.is_boolean(),
            Control::Choice { options, .. } => {
                value.as_str().is_some_and(|value| options.contains(&value))
            }
            Control::Theme => value.is_string(),
            Control::StartScreen => true,
        };
        match value.filter(|value| fits(value)) {
            Some(value) => (layer, value.clone()),
            None => (Layer::Default, self.default_value()),
        }
    }

    /// The built-in value.
    pub fn default_value(&self) -> Value {
        match self.control {
            Control::Toggle { default } => Value::Bool(default),
            Control::Choice { default, .. } => Value::String(default.to_string()),
            Control::Theme => Value::String(crate::config::DEFAULT_THEME.to_string()),
            Control::StartScreen => Value::String(START_SCREENS[0].to_string()),
        }
    }

    /// What goes in the file for a value [`Self::current`] and [`Self::stepped`] speak.
    ///
    /// The same value for every control but the start screen, whose choices are three shapes of
    /// one key.
    pub fn file_value(&self, value: Value) -> Value {
        match (self.control, value.as_str()) {
            (Control::StartScreen, Some("off")) => Value::Bool(false),
            (Control::StartScreen, Some("minimal")) => serde_json::json!({ "style": "minimal" }),
            (Control::StartScreen, _) => Value::Bool(true),
            _ => value,
        }
    }

    /// The value after one press: a toggle flips, a choice steps `forward` or back and wraps.
    ///
    /// `None` for the theme, which is chosen in its own picker rather than cycled.
    pub fn stepped(&self, current: &Value, forward: bool) -> Option<Value> {
        match self.control {
            Control::Toggle { default } => Some(Value::Bool(!current.as_bool().unwrap_or(default))),
            Control::StartScreen => Self::cycle(&START_SCREENS, START_SCREENS[0], current, forward),
            Control::Choice { options, default } => Self::cycle(options, default, current, forward),
            Control::Theme => None,
        }
    }

    fn cycle(options: &[&str], default: &str, current: &Value, forward: bool) -> Option<Value> {
        {
            {
                let at = current
                    .as_str()
                    .and_then(|value| options.iter().position(|option| *option == value))
                    .or_else(|| options.iter().position(|option| *option == default))
                    .unwrap_or(0);
                let next = match forward {
                    true => (at + 1) % options.len(),
                    false => (at + options.len() - 1) % options.len(),
                };
                Some(Value::String(options[next].to_string()))
            }
        }
    }
}

/// The schema node at `path`, through each object's `properties`.
fn schema_node(path: &[&str]) -> Option<&'static Value> {
    static SCHEMA: LazyLock<Value> = LazyLock::new(|| {
        serde_json::from_str(crate::config::Config::schema_json())
            .expect("the published schema is valid JSON, and a test parses it")
    });
    path.iter().try_fold(&*SCHEMA, |node, key| node.get("properties")?.get(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table and the schema are two descriptions of one file, so they are held together: a
    /// path the schema does not know is a setting that writes nothing the loader reads, and a
    /// choice the schema does not allow is one the loader throws away.
    #[test]
    fn every_setting_is_a_key_the_schema_describes() {
        for setting in SETTINGS {
            let node = schema_node(setting.path)
                .unwrap_or_else(|| panic!("{} is not in the schema", setting.key()));
            assert!(setting.description().is_some(), "{} has no description", setting.key());
            match setting.control {
                Control::Toggle { default } => {
                    assert_eq!(node["type"], "boolean", "{}", setting.key());
                    if let Some(schema_default) = node.get("default") {
                        assert_eq!(schema_default, &Value::Bool(default), "{}", setting.key());
                    }
                }
                Control::Choice { options, default } => {
                    let allowed: Vec<&str> = node["enum"]
                        .as_array()
                        .unwrap_or_else(|| panic!("{} has no enum", setting.key()))
                        .iter()
                        .filter_map(Value::as_str)
                        .collect();
                    let mut offered = options.to_vec();
                    offered.sort_unstable();
                    let mut allowed_sorted = allowed.clone();
                    allowed_sorted.sort_unstable();
                    assert_eq!(
                        offered,
                        allowed_sorted,
                        "{} offers what the schema allows",
                        setting.key()
                    );
                    assert_eq!(node["default"], default, "{}", setting.key());
                }
                Control::Theme => assert_eq!(node["type"], "string"),
                Control::StartScreen => {
                    let styles = &node["oneOf"][1]["properties"]["style"]["enum"];
                    assert_eq!(styles, &serde_json::json!(["launcher", "minimal"]));
                }
            }
        }
    }

    #[test]
    fn every_page_has_settings_and_no_key_is_listed_twice() {
        for page in Page::ALL {
            assert!(on(page).next().is_some(), "{} has nothing on it", page.label());
        }
        let mut keys: Vec<String> = SETTINGS.iter().map(Setting::key).collect();
        keys.sort();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len(), "a setting is listed twice");
    }

    #[test]
    fn a_toggle_flips_and_a_choice_wraps_both_ways() {
        let mouse = find(&["mouse"]).expect("mouse");
        assert_eq!(mouse.stepped(&Value::Bool(true), true), Some(Value::Bool(false)));
        let growth = find(&["conversation", "growth"]).expect("growth");
        assert_eq!(growth.stepped(&"bottom".into(), true), Some("top".into()));
        assert_eq!(growth.stepped(&"top".into(), true), Some("bottom".into()), "wraps forward");
        let glyphs = find(&["glyphs"]).expect("glyphs");
        assert_eq!(glyphs.stepped(&"auto".into(), false), Some("ascii".into()), "wraps back");
        assert_eq!(glyphs.stepped(&"nonsense".into(), true), Some("nerd".into()), "from default");
        assert_eq!(find(&["theme"]).expect("theme").stepped(&"luxe".into(), true), None);
        let start = find(&["dashboard"]).expect("start screen");
        assert_eq!(start.stepped(&"off".into(), true), Some("launcher".into()), "wraps");
    }

    /// The start screen is three shapes of one key: absent or `true`, a style, or `false`.
    #[test]
    fn the_start_screen_reads_and_writes_every_shape_of_its_key() {
        let start = find(&["dashboard"]).expect("start screen");
        let read =
            |global: Value| start.current(&ConfigLayers { global: Some(global), project: None }).1;
        assert_eq!(read(serde_json::json!({})), Value::from("launcher"));
        assert_eq!(read(serde_json::json!({ "dashboard": false })), Value::from("off"));
        assert_eq!(read(serde_json::json!({ "dashboard": true })), Value::from("launcher"));
        assert_eq!(
            read(serde_json::json!({ "dashboard": { "style": "minimal" } })),
            Value::from("minimal")
        );
        assert_eq!(start.file_value("off".into()), Value::Bool(false));
        assert_eq!(start.file_value("minimal".into()), serde_json::json!({ "style": "minimal" }));
    }

    /// The value shown is the one the loader would use: the project's over the reader's, and the
    /// default for a key nobody set or set to something the control cannot hold.
    #[test]
    fn the_current_value_is_the_one_in_force() {
        let layers = ConfigLayers {
            global: Some(serde_json::json!({ "mouse": false, "glyphs": "sparkly" })),
            project: Some(serde_json::json!({ "conversation": { "growth": "top" } })),
        };
        let current = |path: &[&str]| find(path).expect("listed").current(&layers);
        assert_eq!(current(&["mouse"]), (Layer::Global, Value::Bool(false)));
        assert_eq!(current(&["conversation", "growth"]), (Layer::Project, "top".into()));
        assert_eq!(current(&["glyphs"]), (Layer::Default, "auto".into()), "an unknown value");
        assert_eq!(current(&["bell"]), (Layer::Default, Value::Bool(false)));
    }
}
