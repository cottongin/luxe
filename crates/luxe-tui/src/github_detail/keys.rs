//! Detail keys — the hint for the open tab, and what each key does on it.

use super::{
    action_hint, merge, overview, pull_request_url, CheckAction, DetailKey, DetailTab,
    GithubDetail, MergeDialog, Stop, Subject,
};

impl GithubDetail {
    /// The key hint for the open tab. `leave` says where Esc goes: `← list` beside the list,
    /// `Esc close` on its own.
    pub fn hint(&self, leave: &str) -> String {
        let parts: Vec<String> = match self.tab {
            DetailTab::Overview => {
                let acting = self.overview_stops().get(self.cursor).copied().flatten();
                let enter = acting.map(|action| format!("enter {}", action_hint(action).1));
                enter
                    .into_iter()
                    .chain(["tab conversation", "↑/↓ move", "o open"].map(String::from))
                    .collect()
            }
            DetailTab::Conversation => {
                let enter = match self.conversation_stops().get(self.cursor) {
                    Some(Stop::Entry(index)) if self.open.contains(index) => Some("enter collapse"),
                    Some(Stop::Entry(_)) => Some("enter expand"),
                    Some(Stop::Draft(_)) => Some("enter post · e edit"),
                    None => None,
                };
                let bots = if self.hide_bots { "b show bots" } else { "b hide bots" };
                enter
                    .into_iter()
                    .chain(["r reply", "d draft a reply", bots, "tab overview"])
                    .map(String::from)
                    .collect()
            }
        };
        let mut hint = parts.join(" · ");
        hint.push_str(&format!(" · {leave}"));
        hint
    }

    /// What `key` does while the detail has the keys.
    pub fn on_key(&mut self, key: &crossterm::event::KeyEvent) -> DetailKey {
        use crossterm::event::KeyCode;
        if self.merging.is_some() {
            return match merge::on_key(self, key) {
                Some(write) => DetailKey::Merge(write),
                None => DetailKey::Held,
            };
        }
        if self.composing.is_some() {
            return self.on_draft_key(key);
        }
        let stops = match self.tab {
            DetailTab::Overview => self.overview_stops().len(),
            DetailTab::Conversation => self.conversation_stops().len(),
        };
        match key.code {
            KeyCode::Esc | KeyCode::Left => return DetailKey::Leave,
            KeyCode::Tab | KeyCode::BackTab => {
                if let Some(key) = self.switch_tab() {
                    return key;
                }
            }
            KeyCode::Char('r') if self.tab == DetailTab::Conversation => self.reply(),
            KeyCode::Char('d') if self.tab == DetailTab::Conversation => {
                if let Some(ask) = self.draft_reply() {
                    return DetailKey::Draft(ask);
                }
            }
            KeyCode::Down if stops > 0 => self.cursor = (self.cursor + 1).min(stops - 1),
            KeyCode::Up => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Char('b') if self.tab == DetailTab::Conversation => {
                self.hide_bots = !self.hide_bots;
                self.cursor = 0;
            }
            KeyCode::Enter if self.tab == DetailTab::Overview => {
                if let Some(key) = self.enter_overview_row() {
                    return key;
                }
            }
            // A row's own letter acts on it from anywhere in the Overview; merging has none
            KeyCode::Char(letter) if self.tab == DetailTab::Overview => {
                let wanted = self.overview_stops().into_iter().flatten().find(|action| {
                    action_hint(*action).0 == letter.to_string() && *action != CheckAction::Merge
                });
                if let Some(action) = wanted {
                    return DetailKey::Act(action);
                }
                if letter == 'o' {
                    return DetailKey::Open(self.url().to_owned());
                }
            }
            KeyCode::Enter if self.tab == DetailTab::Conversation => {
                match self.conversation_stops().get(self.cursor) {
                    Some(Stop::Entry(index)) => {
                        if !self.open.remove(index) {
                            self.open.insert(*index);
                        }
                    }
                    Some(Stop::Draft(index)) => self.open_draft(*index),
                    None => {}
                }
            }
            KeyCode::Char('o') => return DetailKey::Open(self.url().to_owned()),
            _ => {}
        }
        DetailKey::Held
    }

    /// Enter on an Overview row: opens the merge dialog, acts on the row, or follows it.
    fn enter_overview_row(&mut self) -> Option<DetailKey> {
        let acting = self.overview_stops().get(self.cursor).copied().flatten();
        match (acting, &self.subject) {
            (Some(CheckAction::Merge), Subject::PullRequest(pr)) => {
                self.merging = Some(MergeDialog::new(pr));
            }
            (Some(CheckAction::EnableAutoMerge), Subject::PullRequest(pr)) => {
                self.merging = Some(MergeDialog::auto(pr));
            }
            (Some(action), _) => return Some(DetailKey::Act(action)),
            // An issue's rows: each pull request that would close it, then your session
            (None, Subject::Issue(issue)) => {
                let closing = overview::closing_pull_requests(issue);
                if let Some(linked) = closing.get(self.cursor) {
                    return Some(DetailKey::Open(pull_request_url(&issue.url, linked.number)));
                }
                if let Some(local) = &self.local {
                    return Some(DetailKey::Resume(local.session.clone()));
                }
            }
            (None, _) => {}
        }
        None
    }

    /// Flips between the tabs; the first look at the conversation is what reads it.
    fn switch_tab(&mut self) -> Option<DetailKey> {
        self.tab = match self.tab {
            DetailTab::Overview => DetailTab::Conversation,
            DetailTab::Conversation => DetailTab::Overview,
        };
        self.cursor = 0;
        // Read only when looked at: most details are glanced at on their Overview
        if self.tab == DetailTab::Conversation && !self.conversation_asked {
            self.conversation_asked = true;
            return Some(DetailKey::ReadConversation);
        }
        None
    }
}
