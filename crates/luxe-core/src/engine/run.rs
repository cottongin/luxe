//! Run — the engine's action loop: one action at a time, routed to what handles it, until close.

use super::*;

impl Engine {
    /// Main event loop.
    pub(super) async fn run(
        mut self,
        mut action_rx: mpsc::Receiver<Action>,
        event_tx: mpsc::Sender<Event>,
    ) -> Carryover {
        self.start_review_desk(&event_tx);
        self.start(&event_tx).await;

        while let Some(action) = self.next_action(&mut action_rx).await {
            // Spend recorded by work that runs detached from the store — a review's guide pass —
            // has no leaf of its own to chain from. The next action does, and the ledger has been
            // live all along, so the transcript catches up here rather than losing the record.
            self.flush_usage_at_current_leaf(&event_tx).await;
            if ends_cache_warming(&action) {
                self.cache_warmer.stop();
            }
            match action {
                // A message with nothing in it is not one. Committed, its only block is scrubbed
                // from every request after it, so each ends on the agent's last reply, which
                // Anthropic refuses for good: one stray Alt+Enter on a blank composer ended a session
                Action::SubmitUserMessage { content, images, .. }
                    if content.trim().is_empty() && images.is_empty() =>
                {
                    debug!("ignoring a submission with nothing in it");
                }
                Action::SubmitUserMessage { content, images, remediation } => {
                    let flow = self
                        .submit_user_message(
                            (content, images, remediation),
                            &event_tx,
                            &mut action_rx,
                        )
                        .await;
                    if matches!(flow, TurnFlow::Stop) {
                        break;
                    }
                }
                Action::DraftGithubReply(request) => {
                    let flow = self.run_draft_turn(request, &event_tx, &mut action_rx).await;
                    if matches!(flow, TurnFlow::Stop) {
                        break;
                    }
                }
                // A steer that lands with no turn in flight has nothing to steer, so it runs as an
                // ordinary turn — the same thing the user would have gotten a moment later.
                Action::SteerTurn { content } => self.steer_into_turn(Some(content)).await,
                // With no turn in flight there is normally nothing to drop — a steer that arrives
                // idle becomes a turn of its own — so this is here for the race where one is
                // admitted just as the turn ends.
                Action::WithdrawSteering => self.withdraw_pending_steers().await,
                Action::Cancel => self.cancel(&event_tx).await,
                // Either reply with no turn in flight (stale or late) is a harmless no-op
                Action::RespondPermission { id, reply } => {
                    self.permission_broker.resolve(&id, reply)
                }
                Action::AnswerQuestion { id, reply } => self.question_broker.resolve(&id, reply),
                Action::SetPermissionAutoApprove(on) => {
                    self.permission_auto_approve.store(on, std::sync::atomic::Ordering::Relaxed);
                }
                Action::SetMode(name) => self.set_mode(&name, &event_tx).await,
                Action::RequestReview { focus } => self.request_review(focus, &event_tx).await,
                Action::ReviewReturned(outcome) => self.review_returned(outcome, &event_tx).await,
                Action::SetModel(model) => self.set_model(model, &event_tx).await,
                Action::SetThinkingLevel(level) => self.set_thinking_level(level, &event_tx).await,
                Action::Close => {
                    debug!("engine closing");
                    break;
                }
                Action::LoadSession(session_id) => {
                    self.load_requested_session(session_id, &event_tx).await
                }
                Action::RefreshSessions => self.refresh_sessions(&event_tx).await,
                Action::RefreshWorkspace => {
                    debug!("workspace refresh requested");
                    let _ = event_tx.send(Event::WorkspaceRefreshRequested).await;
                }
                Action::LinkPullRequest { number, url } => {
                    self.link_pull_request(number, url).await
                }
                Action::CheckWorktreeRemoval => self.check_worktree_removal(&event_tx),
                Action::SetSessionBrief(brief) => *self.pending_brief.lock().await = Some(brief),
                Action::FocusChanged { focused } => {
                    let _ = event_tx.send(Event::TerminalFocusChanged { focused }).await;
                }
                Action::NewSession => self.new_session(&event_tx).await,
                Action::LoadGuidedReview { request_id, target } => {
                    self.handle_load_guided_review(request_id, target, &event_tx).await;
                }
                Action::PrepareThreadAnswers { request_id, review_id } => {
                    self.handle_prepare_thread_answers(request_id, review_id, &event_tx).await;
                }
                Action::AnswerThreads { request_id, review_id, answers } => {
                    self.handle_answer_threads(request_id, review_id, answers, &event_tx).await;
                }
                Action::PlanRestack { request_id } => {
                    self.handle_plan_restack(request_id, &event_tx)
                }
                Action::ListGithub { sections } => self.handle_list_github(sections, &event_tx),
                Action::SearchGithub { request, section, after } => {
                    self.handle_search_github(request, section, after, &event_tx);
                }
                Action::ReadGithubOverview { repository, number, pull_request } => {
                    self.handle_read_github_overview(repository, number, pull_request, &event_tx)
                        .await
                }
                Action::PostGithubComment(post) => self.handle_post_github_comment(post, &event_tx),
                Action::ChangePullRequest { repository, number, write } => {
                    self.handle_change_pull_request(repository, number, write, &event_tx)
                }
                Action::ReadGithubConversation { repository, number, pull_request } => self
                    .handle_read_github_conversation(repository, number, pull_request, &event_tx),
                Action::PrepareStack { request_id, review_id } => {
                    self.handle_prepare_stack(request_id, review_id, &event_tx).await;
                }
                Action::OpenStack { request_id, draft } => {
                    self.handle_open_stack(request_id, draft, &event_tx).await;
                }
                Action::Restack { request_id } => self.handle_restack(request_id, &event_tx),
                Action::CheckPullRequestFix { number, repository } => {
                    self.handle_check_pull_request_fix(number, repository, &event_tx);
                }
                Action::PreparePullRequest { request_id, review_id } => {
                    self.handle_prepare_pull_request(request_id, review_id, &event_tx).await;
                }
                Action::OpenPullRequest { request_id, title, body, draft } => {
                    self.handle_open_pull_request(request_id, title, body, draft, &event_tx).await;
                }
                Action::PreparePullRequestPost { request_id, review_id } => {
                    self.handle_prepare_pull_request_post(request_id, review_id, &event_tx).await;
                }
                Action::PostPullRequestReview { request_id, verdict, session_revision } => {
                    self.handle_post_pull_request_review(
                        request_id,
                        verdict,
                        session_revision,
                        &event_tx,
                    )
                    .await;
                }
                Action::LoadFileDiff { .. }
                | Action::ListDirectory { .. }
                | Action::ReadProjectFile { .. } => self.handle_project_read(action, &event_tx),
                Action::RefreshGuidedReview { request_id, review_id, expected_revision } => {
                    self.handle_refresh_guided_review(
                        request_id,
                        review_id,
                        expected_revision,
                        &event_tx,
                    )
                    .await;
                }
                Action::SubmitReviewFeedback {
                    submission_id,
                    review_id,
                    source,
                    mode,
                    findings,
                } => {
                    self.handle_submit_review_feedback(
                        submission_id,
                        review_id,
                        source,
                        mode,
                        findings,
                        &event_tx,
                    )
                    .await;
                }
                Action::CancelQueuedReviewFeedback { submission_id } => {
                    self.cancel_queued_review_feedback(&submission_id);
                }
                Action::DispatchQueuedReviewFeedback { submission_id } => {
                    self.dispatch_queued_review_feedback(submission_id, &event_tx).await;
                }
                Action::EditReview { .. }
                | Action::RecordReviewProgress { .. }
                | Action::RecordReviewSeen { .. }
                | Action::ShowOpenReview { .. } => self.forward_to_review_desk(action),
                Action::CommitReviewChapters {
                    request_id,
                    review_id,
                    expected_revision,
                    chapter_ids,
                    squash,
                } => {
                    self.handle_commit_review_chapters(
                        request_id,
                        review_id,
                        expected_revision,
                        chapter_ids,
                        squash,
                        &event_tx,
                    )
                    .await;
                }
                Action::RequestContextComposition => self.report_context(&event_tx).await,
                Action::CompactNow { focus } => {
                    self.compact_on_demand(focus, &event_tx, &mut action_rx).await
                }
                Action::ResumeLatest => self.resume_latest(&event_tx).await,
                Action::OpenPicker(kind) => self.open_picker(kind, &event_tx).await,
                // Each readies the session and then runs an ordinary turn, through the same door
                // a submitted message takes. Run inline they never serviced the action channel,
                // so a permission prompt in one waited on an answer nothing read, and an interrupt
                // never arrived either.
                Action::EditResend { content } => {
                    debug!(content = %content, "edit-and-resend");
                    let ready = self.rewind_for_edit().await.map(|()| content);
                    let flow = self
                        .run_prepared_turn(ready, "edit-resend", &event_tx, &mut action_rx)
                        .await;
                    if matches!(flow, TurnFlow::Stop) {
                        break;
                    }
                }
                Action::Retry => {
                    debug!("retrying last turn");
                    let ready = self.prepare_retry().await;
                    let flow =
                        self.run_prepared_turn(ready, "retry", &event_tx, &mut action_rx).await;
                    if matches!(flow, TurnFlow::Stop) {
                        break;
                    }
                }
                Action::Regenerate => {
                    debug!("regenerating last assistant turn");
                    let ready = self.prepare_regenerate().await;
                    let flow = self
                        .run_prepared_turn(ready, "regenerate", &event_tx, &mut action_rx)
                        .await;
                    if matches!(flow, TurnFlow::Stop) {
                        break;
                    }
                }
            }
            // Turns run inside the arms above, so whatever was just handled, none is running now
            self.cache_warmer.settle();
        }
        self.stop(&event_tx).await
    }
}

impl Engine {
    /// Runs a submitted message as a turn. A review's questions are answered, never acted on, so
    /// the turn is read-only for as long as it runs.
    ///
    /// Args: `((content, images, remediation), event_tx, action_rx)`.
    async fn submit_user_message(
        &mut self,
        (content, images, remediation): (
            String,
            Vec<ImageAttachment>,
            Option<RemediationTurnContext>,
        ),
        event_tx: &mpsc::Sender<Event>,
        action_rx: &mut mpsc::Receiver<Action>,
    ) -> TurnFlow {
        let answering = remediation.as_ref().is_some_and(|context| context.answers_only);
        if answering {
            self.read_only = Some(read_only::ReadOnlyTurn::Answering);
        }
        let flow = self.run_agent_turn(content, images, remediation, event_tx, action_rx).await;
        if answering {
            self.read_only = None;
        }
        flow
    }

    async fn cancel(&self, event_tx: &mpsc::Sender<Event>) {
        debug!("cancellation requested");
        self.set_cancelled(true);
        self.permission_broker.cancel_all();
        self.question_broker.cancel_all();
        self.pending_steers.lock().await.clear();
        let state = self.state_snapshot("cancelled", false).await;
        let _ = event_tx.send(Event::StateChanged(state)).await;
    }

    async fn load_requested_session(&mut self, session_id: String, event_tx: &mpsc::Sender<Event>) {
        debug!(session_id = %session_id, "loading session");
        if let Err(error) = self.load_session(session_id, "loaded", event_tx.clone()).await {
            error!(%error, "failed to load session");
            let _ = event_tx.send(Event::Error(format!("load session: {error}"))).await;
        }
    }

    async fn refresh_sessions(&self, event_tx: &mpsc::Sender<Event>) {
        debug!("refreshing sessions");
        let sessions = self.sessions().list_sessions();
        let session_ids: Vec<String> = sessions.iter().map(|s| s.0.clone()).collect();
        debug!(count = session_ids.len(), "sessions listed");
        let _ = event_tx.send(Event::SessionsRefreshed(session_ids)).await;
    }

    async fn report_context(&self, event_tx: &mpsc::Sender<Event>) {
        match self.measure_context().await {
            Ok(composition) => {
                let _ = event_tx.send(Event::ContextComposition(Box::new(composition))).await;
            }
            Err(error) => {
                warn!(%error, "could not measure the context window");
                let _ = event_tx.send(Event::Error(format!("context report: {error}"))).await;
            }
        }
    }

    async fn compact_on_demand(
        &mut self,
        focus: Option<String>,
        event_tx: &mpsc::Sender<Event>,
        action_rx: &mut mpsc::Receiver<Action>,
    ) {
        let compacting = self.compact_on_request(focus, event_tx);
        if let Err(error) = self.serviced(compacting, event_tx, action_rx).await {
            warn!(%error, "could not compact on request");
            let _ = event_tx.send(Event::Error(format!("compact: {error}"))).await;
        }
        self.steers_after_compaction().await;
    }

    async fn open_picker(
        &self,
        picker_kind: crate::dashboard::PickerKind,
        event_tx: &mpsc::Sender<Event>,
    ) {
        debug!(?picker_kind, "opening picker");
        let state = self.state_snapshot(format!("picker: {picker_kind:?}"), false).await;
        let _ = event_tx.send(Event::StateChanged(state)).await;
    }
}

/// Whether `action` changes what the next request will look like, so the cache being kept warm
/// would never be read: another model or thinking level, another session, a summarized or rewound
/// conversation, or the user calling a halt. A mode switch is not one: it changes neither the
/// system prompt nor the tools, and its note is appended to the next message.
fn ends_cache_warming(action: &Action) -> bool {
    matches!(
        action,
        Action::SetModel(_)
            | Action::SetThinkingLevel(_)
            | Action::LoadSession(_)
            | Action::NewSession
            | Action::ResumeLatest
            | Action::CompactNow { .. }
            | Action::EditResend { .. }
            | Action::Retry
            | Action::Regenerate
            | Action::Cancel
            | Action::Close
    )
}

#[cfg(test)]
mod cache_warming_tests {
    use super::*;

    /// Which actions end warming is a decision, so it is written down. One that leaves the next
    /// request unchanged must not stop it, or idle warming would end at the first glance at a diff.
    #[test]
    fn only_an_action_that_changes_the_next_request_ends_warming() {
        for ending in [
            Action::SetModel(crate::provider::ModelInfo::default()),
            Action::SetThinkingLevel(crate::provider::ThinkingLevel::High),
            Action::NewSession,
            Action::ResumeLatest,
            Action::CompactNow { focus: None },
            Action::Retry,
            Action::Cancel,
            Action::Close,
        ] {
            assert!(ends_cache_warming(&ending), "{ending:?} changes the next request");
        }
        for keeping in [
            Action::RefreshSessions,
            Action::RefreshWorkspace,
            Action::FocusChanged { focused: false },
            Action::RequestContextComposition,
            Action::SetPermissionAutoApprove(true),
            Action::SetMode(crate::mode::PLAN.into()),
        ] {
            assert!(!ends_cache_warming(&keeping), "{keeping:?} leaves the next request alone");
        }
    }
}
