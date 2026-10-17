//! Modal surfaces — the overlays that own every key while they are open.
//!
//! A permission ask, a question, the login wizard, the Configuration surface and the image viewer
//! all cover what is underneath them, so a keystroke that leaked past one would land somewhere the
//! reader cannot see — in the worst case echoing a secret into the composer.
//!
//! A paste is a keystroke by another name, and [`modal_took_paste`] walks the same list in the same
//! order for exactly that reason: a paste that went straight to the composer would leave an API key
//! pasted into the login wizard looking like it did nothing and surfacing later in the next prompt.
//! A surface with a text field takes the paste; one without swallows it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::super::{commands, App, Msg, ViewMode};
use crate::github_detail::DetailKey;
use crate::github_panel::GithubKey;
use luxe_core::engine::Action;

/// Answers whether this layer took the key: the first open surface in this list owns it.
pub(super) fn modal_took(app: &mut App, key: KeyEvent) -> bool {
    context_menu_key(app, key)
        || permission_key(app, key)
        || github_panel_key(app, key)
        || pull_request_card_key(app, key)
        || restack_key(app, key)
        || question_key(app, key)
        || auth_wizard_key(app, key)
        || configuration_key(app, key)
        || keys_card_key(app, key)
        || image_viewer_key(app, key)
}

/// A right-click's menu is the nearest thing to the reader while it is open.
fn context_menu_key(app: &mut App, key: KeyEvent) -> bool {
    let Some(menu) = app.overlays.context_menu.as_mut() else {
        return false;
    };
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => menu.step(false),
        KeyCode::Down | KeyCode::Char('j') => menu.step(true),
        KeyCode::Enter => {
            let chosen = menu.selected;
            app.update(Msg::ChooseMenuItem(chosen));
        }
        _ => app.overlays.context_menu = None,
    }
    true
}

/// A pending permission prompt owns all input until answered, so an approval is always deliberate
/// and stray keystrokes never leak into the composer.
fn permission_key(app: &mut App, key: KeyEvent) -> bool {
    use crate::permission_prompt::PermissionPromptOutcome;
    let now = app.frame_now();
    let Some(prompt) = app.overlays.permission.as_mut() else {
        return false;
    };
    if let PermissionPromptOutcome::Respond(reply) = prompt.on_key_at(&key, now) {
        let id = prompt.id.clone();
        if deliver_answer(app, Action::RespondPermission { id, reply }) {
            app.overlays.permission = None;
        }
    }
    true
}

/// `/github`'s panel; each row's action leaves it.
fn github_panel_key(app: &mut App, key: KeyEvent) -> bool {
    let Some(panel) = app.overlays.github.as_mut() else {
        return false;
    };
    match panel.on_key(&key) {
        GithubKey::Held => {}
        GithubKey::Close => app.overlays.github = None,
        GithubKey::Refresh => {
            let sections = panel.searches.clone();
            if app.send_action(Action::ListGithub { sections }, "refresh GitHub") {
                if let Some(panel) = app.overlays.github.as_mut() {
                    panel.sections = None;
                    panel.pending.clear();
                    panel.forget_details();
                }
            }
        }
        GithubKey::Search { request, section, after } => {
            // A new search is the tab's for the session; a page is just more of it
            if after.is_none() {
                for (configured, search) in panel.configured.iter().zip(&panel.searches) {
                    match configured.query == search.query {
                        true => app.github_searches.remove(&configured.query),
                        false => app
                            .github_searches
                            .insert(configured.query.clone(), search.query.clone()),
                    };
                }
            }
            let asked = Action::SearchGithub { request, section, after };
            if !app.send_action(asked, "search GitHub") {
                if let Some(panel) = app.overlays.github.as_mut() {
                    panel.unasked(request);
                }
            }
        }
        GithubKey::Refer(item) => {
            app.overlays.github = None;
            app.editor.insert_reference(&crate::mentions::github_token(&item));
            app.editor.insert_text(" ");
            app.github_rows.insert(crate::mentions::github_target(&item), *item);
        }
        GithubKey::Review { number, repository } => {
            app.overlays.github = None;
            let target = luxe_core::guided_review::ReviewTarget::PullRequest { number, repository };
            app.review.open_when_ready = true;
            commands::start_guided_review(app, target);
        }
        GithubKey::TakeUpIssue(number) => {
            app.overlays.github = None;
            app.outbox.issue = Some(number);
        }
        GithubKey::Refused(said) => app.status.flash_toast(said),
        GithubKey::Resume(session) => {
            app.overlays.github = None;
            app.resume_through_cli(session);
        }
        detail @ (GithubKey::Open(_)
        | GithubKey::Act { .. }
        | GithubKey::Merge { .. }
        | GithubKey::ReadConversation { .. }
        | GithubKey::Post(_)
        | GithubKey::Draft { .. }
        | GithubKey::EditExternally(_)) => detail_action(app, detail),
    }
    true
}

/// `/pr`'s card covers the conversation, so it takes every key until Esc closes it. What its
/// detail asks for is what the same detail asks for under `/github`, for the checkout's own pull
/// request.
fn pull_request_card_key(app: &mut App, key: KeyEvent) -> bool {
    let Some(card) = app.overlays.pull_request_card.as_mut() else {
        return false;
    };
    let number = card.number;
    let action = match card.on_key(&key) {
        DetailKey::Held => return true,
        DetailKey::Leave => {
            app.overlays.pull_request_card = None;
            return true;
        }
        DetailKey::Resume(session) => {
            app.overlays.pull_request_card = None;
            app.resume_through_cli(session);
            return true;
        }
        DetailKey::Act(action) => match card_overview(app) {
            Some(overview) => {
                GithubKey::Act { repository: None, overview: Box::new(overview), action }
            }
            None => return true,
        },
        DetailKey::Open(url) => GithubKey::Open(url),
        DetailKey::Merge(write) => GithubKey::Merge { repository: None, number, write },
        DetailKey::Draft(ask) => {
            GithubKey::Draft { repository: None, number, pull_request: true, ask }
        }
        DetailKey::ReadConversation => {
            GithubKey::ReadConversation { repository: None, number, pull_request: true }
        }
        DetailKey::Post(request) => GithubKey::Post(luxe_core::engine::GithubPost {
            repository: None,
            number,
            thread: request.thread,
            answering: request.answering,
            body: request.body,
        }),
        DetailKey::EditExternally(text) => GithubKey::EditExternally(text),
    };
    detail_action(app, action);
    true
}

/// What a pull request's or an issue's detail asks for, from `/github`'s panel or `/pr`'s card.
fn detail_action(app: &mut App, key: GithubKey) {
    match key {
        GithubKey::Open(url) => app.open_externally(std::ffi::OsStr::new(&url), &url),
        GithubKey::Act { repository, overview, action } => {
            commands::act_on_pull_request(app, repository, &overview, action);
        }
        GithubKey::Merge { repository, number, write } => {
            commands::confirm_change(app, repository, number, write);
        }
        GithubKey::ReadConversation { repository, number, pull_request } => {
            commands::read_conversation(app, repository, number, pull_request);
        }
        GithubKey::Post(post) => commands::post_comment(app, post),
        GithubKey::Draft { repository, number, pull_request, ask } => {
            commands::draft_reply(app, repository, number, pull_request, ask);
        }
        GithubKey::EditExternally(text) => commands::edit_draft_externally(app, text),
        other => tracing::warn!(?other, "a panel's own key reached the detail actions"),
    }
}

/// The restack plan is modal too: nothing typed may land in the composer behind it.
fn restack_key(app: &mut App, key: KeyEvent) -> bool {
    use crate::restack_panel::{PanelKey, RestackPanel};
    let Some(panel) = app.overlays.restack.as_ref() else {
        return false;
    };
    match panel.on_key(&key) {
        PanelKey::Close => app.overlays.restack = None,
        PanelKey::Push(request_id) => {
            if app.send_action(Action::Restack { request_id }, "restack") {
                app.overlays.restack = Some(RestackPanel::Pushing);
            }
        }
        PanelKey::Held => {}
    }
    true
}

/// A pending question is modal until answered or dismissed. The plan handoff asks one of its own,
/// whose answer stays here rather than going to the engine.
fn question_key(app: &mut App, key: KeyEvent) -> bool {
    use crate::question_prompt::QuestionPromptOutcome;
    use luxe_core::question::QuestionReply;
    let handoff = app
        .overlays
        .question
        .as_ref()
        .is_some_and(|prompt| super::super::plan_handoff::is_handoff(app, &prompt.id));
    let Some(prompt) = app.overlays.question.as_mut() else {
        return false;
    };
    let reply = match prompt.on_key(&key) {
        QuestionPromptOutcome::Consumed => return true,
        QuestionPromptOutcome::Answer(answers) if handoff => Err(Some(answers)),
        QuestionPromptOutcome::Cancel if handoff => Err(None),
        QuestionPromptOutcome::Answer(answers) => Ok(QuestionReply::Answered(answers)),
        QuestionPromptOutcome::Cancel => Ok(QuestionReply::Cancelled),
    };
    match reply {
        Ok(reply) => {
            let id = prompt.id.clone();
            if deliver_answer(app, Action::AnswerQuestion { id, reply }) {
                app.overlays.question = None;
            }
        }
        Err(answers) => {
            app.overlays.question = None;
            super::super::plan_handoff::answered(app, answers);
        }
    }
    true
}

/// The login wizard is modal while open: it is collecting a secret, and a stray keystroke must not
/// reach the composer (or worse, be echoed there).
fn auth_wizard_key(app: &mut App, key: KeyEvent) -> bool {
    let Some(wizard) = app.overlays.auth_wizard.as_mut() else {
        return false;
    };
    let outcome = wizard.on_key(&key);
    app.apply_wizard_outcome(outcome);
    true
}

/// The Configuration surface is the whole screen while it is open, so it takes the keys — except
/// the model picker's, which is the last step of setting a provider up and is drawn over it.
///
/// The event loop routes these first; this is the same behavior for anything driving the app
/// through `Msg::Key` (the fixture runner, a test).
fn configuration_key(app: &mut App, key: KeyEvent) -> bool {
    if app.current_view != ViewMode::Configuration
        || app.overlays.model_picker.is_some()
        || app.overlays.theme_picker.is_some()
    {
        return false;
    }
    let outcome = app.overlays.configuration.as_mut().map(|surface| surface.on_key(&key));
    if let Some(outcome) = outcome {
        app.apply_configuration_outcome(outcome);
    }
    true
}

/// The key card is read, not typed into: its close keys put it away, and nothing else leaks to a
/// composer it is drawn over. A key someone bound to help closes it the way it opened it.
fn keys_card_key(app: &mut App, key: KeyEvent) -> bool {
    if !app.overlays.keys_card {
        return false;
    }
    let closes = matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q' | '?'))
        && key.modifiers.is_empty()
        || super::resolve_key_event(&app.keymap, key)
            == Some(luxe_core::keymap::UiAction::ToggleHelp);
    if closes {
        app.update(Msg::CloseKeysCard);
    }
    true
}

/// The image viewer covers the transcript, so it takes input while it is open: there is nothing
/// underneath that the reader can see to type into. Esc, `q` and the chord that opened it all put
/// the transcript back — deliberately not Enter, which submits and is worth never making anyone
/// hesitate over.
fn image_viewer_key(app: &mut App, key: KeyEvent) -> bool {
    if app.overlays.image_viewer.is_none() {
        return false;
    }
    let closes = matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) && key.modifiers.is_empty()
        || (key.code == KeyCode::Char('i') && key.modifiers == KeyModifiers::ALT);
    if closes {
        app.update(Msg::CloseImageViewer);
    }
    true
}

/// Answers whether an open surface took the paste, in the precedence keys use.
///
/// The order is deliberately the same as [`modal_took`]'s, plus the surfaces that are drawn *over*
/// a view rather than instead of it — the pickers and the review's comment draft. Returning `true`
/// without typing anything is a real answer: a modal that has nowhere to put the text still owns
/// it, because the alternative is the composer quietly collecting a key the reader cannot see.
///
/// Empty `text` asks the ownership question without answering it: every field ignores a paste
/// that carries nothing, so the call is free of side effects and an image paste can use it to
/// find out whether it has anywhere to land.
pub(crate) fn modal_took_paste(app: &mut App, text: &str) -> bool {
    // A reply being written takes the paste, in `/pr`'s card or beside `/github`'s list
    if let Some(card) = commands::composing_detail(app).and_then(|detail| detail.composing.as_mut())
    {
        card.paste(text);
        return true;
    }
    if app.overlays.permission.is_some()
        || app.overlays.restack.is_some()
        || app.overlays.pull_request_card.is_some()
        || app.overlays.keys_card
    {
        return true;
    }
    if let Some(panel) = app.overlays.github.as_mut() {
        panel.paste(text);
        return true;
    }
    if let Some(prompt) = app.overlays.question.as_mut() {
        prompt.paste(text);
        return true;
    }
    if let Some(wizard) = app.overlays.auth_wizard.as_mut() {
        wizard.paste(text);
        return true;
    }
    // The overlays drawn over a view rather than instead of it, in the order `picker_took` walks
    // them — the palette first, because it opens over anything.
    if let Some(palette) = app.overlays.palette.as_mut() {
        palette.paste(text);
        return true;
    }
    if let Some(picker) = app.overlays.model_picker.as_mut() {
        picker.paste(text);
        return true;
    }
    if let Some(picker) = app.overlays.session_picker.as_mut() {
        picker.paste(text);
        return true;
    }
    if app.overlays.thinking_picker.is_some()
        || app.overlays.theme_picker.is_some()
        || app.overlays.context_files.is_some()
        || app.overlays.image_viewer.is_some()
    {
        return true;
    }
    match app.current_view {
        // The composer is the one text field on screen, and it is the default for a reason.
        ViewMode::Chat => false,
        // The review's comment draft owns paste while it is open; without it the text lands in
        // the chat editor hidden behind the full-screen review and looks lost.
        ViewMode::GuidedReview => {
            if let Some(draft) = app.guided_review_workspace.finding_draft.as_mut() {
                draft.paste(text);
            } else if let Some(reply) = app
                .guided_review_workspace
                .thread_answers
                .as_mut()
                .and_then(|answers| answers.replying.as_mut())
            {
                reply.insert_text(text);
            } else if let Some((_, editor)) = app
                .guided_review_workspace
                .pull_request_opening
                .as_mut()
                .and_then(|opening| opening.editing.as_mut())
            {
                editor.insert_text(text);
            }
            true
        }
        // The start screen's prompt is the composer, so text lands there; an image waits for the
        // session, since a start screen has nowhere to show one
        ViewMode::Dashboard => {
            if !text.is_empty() {
                app.editor.paste_text(text);
                super::sync_completion_flyouts(app);
                if let Some(dashboard) = app.dashboard.as_mut() {
                    dashboard.sync(&app.editor);
                }
            }
            true
        }
        // Nothing on these surfaces can be typed into, and the composer they would otherwise
        // fall through to is not on screen.
        ViewMode::Configuration | ViewMode::SessionPicker => true,
    }
}

/// The pull request `/pr`'s card shows, once read.
fn card_overview(app: &App) -> Option<luxe_core::github::overview::PullRequestOverview> {
    use crate::github_detail::{CardState, Subject};
    match app.overlays.pull_request_card.as_ref().map(|card| &card.state) {
        Some(CardState::Ready(detail)) => match &detail.subject {
            Subject::PullRequest(pr) => Some(pr.as_ref().clone()),
            Subject::Issue(_) => None,
        },
        _ => None,
    }
}

/// Hands a prompt's answer to the engine, and says whether it went.
///
/// The turn is blocked until this arrives, so an answer that was dropped is a turn that never
/// resumes. Closing the prompt anyway would leave a card running behind a prompt that is gone; kept
/// open, the reader can answer again.
fn deliver_answer(app: &mut App, answer: Action) -> bool {
    app.send_action(answer, "answer a prompt")
}
