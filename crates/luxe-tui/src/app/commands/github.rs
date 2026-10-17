//! GitHub commands — `/pr`'s card, and what a status row's key sets going.
//!
//! The card and the detail beside `/github` both answer a row's key with a [`CheckAction`]; this is
//! where each one becomes the work it names, through the machinery that already does that work.

use super::super::App;
use luxe_core::engine::Action;
use luxe_core::engine::PullRequestWrite;
use luxe_core::github::overview::{CheckAction, PullRequestOverview};
use luxe_core::github::PullRequestStatus;
use luxe_core::guided_review::ReviewTarget;

/// Opens `/pr`'s card on the branch's pull request and asks for its overview.
pub(in crate::app) fn open_pull_request_card(app: &mut App, status: PullRequestStatus) {
    let number = status.number;
    app.overlays.pull_request_card =
        Some(crate::github_detail::PullRequestCard::reading(number, Some(status)));
    read_overview(app, None, number, true);
}

/// Asks the engine for a pull request's or issue's overview; the answer lands as
/// `GithubOverviewRead`.
pub(in crate::app) fn read_overview(
    app: &mut App,
    repository: Option<String>,
    number: u64,
    pull_request: bool,
) {
    let asked = Action::ReadGithubOverview { repository, number, pull_request };
    app.send_action(asked, "read a pull request overview");
}

/// Asks the engine for everything said about a pull request or issue; the answer lands as
/// `GithubConversationRead`.
pub(in crate::app) fn read_conversation(
    app: &mut App,
    repository: Option<String>,
    number: u64,
    pull_request: bool,
) {
    let asked = Action::ReadGithubConversation { repository, number, pull_request };
    app.send_action(asked, "read a pull request conversation");
}

/// Sends a reply or comment the reader confirmed; the answer lands as `GithubCommentPosted`.
pub(in crate::app) fn post_comment(app: &mut App, post: luxe_core::engine::GithubPost) {
    let thread = post.thread.clone();
    if !app.send_action(Action::PostGithubComment(post), "post a comment") {
        // Said where the reader is looking, and the card stays open to try again
        let reason = "The engine is busy — try posting again in a moment.".to_string();
        if let Some(detail) = composing_detail(app) {
            detail.posted(thread.as_deref(), Some(reason));
        }
    }
}

/// Asks the session's agent to draft a reply; the answer lands as `GithubReplyDrafted`.
///
/// A thread on the branch's own pull request may have become a comment in Guided Review, and
/// what the review made of it — whether the reader verified the fix, what the agent reported doing
/// — is evidence only this side holds, so it travels with the request.
pub(in crate::app) fn draft_reply(
    app: &mut App,
    repository: Option<String>,
    number: u64,
    pull_request: bool,
    ask: crate::github_detail::DraftAsk,
) {
    let finding = finding_for(app, &ask.target);
    let request = luxe_core::engine::DraftRequest {
        request_id: ask.request_id,
        repository,
        number,
        pull_request,
        target: ask.target,
        conversation: ask.conversation,
        finding,
        note: ask.note,
        current: ask.current,
    };
    app.send_action(Action::DraftGithubReply(request), "draft a reply");
}

/// The review comment a thread became, from the open feedback session, and where it stands.
fn finding_for(
    app: &App,
    target: &luxe_core::engine::DraftTarget,
) -> Option<luxe_core::engine::FindingEvidence> {
    use luxe_core::guided_review::{FindingMessageAuthor, FindingState};
    let luxe_core::engine::DraftTarget::Thread(thread) = target else { return None };
    let session = app.review.session()?;
    let finding = session.findings.iter().find(|finding| {
        finding.github_thread.as_ref().is_some_and(|linked| linked.id == thread.id)
    })?;
    let remediation = finding
        .thread
        .iter()
        .rev()
        .find(|message| message.author == FindingMessageAuthor::Agent)
        .map(|message| message.body.clone());
    Some(luxe_core::engine::FindingEvidence {
        verified: finding.state == FindingState::Resolved,
        remediation,
    })
}

/// Hands the draft being read to the reader's editor; what comes back replaces it.
pub(in crate::app) fn edit_draft_externally(app: &mut App, text: String) {
    app.outbox.text_edit =
        Some(crate::launcher::TextEdit { text, target: crate::launcher::EditTarget::GithubDraft });
}

/// The detail whose draft card is open: `/pr`'s, or the `/github` row's under the cursor.
pub(in crate::app) fn composing_detail(
    app: &mut App,
) -> Option<&mut crate::github_detail::GithubDetail> {
    use crate::github_detail::CardState;
    if let Some(card) = app.overlays.pull_request_card.as_mut() {
        if let CardState::Ready(detail) = &mut card.state {
            return Some(detail.as_mut());
        }
    }
    let panel = app.overlays.github.as_mut()?;
    let url = panel.rows().get(panel.selected).map(|item| item.url.clone())?;
    panel.details.get_mut(&url)
}

/// Reads pull request `number`'s overview again wherever it is shown, after a change to it landed.
pub(in crate::app) fn reread_overview(app: &mut App, repository: Option<String>, number: u64) {
    let card = app.overlays.pull_request_card.as_ref().is_some_and(|card| card.number == number);
    if card && repository.is_none() {
        read_overview(app, None, number, true);
        return;
    }
    let Some(panel) = app.overlays.github.as_mut() else { return };
    let here = panel.repository.clone();
    let url = panel
        .sections
        .iter()
        .flatten()
        .flat_map(|section| &section.items)
        .find(|item| {
            item.number == number
                && match repository.as_deref() {
                    Some(slug) => item.repository.eq_ignore_ascii_case(slug),
                    None => here
                        .as_deref()
                        .is_none_or(|here| here.eq_ignore_ascii_case(&item.repository)),
                }
        })
        .map(|item| item.url.clone());
    if let Some(url) = url {
        panel.asked.insert(url);
        read_overview(app, repository, number, true);
    }
}

/// Reads the card's overview again when the branch's pull request moved since it last did.
pub(in crate::app) fn refresh_pull_request_card(app: &mut App) {
    let Some(luxe_core::workspace::PullRequestPresence::Found(status)) =
        app.workspace.as_ref().map(|workspace| workspace.pull_request.clone())
    else {
        return;
    };
    let Some(card) = app.overlays.pull_request_card.as_mut() else { return };
    if !card.stale(&status) {
        return;
    }
    card.seen = Some(status);
    let number = card.number;
    read_overview(app, None, number, true);
}

/// Does what a status row on `pr` offers.
///
/// Only the actions [`crate::github_detail::GithubDetail::acts`] allows reach here, since a row
/// shows no key for any other. Work done in this checkout leaves for where it is done — the
/// composer, Guided Review — and a change asked of GitHub stays where it was asked, saying what
/// became of it when GitHub answers.
pub(in crate::app) fn act_on_pull_request(
    app: &mut App,
    repository: Option<String>,
    pr: &PullRequestOverview,
    action: CheckAction,
) {
    let number = pr.number;
    match action {
        // The threads become findings in a review of the working tree against the base, so a fix
        // lands where it is verified; the link is what lets the session find its way back
        CheckAction::ResolveThreads => {
            close_surfaces(app);
            let link = Action::LinkPullRequest { number, url: pr.url.clone() };
            app.send_quietly(link, "link the pull request");
            super::start_guided_review(app, ReviewTarget::OwnPullRequest { number });
        }
        CheckAction::FixChecks => {
            close_surfaces(app);
            ask_agent(app, &fix_checks_request(pr));
        }
        CheckAction::ResolveConflicts => {
            close_surfaces(app);
            ask_agent(app, &conflicts_request(pr));
        }
        // In a stack, the branch moves with the branches above it, which is what `/stack` is for
        CheckAction::UpdateBranch if pr.stack.is_some() => {
            close_surfaces(app);
            if app.send_action(
                Action::PlanRestack { request_id: luxe_core::restack::RestackId::new() },
                "plan a restack",
            ) {
                app.overlays.restack = Some(crate::restack_panel::RestackPanel::Planning);
            }
        }
        CheckAction::UpdateBranch => {
            let write = PullRequestWrite::UpdateBranch { head_oid: pr.head_oid.clone() };
            change(app, repository, number, write);
            app.status.flash_toast(format!("Asking GitHub to bring #{number} up to date…"));
        }
        CheckAction::MarkReady => {
            change(app, repository, number, PullRequestWrite::MarkReady);
            app.status.flash_toast(format!("Marking #{number} ready for review…"));
        }
        CheckAction::OpenBelow(below) => {
            app.overlays.github = None;
            app.overlays.pull_request_card =
                Some(crate::github_detail::PullRequestCard::other(below));
            read_overview(app, repository, below, true);
        }
        // The confirmation sends these, through `confirm_change`
        CheckAction::Merge | CheckAction::EnableAutoMerge => {}
        CheckAction::DisableAutoMerge => {
            let write = PullRequestWrite::DisableAutoMerge { node_id: pr.node_id.clone() };
            change(app, repository, number, write);
            app.status.flash_toast(format!("Turning auto-merge off for #{number}…"));
        }
    }
}

/// Makes the change a confirmation was left with — a merge, or auto-merge turned on; the
/// confirmation stays open saying so until GitHub answers.
pub(in crate::app) fn confirm_change(
    app: &mut App,
    repository: Option<String>,
    number: u64,
    write: PullRequestWrite,
) {
    change(app, repository, number, write);
}

fn change(app: &mut App, repository: Option<String>, number: u64, write: PullRequestWrite) {
    app.send_action(
        Action::ChangePullRequest { repository, number, write },
        "change a pull request",
    );
}

/// Puts `request` in the composer for the reader to read, add to and send: work in this checkout
/// is the agent's, but what it is asked to do is still the reader's to say.
fn ask_agent(app: &mut App, request: &str) {
    if !app.editor.current_input().trim().is_empty() {
        app.editor.insert_text("\n\n");
    }
    app.editor.insert_text(request);
    app.status.flash_toast("Written in the composer — enter sends it to the agent".to_string());
}

fn close_surfaces(app: &mut App) {
    app.overlays.github = None;
    app.overlays.pull_request_card = None;
}

/// What the agent is asked when checks fail: which ones, where to read why, and to check its fix.
fn fix_checks_request(pr: &PullRequestOverview) -> String {
    let names: Vec<String> = pr.failing.iter().map(|check| format!("`{}`", check.name)).collect();
    let which = match names.as_slice() {
        [] => String::new(),
        names => format!(" ({})", names.join(", ")),
    };
    format!(
        "Fix the failing checks on pull request #{}{which}. Read what failed with the git tool's \
         `checks` action, fix it in this checkout, and run what covers it before telling me what \
         you changed.",
        pr.number
    )
}

/// What the agent is asked when the branch conflicts with its base: merge it in, never rewrite.
fn conflicts_request(pr: &PullRequestOverview) -> String {
    format!(
        "Pull request #{} conflicts with `{base}`. Bring `{base}` into this branch with a merge — no \
         rebase and no force-push — resolve the conflicts, run the tests, and tell me what you \
         resolved and why.",
        pr.number,
        base = pr.base_ref
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The agent is told which checks failed and where to read why; a conflict is merged, never
    /// rebased, since the branch is published.
    #[test]
    fn what_the_agent_is_asked_names_the_work() {
        let pr = crate::fixture::pull_request_overview();
        let fix = fix_checks_request(&pr);
        assert!(
            fix.starts_with("Fix the failing checks on pull request #142 (`test (ubuntu-latest)`)")
        );
        assert!(fix.contains("`checks` action"));
        let conflicts = conflicts_request(&pr);
        assert!(conflicts.contains("conflicts with `main`") && conflicts.contains("no rebase"));
    }

    /// A thread on your own pull request that became a review comment carries what the review made
    /// of it; another thread carries nothing.
    #[test]
    fn a_thread_that_became_a_review_comment_carries_its_evidence() {
        use crate::fixture::{seeded_app, FixtureScenario};
        let app = seeded_app("nord", FixtureScenario::GuidedReviewOwnPullRequest).expect("fixture");
        let linked = app
            .review
            .session()
            .and_then(|session| session.findings.iter().find(|f| f.github_thread.is_some()))
            .expect("a finding from a thread")
            .clone();
        let thread = |id: &str| {
            luxe_core::engine::DraftTarget::Thread(luxe_core::github::ReviewThread {
                id: id.into(),
                path: String::new(),
                line: None,
                start_line: None,
                side: luxe_core::github::ThreadSide::Right,
                outdated: false,
                resolved: false,
                file_level: false,
                comments: Vec::new(),
                total_comments: 0,
                anchor_text: None,
                can_reply: true,
                can_resolve: true,
            })
        };
        let id = linked.github_thread.as_ref().map(|t| t.id.clone()).unwrap_or_default();
        let evidence = finding_for(&app, &thread(&id)).expect("evidence");
        assert_eq!(
            evidence.verified,
            linked.state == luxe_core::guided_review::FindingState::Resolved
        );
        assert!(finding_for(&app, &thread("somewhere-else")).is_none());
    }
}
