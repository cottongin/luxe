//! Engine-event reduction — core events into TUI state.
//!
//! Kept separate from input and command dispatch so workspace state evolves through one
//! inspectable reducer. [`reduce`] is the dispatch table; an arm with real work behind it hands
//! off to the module that owns its domain.

use super::{App, ConvoBlock};
use luxe_core::config::NotificationEvent;
use luxe_core::event::Event;

mod compaction;
mod github;
mod review;
mod transcript;
mod turn;

pub(super) use transcript::flush_startup_notices;
#[cfg(test)]
pub(super) use turn::INTERRUPT_CONFIRM;
pub(super) use turn::{expire_unconfirmed_interrupt, has_unfinished_tools};

pub(super) fn reduce(app: &mut App, event: Event) {
    // The Changes pane correlates its own capture by request id, so it claims these before the
    // main table sees them and a superseded answer never reaches the surface.
    if matches!(event, Event::DirectoryListed { .. } | Event::DirectoryListFailed { .. }) {
        super::messages::files::apply(app, &event);
        return;
    }
    if matches!(
        event,
        Event::FileDiffLoaded { .. }
            | Event::FileDiffFailed { .. }
            | Event::ProjectFileRead { .. }
            | Event::ProjectFileReadFailed { .. }
    ) {
        if !super::messages::changes::apply_capture(app, &event) {
            tracing::debug!("ignoring a superseded file diff");
        }
        return;
    }
    // A retry is over the moment the provider says anything, or the turn does
    if matches!(
        event,
        Event::ThinkingDelta { .. }
            | Event::TokenDelta { .. }
            | Event::ToolCallComposing { .. }
            | Event::ToolStarted { .. }
            | Event::MessageCommitted { .. }
            | Event::TurnEnded { .. }
    ) {
        app.status.retrying = None;
    }
    match event {
        Event::ProviderRetrying { attempt, attempts, .. } => {
            app.status.retrying = Some((attempt, attempts));
        }
        Event::MessageCommitted { session_id, entry_id, message } => {
            transcript::message_committed(app, session_id, entry_id.0, message);
        }
        Event::SessionLoaded { session_id, messages } => {
            transcript::session_loaded(app, session_id, messages);
        }
        Event::ThinkingDelta { block, delta } => turn::thinking_delta(app, block, delta),
        Event::TokenDelta { delta, .. } => turn::token_delta(app, delta),
        Event::LastTurnAvailable { session_id, turn_id } => {
            app.review.last_turn = Some((session_id, turn_id));
        }
        Event::TurnEnded { .. } => turn::turn_ended(app),
        Event::PlanWritten { path } => app.plan_written = Some(path),
        Event::ReviewStarted { model } => app.push_convo(ConvoBlock::System(format!(
            "A reviewer on {model} is reading the uncommitted changes in a fresh context. Its \
             findings will show here."
        ))),
        Event::ReviewFinished { verdict, findings } => {
            app.push_convo(ConvoBlock::System(turn::review_summary(verdict, &findings)));
        }
        // The running turn took it in: retire the tray row. The transcript copy arrives (or has
        // already arrived) as the committed user entry the engine appended for it.
        Event::SteeringDelivered { .. } => app.retire_steering(),
        // A notice is informational and the turn keeps running, so it lands in the transcript
        // without touching streaming/turn state.
        // On the dashboard there is no conversation to put it in — and whichever row the reader
        // picks loads a session, which replaces the transcript. So it waits.
        Event::Notice(msg) => match app.dashboard.is_some() {
            true => app.startup_notices.push(msg),
            false => app.push_convo(ConvoBlock::System(msg)),
        },
        Event::Error(msg) => turn::failed(app, msg),
        Event::ToolStarted { name, .. } => turn::tool_started(app, name),
        Event::ToolProgress { .. } => {}
        // Over before its batch commits, so the clock beside it stops now rather than when the
        // last call of the message does
        Event::ToolCompleted { .. } => {
            app.status.active_tool = None;
            app.status.tool_started = None;
        }
        Event::ToolEnded { call_id, .. } => {
            app.status.active_tool = None;
            app.status.tool_started = None;
            if app.pull_request_pushes.remove(&call_id) {
                app.pull_request_link_pending = true;
            }
        }
        Event::StateChanged(state) => turn::state_changed(app, state),
        Event::ContextUsage { used_tokens, context_window } if context_window > 0 => {
            app.status.context_window = context_window;
            app.status.context_pct =
                (used_tokens as f32 / context_window as f32 * 100.0).min(100.0);
        }
        Event::TurnUsage {
            prompt_tokens, completion_tokens, cost_usd, cache_read_tokens, ..
        } => {
            app.status.on_call_usage(prompt_tokens, completion_tokens, cache_read_tokens, cost_usd);
        }
        Event::SessionUsage(usage) => app.status.on_session_usage(usage),
        Event::PlanUsage(plan) => app.status.plan_usage = Some(plan),
        Event::ContextComposition(report) => app.push_convo(ConvoBlock::Context(report)),
        // Compaction rewrites what the model can see and spends a provider call doing it. Saying
        // so is the difference between "the agent forgot" and "the conversation was summarized".
        Event::CompactionStarted { .. } => compaction::started(app),
        Event::CompactionDone { tokens_before, tokens_after } => {
            compaction::done(app, tokens_before, tokens_after);
        }
        Event::CompactionStopped => compaction::ended(app),
        Event::WorkspaceUpdated(snapshot) => workspace_updated(app, *snapshot),
        Event::ToolActivityUpdated(activity) => app.apply_tool_activity(*activity),
        Event::PermissionRequested { id, tool, summary, always_patterns, .. } => {
            // The prompt is a question to the user; nothing above it should still be arriving.
            app.reveal_catch_up();
            // The turn stops until someone answers, which is the moment most worth hearing about
            let body = turn::permission_body(&tool, &summary);
            turn::alert(app, NotificationEvent::Permission, &body);
            app.overlays.permission = Some(
                crate::permission_prompt::PermissionPromptState::new(
                    id,
                    tool,
                    summary,
                    always_patterns,
                )
                .opened(app.frame_now()),
            );
        }
        Event::QuestionAsked { id, prompt, options, multi, .. } => {
            app.reveal_catch_up();
            turn::alert(app, NotificationEvent::Question, &format!("Question: {prompt}"));
            app.overlays.question =
                Some(crate::question_prompt::QuestionPromptState::new(id, prompt, options, multi));
        }
        Event::GuidedReviewLoading { request_id, .. }
            if app.active_guided_review_request.as_ref() != Some(&request_id) =>
        {
            tracing::debug!(
                ?request_id,
                "ignoring guided review loading event for inactive request"
            );
        }
        // No transcript entry: the tray above the composer is already saying a review is being
        // prepared, and it says it where the reader is looking rather than at whatever point the
        // stream happened to be.
        Event::GuidedReviewLoading { request_id: _, target } => {
            app.guided_review_workspace.begin_loading(target);
        }
        Event::GuidedReviewLoaded { request_id, review } => review::loaded(app, request_id, review),
        Event::WorktreeRemovalChecked { uncommitted } => {
            super::commands::worktree_removal_checked(app, uncommitted);
        }
        Event::ThreadAnswersPrepared { request_id, pushed, uncommitted } => {
            super::messages::thread_answers::prepared(app, &request_id, pushed, uncommitted);
        }
        Event::ThreadsAnswered { request_id, results } => {
            super::messages::thread_answers::answered(app, &request_id, results);
        }
        Event::ThreadAnswersFailed { request_id, message } => {
            super::messages::thread_answers::failed(app, &request_id, message);
        }
        Event::PullRequestPostPrepared { request_id, patches, head_moved, own_pull_request } => {
            super::messages::github_post::prepared(
                app,
                &request_id,
                crate::guided_review::PreparedPost { patches, head_moved, own_pull_request },
            );
        }
        Event::PullRequestReviewPosted { request_id, github_review_id, url, .. } => {
            super::messages::github_post::posted(
                app,
                &request_id,
                luxe_core::guided_review::PostedComment { review_id: github_review_id, url },
            );
        }
        Event::PullRequestPostFailed { request_id, message } => {
            super::messages::github_post::failed(app, &request_id, message);
        }
        Event::GithubOverviewRead { repository, number, overview, error, local } => {
            github::overview_read(app, repository, number, overview, error, local);
        }
        Event::GithubConversationRead { repository, number, entries, error } => {
            github::conversation_read(app, repository, number, entries, error);
        }
        Event::GithubReplyDrafted { request_id, body, sources, error, note } => {
            github::reply_drafted(app, request_id, body, sources, error, note);
        }
        Event::PullRequestChanged { repository, number, change, error } => {
            github::pull_request_changed(app, repository, number, change, error);
        }
        Event::GithubCommentPosted { repository, number, thread, url, error } => {
            github::comment_posted(app, repository, number, thread, url, error);
        }
        Event::GithubListed { repository, sections } => github::listed(app, repository, sections),
        Event::GithubSearched { request, repository, section } => {
            github::searched(app, request, repository, section);
        }
        Event::StackPrepared { request_id, layers, refusal } => {
            super::messages::stack_opening::prepared(app, &request_id, layers, refusal);
        }
        Event::StackOpened { request_id, numbers, urls, failed } => {
            super::messages::stack_opening::opened(app, &request_id, numbers, urls, failed);
        }
        Event::RestackPlanned { request_id, plan, merged, refusal } => {
            github::restack_planned(app, request_id, plan, merged, refusal);
        }
        Event::Restacked { moved, retargeted, retarget_failed, .. } => {
            github::restacked(app, moved, retargeted, retarget_failed);
        }
        Event::RestackFailed { message, .. } => github::restack_failed(app, message),
        Event::PullRequestFixChecked { number, branch, refusal } => {
            super::messages::pull_request_fix::checked(app, number, branch, refusal);
        }
        Event::PullRequestPrepared {
            request_id,
            pull_request,
            publication,
            uncommitted,
            commits,
        } => {
            super::messages::pull_request_opening::prepared(
                app,
                &request_id,
                crate::guided_review::PreparedOpening {
                    pull_request,
                    publication,
                    uncommitted,
                    commits,
                },
            );
        }
        Event::PullRequestOpened { request_id, number, url, committed } => {
            super::messages::pull_request_opening::opened(
                app,
                &request_id,
                number,
                url,
                committed.len(),
            );
        }
        Event::PullRequestOpenFailed { request_id, message } => {
            super::messages::pull_request_opening::failed(app, &request_id, message);
        }
        Event::GuidedReviewGuideUpdated {
            review_id,
            revision_id,
            chapters,
            provenance,
            brief,
            ..
        } => review::guide_updated(app, review_id, revision_id, chapters, provenance, brief),
        Event::ReviewCommitted { request_id, commits, skipped } => {
            review::committed(app, request_id, commits, skipped);
        }
        Event::ReviewCommitFailed { request_id, message } => {
            review::commit_failed(app, request_id, message);
        }
        Event::ReviewFeedbackTurnFailed { message, .. } => {
            app.push_convo(ConvoBlock::System(format!(
                "[review feedback] The agent did not finish it: {}. It can be sent again from the \
                 review.",
                message.trim_end_matches('.')
            )));
        }
        Event::ReviewSessionRestored { session } => review::session_restored(app, session),
        Event::ReviewSessionSaved { request_id, session } => {
            review::session_saved(app, request_id, session);
        }
        Event::ReviewSessionSaveFailed { request_id, message, .. } => {
            review::session_save_failed(app, request_id, message);
        }
        Event::GuidedReviewFailed { request_id, target, error } => {
            review::failed(app, request_id, target, error);
        }
        Event::ImageAttached { mime, width, height } => {
            // Attach an image to the current message.
            tracing::info!(mime, width, height, "image attached");
            // The actual image data is carried via the editor's attached_images vec,
            // which is populated when the user pastes an image from the clipboard.
            // This event just signals that an image was attached so the UI can update.
        }
        _ => {}
    }
}

fn workspace_updated(app: &mut App, snapshot: luxe_core::WorkspaceSnapshot) {
    app.apply_workspace_snapshot(snapshot);
    super::messages::files::sync(app);
    // The agent may have just made or moved the file someone is about to mention
    if app.overlays.mention.is_some() {
        if let Some(workspace) = app.workspace.as_ref() {
            app.file_provider.set_working(&workspace.changes.files);
        }
        app.file_listing.request();
    }
    github::link_pushed_pull_request(app);
    super::commands::refresh_pull_request_card(app);
    // An open pane walks the list the dock draws, and the dock has just been redrawn.
    super::messages::changes::resync(app);
}
