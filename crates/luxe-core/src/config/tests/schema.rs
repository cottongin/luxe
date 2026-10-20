//! The schema — what an editor checks a config against, held to the code it describes.

use super::*;

#[test]
fn schema_is_valid_json() {
    let schema: serde_json::Value = serde_json::from_str(Config::schema_json()).unwrap();
    assert_eq!(schema["title"], "luxe config");
    assert!(schema.get("properties").is_some());
    assert!(schema["properties"].get("$schema").is_some());
    assert!(schema["properties"].get("notifications").is_some());
    assert_eq!(schema["properties"]["bell"]["default"], serde_json::json!(false));
    assert_eq!(schema["properties"]["dock"]["properties"]["mode"]["default"], "auto");
    assert_eq!(
        schema["properties"]["statusline"]["properties"]["right"]["items"]["enum"],
        serde_json::json!(["context", "cost", "tokens", "thinking", "model"])
    );
    assert_eq!(
        schema["properties"]["statusline"]["properties"]["left"]["items"]["enum"],
        serde_json::json!(["project", "branch", "pr", "todo", "changes", "cwd", "activity"])
    );
    assert_eq!(
        schema["properties"]["statusline"]["properties"]["style"]["enum"],
        serde_json::json!(["slab", "flat"])
    );
    assert_eq!(schema["properties"]["statusline"]["properties"]["style"]["default"], "slab");
    assert_eq!(schema["properties"]["dock"]["properties"]["position"]["default"], "left");
    assert_eq!(schema["properties"]["statusline"]["properties"]["visibility"]["default"], "auto");
    assert_eq!(
        schema["properties"]["statusline"]["properties"]["thinking_colors"]["default"],
        false
    );
}

/// The schema is what an editor validates against, so a setting missing from it is a setting
/// that silently stops being checked — and nothing about adding a field to the struct makes
/// anyone remember to describe it.
///
/// Nested, because the top-level-only version of this test passed for months while
/// `conversation.image_max_rows` and `conversation.tool_runs` were absent from the schema.
/// Every block sets `additionalProperties: false`, so an editor was reporting two documented
/// settings as errors — the exact failure the test exists to prevent, one level down.
///
/// Both directions: a key in the schema that no longer exists is a setting the docs still
/// offer and the code ignores, which is worse than an undocumented one.
#[test]
fn every_setting_is_described_by_the_schema_at_every_level() {
    let schema: serde_json::Value = serde_json::from_str(Config::schema_json()).unwrap();
    let written = serde_json::to_value(Config::default()).expect("serializes");
    let mut problems = Vec::new();
    compare_against_schema(&written, &schema, "", &mut problems);
    assert!(problems.is_empty(), "schema and settings disagree: {problems:#?}");
}

/// Walks a serialized config beside the schema describing it, collecting disagreements.
///
/// Only objects with a declared `properties` map are descended into: a free-form map like
/// `colors` describes its values with `additionalProperties` and has no key list to check.
fn compare_against_schema(
    written: &serde_json::Value,
    schema: &serde_json::Value,
    path: &str,
    problems: &mut Vec<String>,
) {
    let (Some(written), Some(described)) = (written.as_object(), schema["properties"].as_object())
    else {
        return;
    };
    let at = |key: &str| if path.is_empty() { key.to_string() } else { format!("{path}.{key}") };
    for key in written.keys() {
        match described.get(key) {
            Some(child) => compare_against_schema(&written[key], child, &at(key), problems),
            None => problems.push(format!("{} is not in the schema", at(key))),
        }
    }
    for key in described.keys().filter(|key| *key != "$schema") {
        if !written.contains_key(key) {
            problems.push(format!("{} is in the schema but not in the struct", at(key)));
        }
    }
}

/// One lenient setting held to its schema list: every value a person can write is offered, and
/// every value offered is one this build understands.
///
/// A variant added to the enum but not to the list stops being offered and starts being reported
/// as invalid by every editor validating against it, and one added to the list but not the enum
/// reads back as the unknown fallback — a silent failure in the one place a reader goes looking for
/// the answer. Nothing about adding a variant makes anyone remember the other half, so this does.
///
/// Args: `(pointer to the schema's enum list, every variant but the unknown fallback, the unknown
/// fallback)`
fn offers_exactly<T>(pointer: &str, variants: &[T], unknown: T)
where
    T: PartialEq + serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug,
{
    let schema: serde_json::Value = serde_json::from_str(Config::schema_json()).unwrap();
    let offered = schema
        .pointer(pointer)
        .and_then(|list| list.get("enum"))
        .and_then(|list| list.as_array())
        .unwrap_or_else(|| panic!("no enum list at {pointer}"));
    for variant in variants {
        let written = serde_json::to_value(variant).expect("serializes");
        assert!(offered.contains(&written), "{variant:?} is not offered at {pointer}: {offered:?}");
    }
    for value in offered {
        let read: T =
            serde_json::from_value(value.clone()).expect("a lenient setting always reads");
        assert!(read != unknown, "{value} at {pointer} reads back as unknown");
        assert!(variants.contains(&read), "{value} at {pointer} is offered but not listed here");
    }
}

#[test]
fn the_schema_offers_every_value_each_config_enum_reads() {
    use crate::cache_warming::CacheWarming;
    use crate::config::behavior::OnAsk;
    use crate::config::display::*;
    offers_exactly(
        "/properties/glyphs",
        &[
            GlyphStyle::Auto,
            GlyphStyle::Nerd,
            GlyphStyle::NerdWide,
            GlyphStyle::Unicode,
            GlyphStyle::Ascii,
        ],
        GlyphStyle::Unknown,
    );
    offers_exactly(
        "/properties/images",
        &[ImageStyle::Auto, ImageStyle::Pixels, ImageStyle::Blocks, ImageStyle::Off],
        ImageStyle::Unknown,
    );
    offers_exactly(
        "/properties/diff/properties/layout",
        &[DiffLayout::Auto, DiffLayout::Unified, DiffLayout::Split],
        DiffLayout::Unknown,
    );
    offers_exactly(
        "/properties/dock/properties/mode",
        &[DockMode::Auto, DockMode::Shown, DockMode::Hidden],
        DockMode::Unknown,
    );
    offers_exactly(
        "/properties/dock/properties/position",
        &[DockPosition::Left, DockPosition::Right],
        DockPosition::Unknown,
    );
    offers_exactly(
        "/properties/statusline/properties/visibility",
        &[StatuslineVisibility::Auto, StatuslineVisibility::Shown, StatuslineVisibility::Hidden],
        StatuslineVisibility::Unknown,
    );
    offers_exactly(
        "/properties/statusline/properties/style",
        &[StatuslineStyle::Slab, StatuslineStyle::Flat],
        StatuslineStyle::Unknown,
    );
    offers_exactly(
        "/properties/conversation/properties/activity_bar",
        &[ActivityBarStyle::Bars, ActivityBarStyle::Dots, ActivityBarStyle::Off],
        ActivityBarStyle::Unknown,
    );
    offers_exactly(
        "/properties/conversation/properties/alignment",
        &[ConversationAlignment::Left, ConversationAlignment::Center, ConversationAlignment::Full],
        ConversationAlignment::Unknown,
    );
    offers_exactly(
        "/properties/conversation/properties/growth",
        &[ConversationGrowth::Top, ConversationGrowth::Bottom],
        ConversationGrowth::Unknown,
    );
    offers_exactly(
        "/properties/conversation/properties/stream_reveal",
        &[StreamReveal::Smooth, StreamReveal::Off],
        StreamReveal::Unknown,
    );
    offers_exactly(
        "/properties/conversation/properties/thinking",
        &[ThinkingDisplay::Shown, ThinkingDisplay::Collapsed, ThinkingDisplay::Hidden],
        ThinkingDisplay::Unknown,
    );
    offers_exactly(
        "/properties/conversation/properties/tool_details",
        &[
            ToolDetailsPreference::Errors,
            ToolDetailsPreference::All,
            ToolDetailsPreference::Collapsed,
        ],
        ToolDetailsPreference::Unknown,
    );
    offers_exactly(
        "/properties/dashboard/oneOf/1/properties/style",
        &[DashboardStyle::Launcher, DashboardStyle::Minimal],
        DashboardStyle::Unknown,
    );
    offers_exactly(
        "/properties/notification_method",
        &[NotificationMethod::Auto, NotificationMethod::Osc9, NotificationMethod::Osc777],
        NotificationMethod::Unknown,
    );
    offers_exactly(
        "/properties/agent/properties/on_ask",
        &[OnAsk::Allow, OnAsk::Deny],
        OnAsk::Unknown,
    );
    offers_exactly(
        "/properties/agent/properties/cache_warming",
        &[CacheWarming::Off, CacheWarming::Streaming, CacheWarming::Idle],
        CacheWarming::Unknown,
    );
    offers_exactly(
        "/properties/conversation/properties/tool_runs",
        &[ToolRuns::Fold, ToolRuns::Cards],
        ToolRuns::Unknown,
    );
}

/// The schema's account of when `auto` pairs a diff's sides quotes the width the code uses.
///
/// The break this catches: the description said 56 cells while [`MIN_SPLIT_COLUMN`] was 44, and
/// nothing compared them.
#[test]
fn the_schema_states_the_split_width_the_code_uses() {
    let schema: serde_json::Value = serde_json::from_str(Config::schema_json()).unwrap();
    let layout = &schema["properties"]["diff"]["properties"]["layout"]["description"];
    let claim = format!("give each column {MIN_SPLIT_COLUMN} cells of code");
    assert!(layout.as_str().unwrap().contains(&claim), "{layout}");
}
