//! Prompt snapshot — the system prompt a session sends, written once.
//!
//! Every provider that caches matches an exact prefix, and the system prompt heads every request.
//! So it is assembled on a session's first turn and sent byte for byte after: an `AGENTS.md` or
//! skill edit reaches the next session (`/new`, or resuming one), and what does change mid-session
//! reaches the model appended to the conversation instead — the resume briefing once, and a note
//! when the checkout moves to another branch.

use super::*;
use crate::context::MeasuredSection;

/// The system prompt a session sends, and the one fact in it that can go stale.
pub(crate) struct PromptSnapshot {
    sections: Vec<MeasuredSection>,
    text: String,
    shared_len: usize,
    /// The branch the prompt names
    branch: Option<String>,
}

impl PromptSnapshot {
    pub(crate) fn new(sections: Vec<MeasuredSection>, branch: Option<String>) -> Self {
        let text = crate::context::join_sections(&sections);
        let shared_len = crate::context::shared_len(&sections);
        Self { sections, text, shared_len, branch }
    }
}

impl Engine {
    fn snapshot(&self) -> std::sync::MutexGuard<'_, Option<PromptSnapshot>> {
        self.prompt_snapshot.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The session's system prompt, written from the files on its first use and sent as is after.
    pub(crate) async fn system_prompt(&self) -> Result<String, EngineError> {
        if let Some(snapshot) = self.snapshot().as_ref() {
            return Ok(snapshot.text.clone());
        }
        let (sections, branch) = self.assemble_prompt().await?;
        let snapshot = PromptSnapshot::new(sections, branch);
        let text = snapshot.text.clone();
        *self.snapshot() = Some(snapshot);
        Ok(text)
    }

    /// [`crate::provider::CompletionRequest::shared_system_len`] of the session's prompt.
    pub(crate) fn shared_prompt_len(&self) -> usize {
        self.snapshot().as_ref().map_or(0, |snapshot| snapshot.shared_len)
    }

    /// The sections the session sends, or would send on its first turn: what `/context` measures.
    pub(crate) async fn measure_prompt(&self) -> Result<Vec<MeasuredSection>, EngineError> {
        if let Some(snapshot) = self.snapshot().as_ref() {
            return Ok(snapshot.sections.clone());
        }
        Ok(self.assemble_prompt().await?.0)
    }

    /// Lets the next turn write the prompt afresh, from the files as they are then.
    pub(crate) fn forget_prompt(&self) {
        *self.snapshot() = None;
    }

    /// A note for the next message when the checkout is on another branch than the one the model
    /// last heard about.
    ///
    /// What it last heard is read off the conversation it is sent — the newest note on it, else the
    /// prompt — rather than remembered, so a note that a rewind or a compaction took away is told
    /// again. Before the prompt is written there is nothing stale to correct but an old note.
    async fn branch_note(&self) -> Option<String> {
        let now = super::prompt::current_branch()?;
        let heard = match self.branch_last_noted().await {
            Some(noted) => Some(noted),
            None => self.snapshot().as_ref()?.branch.clone(),
        };
        (heard.as_deref() != Some(now.as_str())).then(|| branch_note(&now))
    }

    /// The branch the newest note on the conversation the model is sent names.
    ///
    /// Only past a message's first block, which is the reader's whatever it starts with.
    async fn branch_last_noted(&self) -> Option<String> {
        let leaf = match self.resolve_leaf().await {
            Ok(leaf) => leaf?,
            Err(error) => {
                warn!(%error, "could not read which branch the model last heard of");
                return None;
            }
        };
        let sid = self.current_session_id().await;
        let entries = match self.sessions().active_path(&sid, &leaf) {
            Ok(entries) => entries,
            Err(error) => {
                warn!(%error, "could not read which branch the model last heard of");
                return None;
            }
        };
        let messages = crate::compaction::context_messages(&entries);
        let users = messages.iter().rev().filter(|message| message.role == Role::User);
        users.flat_map(|message| message.content.iter().skip(1).rev()).find_map(|block| match block
        {
            ContentBlock::Text(text) => noted_branch(text),
            _ => None,
        })
    }

    /// What the next user message carries for the model alone, in the order it is added: an
    /// issue's brief, the mode it is in, the resume briefing, and the branch it moved to.
    pub(crate) async fn one_time_context(&self) -> Vec<ContentBlock> {
        let brief = self.pending_brief.lock().await.take();
        let mode = self.take_mode_note().await;
        let resuming =
            self.resume_briefing_pending.swap(false, std::sync::atomic::Ordering::Relaxed);
        let briefing = match resuming {
            true => self.build_resume_briefing(&self.current_session_id().await.0).await,
            false => None,
        };
        let branch = self.branch_note().await;
        [brief, mode, briefing, branch].into_iter().flatten().map(ContentBlock::Text).collect()
    }
}

/// What follows the branch in a note, which is also where reading one back stops: a branch name
/// may hold a `]` of its own.
const BRANCH_NOTE_REST: &str = "] The checkout is now on branch";

/// The note saying the checkout is now on `branch`.
fn branch_note(branch: &str) -> String {
    format!(
        "{}{branch}{BRANCH_NOTE_REST} `{branch}`; the branch your instructions name is out of date.",
        crate::briefs::BRANCH_NOTE
    )
}

/// The branch a note names, when `text` is one.
fn noted_branch(text: &str) -> Option<String> {
    let rest = text.strip_prefix(crate::briefs::BRANCH_NOTE)?;
    rest.split_once(BRANCH_NOTE_REST).map(|(branch, _)| branch.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A note names the branch it was written for, and reads back as it, slashes and all; a
    /// block that is not a note names none.
    #[test]
    fn a_branch_note_reads_back_as_its_branch() {
        let note = branch_note("fix/limits");
        assert!(note.contains("`fix/limits`"), "{note}");
        assert_eq!(noted_branch(&note).as_deref(), Some("fix/limits"));
        assert_eq!(noted_branch(&branch_note("fix]x")).as_deref(), Some("fix]x"));
        assert_eq!(noted_branch("[mode: plan] Still in plan."), None);
        let message = [ContentBlock::Text("hi".into()), ContentBlock::Text(note)];
        assert_eq!(crate::briefs::readers_words(&message).len(), 1, "hidden from the reader");
    }
}
