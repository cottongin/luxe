//! Engine lifecycle helpers — session load/snapshot, user goal, review gate.

use super::*;

impl Engine {
    pub(crate) async fn latest_user_goal(&self) -> Option<String> {
        let session_id = self.current_session_id().await;
        let leaf = self.current_leaf.lock().await.clone()?;
        let store = self.config.session_store.read().ok()?;
        let goal = crate::session::with_active_path(&**store, &session_id, &leaf, |entries| {
            entries.iter().rev().find_map(|entry| match &entry.kind {
                EntryKind::Message(message) if message.role == Role::User => {
                    let text = message.text();
                    (!text.trim().is_empty()).then_some(text)
                }
                _ => None,
            })
        });
        goal.ok()?
    }

    /// Reads what produced the current changes off the active transcript.
    ///
    /// Bounded to the recent exchange: a review is about the work in front of the reviewer, not
    /// the whole session history.
    pub(crate) async fn turn_provenance(&self) -> Option<crate::guided_review::TurnProvenance> {
        const RECENT_ENTRIES: usize = 120;
        let session_id = self.current_session_id().await;
        let leaf = self.current_leaf.lock().await.clone()?;
        let store = self.config.session_store.read().ok()?;
        let recent = crate::session::with_active_path(&**store, &session_id, &leaf, |entries| {
            let from = entries.len().saturating_sub(RECENT_ENTRIES);
            let messages = entries[from..].iter().filter_map(|entry| match &entry.kind {
                EntryKind::Message(message) => Some(message.clone()),
                _ => None,
            });
            messages.collect::<Vec<_>>()
        });
        let messages = recent.ok()?;
        drop(store);
        let provenance = crate::guided_review::TurnProvenance::from_messages(
            self.config.guided_review_project_root.as_deref(),
            &messages,
        );
        (!provenance.is_empty()).then_some(provenance)
    }

    pub(crate) async fn state_snapshot(
        &self,
        status: impl Into<String>,
        streaming: bool,
    ) -> EngineState {
        let model = self.effective_model().await;
        EngineState {
            model: ModelRef::new(model.r#ref.provider.clone(), model.r#ref.id.clone())
                .with_display(model.display_name.clone()),
            streaming,
            status: status.into(),
            mode: self.current_mode.lock().await.clone(),
            // Report the level the active model would actually use, so the UI shows the real
            // (clamped) thinking level rather than a raw request the model can't honor.
            thinking: model.effective_thinking_level(*self.active_thinking.lock().await),
        }
    }

    /// The model the next turn should use: an in-session override if there is one, else the launch
    /// model.
    pub(crate) async fn effective_model(&self) -> crate::provider::ModelInfo {
        self.active_model_info.lock().await.clone().unwrap_or_else(|| self.config.model.clone())
    }

    /// Drops every steer admitted but not yet delivered.
    ///
    /// The front-end is taking the text back to be edited, so the buffered copy must not arrive at
    /// the next checkpoint under a composer that now shows it as a draft. Nothing is reported: the
    /// UI holds what the user typed — the images and the mention syntax included — and the engine
    /// only ever held the expanded string.
    pub(crate) async fn withdraw_pending_steers(&self) {
        let dropped = self.pending_steers.lock().await.drain(..).count();
        debug!(dropped, "withdrew buffered steering");
    }

    /// Moves the reasoning level to what `model` should run at, having just become active.
    ///
    /// A level you chose for this model earlier in the session wins; otherwise its configured
    /// starting level (`models.json`'s `thinking`) applies, because a level is worth wildly
    /// different amounts of time on different models and carrying one across a switch is how a
    /// local 27B ends up thinking as hard as the frontier model you left. A model that declares
    /// nothing keeps the current level, which is the behavior every model had before.
    ///
    /// Deliberately does *not* persist: an automatic application is not a decision, and writing it
    /// to the remembered level would let it outlive the switch that caused it.
    pub(crate) async fn apply_thinking_for_model(&self, model: &crate::provider::ModelInfo) {
        let chosen = self.chosen_thinking.lock().await.get(&model.r#ref.to_string()).copied();
        let Some(level) = chosen.or(model.default_thinking) else {
            return;
        };
        *self.active_thinking.lock().await = level;
        debug!(model = %model.r#ref, ?level, chosen = chosen.is_some(), "thinking level for model");
    }

    /// The provider adapter that serves `model`'s provider, falling back to the default provider
    /// when the registry has no entry for it (the common single-provider / test case).
    pub(crate) fn provider_for(
        &self,
        model: &crate::provider::ModelInfo,
    ) -> Arc<dyn crate::provider::Provider + Send + Sync> {
        self.config
            .providers
            .get(&model.r#ref.provider)
            .cloned()
            .unwrap_or_else(|| self.config.provider.clone())
    }

    pub(crate) async fn current_session_id(&self) -> crate::session::SessionId {
        self.session_id.lock().await.clone()
    }

    /// The context a hook fires with, naming the session the engine is on now.
    ///
    /// Resolved at every point rather than held: a switch replaces the session under the engine,
    /// and a context kept from launch named the old one to every hook after it.
    pub(crate) async fn hook_context(&self, event_tx: &mpsc::Sender<Event>) -> HookCtx {
        HookCtx {
            session_id: self.current_session_id().await.to_string(),
            event_tx: event_tx.clone(),
            hook_registry: self.config.hook_registry.clone(),
        }
    }

    /// Fires a session lifecycle `point`, `SessionStart` or `SessionEnd`, for the current session.
    pub(crate) async fn session_hook(&self, point: HookPoint, event_tx: &mpsc::Sender<Event>) {
        let hook_cx = self.hook_context(event_tx).await;
        let event = HookEvent::Other(serde_json::Value::Null);
        self.config.hook_registry.invoke(point, &event, &hook_cx).await;
    }

    /// The session store, read-locked.
    ///
    /// A poisoned lock means another thread panicked partway through a write, so the store's
    /// invariants are unknown and the transcript can no longer be trusted — there is nothing safe
    /// to continue with. That makes this one of the few earned `expect`s in the engine, written
    /// down once here rather than at every caller.
    pub(crate) fn sessions(
        &self,
    ) -> std::sync::RwLockReadGuard<'_, Box<dyn SessionStore + Send + Sync>> {
        self.config.session_store.read().expect("session store lock poisoned")
    }

    /// The session store, write-locked. Poisoning is fatal for the reason [`Self::sessions`] gives.
    pub(crate) fn sessions_mut(
        &self,
    ) -> std::sync::RwLockWriteGuard<'_, Box<dyn SessionStore + Send + Sync>> {
        self.config.session_store.write().expect("session store lock poisoned")
    }

    /// Whether the active session already carries a conversation message.
    ///
    /// A fresh session holds only its header entry, so `New session` can reuse it instead of
    /// spawning an orphan empty `.jsonl` alongside it. Returns `false` on any read error — better
    /// to create a new session than to clobber one we couldn't inspect.
    pub(crate) async fn current_session_has_messages(&self) -> bool {
        let id = self.current_session_id().await;
        let Ok(store) = self.config.session_store.read() else {
            return false;
        };
        store.load(&id).is_ok_and(|entries| {
            entries.iter().any(|entry| matches!(entry.kind, EntryKind::Message(_)))
        })
    }

    /// Hold the claim on the session just loaded, letting go of the one before it.
    fn hold_claim(&self, claim: Option<crate::session::SessionClaim>) {
        // Only `load_session` takes it, and never across an await
        let mut held = self.session_claim.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        *held = claim;
    }

    pub(crate) async fn load_session(
        &self,
        session_id: String,
        status_verb: &str,
        event_tx: mpsc::Sender<Event>,
    ) -> Result<(), SessionError> {
        let id = SessionId(session_id.clone());
        let (leaf, messages, used_tokens, claim) = {
            let store = self
                .config
                .session_store
                .read()
                .map_err(|_| SessionError::Other("session store lock poisoned".to_string()))?;
            let entries = store.load(&id)?;
            // Before anything changes, so a session open in another luxe leaves this one as it was
            let claim = store.claim(&id)?;
            let leaf = crate::session::latest_leaf(&entries);
            let active = match leaf.as_ref() {
                Some(leaf) => store.active_path(&id, leaf)?,
                None => Vec::new(),
            };
            // What this transcript already occupies, so the UI shows real fullness immediately
            // rather than 0% until the first live turn reports usage. Prefer the figure the provider
            // measured on the last call — it is on disk, in the usage ledger — and fall back to the
            // estimate over what a request would actually carry, which is not the whole path once a
            // compaction has cut it.
            let used_tokens = crate::compaction::last_observed_context_tokens(&active)
                .unwrap_or_else(|| crate::compaction::context_tokens(&active));
            let messages = active
                .into_iter()
                .filter_map(|entry| match entry.kind {
                    EntryKind::Message(message) => {
                        Some(SessionMessage { entry_id: entry.id, message })
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            (leaf, messages, used_tokens, claim)
        };

        // A session that already carries messages is a resume (CLI `--resume`, `-c`, or the
        // in-session picker) — arm a one-time briefing. A fresh/new session (no messages) clears
        // it, so `/new` never shows a stale resume briefing.
        self.resume_briefing_pending
            .store(!messages.is_empty(), std::sync::atomic::Ordering::Relaxed);

        // A switch ends one logical session and starts the next; reopening the same one is neither
        let switching = self.current_session_id().await != id;
        if switching {
            self.session_hook(HookPoint::SessionEnd, &event_tx).await;
            self.retire_session_ledger(self.current_session_id().await);
        }
        *self.session_id.lock().await = id.clone();
        self.hold_claim(claim);
        *self.current_leaf.lock().await = leaf;
        // Loading a session is how its prompt picks up instruction and skill edits
        self.forget_prompt();
        debug!(count = messages.len(), "session loaded");
        self.adopt_announced_mode(&messages).await;

        // Adopt the tally this session already recorded. Without it a resumed session reports
        // zero spend and every later figure understates what the work actually cost.
        self.restore_session_usage(&id);
        // Occupancy is not just a readout: the compaction check and the loop's context-aware
        // termination both read it, and both treat zero as "plenty of room". A resumed session that
        // was nearly full would sail past its threshold without compacting once.
        self.last_context_tokens.store(
            used_tokens.min(u64::from(u32::MAX)) as u32,
            std::sync::atomic::Ordering::Relaxed,
        );

        // Another session's transcript never carried what this one's drafts sent
        self.sent_digests.lock().await.forget();
        let _ =
            event_tx.send(Event::SessionLoaded { session_id: session_id.clone(), messages }).await;
        let _ = event_tx.send(Event::SessionUsage(self.usage_snapshot())).await;
        // Seed context-window fullness from the loaded transcript (estimated). A live turn's
        // provider usage overwrites this with an exact count.
        let window = self.effective_model().await.context_window;
        if used_tokens > 0 && window > 0 {
            let used = used_tokens.min(window as u64) as u32;
            let _ = event_tx
                .send(Event::ContextUsage { used_tokens: used, context_window: window })
                .await;
        }
        let state =
            self.state_snapshot(format!("{status_verb} session: {session_id}"), false).await;
        let _ = event_tx.send(Event::StateChanged(state)).await;
        if switching {
            self.session_hook(HookPoint::SessionStart, &event_tx).await;
        }
        Ok(())
    }

    /// Run the review gate hook (used by the continuous loop at phase boundaries).
    pub async fn run_review_gate(
        &self,
        event_tx: mpsc::Sender<Event>,
    ) -> Result<String, EngineError> {
        let hook_cx = self.hook_context(&event_tx).await;
        let outcome = self
            .config
            .hook_registry
            .invoke(HookPoint::ReviewGate, &HookEvent::Other(serde_json::Value::Null), &hook_cx)
            .await;

        match outcome {
            HookOutcome::InjectContext(ctx) => Ok(ctx),
            _ => Ok("VERDICT: SKIP\nNo review gate hook registered.".into()),
        }
    }
}

impl Engine {
    /// Starts a new session — or reuses the current one while it is still empty (header only), so
    /// asking for a new session from a fresh start does not leave orphan files.
    pub(super) async fn new_session(&self, event_tx: &mpsc::Sender<Event>) {
        debug!("creating new session");
        let failed = |error: String| Event::Error(format!("create session: {error}"));
        if !self.current_session_has_messages().await {
            let sid = self.current_session_id().await;
            debug!(session_id = %sid, "reusing empty session for new session");
            if let Err(error) = self.load_session(sid.to_string(), "new", event_tx.clone()).await {
                error!(%error, "failed to reload empty session");
                let _ = event_tx.send(failed(error.to_string())).await;
            }
            return;
        }
        let cwd = match std::env::current_dir() {
            Ok(path) => path,
            Err(error) => {
                error!(%error, "failed to resolve current directory for new session");
                let _ = event_tx.send(failed(error.to_string())).await;
                return;
            }
        };
        let created = match self.config.session_store.write() {
            Ok(mut store) => {
                store.create(&cwd.to_string_lossy()).map_err(|error| error.to_string())
            }
            Err(_) => Err("session store lock poisoned".to_string()),
        };
        let new_session_id = match created {
            Ok(session_id) => session_id,
            Err(error) => {
                error!(%error, "failed to create session");
                let _ = event_tx.send(failed(error)).await;
                return;
            }
        };
        debug!(session_id = %new_session_id, "new session created");
        if let Err(error) =
            self.load_session(new_session_id.to_string(), "new", event_tx.clone()).await
        {
            error!(%error, "failed to load newly created session");
            let _ = event_tx.send(failed(error.to_string())).await;
        }
    }

    /// Resumes the most recently modified session — the store's mtime-ordered answer, since
    /// `list_sessions` returns map keys in no particular order.
    pub(super) async fn resume_latest(&self, event_tx: &mpsc::Sender<Event>) {
        debug!("resuming latest session");
        let latest = self.sessions().most_recent_session();
        let Some(latest) = latest else {
            debug!("no sessions to resume");
            let state = self.state_snapshot("no sessions to resume", false).await;
            let _ = event_tx.send(Event::StateChanged(state)).await;
            return;
        };
        debug!(session_id = %latest.0, "resuming latest session");
        if let Err(error) = self.load_session(latest.0, "resumed", event_tx.clone()).await {
            error!(%error, "failed to resume session");
            let _ = event_tx.send(Event::Error(format!("resume session: {error}"))).await;
        }
    }
}
