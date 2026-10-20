//! Engine settings — the mode, model and thinking level a session runs with.

use super::*;

/// The session's own model and thinking level, as they stood before a mode overrode them.
#[derive(Debug, Clone)]
pub(super) struct ModeBaseline {
    model: Option<crate::provider::ModelInfo>,
    thinking: crate::provider::ThinkingLevel,
}

/// What the reader set an engine to: the mode, the model and thinking level, the level chosen on
/// each model, what a mode overrode, and whether tool asks are approved without asking.
///
/// An engine keeps these across the sessions it loads. One rebuilt to resume a session somewhere
/// else, in another checkout, starts from the last one's ([`Engine::spawn_task_carrying`]), so the
/// trip keeps them too.
#[derive(Debug, Clone, Default)]
pub struct Carryover {
    mode: String,
    model: Option<crate::provider::ModelInfo>,
    thinking: crate::provider::ThinkingLevel,
    chosen_thinking: HashMap<String, crate::provider::ThinkingLevel>,
    before_mode: Option<ModeBaseline>,
    auto_approve: bool,
}

impl Carryover {
    /// Whether tool asks were being approved without asking, which a front-end shows.
    pub fn auto_approves(&self) -> bool {
        self.auto_approve
    }
}

impl Engine {
    /// What this engine was set to, for the one that takes over from it.
    pub(super) async fn carryover(&self) -> Carryover {
        Carryover {
            mode: self.current_mode.lock().await.clone(),
            model: self.active_model_info.lock().await.clone(),
            thinking: *self.active_thinking.lock().await,
            chosen_thinking: self.chosen_thinking.lock().await.clone(),
            before_mode: self.before_mode.lock().await.clone(),
            auto_approve: self.permission_auto_approve.load(std::sync::atomic::Ordering::Relaxed),
        }
    }

    /// Starts from `carryover`, before the engine runs. A mode this project no longer has is
    /// left at the default rather than adopted by name.
    pub(super) fn adopt(&mut self, carryover: Carryover) {
        if self.config.modes.get(&carryover.mode).is_some() {
            *self.current_mode.get_mut() = carryover.mode;
        }
        *self.active_model_info.get_mut() = carryover.model;
        *self.active_thinking.get_mut() = carryover.thinking;
        *self.chosen_thinking.get_mut() = carryover.chosen_thinking;
        *self.before_mode.get_mut() = carryover.before_mode;
        self.permission_auto_approve
            .store(carryover.auto_approve, std::sync::atomic::Ordering::Relaxed);
    }

    /// How `ask` resolves right now; the runtime auto-approve toggle overrides the configured answer.
    pub(super) fn ask_resolution_now(&self) -> AskResolution {
        if self.permission_auto_approve.load(std::sync::atomic::Ordering::Relaxed) {
            return AskResolution::AutoAllow;
        }
        self.ask_resolution
    }

    /// Switches the session's mode, applying its model and thinking level when it has them.
    ///
    /// Nothing about the request changes but what a mode carries: the system prompt and the tools
    /// stay as they were, and the model hears about the switch in a note on the next message. An
    /// unknown name changes nothing and says so.
    pub(super) async fn set_mode(&self, name: &str, event_tx: &mpsc::Sender<Event>) {
        let Some(mode) = self.config.modes.get(name).cloned() else {
            let known = self.config.modes.names().join(", ");
            let _ =
                event_tx.send(Event::Notice(format!("No mode named `{name}`. Try {known}."))).await;
            return;
        };
        debug!(mode = %mode.name, "switching mode");
        *self.current_mode.lock().await = mode.name.clone();
        // The last mode's overrides go first, so a mode without its own model runs on the
        // session's rather than on whatever the previous mode brought
        let mut before_mode = self.before_mode.lock().await;
        if let Some(baseline) = before_mode.take() {
            *self.active_model_info.lock().await = baseline.model;
            *self.active_thinking.lock().await = baseline.thinking;
        }
        if mode.model.is_some() || mode.thinking.is_some() {
            *before_mode = Some(ModeBaseline {
                model: self.active_model_info.lock().await.clone(),
                thinking: *self.active_thinking.lock().await,
            });
        }
        if let Some(model) = &mode.model {
            *self.active_model_info.lock().await = Some(model.clone());
        }
        if let Some(thinking) = mode.thinking {
            // Clamped against the model the mode will actually run on: asking for `high` on a
            // model with no reasoning control is a request it cannot honor
            let reference = self.effective_model().await;
            *self.active_thinking.lock().await = reference.effective_thinking_level(thinking);
        }
        drop(before_mode);
        let state = self.state_snapshot(format!("mode: {}", mode.name), false).await;
        let _ = event_tx.send(Event::StateChanged(state)).await;
    }

    /// Switches mode while a turn runs, which is when "stop writing" matters most.
    ///
    /// The mode itself lands at once: its tool rules are judged per call, so the turn's next call
    /// meets them, and a refusal names the mode, which is how the agent hears of the switch before
    /// its next message carries the full note. A model or thinking level the switch would change,
    /// entering a mode that brings one or leaving one that did, waits for the turn to end:
    /// swapping the model under a turn in flight hands one provider another's reasoning.
    pub(super) async fn set_mode_mid_turn(&self, name: String, event_tx: &mpsc::Sender<Event>) {
        let Some(mode) = self.config.modes.get(&name) else {
            // Says the name is unknown, and changes nothing
            return self.set_mode(&name, event_tx).await;
        };
        debug!(mode = %mode.name, "switching mode mid-turn");
        *self.current_mode.lock().await = mode.name.clone();
        // Reported as streaming: the turn is still running, and an idle report ends its spinner
        let state = self.state_snapshot(format!("mode: {}", mode.name), true).await;
        let _ = event_tx.send(Event::StateChanged(state)).await;
        let moves_model = mode.model.is_some()
            || mode.thinking.is_some()
            || self.before_mode.lock().await.is_some();
        if moves_model {
            // The full switch after the turn puts the model right; re-entering the mode it is
            // already in changes nothing else
            self.defer(Action::SetMode(name));
        }
    }

    /// The mode the session is in now.
    pub(crate) async fn mode(&self) -> crate::mode::Mode {
        let current = self.current_mode.lock().await;
        self.config.modes.get(&current).unwrap_or_else(|| self.config.modes.default_mode()).clone()
    }

    /// The directory paths are judged against: the project, or where the process stands.
    pub(crate) fn project_root(&self) -> std::path::PathBuf {
        self.config.guided_review_project_root.clone().unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
        })
    }

    /// The note the next message carries about the mode, and the mode it leaves announced.
    ///
    /// A switch since the last message gets the mode's full note; a mode the session switched into
    /// gets its one-line reminder on every later message. The default mode gets nothing, which is
    /// every message of a session that never plans, and of a review engine, whose one read-only mode
    /// is not a state it entered and its brief already explains.
    pub(super) async fn take_mode_note(&self) -> Option<String> {
        let current = self.current_mode.lock().await.clone();
        let mut announced = self.announced_mode.lock().await;
        let mode = self.config.modes.get(&current)?;
        if *announced != current {
            *announced = current;
            return Some(mode.entered());
        }
        (mode.name != self.config.modes.default_mode().name).then(|| mode.reminder()).flatten()
    }

    /// Takes the mode a loaded transcript last told the model about as the one announced.
    ///
    /// A resumed session whose last note said `plan` has a model that still believes it is
    /// planning, so a session now in `build` has to say so on the next message. A transcript with
    /// no note was in the default mode the whole time.
    pub(super) async fn adopt_announced_mode(&self, messages: &[crate::event::SessionMessage]) {
        let told = messages
            .iter()
            .rev()
            .filter(|message| message.message.role == Role::User)
            .flat_map(|message| message.message.content.iter().rev())
            .find_map(|block| match block {
                ContentBlock::Text(text) => {
                    crate::mode::note(text).map(|note| note.mode.to_string())
                }
                _ => None,
            })
            .filter(|name| self.config.modes.get(name).is_some());
        *self.announced_mode.lock().await =
            told.unwrap_or_else(|| self.config.modes.default_mode().name.clone());
    }

    /// Switches the active model, remembered for the next launch: re-picking a model you already
    /// chose is a tax on the person who already decided.
    pub(super) async fn set_model(
        &self,
        model: crate::provider::ModelInfo,
        event_tx: &mpsc::Sender<Event>,
    ) {
        debug!(model = %model.r#ref, "switching active model");
        if let Some(project) = &self.config.remember_choices_in {
            crate::runtime_state::RuntimeState::remember_model(
                project,
                &model.r#ref.provider,
                &model.r#ref.id,
            );
        }
        *self.before_mode.lock().await = None;
        *self.active_model_info.lock().await = Some(model.clone());
        self.apply_thinking_for_model(&model).await;
        let state = self.state_snapshot(format!("model: {}", model.r#ref), false).await;
        let _ = event_tx.send(Event::StateChanged(state)).await;
    }

    /// Sets the thinking level. The *requested* level is what is remembered, not what a model
    /// clamps it to, and a deliberate choice outranks the active model's configured starting level
    /// for as long as the session lasts, including across a switch away and back.
    pub(super) async fn set_thinking_level(
        &self,
        level: crate::provider::ThinkingLevel,
        event_tx: &mpsc::Sender<Event>,
    ) {
        debug!(?level, "setting thinking level");
        if self.config.remember_choices_in.is_some() {
            crate::runtime_state::RuntimeState::remember_thinking(level);
        }
        *self.before_mode.lock().await = None;
        *self.active_thinking.lock().await = level;
        let key = self.effective_model().await.r#ref.to_string();
        self.chosen_thinking.lock().await.insert(key, level);
        let state = self.state_snapshot(format!("thinking: {}", level.as_str()), false).await;
        let _ = event_tx.send(Event::StateChanged(state)).await;
    }
}
