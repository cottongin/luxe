//! `/github` and `/pr` seeded for screenshot review, on the `acme/notify` repository the other
//! GitHub fixtures use and a fixed clock, so no capture's ages drift between runs.
//!
//! `LUXE_FIXTURE_OVERLAY` picks the state: `github` (the list, your pull request's Overview
//! beside it), `github-detail` (the same, the keys in the detail), `github-conversation`,
//! `github-draft` (an agent's reply open to read), `github-issue` (an issue you have a session
//! on), `github-pulls` (every open pull request, more than one page of them), `github-search` (a
//! search being typed over them), `github-merged` (that tab stepped to merged), `pr` (your pull
//! request on its own, with nothing left in the way), and `pr-merge` (its merge confirmation
//! open).

mod conversation;
mod overview;
mod rows;

use luxe_core::github::overview::{ConversationEntry, PullRequestOverview, RowFacts};
use luxe_core::github::ListItem;

use crate::app::App;
use crate::github_detail::{DetailTab, GithubDetail, Subject};
use crate::github_panel::{Focus, GithubPanel};
use conversation::{agent_draft, conversation, JON_THREAD};
use overview::{issue, own_pull_request, ready_pull_request};
#[cfg(test)]
use rows::stamp;
#[cfg(test)]
use rows::HOUR;
use rows::{
    all_issues, facts, issue_url, issues, listed, merged, mine, pull_requests, review_requested,
    url, DAY, NOW,
};

/// Your own pull request as the fixtures draw it: one check failing, changes requested, two threads
/// open. A starting point for a test that needs an overview and cares about a field or two.
pub fn pull_request_overview() -> PullRequestOverview {
    own_pull_request()
}

/// Seeds the state `which` names over whatever scenario is under it.
pub fn seed_github(app: &mut App, which: &str) {
    if which == "pr" || which == "pr-merge" {
        let ready = ready_pull_request();
        let mut detail = GithubDetail::new(Subject::PullRequest(Box::new(ready.clone())));
        detail.clock = Some(NOW);
        detail.on_branch = true;
        if which == "pr-merge" {
            detail.merging = Some(crate::github_detail::MergeDialog::new(&ready));
        }
        app.overlays.pull_request_card = Some(crate::github_detail::PullRequestCard {
            number: ready.number,
            state: crate::github_detail::CardState::Ready(Box::new(detail)),
            seen: None,
            branch: true,
        });
        return;
    }
    let mut panel = panel();
    match which {
        "github-detail" => panel.focus = Focus::Detail,
        "github-conversation" | "github-draft" => {
            panel.focus = Focus::Detail;
            if let Some(detail) = panel.details.get_mut(&url(142)) {
                detail.tab = DetailTab::Conversation;
                // On jon's open thread, opened, with the agent's draft under it
                detail.cursor = 4;
                detail.open.insert(4);
            }
            if which == "github-draft" {
                if let Some(detail) = panel.details.get_mut(&url(142)) {
                    detail.drafts.clear();
                    let mut card = crate::github_detail::DraftCard::reading(
                        agent_draft(),
                        Some(("jon-avery".into(), JON_THREAD.into())),
                    );
                    // What it answers, as an agent's draft always knows: `d` redrafts it
                    card.target = conversation().into_iter().find_map(|entry| match entry {
                        ConversationEntry::Thread(thread) if thread.id == "jon-router" => {
                            Some(luxe_core::engine::DraftTarget::Thread(thread))
                        }
                        _ => None,
                    });
                    detail.composing = Some(card);
                }
            }
        }
        "github-issue" => {
            panel.section = 2;
        }
        "github-pulls" => {
            panel.section = 3;
        }
        "github-search" => {
            panel.section = 3;
            let mut editor = crate::editor::Editor::new();
            editor.insert_text("is:open is:pr sort:updated-desc digest");
            panel.editing = Some(editor);
        }
        "github-merged" => {
            use luxe_core::github::query;
            panel.section = 3;
            let search = &mut panel.searches[3];
            search.query = query::with_state(&search.query, query::State::Merged);
            if let Some(section) = panel.sections.as_mut().and_then(|s| s.get_mut(3)) {
                let facts: std::collections::HashMap<String, RowFacts> =
                    facts().into_iter().collect();
                section.items = merged()
                    .into_iter()
                    .map(|item| ListItem { facts: facts.get(&item.url).cloned(), ..item })
                    .collect();
                section.total = Some(212);
            }
        }
        _ => {}
    }
    app.overlays.github = Some(panel);
}

fn panel() -> GithubPanel {
    let mut panel = GithubPanel {
        repository: Some("acme/notify".into()),
        sections: Some(vec![
            listed("Review requested", review_requested(), None),
            listed("Mine", mine(), None),
            listed("Assigned issues", issues(), None),
            listed("Pull requests", pull_requests(), Some(47)),
            listed("Issues", all_issues(), Some(23)),
        ]),
        section: 1,
        clock: Some(NOW),
        ..GithubPanel::new(&luxe_core::config::GithubConfig::default().sections())
    };
    // Each row's second line, as the search brings it
    let facts: std::collections::HashMap<String, RowFacts> = facts().into_iter().collect();
    for section in panel.sections.iter_mut().flatten() {
        for item in &mut section.items {
            item.facts = facts.get(&item.url).cloned();
        }
    }
    let mut mine = GithubDetail::new(Subject::PullRequest(Box::new(own_pull_request())));
    mine.conversation = Some(conversation());
    mine.drafts.push(agent_draft());
    panel.details.insert(url(142), mine);
    let mut issue = GithubDetail::new(Subject::Issue(Box::new(issue())));
    issue.local = Some(luxe_core::github::overview::LocalSession {
        session: "6f1c2e0a-318".into(),
        branch: Some("318-phantom-category".into()),
        last_used: NOW - 2 * DAY,
    });
    panel.details.insert(issue_url(318), issue);
    panel
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stamp_reads_back_as_the_seconds_it_was_made_from() {
        for seconds in [0, 951_868_800, NOW, NOW - 2 * DAY - 7 * HOUR - 13] {
            assert_eq!(luxe_core::github::rfc3339_seconds(&stamp(seconds)), Some(seconds));
        }
    }
}
