//! Replies — writing in the Conversation, and what comes back from GitHub about it.
//!
//! `r` on an entry opens the draft card on it: a reply in the thread when it is a thread, else a
//! new comment answering it. A draft kept with Esc stays under what it answers, and Enter on it
//! opens it again. The conversation itself is read when the tab is first opened and read again
//! after anything posted, so what you sent is there when the card closes.

use crossterm::event::KeyEvent;
use luxe_core::github::overview::ConversationEntry;

use super::{DetailKey, Draft, DraftAsk, DraftCard, DraftKey, GithubDetail, PostRequest, Stop};

impl GithubDetail {
    /// A key for the draft card, and what it asks of whoever holds the detail.
    pub(super) fn on_draft_key(&mut self, key: &KeyEvent) -> DetailKey {
        let Some(card) = self.composing.as_mut() else {
            return DetailKey::Held;
        };
        match card.on_key(key) {
            DraftKey::Held => DetailKey::Held,
            // Put away while the agent writes it: it is kept, and lands under what it answers
            DraftKey::Close if card.drafting.is_some() => {
                if let Some(card) = self.composing.take() {
                    self.waiting.push(card);
                }
                DetailKey::Held
            }
            DraftKey::Close => {
                self.composing = None;
                DetailKey::Held
            }
            DraftKey::Redraft(note) => {
                let Some(target) = card.target.clone() else {
                    return DetailKey::Held;
                };
                let current = (!card.draft.body.trim().is_empty()).then(|| card.draft.body.clone());
                let request_id = next_request_id(self.subject.number());
                card.drafting = Some(request_id.clone());
                card.error = None;
                DetailKey::Draft(DraftAsk {
                    request_id,
                    target,
                    conversation: self.conversation.clone().unwrap_or_default(),
                    note,
                    current,
                })
            }
            DraftKey::Keep => {
                if let Some(card) = self.composing.take() {
                    self.keep(card.draft);
                }
                DetailKey::Held
            }
            DraftKey::Post => DetailKey::Post(PostRequest {
                thread: card.draft.thread.clone(),
                answering: card.answering.as_ref().map(|(_, said)| said.clone()),
                body: card.draft.body.clone(),
            }),
            DraftKey::EditExternally => DetailKey::EditExternally(card.draft.body.clone()),
        }
    }

    /// Keeps `draft` under what it answers, in place of any draft already answering the same.
    fn keep(&mut self, draft: Draft) {
        self.drafts.retain(|kept| kept.thread != draft.thread || kept.target != draft.target);
        self.drafts.push(draft);
    }

    /// Opens a reply in your own words on the entry under the cursor.
    pub(super) fn reply(&mut self) {
        let Some(Stop::Entry(index)) = self.conversation_stops().get(self.cursor).copied() else {
            return;
        };
        let Some(entry) = self.conversation.as_ref().and_then(|entries| entries.get(index)) else {
            return;
        };
        let (draft, answering) = reply_to(entry, self.subject.number());
        let target = draft_target(entry);
        // A reply already begun on the same thing picks up where it was left
        let kept = self
            .drafts
            .iter()
            .position(|kept| kept.thread == draft.thread && kept.target == draft.target);
        let mut card = match kept {
            Some(at) => DraftCard::reading(self.drafts.remove(at), answering),
            None => DraftCard::writing(draft, answering),
        };
        card.target = target;
        self.composing = Some(card);
    }

    /// Asks the agent to draft a reply to the entry under the cursor, opening the card on it.
    pub(super) fn draft_reply(&mut self) -> Option<DraftAsk> {
        let Some(Stop::Entry(index)) = self.conversation_stops().get(self.cursor).copied() else {
            return None;
        };
        let entry = self.conversation.as_ref()?.get(index)?.clone();
        let target = draft_target(&entry)?;
        let (mut draft, answering) = reply_to(&entry, self.subject.number());
        draft.by_agent = true;
        let request_id = next_request_id(self.subject.number());
        self.composing =
            Some(DraftCard::drafting(draft, answering, target.clone(), request_id.clone()));
        Some(DraftAsk {
            request_id,
            target,
            conversation: self.conversation.clone().unwrap_or_default(),
            note: None,
            current: None,
        })
    }

    /// Lands a draft the agent wrote under `request_id`, answering whether it was this detail's: in
    /// its open card, or kept under what it answers when its card was put away.
    ///
    /// Args: `(request_id, body, sources, error, note)`.
    pub fn drafted(
        &mut self,
        request_id: &str,
        body: Option<String>,
        sources: Vec<String>,
        error: Option<String>,
        note: Option<String>,
    ) -> bool {
        if let Some(card) =
            self.composing.as_mut().filter(|card| card.drafting.as_deref() == Some(request_id))
        {
            card.drafted(body, sources, error, note);
            return true;
        }
        let Some(at) =
            self.waiting.iter().position(|card| card.drafting.as_deref() == Some(request_id))
        else {
            return false;
        };
        let mut card = self.waiting.remove(at);
        card.drafted(body, sources, error, note);
        if !card.draft.body.trim().is_empty() {
            self.keep(card.draft);
        }
        true
    }

    /// Opens kept draft `index` to read before it is sent.
    pub(super) fn open_draft(&mut self, index: usize) {
        if index >= self.drafts.len() {
            return;
        }
        let draft = self.drafts.remove(index);
        let answering = draft.thread.as_ref().and_then(|thread| {
            self.conversation.iter().flatten().find_map(|entry| match entry {
                ConversationEntry::Thread(found) if found.id == *thread => found
                    .comments
                    .first()
                    .map(|first| (format!("@{}", first.author), first.body.clone())),
                _ => None,
            })
        });
        self.composing = Some(DraftCard::reading(draft, answering));
    }

    /// Lands the conversation, or why it could not be read.
    pub fn land_conversation(
        &mut self,
        entries: Option<Vec<ConversationEntry>>,
        error: Option<String>,
    ) {
        match entries {
            Some(entries) => {
                self.conversation = Some(entries);
                self.conversation_error = None;
            }
            None => {
                self.conversation_error =
                    Some(error.unwrap_or_else(|| "No answer from GitHub".into()));
            }
        }
    }

    /// Lands what became of a post on `thread`, answering whether the conversation should be read
    /// again: it should, once something landed.
    pub fn posted(&mut self, thread: Option<&str>, error: Option<String>) -> bool {
        let Some(card) =
            self.composing.as_mut().filter(|card| card.draft.thread.as_deref() == thread)
        else {
            return false;
        };
        match error {
            Some(reason) => {
                card.posting = false;
                card.error = Some(reason);
                false
            }
            None => {
                self.composing = None;
                self.conversation_asked = false;
                true
            }
        }
    }
}

impl GithubDetail {
    /// Lands what became of a change to this pull request, answering whether the detail said it —
    /// a confirmation shows a refusal itself, and closes when what it sent is `confirmed`.
    pub fn changed(&mut self, confirmed: bool, error: Option<&str>) -> bool {
        let Some(dialog) = self.merging.as_mut().filter(|dialog| dialog.merging) else {
            return false;
        };
        match error {
            Some(reason) => {
                dialog.merging = false;
                dialog.error = Some(reason.to_string());
                true
            }
            None if confirmed => {
                self.merging = None;
                false
            }
            None => false,
        }
    }
}

/// A request id no other draft in this run shares.
fn next_request_id(number: u64) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    format!("draft:{number}:{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

/// What the agent is asked to answer, for an entry it can answer: a thread, a comment, a review.
/// The description is the author's own opening, which a reply comments on as a comment.
fn draft_target(entry: &ConversationEntry) -> Option<luxe_core::engine::DraftTarget> {
    use luxe_core::engine::DraftTarget;
    match entry {
        ConversationEntry::Thread(thread) => Some(DraftTarget::Thread(thread.clone())),
        ConversationEntry::Comment(comment) | ConversationEntry::Review { comment, .. } => {
            Some(DraftTarget::Comment(comment.clone()))
        }
        ConversationEntry::Opening { author, body, .. } => {
            Some(DraftTarget::Comment(luxe_core::github::ConversationComment {
                id: "opening".into(),
                author: author.login.clone(),
                bot: author.bot,
                mine: false,
                body: body.clone(),
                created_at: String::new(),
                url: String::new(),
            }))
        }
    }
}

/// What a reply to `entry` on pull request or issue `number` is: where it goes, and what it
/// answers.
fn reply_to(entry: &ConversationEntry, number: u64) -> (Draft, Option<(String, String)>) {
    let draft = |target: String, thread: Option<String>| Draft {
        target,
        thread,
        body: String::new(),
        by_agent: false,
        sources: Vec::new(),
        note: None,
    };
    let named = |login: &str| format!("@{}", login.trim_end_matches("[bot]"));
    match entry {
        ConversationEntry::Thread(thread) => {
            let first = thread.comments.first();
            let who = first.map(|comment| named(&comment.author)).unwrap_or_default();
            let place = match thread.line {
                Some(line) => format!("{}:{line}", thread.path),
                None => thread.path.clone(),
            };
            (
                draft(format!("Reply to {who} on `{place}`"), Some(thread.id.clone())),
                first.map(|comment| (who.clone(), comment.body.clone())),
            )
        }
        ConversationEntry::Opening { author, body, .. } => {
            let who = named(&author.login);
            (
                draft(format!("Comment on #{number}, answering {who}"), None),
                Some((who, body.clone())),
            )
        }
        ConversationEntry::Comment(comment) | ConversationEntry::Review { comment, .. } => {
            let who = named(&comment.author);
            (
                draft(format!("Comment on #{number}, answering {who}"), None),
                Some((who, comment.body.clone())),
            )
        }
    }
}

#[cfg(test)]
mod tests;
