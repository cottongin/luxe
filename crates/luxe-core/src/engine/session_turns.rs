//! Session turns — where a turn starts from, and rewinding for an edit, a retry or a regenerate.

use super::*;

impl Engine {
    /// Resolve the current leaf, lazily seeding it from the session's last entry.
    ///
    /// For a fresh session that is the header; for a resumed one it is the real tail, so
    /// turns chain onto the existing conversation.
    pub(crate) async fn resolve_leaf(&self) -> Result<Option<EntryId>, EngineError> {
        let mut guard = self.current_leaf.lock().await;
        if guard.is_some() {
            return Ok(guard.clone());
        }
        let sid = self.current_session_id().await;
        let entries = self.sessions().load(&sid)?;
        let leaf = crate::session::latest_leaf(&entries);
        guard.clone_from(&leaf);
        Ok(leaf)
    }

    /// Record the newest committed entry as the leaf future turns chain onto.
    pub(crate) async fn set_leaf(&self, id: EntryId) {
        *self.current_leaf.lock().await = Some(id);
    }

    /// Readies the session for an edit of its last user message (`EditResend`): the next turn
    /// hangs where that message did, off the entry before it.
    ///
    /// So the model never sees the message the edit replaces or the reply to it, and the original
    /// stays in the session as a branch of its own. Appending the edit after the reply instead put
    /// both versions in front of the model, which then answered the pair.
    pub(crate) async fn rewind_for_edit(&self) -> Result<(), EngineError> {
        let leaf = self.resolve_leaf().await?;
        let last = self.last_user_message(leaf, "edit").await?;
        self.rewind_to(&last).await
    }

    /// Readies the session to send its last user message again (Retry, "ask again"), returning
    /// the message.
    ///
    /// After a reply, the question goes in again after it, so the model has its earlier answer to
    /// do better than. With no reply to it, which is how a failed or interrupted turn ends, it is
    /// asked again in place: sending it after itself would put the same question in front of the
    /// model twice in a row.
    pub(crate) async fn prepare_retry(&self) -> Result<String, EngineError> {
        let leaf = self.resolve_leaf().await?;
        let last = self.last_user_message(leaf, "retry").await?;
        if !last.answered {
            self.rewind_to(&last).await?;
        }
        Ok(last.text)
    }

    /// The most recent user message on the active path.
    ///
    /// `purpose` names the caller in the error a transcript with no user message produces, so each
    /// of `EditResend`, `Retry` and `Regenerate` still says which one had nothing to work with.
    async fn last_user_message(
        &self,
        leaf: Option<EntryId>,
        purpose: &str,
    ) -> Result<LastUserMessage, EngineError> {
        let sid = self.current_session_id().await;
        let from = leaf.unwrap_or(EntryId(String::new()));
        let entries = self.sessions().active_path(&sid, &from)?;
        let position =
            entries.iter().rposition(|entry| has_role(entry, Role::User)).ok_or_else(|| {
                EngineError::Session(crate::session::SessionError::Other(format!(
                    "no user message to {purpose}"
                )))
            })?;
        let entry = &entries[position];
        let EntryKind::Message(message) = &entry.kind else {
            // `rposition` already matched on this, so the shape cannot change underneath us.
            unreachable!("the entry just matched as a user message")
        };
        Ok(LastUserMessage {
            parent: entry.parent_id.clone(),
            // What the reader wrote, without what rode along for the model on the first try
            text: crate::briefs::readers_words(&message.content)
                .into_iter()
                .map(|(_, text)| text)
                .collect::<Vec<_>>()
                .join("\n"),
            answered: entries[position + 1..].iter().any(|entry| has_role(entry, Role::Assistant)),
            briefed: message.content.iter().skip(1).any(|block| {
                matches!(block, ContentBlock::Text(text) if text.starts_with(crate::briefs::RESUME_BRIEFING))
            }),
        })
    }

    /// Makes the entry `last` hangs off the leaf the next turn chains onto, so what follows it
    /// becomes a branch — and arms the resume briefing again if `last` carried it, or the
    /// message sent in its place would go without.
    async fn rewind_to(&self, last: &LastUserMessage) -> Result<(), EngineError> {
        // Every session opens with a header, so a user message always hangs off something
        let parent = last.parent.clone().ok_or_else(|| {
            EngineError::Session(crate::session::SessionError::Other(
                "the last user message has nothing to branch from".to_string(),
            ))
        })?;
        // The warmer is keeping a request whose tail is about to stop being the conversation
        self.cache_warmer.stop();
        self.set_leaf(parent).await;
        if last.briefed {
            self.resume_briefing_pending.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(())
    }

    /// Readies the session to answer its last user message afresh (Regenerate), returning the
    /// message.
    ///
    /// The question is re-asked in place, off the entry before it, so the reply it replaces is not
    /// carried forward.
    pub(crate) async fn prepare_regenerate(&self) -> Result<String, EngineError> {
        let leaf = self.resolve_leaf().await?;
        let last = self.last_user_message(leaf, "regenerate").await?;
        self.rewind_to(&last).await?;
        Ok(last.text)
    }
}

/// Whether `entry` is a message from `role`.
fn has_role(entry: &crate::session::Entry, role: Role) -> bool {
    matches!(&entry.kind, EntryKind::Message(message) if message.role == role)
}

/// The last user message on the active path, as `EditResend`, `Retry` and `Regenerate` need it.
struct LastUserMessage {
    /// The entry it hangs off, which is where a replacement hangs too
    parent: Option<EntryId>,
    text: String,
    /// Whether a reply to it is on the path
    answered: bool,
    /// Whether it carried the resume briefing
    briefed: bool,
}
