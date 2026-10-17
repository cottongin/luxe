//! Fixture models — the catalog a seeded app picks from, without a provider behind it.

/// The fixture's active model: reasons, tops out at High — the shape of an extended-thinking Claude.
/// A few models for the picker the Configuration surface hands a finished login to.
pub(super) fn configurable_models() -> Vec<crate::model_picker::ModelEntry> {
    [
        ("openai", "gpt-5.6", "GPT-5.6"),
        ("openai", "gpt-5.6-mini", "GPT-5.6 mini"),
        ("anthropic", "claude-sonnet-5", "Claude Sonnet 5"),
        ("lmstudio", "qwen3-coder-30b", "Qwen3 Coder 30B"),
    ]
    .into_iter()
    .map(|(provider, id, label)| crate::model_picker::ModelEntry {
        info: luxe_core::provider::ModelInfo {
            r#ref: luxe_core::provider::ModelRef::new(provider, id),
            display_name: Some(label.into()),
            context_window: 200_000,
            ..Default::default()
        },
        label: label.into(),
        configured: true,
        unverified: false,
    })
    .collect()
}

pub(super) fn fixture_model() -> luxe_core::provider::ModelInfo {
    luxe_core::provider::ModelInfo {
        r#ref: luxe_core::provider::ModelRef::new("lmstudio", "qwen3.8-27b"),
        display_name: Some("Qwen3.8 27B".into()),
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
