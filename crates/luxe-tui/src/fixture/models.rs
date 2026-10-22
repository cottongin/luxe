//! Fixture models — the catalog a seeded app picks from, without a provider behind it.

/// A few models for the picker the Configuration surface hands a finished login to, labeled by
/// id as the catalog labels anything config does not name.
pub(super) fn configurable_models() -> Vec<crate::model_picker::ModelEntry> {
    [
        ("openai", "gpt-5.6"),
        ("openai", "gpt-5.6-mini"),
        ("anthropic", "claude-sonnet-5"),
        ("lmstudio", "qwen3-coder-30b"),
    ]
    .into_iter()
    .map(|(provider, id)| crate::model_picker::ModelEntry {
        info: luxe_core::provider::ModelInfo {
            r#ref: luxe_core::provider::ModelRef::new(provider, id),
            context_window: 200_000,
            ..Default::default()
        },
        label: id.into(),
        configured: true,
        unverified: false,
    })
    .collect()
}

/// The fixture's active model: reasons, tops out at High, the shape of an extended-thinking Claude.
pub(super) fn fixture_model() -> luxe_core::provider::ModelInfo {
    luxe_core::provider::ModelInfo {
        r#ref: luxe_core::provider::ModelRef::new("lmstudio", "qwen3.8-27b"),
        reasoning: true,
        context_window: 262_144,
        thinking_level_map: std::collections::HashMap::from([
            ("minimal".into(), Some("minimal".into())),
            ("low".into(), Some("low".into())),
            ("medium".into(), Some("medium".into())),
            ("high".into(), Some("high".into())),
            ("xhigh".into(), None),
            ("ultra".into(), None),
        ]),
        thinking_controls: luxe_core::provider::ThinkingControls {
            effort: true,
            ..Default::default()
        },
        ..Default::default()
    }
}
