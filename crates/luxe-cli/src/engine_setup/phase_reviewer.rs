//! The reviewer, assembled from configuration — for the loop's phase gate and for `/review`.
//!
//! Two stages gate a claimed phase, both opt-in: the acceptance commands from `gate.commands`, and a
//! reviewer when `gate.review` says so (on by default once `reviewer.model` is named). With neither
//! configured a claim is taken at its word, which is what a loop that has not been told how to check
//! itself deserves. `/review` asks the same reviewer on demand, in any session.
//!
//! Either way the reviewer is an engine of its own, in a fresh session, so it judges the change
//! without the reasoning that produced it. It runs on `reviewer.model` when one is named and on the
//! session's model otherwise: the fresh context is most of what a review is worth, and a second
//! vendor's blind spots are a bonus for whoever has one.

use std::sync::Arc;

use luxe_core::phase_gate::PhaseReviewContext;
use luxe_core::phase_review::{PhaseReviewOutcome, PhaseReviewRequest, PhaseReviewer};

use super::AgentSettings;

#[cfg(test)]
mod tests;

pub(crate) fn phase_gate_config(settings: &AgentSettings) -> luxe_core::config::Config {
    luxe_core::config::Config::load_trusted(&settings.state_root, settings.trust_project)
}

/// The gate a claimed phase has to clear, assembled from config.
pub(crate) async fn build_phase_gate(
    settings: &AgentSettings,
    controller: Arc<luxe_core::loop_controller::LoopController>,
) -> Arc<luxe_core::phase_gate::PhaseGate> {
    let config = phase_gate_config(settings);
    let mut gate = luxe_core::phase_gate::PhaseGate::new();

    let commands = config.gate.commands();
    if commands.is_empty() {
        tracing::info!("no acceptance commands configured for the phase gate");
    } else {
        tracing::info!(count = commands.len(), "acceptance commands configured");
        gate = gate.with_commands(Arc::new(
            luxe_core::hooks::review_gate::ReviewGateHook::with_commands(commands)
                .with_timeout(config.gate.timeout()),
        ));
    }

    if config.gate.reviews(&config.reviewer) {
        if let Some((reviewer, context)) = build_reviewer(settings, &config, controller).await {
            gate = gate.with_reviewer(reviewer, context);
        }
    } else {
        tracing::info!("no reviewer for the phase gate (gate.review)");
    }
    Arc::new(gate)
}

/// The reviewer `/review` asks, started the first time it is asked.
pub(crate) fn on_demand_review(
    settings: &AgentSettings,
    config: &luxe_core::config::Config,
) -> luxe_core::engine::OnDemandReview {
    let review_settings = reviewer_settings(settings, config);
    luxe_core::engine::OnDemandReview {
        label: review_settings.launch_model().r#ref.to_string(),
        reviewer: Arc::new(LazyReviewer {
            settings: review_settings,
            agent: tokio::sync::OnceCell::new(),
        }),
        context: Arc::new(
            crate::phase_review_context::WorktreeReviewContext::new(settings.work_root.clone())
                .whole_plan(),
        ),
    }
}

/// The settings a review engine is built from: the session's, on the reviewer's model.
///
/// The reviewer's model brings its provider's own credential, base URL and compat flags with it
/// (`for_model`); without one named, the review runs on the session's model, provider and all.
fn reviewer_settings(
    settings: &AgentSettings,
    config: &luxe_core::config::Config,
) -> AgentSettings {
    let reviewer = &config.reviewer;
    let mut review_settings = match (reviewer.model.as_deref(), reviewer.model_ref()) {
        (Some(_), Some((provider, id))) => {
            let catalog = super::load_luxe_config();
            settings.for_model(crate::model_catalog::resolve_model(&catalog, provider, id).0)
        }
        (Some(raw), None) => {
            tracing::warn!(model = %raw, "ignoring a reviewer.model that is not `provider/id`");
            settings.clone()
        }
        (None, _) => settings.clone(),
    };
    if reviewer.thinking.is_some() {
        review_settings.initial_thinking = reviewer.thinking;
    }
    // A review is not a loop iteration and not a session anyone resumes: no `loop_control`, a
    // fresh session each time, and the reviewer's own read-only mode and tools
    review_settings.loop_driven = false;
    review_settings.resume = super::ResumeMode::New;
    review_settings.review_engine = true;
    review_settings.initial_mode = None;
    review_settings.carryover = None;
    // The issue's brief and the startup notices are the session's, not the reviewer's
    review_settings.issue = None;
    review_settings.pull_request = None;
    review_settings.startup_notices.clear();
    review_settings
}

/// The reviewer half of the phase gate.
async fn build_reviewer(
    settings: &AgentSettings,
    config: &luxe_core::config::Config,
    controller: Arc<luxe_core::loop_controller::LoopController>,
) -> Option<(Arc<dyn PhaseReviewer>, Arc<dyn PhaseReviewContext>)> {
    let review_settings = reviewer_settings(settings, config);
    let model = review_settings.launch_model().r#ref.to_string();
    if review_settings.launch_model().r#ref == settings.launch_model().r#ref {
        tracing::info!(%model, "the reviewer runs on the loop's own model, in a fresh context");
    }
    match crate::review_agent::ReviewAgent::spawn(review_settings).await {
        Ok(agent) => {
            tracing::info!(%model, "the phase gate has a reviewer");
            Some((
                Arc::new(agent),
                Arc::new(
                    crate::phase_review_context::WorktreeReviewContext::new(
                        settings.work_root.clone(),
                    )
                    .anchored_to(controller),
                ),
            ))
        }
        Err(error) => {
            // A gate with no reviewer is the documented, safe state: the acceptance commands still
            // run and a claim is judged on them.
            tracing::warn!(%error, "could not start the reviewer; the gate is its commands only");
            None
        }
    }
}

/// A reviewer whose engine is started the first time a review is asked for.
///
/// Most sessions never ask, and a review engine is a session file and a provider adapter that
/// would otherwise be made for every one of them.
struct LazyReviewer {
    settings: AgentSettings,
    agent: tokio::sync::OnceCell<Result<crate::review_agent::ReviewAgent, String>>,
}

impl std::fmt::Debug for LazyReviewer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("LazyReviewer").finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
impl PhaseReviewer for LazyReviewer {
    async fn review(&self, request: PhaseReviewRequest) -> PhaseReviewOutcome {
        let started = self
            .agent
            .get_or_init(|| async {
                crate::review_agent::ReviewAgent::spawn(self.settings.clone())
                    .await
                    .map_err(|error| format!("the reviewer could not start: {error}"))
            })
            .await;
        match started {
            Ok(agent) => agent.review(request).await,
            Err(reason) => PhaseReviewOutcome::Unavailable(reason.clone()),
        }
    }
}
