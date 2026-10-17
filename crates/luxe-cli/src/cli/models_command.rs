//! `luxe models` — inspect and refresh the model catalog.
//!
//! Exists mostly for one question: *why does Luxe think my model has a 128k window?* `show`
//! answers it directly by printing which layer won each field, so a stale cache, a missing
//! provider mapping, and a forgotten config override are all distinguishable at a glance.

use std::fmt::Write as _;

use luxe_core::provider::ModelInfo;
use luxe_providers::catalog::Layer;

use super::args::ModelsCommand;
use crate::engine_setup::{load_luxe_config, AgentSettings};
use crate::model_catalog;

/// Run a models subcommand.
pub(crate) async fn run_models_command(
    cmd: ModelsCommand,
    settings: AgentSettings,
) -> anyhow::Result<()> {
    let output = match cmd {
        ModelsCommand::List { provider, configured } => list(&settings, provider, configured),
        ModelsCommand::Show { model } => show(&settings, &model),
        ModelsCommand::Refresh { force } => refresh(force).await,
    };
    crate::output::write(&output);
    Ok(())
}

fn list(settings: &AgentSettings, provider: Option<String>, configured_only: bool) -> String {
    let menu = model_catalog::build_menu(settings);
    // No model chosen means no row is marked, which is the honest answer: nothing here is active.
    let active = settings.model.as_ref().map(|model| model.r#ref.to_string()).unwrap_or_default();

    let mut rows: Vec<_> = menu
        .catalog
        .iter()
        .filter(|entry| provider.as_deref().is_none_or(|p| entry.info.r#ref.provider == p))
        .filter(|entry| !configured_only || entry.configured)
        .collect();
    // Sort by the id shown rather than the display name the picker sorts on, so the column reads
    // in order.
    rows.sort_by(|a, b| {
        b.configured.cmp(&a.configured).then_with(|| key_of(&a.info).cmp(&key_of(&b.info)))
    });

    if rows.is_empty() {
        let models = luxe_core::user_dirs::config_path_display("models.json");
        return format!("No models matched. Configure a provider in {models}.\n");
    }

    let width = rows.iter().map(|entry| key_of(&entry.info).len()).max().unwrap_or(0);
    let mut out = String::new();
    for entry in rows {
        let key = key_of(&entry.info);
        let marker = if key == active {
            "*"
        } else if entry.configured {
            "+"
        } else {
            " "
        };
        let row = format!(
            "{marker} {key:width$}  {:>6}  {:>9}  {}",
            human_tokens(entry.info.context_window),
            if entry.info.reasoning { "reasoning" } else { "" },
            price(&entry.info),
        );
        let _ = writeln!(out, "{}", row.trim_end());
    }
    let _ = writeln!(out, "\n* active   + configured in models.json/auth.json");
    out
}

fn show(settings: &AgentSettings, model: &str) -> String {
    let (provider, id) = match model.split_once('/') {
        Some((provider, id)) => (provider.to_string(), id.to_string()),
        None => (settings.provider.clone(), model.to_string()),
    };
    let config = load_luxe_config();
    let (info, from) = model_catalog::resolve_model(&config, &provider, &id);

    let mut out = String::new();
    let mut field = |label: &str, value: String, layer: Layer| {
        let _ = writeln!(out, "  {label:<16}  {value}  [{}]", layer.label());
    };
    if let Some(name) = &info.display_name {
        field("name", name.clone(), from.display_name);
    }
    field("context window", human_tokens(info.context_window), from.context_window);
    let max_tokens =
        if info.max_tokens == 0 { "(omitted)".to_string() } else { human_tokens(info.max_tokens) };
    field("max output", max_tokens, from.max_tokens);
    field("reasoning", info.reasoning.to_string(), from.reasoning);
    let mut levels: Vec<_> = info
        .thinking_level_map
        .iter()
        .filter(|(_, value)| value.is_some())
        .map(|(level, _)| level.as_str())
        .collect();
    levels.sort_unstable();
    let levels = if levels.is_empty() { "(defaults)".to_string() } else { levels.join(", ") };
    field("thinking levels", levels, from.thinking_level_map);
    // Only when configured, and showing the clamped level rather than the written one: a starting
    // level the map marks unsupported is answered here rather than at the first surprising turn.
    if let Some(level) = info.default_thinking {
        field("thinking default", level.as_str().to_string(), from.default_thinking);
    }
    // Which dial the provider expects is the difference between a working turn and a 400 on
    // Anthropic, so name it here rather than making someone read a rejected request body.
    field("thinking control", thinking_control(&info), from.thinking_controls);
    // Only when this model shapes its requests differently from its provider — the case that is
    // otherwise invisible, since both models on one endpoint answer to the same adapter.
    if let Some(model_compat) = &info.compat {
        let provider_compat = luxe_providers::config::resolve_provider_compat(
            &provider,
            config.providers.get(&provider),
        );
        field("compat", compat_overrides(model_compat, &provider_compat), from.compat);
    }
    // Only when configured: a sampling recipe is silent on the wire and off-spec sampling looks
    // like a bad model rather than a bad request, so "did my min_p land?" needs an answer here.
    if !info.sampling_params.is_empty() {
        field("sampling", sampling(&info), from.sampling_params);
    }
    let modalities: Vec<_> =
        info.input.iter().map(|modality| format!("{modality:?}").to_lowercase()).collect();
    field("modalities", modalities.join(", "), from.input);
    field("cost /Mtok", price(&info), from.cost);

    if from.fields().iter().all(|(_, layer)| *layer == Layer::Default) {
        let models = luxe_core::user_dirs::config_path_display("models.json");
        let _ = writeln!(
            out,
            "\nNothing knows this model — it's running on fallback defaults. Pin it in \
             {models}, or run `luxe models refresh` if it shipped recently."
        );
    }
    format!("{provider}/{id}\n{out}")
}

/// The model's sampling recipe as `key=value`, in the order a reader would look for it.
fn sampling(info: &ModelInfo) -> String {
    let mut pairs: Vec<String> =
        info.sampling_params.iter().map(|(key, value)| format!("{key}={value}")).collect();
    pairs.sort();
    pairs.join(", ")
}

/// What this model does differently from its provider, as `key=value`.
///
/// A diff rather than the whole set, because the whole set is the provider's block plus these, and
/// the reader's question is which of them this model changed. An empty diff still prints: a block
/// that was read and changed nothing is a different answer from a block that was never read.
fn compat_overrides(
    model: &luxe_core::provider::CompatFlags,
    provider: &luxe_core::provider::CompatFlags,
) -> String {
    let object = |flags: &luxe_core::provider::CompatFlags| match serde_json::to_value(flags) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    let (model, provider) = (object(model), object(provider));
    let mut changed: Vec<String> = model
        .iter()
        .filter(|(key, value)| provider.get(*key) != Some(value))
        .map(|(key, value)| match value {
            serde_json::Value::String(text) => format!("{key}={text}"),
            other => format!("{key}={other}"),
        })
        .collect();
    changed.sort();
    match changed.is_empty() {
        true => "(same as the provider)".to_string(),
        false => changed.join(", "),
    }
}

/// How the model expects reasoning to be requested, in the provider's own terms.
///
/// The off switch is named too, because a model that cannot stop thinking answers a request for
/// `off` with reasoning anyway, and "why is this still thinking" deserves an answer here rather
/// than on a bill.
fn thinking_control(info: &ModelInfo) -> String {
    let dial = match (info.thinking_controls.effort, info.thinking_controls.budget_tokens) {
        (true, true) => "effort + token budget",
        (true, false) => "effort (adaptive thinking)",
        (false, true) => "token budget (extended thinking)",
        (false, false) => "(undeclared)",
    };
    match info.reasoning && !info.thinking_controls.can_disable {
        true => format!("{dial}, never off"),
        false => dial.to_string(),
    }
}

async fn refresh(force: bool) -> String {
    // Deployments first and unconditionally: they are not the catalog's to keep current, and a
    // resource's list moves whenever somebody deploys something
    let mut report = String::new();
    if luxe_providers::auth::ProviderAuth::new().refresh_deployments().await {
        report.push_str("Recorded what your Azure resources have deployed.\n");
    }
    report.push_str(&refresh_catalog(force).await);
    report
}

async fn refresh_catalog(force: bool) -> String {
    let path = model_catalog::cache_path();
    if !force && !model_catalog::cache_is_stale(&path) {
        return format!("Cache is current ({} models). Use --force to refetch.\n", cached_len());
    }
    match model_catalog::refresh(&path).await {
        Ok(changed) => {
            model_catalog::invalidate_cached_catalog();
            let verb = if changed { "updated" } else { "already current" };
            format!("models.dev {verb} — {} models cached at {}\n", cached_len(), path.display())
        }
        // Not fatal: the embedded snapshot still answers every lookup.
        Err(error) => format!(
            "Refresh failed: {error}\nFalling back to the embedded snapshot ({} models).\n",
            luxe_providers::catalog::snapshot().len()
        ),
    }
}

/// Models in the on-disk cache, or in the embedded snapshot when there is none.
fn cached_len() -> usize {
    let cached = model_catalog::cached_catalog();
    if cached.is_empty() {
        luxe_providers::catalog::snapshot().len()
    } else {
        cached.len()
    }
}

fn key_of(info: &ModelInfo) -> String {
    format!("{}/{}", info.r#ref.provider, info.r#ref.id)
}

/// `1.0M` / `200k` / `4096` — readable at a glance in a column.
fn human_tokens(tokens: u32) -> String {
    match tokens {
        0 => "-".to_string(),
        t if t >= 1_000_000 => format!("{:.1}M", t as f64 / 1_000_000.0),
        t if t >= 10_000 => format!("{}k", t / 1_000),
        t => t.to_string(),
    }
}

fn price(info: &ModelInfo) -> String {
    // On a plan the published rates are what the tokens would have cost elsewhere, so they are
    // marked as an equivalent rather than presented as a bill the user will receive.
    if info.billing == luxe_core::provider::Billing::Subscription {
        return match info.cost.is_known() {
            true => format!("plan (≈${:.2}/${:.2})", info.cost.input, info.cost.output),
            false => "plan".to_string(),
        };
    }
    if !info.cost.is_known() {
        return "-".to_string();
    }
    format!("${:.2}/${:.2}", info.cost.input, info.cost.output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_counts_read_at_a_glance() {
        assert_eq!(human_tokens(1_000_000), "1.0M");
        assert_eq!(human_tokens(200_000), "200k");
        assert_eq!(human_tokens(4_096), "4096");
        assert_eq!(human_tokens(0), "-");
    }

    /// The row answers "what does this model do differently", not "what are its flags".
    ///
    /// Both halves matter: a diff nobody can read is as useless as no row, and a block that was read
    /// and changed nothing has to be distinguishable from one that was never read at all — that is
    /// the confusion the row exists to end.
    #[test]
    fn the_compat_row_names_only_what_the_model_changed() {
        use luxe_core::provider::CompatFlags;
        let provider = CompatFlags {
            thinking_format: Some("openai".into()),
            supports_reasoning_effort: true,
            supports_developer_role: true,
            ..Default::default()
        };
        let model = CompatFlags {
            thinking_format: Some("chat_template".into()),
            supports_developer_role: false,
            ..provider.clone()
        };

        let row = compat_overrides(&model, &provider);
        assert_eq!(row, "supports_developer_role=false, thinking_format=chat_template");

        // Flags it inherited are not "changes" — the provider's block already says them.
        assert!(!row.contains("supports_reasoning_effort"), "{row}");
        assert!(!row.contains("max_tokens_field"), "{row}");

        assert_eq!(compat_overrides(&provider, &provider), "(same as the provider)");
    }

    #[test]
    fn a_subscription_model_prices_as_plan_rather_than_a_bill() {
        let mut info = ModelInfo {
            billing: luxe_core::provider::Billing::Subscription,
            ..ModelInfo::default()
        };
        assert_eq!(price(&info), "plan");
        info.cost.input = 5.0;
        info.cost.output = 30.0;
        assert_eq!(price(&info), "plan (≈$5.00/$30.00)");
    }

    #[test]
    fn price_is_omitted_when_unknown() {
        let mut info = ModelInfo::default();
        assert_eq!(price(&info), "-");
        info.cost.input = 5.0;
        info.cost.output = 25.0;
        assert_eq!(price(&info), "$5.00/$25.00");
    }
}
