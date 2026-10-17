//! GitHub panel keys — the list's own keys, and the detail's answers turned into what the panel
//! asks of the app, for the pull request or issue on screen.

use super::*;

impl GithubPanel {
    /// A key while the detail has focus, answered for the item its row names; a detail that no
    /// longer matches its row hands focus back and takes nothing.
    pub(super) fn detail_key_outcome(&mut self, key: &KeyEvent) -> GithubKey {
        let target = self.selected_target();
        // the detail and its row are one pull request; one that disagrees takes no key at all,
        // rather than acting and then having its request dropped
        let agreed = self.detail().zip(target).filter(|(d, (n, _))| d.subject.number() == *n);
        let Some((_, (_, repository))) = agreed else {
            self.focus = Focus::List;
            return GithubKey::Held;
        };
        let Some(detail) = self.detail_mut() else {
            self.focus = Focus::List;
            return GithubKey::Held;
        };
        let answer = detail.on_key(key);
        if answer == DetailKey::Leave {
            self.focus = Focus::List;
            return GithubKey::Held;
        }
        detail_request(&detail.subject, repository, answer)
    }

    /// A key while the list has focus: tabs, the cursor, the search field, and the row's actions.
    pub(super) fn list_key_outcome(&mut self, key: &KeyEvent) -> GithubKey {
        if self.editing.is_some() {
            return self.on_editing_key(key);
        }
        let count = self.sections.as_ref().map_or(0, Vec::len);
        let rows = self.rows().len();
        match key.code {
            KeyCode::Esc => return GithubKey::Close,
            KeyCode::Char('r') => return GithubKey::Refresh,
            KeyCode::Char('/') if count > 0 => self.start_editing(),
            KeyCode::Char('s') if count > 0 => return self.cycle_state(),
            KeyCode::Char('x') if self.edited(self.section) => return self.reset(),
            KeyCode::Tab if count > 0 => {
                self.section = (self.section + 1) % count;
                self.selected = 0;
            }
            KeyCode::BackTab if count > 0 => {
                self.section = self.section.checked_sub(1).unwrap_or(count - 1);
                self.selected = 0;
            }
            KeyCode::Right if self.detail().is_some() => self.focus = Focus::Detail,
            KeyCode::Down if rows > 0 => {
                self.selected = (self.selected + 1).min(rows - 1);
                return self.next_page_if_due();
            }
            KeyCode::PageDown if rows > 0 => {
                self.selected = (self.selected + PAGE_ROWS).min(rows - 1);
                return self.next_page_if_due();
            }
            KeyCode::End if rows > 0 => {
                self.selected = rows - 1;
                return self.next_page_if_due();
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(PAGE_ROWS),
            KeyCode::Home => self.selected = 0,
            KeyCode::Enter | KeyCode::Char('c') | KeyCode::Char('o') => {
                return self.row_action(key.code);
            }
            _ => {}
        }
        GithubKey::Held
    }

    /// What Enter, `c` and `o` do with the row under the cursor.
    fn row_action(&self, code: KeyCode) -> GithubKey {
        let Some(item) = self.rows().get(self.selected).copied().cloned() else {
            return GithubKey::Held;
        };
        let elsewhere = self.elsewhere(&item);
        match code {
            KeyCode::Char('o') => GithubKey::Open(item.url),
            KeyCode::Char('c') => GithubKey::Refer(Box::new(item)),
            _ if item.pull_request => GithubKey::Review {
                number: item.number,
                repository: elsewhere.then(|| item.repository.clone()),
            },
            _ if elsewhere => GithubKey::Refused(format!(
                "#{} is on {}; a session on it starts from a checkout of {}.",
                item.number, item.repository, item.repository
            )),
            // Your own session on it, when one took it up, before a second one
            _ => match self.details.get(&item.url).and_then(|d| d.local.as_ref()) {
                Some(local) => GithubKey::Resume(local.session.clone()),
                None => GithubKey::TakeUpIssue(item.number),
            },
        }
    }
}

/// What a detail's answer asks of the app, for `subject` in `repository`.
fn detail_request(
    subject: &crate::github_detail::Subject,
    repository: Option<String>,
    answer: DetailKey,
) -> GithubKey {
    match answer {
        DetailKey::Held | DetailKey::Leave => GithubKey::Held,
        DetailKey::Open(url) => GithubKey::Open(url),
        DetailKey::Act(action) => match subject {
            crate::github_detail::Subject::PullRequest(pr) => {
                GithubKey::Act { repository, overview: pr.clone(), action }
            }
            crate::github_detail::Subject::Issue(_) => GithubKey::Held,
        },
        DetailKey::Merge(write) => GithubKey::Merge { repository, number: subject.number(), write },
        DetailKey::Resume(session) => GithubKey::Resume(session),
        DetailKey::ReadConversation => {
            let number = subject.number();
            let pull_request = matches!(subject, crate::github_detail::Subject::PullRequest(_));
            GithubKey::ReadConversation { repository, number, pull_request }
        }
        DetailKey::Post(request) => GithubKey::Post(luxe_core::engine::GithubPost {
            repository,
            number: subject.number(),
            thread: request.thread,
            answering: request.answering,
            body: request.body,
        }),
        DetailKey::EditExternally(text) => GithubKey::EditExternally(text),
        DetailKey::Draft(ask) => {
            let number = subject.number();
            let pull_request = matches!(subject, crate::github_detail::Subject::PullRequest(_));
            GithubKey::Draft { repository, number, pull_request, ask }
        }
    }
}
