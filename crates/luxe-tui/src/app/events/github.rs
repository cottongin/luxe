//! GitHub answers — overviews, conversations, drafts, changes and posts landing in the detail that
//! asked, the `/github` listing, restacks, and linking the session to a pushed pull request.

use crate::app::{App, ConvoBlock};
use luxe_core::github::overview::{ConversationEntry, LocalSession, Overview};
use luxe_core::github::ListedSection;

/// Lands an overview where it was asked from: `/pr`'s card for the checkout's own pull request,
/// or the detail of the `/github` row it answers.
pub(super) fn overview_read(
    app: &mut App,
    repository: Option<String>,
    number: u64,
    overview: Option<Overview>,
    error: Option<String>,
    local: Option<LocalSession>,
) {
    let card = app.overlays.pull_request_card.as_mut().filter(|card| card.number == number);
    if let Some(card) = card.filter(|_| repository.is_none()) {
        card.land(overview.clone(), error.clone());
    }
    let branch = match app.workspace.as_ref().map(|workspace| &workspace.pull_request) {
        Some(luxe_core::workspace::PullRequestPresence::Found(pr)) => Some(pr.number),
        _ => None,
    };
    if let Some(panel) = app.overlays.github.as_mut() {
        panel.land(repository.as_deref(), number, overview, error, local, branch);
    }
}

/// Lands a conversation in the detail that asked: `/pr`'s card, or the `/github` row.
pub(super) fn conversation_read(
    app: &mut App,
    repository: Option<String>,
    number: u64,
    entries: Option<Vec<ConversationEntry>>,
    error: Option<String>,
) {
    for detail in details_answering(app, repository.as_deref(), number) {
        detail.land_conversation(entries.clone(), error.clone());
    }
}

/// Lands a draft the agent wrote in the card waiting on it, or under what it answers.
pub(super) fn reply_drafted(
    app: &mut App,
    request_id: String,
    body: Option<String>,
    sources: Vec<String>,
    error: Option<String>,
    note: Option<String>,
) {
    let mut landed = crate::app::messages::review_draft::landed(
        app,
        &request_id,
        body.clone(),
        error.clone(),
        note.clone(),
    );
    if let Some(card) = app.overlays.pull_request_card.as_mut().filter(|_| !landed) {
        if let crate::github_detail::CardState::Ready(detail) = &mut card.state {
            landed = detail.drafted(
                &request_id,
                body.clone(),
                sources.clone(),
                error.clone(),
                note.clone(),
            );
        }
    }
    if let Some(panel) = app.overlays.github.as_mut().filter(|_| !landed) {
        for detail in panel.details.values_mut() {
            if detail.drafted(
                &request_id,
                body.clone(),
                sources.clone(),
                error.clone(),
                note.clone(),
            ) {
                landed = true;
                break;
            }
        }
    }
    match (landed, error) {
        (false, Some(reason)) => app.status.flash_toast(reason),
        (false, None) if body.is_some() => app.status.flash_toast(
            "The draft landed after /github closed; draft it again to see it".to_string(),
        ),
        _ => {}
    }
}

/// Says what became of a change made from a status row, and reads the overview again to show it.
pub(super) fn pull_request_changed(
    app: &mut App,
    repository: Option<String>,
    number: u64,
    change: Option<luxe_core::engine::PullRequestChange>,
    error: Option<String>,
) {
    use luxe_core::engine::PullRequestChange;
    // What a confirmation sends: closed once it lands
    let confirmed = matches!(
        change,
        Some(PullRequestChange::Merged { .. } | PullRequestChange::AutoMergeEnabled { .. })
    );
    let mut said = false;
    for detail in details_answering(app, repository.as_deref(), number) {
        said |= detail.changed(confirmed, error.as_deref());
    }
    match (change, error) {
        (Some(change), _) => {
            let toast = match change {
                PullRequestChange::BranchUpdated => {
                    format!("GitHub is bringing #{number} up to date with its base")
                }
                PullRequestChange::MarkedReady => format!("#{number} is ready for review"),
                PullRequestChange::Merged { branch_deleted: true, .. } => {
                    format!("Merged #{number}, and deleted its branch")
                }
                PullRequestChange::Merged { branch_deleted: false, .. } => {
                    format!("Merged #{number}")
                }
                PullRequestChange::AutoMergeEnabled { method } => {
                    format!("Auto-merge is on for #{number} · {} when ready", method.label())
                }
                PullRequestChange::AutoMergeDisabled => {
                    format!("Auto-merge is off for #{number}")
                }
            };
            app.status.flash_toast(toast);
            crate::app::commands::reread_overview(app, repository, number);
            let refresh = luxe_core::engine::Action::RefreshWorkspace;
            app.send_quietly(refresh, "refresh the workspace");
        }
        (None, Some(reason)) if !said => app.status.flash_toast(reason),
        (None, _) => {}
    }
}

/// Closes the card of a post that landed and reads the conversation again to show it.
pub(super) fn comment_posted(
    app: &mut App,
    repository: Option<String>,
    number: u64,
    thread: Option<String>,
    url: Option<String>,
    error: Option<String>,
) {
    let mut again = None;
    for detail in details_answering(app, repository.as_deref(), number) {
        if detail.posted(thread.as_deref(), error.clone()) {
            detail.conversation_asked = true;
            let pull_request =
                matches!(detail.subject, crate::github_detail::Subject::PullRequest(_));
            again = Some(pull_request);
        }
    }
    if let Some(pull_request) = again {
        if url.is_some() {
            app.status.flash_toast("Posted to GitHub".to_string());
        }
        crate::app::commands::read_conversation(app, repository, number, pull_request);
    }
}

pub(super) fn listed(app: &mut App, repository: Option<String>, sections: Vec<ListedSection>) {
    // One listing for both places that show it: every row once, in section order
    let mut listing: Vec<luxe_core::github::ListItem> = Vec::new();
    for item in sections.iter().flat_map(|section| &section.items) {
        if !listing.iter().any(|kept| kept.url == item.url) {
            listing.push(item.clone());
        }
    }
    let error = sections.iter().find_map(|section| section.error.clone());
    if let Some(mention) = app.overlays.mention.as_mut() {
        mention.set_github(listing.clone(), error.clone());
    }
    // A listing that failed is not kept, so the next look asks again: signing in with
    // `gh auth login` meanwhile is the usual reason it failed
    app.github_listing = (error.is_none() || !listing.is_empty()).then_some(listing);
    if let Some(panel) = app.overlays.github.as_mut() {
        panel.repository = repository;
        // From an empty first section, on to the first with something in it; anywhere else
        // it keeps its place across a refresh
        let empty_first = sections.first().is_some_and(|section| section.items.is_empty());
        let first = sections.iter().position(|section| !section.items.is_empty());
        panel.section = match first {
            Some(first) if panel.section == 0 && empty_first => first,
            _ => panel.section.min(sections.len().saturating_sub(1)),
        };
        panel.selected = 0;
        panel.sections = Some(sections);
        // Every tab was just answered whole; a page still out belongs to the old rows
        panel.pending.clear();
    }
}

pub(super) fn searched(
    app: &mut App,
    request: u64,
    repository: Option<String>,
    section: ListedSection,
) {
    let failed = app
        .overlays
        .github
        .as_mut()
        .and_then(|panel| panel.land_search(request, repository, section));
    if let Some(reason) = failed {
        app.status.flash_toast(format!("Could not load more: {reason}"));
    }
}

pub(super) fn restack_planned(
    app: &mut App,
    request_id: luxe_core::restack::RestackId,
    plan: Option<luxe_core::restack::RestackPlan>,
    merged: Option<luxe_core::restack::MergedBelow>,
    refusal: Option<String>,
) {
    use crate::restack_panel::RestackPanel;
    if app.overlays.restack.is_some() {
        app.overlays.restack = Some(match (plan, refusal) {
            (Some(plan), _) => RestackPanel::Planned { request_id, plan, merged },
            (None, refusal) => RestackPanel::Said(
                refusal.unwrap_or_else(|| "Nothing came back from planning.".into()),
            ),
        });
    }
}

pub(super) fn restacked(
    app: &mut App,
    moved: Vec<u64>,
    retargeted: Vec<u64>,
    retarget_failed: Option<String>,
) {
    app.overlays.restack = None;
    let list =
        |numbers: &[u64]| numbers.iter().map(|n| format!("#{n}")).collect::<Vec<_>>().join(", ");
    let mut said = match moved.len() {
        0 => "[stack] No branch had to move".to_string(),
        _ => format!("[stack] Restacked {}", list(&moved)),
    };
    if !retargeted.is_empty() {
        said.push_str(&format!("; moved the base of {}", list(&retargeted)));
    }
    said.push('.');
    if let Some(failed) = retarget_failed {
        said.push_str(&format!(" {failed}"));
    }
    app.push_convo(ConvoBlock::System(said));
}

pub(super) fn restack_failed(app: &mut App, message: String) {
    if app.overlays.restack.is_some() {
        app.overlays.restack = Some(crate::restack_panel::RestackPanel::Said(message));
    } else {
        app.push_convo(ConvoBlock::System(format!("[stack] {message}")));
    }
}

/// Remembers the agent's calls that push or open a pull request, to link the session once they
/// finish and the workspace names the pull request.
pub(super) fn note_pull_request_pushes(
    app: &mut App,
    content: &[luxe_core::message::ContentBlock],
) {
    for block in content {
        if let luxe_core::message::ContentBlock::ToolCall { id, name, arguments } = block {
            if luxe_core::github::refresh::touch(name, arguments)
                == luxe_core::github::refresh::Touch::Started
            {
                app.pull_request_pushes.insert(id.clone());
            }
        }
    }
}

/// Links the session to the branch's pull request once a push the agent made has finished and the
/// workspace names one: it may take a snapshot or two, since a pull request opened by that push
/// is only on GitHub once it is done.
pub(super) fn link_pushed_pull_request(app: &mut App) {
    if !app.pull_request_link_pending {
        return;
    }
    let Some(luxe_core::workspace::PullRequestPresence::Found(pr)) =
        app.workspace.as_ref().map(|workspace| &workspace.pull_request)
    else {
        return;
    };
    let action = luxe_core::Action::LinkPullRequest { number: pr.number, url: pr.url.clone() };
    if app.send_quietly(action, "link the pull request") {
        app.pull_request_link_pending = false;
    }
}

/// The details on screen that an answer about `number` on `repository` is for: `/pr`'s card for
/// the checkout's own, and the `/github` row it names.
fn details_answering<'a>(
    app: &'a mut crate::app::App,
    repository: Option<&str>,
    number: u64,
) -> Vec<&'a mut crate::github_detail::GithubDetail> {
    use crate::github_detail::CardState;
    let mut found = Vec::new();
    if let Some(card) = app.overlays.pull_request_card.as_mut().filter(|card| card.number == number)
    {
        if let (None, CardState::Ready(detail)) = (repository, &mut card.state) {
            found.push(detail.as_mut());
        }
    }
    if let Some(panel) = app.overlays.github.as_mut() {
        let here = panel.repository.clone();
        let url = panel
            .sections
            .iter()
            .flatten()
            .flat_map(|section| &section.items)
            .find(|item| {
                item.number == number
                    && match repository {
                        Some(slug) => item.repository.eq_ignore_ascii_case(slug),
                        None => here
                            .as_deref()
                            .is_none_or(|here| here.eq_ignore_ascii_case(&item.repository)),
                    }
            })
            .map(|item| item.url.clone());
        if let Some(detail) = url.and_then(|url| panel.details.get_mut(&url)) {
            found.push(detail);
        }
    }
    found
}
