//! Construction — spawning the engine task from its configuration.

use super::*;

impl Engine {
    /// Spawn the engine on a background task.
    ///
    /// Returns `(ActionTx, EventRx)` for communicating with the engine.
    pub fn spawn(config: EngineConfig) -> (mpsc::Sender<Action>, mpsc::Receiver<Event>) {
        let (actions, events, _task) = Self::spawn_task(config);
        (actions, events)
    }

    /// [`Engine::spawn`], keeping the engine's task.
    ///
    /// A front-end about to exit awaits it after sending [`Action::Close`], because the
    /// `SessionEnd` hooks run on the way out of that task, and a process that returns from `main`
    /// first drops it mid-hook. The task ends with the engine's [`Carryover`].
    pub fn spawn_task(
        config: EngineConfig,
    ) -> (mpsc::Sender<Action>, mpsc::Receiver<Event>, tokio::task::JoinHandle<Carryover>) {
        Self::spawn_task_carrying(config, None)
    }

    /// [`Engine::spawn_task`], starting from the mode, model and thinking level an earlier engine
    /// ended with. The state the engine reports when its session loads already shows them.
    ///
    /// For an engine rebuilt to resume a session in another checkout: one that loaded it in place
    /// would have kept them.
    pub fn spawn_task_carrying(
        config: EngineConfig,
        carryover: Option<Carryover>,
    ) -> (mpsc::Sender<Action>, mpsc::Receiver<Event>, tokio::task::JoinHandle<Carryover>) {
        let (action_tx, action_rx) = mpsc::channel(64);
        let (event_tx, event_rx) = mpsc::channel(256);

        let initial_session_id = config.session_id.clone();
        let permission_engine = config.permission.engine.clone();
        let ask_resolution = config.permission.ask;
        // Where the session starts thinking: the launch model's own configured level, if it named
        // one. Both read sites clamp, so the raw request is what's held. A front-end that has an
        // explicit level (`--thinking`, or what the last session chose) sends `SetThinkingLevel`
        // over the top of this.
        let initial_thinking = config.model.default_thinking.unwrap_or_default();
        let cache_warmer = cache_warmer::CacheWarmer::new(config.cache_warming);
        let (review_desk, review_desk_inbox) = review_desk::ReviewDeskHandle::channel();
        let default_mode = config.modes.default_mode().name.clone();
        let mut engine = Self {
            config,
            cancel_tx: watch::Sender::new(false),
            current_mode: tokio::sync::Mutex::new(default_mode.clone()),
            announced_mode: tokio::sync::Mutex::new(default_mode),
            review_running: std::sync::atomic::AtomicBool::new(false),
            active_model_info: tokio::sync::Mutex::new(None),
            active_thinking: tokio::sync::Mutex::new(initial_thinking),
            chosen_thinking: tokio::sync::Mutex::new(HashMap::new()),
            before_mode: tokio::sync::Mutex::new(None),
            current_leaf: tokio::sync::Mutex::new(None),
            session_id: tokio::sync::Mutex::new(initial_session_id),
            session_claim: std::sync::Mutex::new(None),
            last_turn_baseline: tokio::sync::Mutex::new(None),
            active_guided_review: Arc::new(tokio::sync::Mutex::new(ActiveGuidedReview::default())),
            review_desk,
            review_desk_inbox: std::sync::Mutex::new(Some(review_desk_inbox)),
            previews: Arc::default(),
            accepted_feedback_submissions: HashSet::new(),
            pending_feedback: HashMap::new(),
            resume_briefing_pending: AtomicBool::new(false),
            pending_brief: tokio::sync::Mutex::new(None),
            read_only: None,
            turn_source: crate::usage::UsageSource::Agent,
            draft_answer: tokio::sync::Mutex::new(None),
            sent_digests: tokio::sync::Mutex::new(drafting::SentDigests::default()),
            permission: Arc::new(tokio::sync::Mutex::new(permission_engine)),
            ask_resolution,
            permission_broker: Arc::new(crate::permission::PermissionBroker::new()),
            question_broker: Arc::new(crate::question::QuestionBroker::new()),
            permission_auto_approve: Arc::new(AtomicBool::new(false)),
            recent_tool_call: tokio::sync::Mutex::new(None),
            test_failure_state: tokio::sync::Mutex::new((0, false)),
            last_context_tokens: AtomicU32::new(0),
            prompt_snapshot: std::sync::Mutex::new(None),
            last_prefix_tokens: AtomicU32::new(0),
            compaction_failures: AtomicU32::new(0),
            pending_steers: tokio::sync::Mutex::new(Vec::new()),
            deferred_actions: std::sync::Mutex::new(std::collections::VecDeque::new()),
            continuations: std::sync::Mutex::new(std::collections::VecDeque::new()),
            usage_ledger: usage::SessionLedgers::default(),
            cache_warmer,
        };

        engine.config.action_tx = Some(action_tx.clone());
        if let Some(carryover) = carryover {
            engine.adopt(carryover);
        }

        let task = tokio::spawn(async move { engine.run(action_rx, event_tx).await });

        (action_tx, event_rx, task)
    }
}
