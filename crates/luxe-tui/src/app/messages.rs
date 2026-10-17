//! App-message reduction — input, core events, and local UI messages.
//!
//! This is the single entry point for `Msg` state changes. Specialized reducers own their
//! domains beneath it, keeping the top-level app composition small: [`reduce`] stays a dispatch
//! table, and an arm with real work behind it delegates to the module that owns that work.

use super::input::unsent_finding_ids;
use super::{events, App, ConvoBlock, Msg};

pub(crate) mod changes;
pub(crate) mod files;
pub(super) mod github_post;
pub(super) mod pull_request_fix;
pub(crate) mod pull_request_opening;
pub(super) mod review;
pub(crate) mod review_draft;
mod review_nav;
pub(super) mod stack_opening;
pub(crate) mod thread_answers;

pub(super) use review_nav::persist_review_progress;

pub(super) fn reduce(app: &mut App, msg: Msg) {
    reduce_inner(app, msg);
    // Whether the turn is blocked on a person is a property of what is on screen, not of any one
    // message — a prompt opens on an event and closes on a keystroke, an interrupt, or the turn
    // ending. Reconciling it here is what keeps the clock honest through all four.
    let waiting = app.overlays.permission.is_some() || app.overlays.question.is_some();
    if let Some(waited) = app.status.waiting_on_user(waiting, app.frame_now()) {
        // Dispatch runs one call at a time, so the wait that just closed belonged to whichever
        // call is in flight — its card would otherwise report the reader's own deliberation as
        // how long the tool took.
        let waited = u64::try_from(waited.as_millis()).unwrap_or(u64::MAX);
        let blocked = app
            .tool_items
            .iter()
            .find(|(_, item)| item.activity.state == luxe_core::ToolActivityState::Running)
            .map(|(key, _)| key.clone());
        if let Some(key) = blocked {
            let item = app.tool_items.get_mut(&key).map(|item| {
                item.waited_on_user_ms = item.waited_on_user_ms.saturating_add(waited);
                item.clone()
            });
            // The transcript holds its own copy of the card, so the block has to be refreshed or
            // the figure would only appear on the next lifecycle update.
            if let Some(item) = item {
                let id = crate::app::ConversationItemId::Tool { session_id: key.0, call_id: key.1 };
                if let Some(existing) = app.convo.iter_mut().find(|existing| existing.id == id) {
                    existing.set_block(ConvoBlock::Tool(Box::new(item)));
                }
            }
        }
    }
}

fn reduce_inner(app: &mut App, msg: Msg) {
    match msg {
        Msg::OpenChanges(_)
        | Msg::CloseChanges
        | Msg::ChangesStepFile(_)
        | Msg::ChangesOpenReview
        | Msg::ToggleExternalPlace(_)
        | Msg::ScrollChanges { .. }
        | Msg::PageChanges(_)
        | Msg::JumpChanges(_)
        | Msg::ToggleChangesWordWrap
        | Msg::ToggleChangesSplit
        | Msg::OpenProjectFile(_)
        | Msg::TogglePeekView
        | Msg::StartPeekPick(_)
        | Msg::ExtendPeekPick(_)
        | Msg::PeekSelectionAction(_) => changes::reduce(app, msg),
        Msg::ToggleFolder(_)
        | Msg::ToggleFilesScope
        | Msg::ToggleIgnoredFiles
        | Msg::FocusFiles
        | Msg::FocusPeek
        | Msg::ChooseMenuItem(_) => files::reduce(app, msg),
        Msg::Key(key) => {
            app.handle_key(key);
            // The start screen's flyouts edit the prompt through the app, so its list catches up
            if app.current_view == super::ViewMode::Dashboard {
                if let Some(dashboard) = app.dashboard.as_mut() {
                    dashboard.sync(&app.editor);
                }
            }
        }
        Msg::Engine(event) => events::reduce(app, event),
        Msg::Tick => {
            // `/github` reads the row the cursor has come to rest on
            let now = app.frame_now();
            let due = app.overlays.github.as_mut().and_then(|panel| panel.due_read(now));
            if let Some(read) = due {
                super::commands::read_overview(
                    app,
                    read.repository,
                    read.number,
                    read.pull_request,
                );
            }
            // An open Files tab reads the disk again now and then, for a file made elsewhere
            let on_files =
                app.overlays.mention.as_ref().is_some_and(|mention| {
                    *mention.tab() == crate::mention_flyout::MentionTab::Files
                });
            if on_files && app.file_listing.due(std::time::Instant::now()) {
                app.file_listing.request();
            }
            super::events::expire_unconfirmed_interrupt(app);
            let waiting = app.panel_waiting();
            app.status.tick(waiting);
            app.banner.advance();
            // The review's own waits happen while the app is idle, so the status model's spinner
            // is not moving for them.
            if app.current_view == crate::app::ViewMode::GuidedReview {
                app.guided_review_workspace.tick();
            }
        }
        Msg::OpenAuthWizard(provider) => app.open_auth_wizard(provider.as_deref()),
        Msg::Auth(message) => {
            // The flow's own messages can end the wizard — a credential that landed closes it and
            // opens the model picker — so the outcome goes through the same door a keypress does.
            let outcome = app.overlays.auth_wizard.as_mut().map(|wizard| wizard.apply(message));
            if let Some(outcome) = outcome {
                app.apply_wizard_outcome(outcome);
            }
        }
        Msg::TerminalResized { width, height } => {
            app.workspace_layout = crate::workspace_layout::resolve_workspace_layout(width).layout;
            app.guided_review_workspace.set_terminal_size(width, height);
            // A font-size change arrives as a resize and nothing else, and it moves the only
            // number an image is scaled against — so ask the terminal again rather than trusting
            // what it said at startup.
            app.images.set_cell_size(crate::graphics::CellSize::detect());
            app.with_review(|workspace, review| workspace.clamp_column_offset(&review.files));
        }
        Msg::ExitGuidedReview => super::navigation::exit_guided_review(app),
        Msg::GuidedReviewSelectDiffRow(_)
        | Msg::GuidedReviewSelectFile(_)
        | Msg::GuidedReviewToggleFileFold(_)
        | Msg::GuidedReviewBeginSearch
        | Msg::GuidedReviewSearchInput(_)
        | Msg::GuidedReviewSearchBackspace
        | Msg::GuidedReviewCancelSearch
        | Msg::GuidedReviewSearchNext
        | Msg::GuidedReviewSearchPrevious
        | Msg::GuidedReviewLineNext
        | Msg::GuidedReviewLinePrevious
        | Msg::GuidedReviewOpenSelection
        | Msg::GuidedReviewScrollGuide { .. }
        | Msg::GuidedReviewPageNext
        | Msg::GuidedReviewHalfPage(_)
        | Msg::GuidedReviewPagePrevious
        | Msg::GuidedReviewHunkNext
        | Msg::GuidedReviewHunkPrevious
        | Msg::GuidedReviewChapterNext
        | Msg::GuidedReviewChapterPrevious
        | Msg::GuidedReviewFileNext
        | Msg::GuidedReviewFilePrevious
        | Msg::GuidedReviewLineStart
        | Msg::GuidedReviewColumnNext
        | Msg::GuidedReviewColumnPrevious
        | Msg::GuidedReviewToggleGuide
        | Msg::GuidedReviewToggleNoise
        | Msg::GuidedReviewToggleNarration
        | Msg::GuidedReviewToggleNoiseDetails
        | Msg::GuidedReviewToggleSplit
        | Msg::GuidedReviewToggleWordWrap
        | Msg::GuidedReviewToggleHelp
        | Msg::GuidedReviewToggleSourcePicker
        | Msg::GuidedReviewSourcePickerNext
        | Msg::GuidedReviewSourcePickerPrevious
        | Msg::GuidedReviewSelectSourcePicker
        | Msg::GuidedReviewSourceReferenceInsert(_)
        | Msg::GuidedReviewSourceReferenceBackspace
        | Msg::GuidedReviewSubmitSourceReference
        | Msg::GuidedReviewCancelSourceReference
        | Msg::GuidedReviewToggleFindings
        | Msg::GuidedReviewSelectFinding(_)
        | Msg::GuidedReviewOpenChapter(_) => review_nav::reduce(app, msg),
        Msg::DispatchUiAction(action) => {
            super::commands::dispatch_action(app, action);
        }
        Msg::ToggleConversationBlockDetails(index) => {
            app.toggle_block_details_at(index);
        }
        Msg::ScrollDock { up } => app.dock.scroll_by(up, app.dock_scroll_max.get()),
        Msg::ScrollContextFiles { up } => {
            if let Some(files) = app.overlays.context_files.as_mut() {
                files.scroll_by(up);
            }
        }
        Msg::ScrollDockList { list, up } => {
            app.dock.scroll_list_by(list, up, app.dock_list_metrics(list));
        }
        Msg::DragDockList { list, row, height } => {
            app.dock.drag_list(list, row, height, app.dock_list_metrics(list));
        }
        Msg::DragScrollbar { region, row, height } => drag_scrollbar(app, region, row, height),
        Msg::BeginTextSelection { hit, span } => super::selection::begin(app, hit, span),
        Msg::ExtendTextSelection { hit } => super::selection::extend(app, hit),
        Msg::ClearTextSelection => super::selection::clear(app),
        Msg::CopySelection => super::selection::copy(app),
        Msg::HoverTarget(target) => app.pointer.hover = target,
        Msg::ResizePane { pane, width } => app.size_pane(pane, Some(width)),
        Msg::ResetPaneWidth(pane) => {
            app.size_pane(pane, None);
            app.panes.save();
        }
        Msg::FinishPaneResize => app.panes.save(),
        Msg::OpenModelPicker => app.open_model_picker(),
        Msg::CopyText(text) => app.record_copy(text),
        Msg::OpenImageExternally(key) => app.open_image_externally(&key),
        Msg::OpenImageViewer(key) => {
            if app.images.contains(&key) {
                app.overlays.image_viewer = Some(key);
            }
        }
        Msg::CloseImageViewer => app.overlays.image_viewer = None,
        Msg::CloseKeysCard => app.overlays.keys_card = false,
        Msg::GuidedReviewOpenInBrowser => review::open_in_browser(app),
        Msg::GuidedReviewBeginQuestion => {
            app.with_review(|workspace, review| workspace.begin_question(&review.files));
        }
        Msg::GuidedReviewBeginReply => {
            if let Some(finding) = app.selected_finding().cloned() {
                let severity = if review::reply_asks_for_fix(app, &finding) {
                    luxe_core::guided_review::FindingSeverity::Issue
                } else {
                    finding.severity
                };
                app.guided_review_workspace.begin_reply(&finding, severity);
            }
        }
        // Another question on a question's thread; anything else has no question to continue
        Msg::GuidedReviewBeginFollowUp => {
            use luxe_core::guided_review::FindingSeverity;
            let finding = app.selected_finding().cloned();
            if let Some(finding) =
                finding.filter(|finding| finding.severity == FindingSeverity::Question)
            {
                app.guided_review_workspace.begin_reply(&finding, FindingSeverity::Question);
            }
        }
        Msg::GuidedReviewOpenSendPreview => {
            if github_post::is_pull_request(app) {
                // Someone else's pull request is not checked out here: the agent can fix it in a
                // worktree, or the comments go to GitHub
                pull_request_fix::open(app);
            } else if unsent_finding_ids(app).is_empty() {
                app.push_convo(ConvoBlock::System(
                    "[review feedback] No unsent comments — add one with c.".into(),
                ));
            } else {
                app.guided_review_workspace.open_send_preview(None, None);
            }
        }
        Msg::GuidedReviewMoveSendPreview(forward) => {
            let rows = unsent_finding_ids(app).len();
            app.guided_review_workspace.move_send_preview(rows, forward);
        }
        Msg::GuidedReviewToggleSendPreviewRow => {
            let candidates = unsent_finding_ids(app);
            app.guided_review_workspace.toggle_send_preview_row(&candidates);
        }
        Msg::GuidedReviewConfirmSend => review::confirm_send(app),
        Msg::GuidedReviewOpenGithubPost => github_post::open(app),
        Msg::GuidedReviewOpenThreadAnswers => thread_answers::open(app),
        Msg::GuidedReviewOpenPullRequest => pull_request_opening::open(app),
        Msg::GuidedReviewOpenStack => stack_opening::open(app),
        Msg::GuidedReviewToggleStackDraft => stack_opening::toggle_draft(app),
        Msg::GuidedReviewConfirmStack => stack_opening::confirm(app),
        Msg::GuidedReviewCancelStack => stack_opening::cancel(app),
        Msg::GuidedReviewOpenPullRequestFix => {
            // The post preview gives way only once there is an offer to give way to
            let posting =
                app.guided_review_workspace.github_post.as_ref().is_some_and(|post| post.posting);
            if !posting && pull_request_fix::open(app) {
                app.guided_review_workspace.github_post = None;
            }
        }
        Msg::GuidedReviewConfirmPullRequestFix => pull_request_fix::confirm(app),
        Msg::GuidedReviewCancelPullRequestFix => {
            app.guided_review_workspace.pull_request_fix = None;
        }
        Msg::GuidedReviewEditPullRequest(field) => pull_request_opening::begin_edit(app, field),
        Msg::GuidedReviewEditPullRequestExternally => pull_request_opening::edit_externally(app),
        Msg::GuidedReviewScrollPullRequest(rows) => pull_request_opening::scroll(app, rows),
        Msg::FilesListed(files) => files::listed(app, files),
        Msg::SessionsListed { request, catalog } => app.fill_session_picker(request, catalog),
        Msg::MentionsListed { kind, answer } => {
            if let Some(mention) = app.overlays.mention.as_mut() {
                mention.set_source(&kind, answer.clone());
            }
            // Kept for the session like GitHub's rows; a failure is not, so the next look asks
            if let Ok(rows) = answer {
                app.mention_listings.insert(kind, rows);
            }
        }
        Msg::TextEdited { target, text } => match target {
            crate::launcher::EditTarget::PullRequestBody => pull_request_opening::edited(app, text),
            crate::launcher::EditTarget::GithubDraft => {
                let card = super::commands::composing_detail(app)
                    .and_then(|detail| detail.composing.as_mut());
                if let Some(card) = card {
                    card.edited(text);
                }
            }
        },
        Msg::GuidedReviewSavePullRequestEdit => pull_request_opening::save_edit(app),
        Msg::GuidedReviewCancelPullRequestEdit => {
            if let Some(opening) = app.guided_review_workspace.pull_request_opening.as_mut() {
                opening.editing = None;
            }
        }
        Msg::GuidedReviewTogglePullRequestDraft => pull_request_opening::toggle_draft(app),
        Msg::GuidedReviewConfirmPullRequest => pull_request_opening::confirm(app),
        Msg::GuidedReviewCancelPullRequest => pull_request_opening::cancel(app),
        Msg::GuidedReviewMoveThreadAnswers(forward) => {
            if let Some(answers) = app.guided_review_workspace.thread_answers.as_mut() {
                let rows = answers.rows.len();
                if rows > 0 && answers.replying.is_none() {
                    answers.selected = if forward {
                        (answers.selected + 1).min(rows - 1)
                    } else {
                        answers.selected.saturating_sub(1)
                    };
                }
            }
        }
        Msg::GuidedReviewToggleThreadResolve => thread_answers::toggle_resolve(app),
        Msg::GuidedReviewBeginThreadReply => thread_answers::begin_reply(app),
        Msg::GuidedReviewSaveThreadReply => thread_answers::save_reply(app),
        Msg::GuidedReviewCancelThreadReply => {
            if let Some(answers) = app.guided_review_workspace.thread_answers.as_mut() {
                answers.replying = None;
            }
        }
        Msg::GuidedReviewConfirmThreadAnswers => thread_answers::confirm(app),
        Msg::GuidedReviewCancelThreadAnswers => {
            let workspace = &mut app.guided_review_workspace;
            if workspace.thread_answers.as_ref().is_some_and(|answers| !answers.sending) {
                workspace.thread_answers = None;
            }
        }
        Msg::GuidedReviewMoveGithubVerdict(forward) => {
            if let Some(post) = app.guided_review_workspace.github_post.as_mut() {
                if !post.posting {
                    post.move_verdict(forward);
                }
            }
        }
        Msg::GuidedReviewMoveGithubPostItem(forward) => {
            let count = github_post::item_count(app);
            if let Some(post) = app.guided_review_workspace.github_post.as_mut() {
                post.move_selection(forward, count);
            }
        }
        Msg::GuidedReviewScrollGithubPost(rows) => {
            if let Some(post) = app.guided_review_workspace.github_post.as_mut() {
                post.scroll_by(rows);
            }
        }
        Msg::GuidedReviewConfirmGithubPost => github_post::confirm(app),
        Msg::GuidedReviewDraftPostSummary => review_draft::begin(app),
        Msg::GuidedReviewEditPostSummary => review_draft::edit(app),
        Msg::GuidedReviewBeginPostSummary => {
            if app.guided_review_workspace.github_post.as_ref().is_some_and(|post| !post.posting) {
                app.guided_review_workspace.begin_review_summary();
            }
        }
        Msg::GuidedReviewCancelGithubPost => {
            if app.guided_review_workspace.github_post.as_ref().is_some_and(|post| !post.posting) {
                app.guided_review_workspace.github_post = None;
            }
        }
        Msg::GuidedReviewCancelSend => app.guided_review_workspace.close_send_preview(),
        Msg::GuidedReviewOpenCommitPreview => {
            if app.guided_review.is_some() {
                app.guided_review_workspace.open_commit_preview();
            }
        }
        Msg::GuidedReviewToggleCommitSquash => {
            app.guided_review_workspace.toggle_commit_squash();
        }
        Msg::GuidedReviewCancelCommit => app.guided_review_workspace.close_commit_preview(),
        Msg::GuidedReviewConfirmCommit => review::confirm_commit(app),
        Msg::GuidedReviewRetry
            if matches!(
                app.guided_review_workspace.load_state,
                crate::guided_review::GuidedReviewLoadState::Failed { .. }
            ) =>
        {
            super::commands::refresh_guided_review(app)
        }
        Msg::GuidedReviewRetry => {}

        Msg::GuidedReviewBeginFinding => {
            app.with_review(|workspace, review| workspace.begin_finding_draft(&review.files));
        }
        Msg::GuidedReviewEditSelectedFinding => match app.selected_finding().cloned() {
            // Editing here would not reach the copy on GitHub, and the two would disagree
            Some(finding) if finding.posted.is_some() => {
                app.status.flash_toast("That comment is on GitHub now; edit it there".to_string())
            }
            Some(finding) => {
                app.guided_review_workspace.begin_edit_finding_draft(&finding);
            }
            None => {}
        },
        Msg::GuidedReviewSaveFinding => review::save_finding(app),
        Msg::GuidedReviewSetOutcome(outcome) => review::set_outcome(app, outcome),
        Msg::GuidedReviewCancelQueuedFeedback(submission_id) => {
            review::cancel_queued_feedback(app, submission_id)
        }
        Msg::GuidedReviewRetryFeedback(submission_id) => review::retry_feedback(app, submission_id),
        Msg::GuidedReviewQueueFeedback { mode, finding_ids, outcome } => {
            review::queue_feedback(app, mode, finding_ids, outcome);
        }
        Msg::GuidedReviewToggleSelectedFindingResolved => {
            review::toggle_selected_finding_resolved(app)
        }
        Msg::GuidedReviewDeleteSelectedFindingDraft => review::delete_selected_finding_draft(app),
        Msg::GuidedReviewReanchorSelectedFinding => review::reanchor_selected_finding(app),
        Msg::PasteText(text) => {
            if crate::app::input::modal_took_paste(app, &text) {
                return;
            }
            app.editor.paste_text(&text);
            app.sync_completion_flyouts();
        }
        Msg::ImagePasted { pixels, width, height } => {
            // An image has no single-line field to land in, so an overlay that owns input simply
            // eats it rather than attaching it to a composer nobody can see.
            if crate::app::input::modal_took_paste(app, "") {
                return;
            }
            if !app.editor.attach_image(pixels, width, height) {
                tracing::warn!("clipboard image could not be encoded");
            }
        }
        Msg::DesktopThemeChanged => app.follow_desktop_theme(),
        Msg::ModelCatalogRefreshed(catalog) => app.apply_model_catalog_refresh(catalog),
    }
}

/// Moves the surface behind a scrollbar to where its track was pressed or dragged.
fn drag_scrollbar(app: &mut App, region: crate::mouse::ScrollRegionId, row: u16, height: u16) {
    use crate::mouse::ScrollRegionId;
    let at = |room: usize| crate::widgets::drag_offset(row, height, room);
    match region {
        ScrollRegionId::Changes => {
            if let Some(view) = app.changes.as_mut() {
                view.scroll = at(view.scroll_max.get());
            }
        }
        ScrollRegionId::ReviewDiff => {
            app.with_review(|workspace, review| workspace.scroll_to_row(review, at));
        }
        ScrollRegionId::ReviewGuide => {
            let workspace = &app.guided_review_workspace;
            workspace.guide_scroll.set(Some(at(workspace.guide_room.get())));
        }
        ScrollRegionId::ReviewFeedback => {
            let workspace = &mut app.guided_review_workspace;
            let room = usize::from(workspace.feedback_room.get());
            workspace.feedback_scroll = u16::try_from(at(room)).unwrap_or(u16::MAX);
        }
        ScrollRegionId::PullRequestBody => {
            if let Some(opening) = app.guided_review_workspace.pull_request_opening.as_mut() {
                let room = usize::from(opening.body_room.get());
                opening.body_scroll = u16::try_from(at(room)).unwrap_or(u16::MAX);
            }
        }
        ScrollRegionId::Conversation => app.scroll_state.drag_to(row, height),
        ScrollRegionId::DockList(list) => {
            app.dock.drag_list(list, row, height, app.dock_list_metrics(list));
        }
        ScrollRegionId::Dock | ScrollRegionId::ContextFiles => {}
    }
}
