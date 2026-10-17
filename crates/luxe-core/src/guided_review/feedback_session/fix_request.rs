//! Fix requests — a question whose answer says the code should change, turned into a comment.
//!
//! A question is held to reading: its turn cannot edit, which is the point of asking. So when the
//! answer comes back as "this is a bug", the next thing the reviewer wants is the fix, and the
//! question is where every word of it already is — what was asked, what the agent found, and now
//! what to do. Turning the question into an issue keeps that thread and sends it whole, rather than
//! making the reviewer restate it in a fresh comment at the same line.

use super::{FindingId, FindingSeverity, ReviewSession, ReviewSessionMutationError};

impl ReviewSession {
    /// Appends the reviewer's direction to a finding and, when it was a question, makes it an
    /// issue, so the next submission asks for the fix and carries the thread that led to it.
    ///
    /// Any other finding takes the reply as [`Self::append_reviewer_message`] would: it already
    /// asks for work.
    pub fn ask_for_fix(
        &mut self,
        finding_id: &FindingId,
        body: String,
        created_at_ms: u64,
    ) -> Result<(), ReviewSessionMutationError> {
        let finding = self.push_reviewer_reply(finding_id, body, created_at_ms)?;
        if finding.severity == FindingSeverity::Question {
            finding.severity = FindingSeverity::Issue;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::{FindingMessage, FindingMessageAuthor, FindingState};
    use super::*;
    use crate::guided_review::{
        FindingAnchor, ReviewBase, ReviewFinding, ReviewId, ReviewRevisionId, ReviewSource,
        ReviewTarget,
    };

    fn session_with(severity: FindingSeverity, state: FindingState) -> (ReviewSession, FindingId) {
        let source = ReviewSource {
            base: ReviewBase::EmptyTree,
            content_digest: ReviewRevisionId::from_canonical_bytes(b"diff"),
            includes_staged: false,
            includes_unstaged: true,
            included_untracked_files: 0,
            captured_at_ms: 1,
            uncovered_paths: Vec::new(),
        };
        let mut session = ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source, 1);
        let id = FindingId::new();
        session.findings.push(ReviewFinding {
            posted: None,
            github_thread: None,
            id: id.clone(),
            label: "R1".into(),
            anchor: FindingAnchor::Review,
            severity,
            title: "Why twice?".into(),
            body: "Why is the fallback decided twice?".into(),
            proposed_fix: None,
            state,
            thread: vec![FindingMessage {
                author: FindingMessageAuthor::Agent,
                body: "It is a bug: the router repeats the scheduler's decision.".into(),
                created_at_ms: 2,
            }],
        });
        (session, id)
    }

    /// The question becomes an issue, the direction joins the answer it follows, and a resolved
    /// one is live again.
    #[test]
    fn a_question_asked_for_a_fix_becomes_an_issue_with_its_thread() {
        let (mut session, id) = session_with(FindingSeverity::Question, FindingState::Resolved);
        session.ask_for_fix(&id, "Fix it in the router.".into(), 3).unwrap();
        let finding = &session.findings[0];
        assert_eq!(finding.severity, FindingSeverity::Issue);
        assert_eq!(finding.state, FindingState::Open);
        let thread: Vec<_> = finding.thread.iter().map(|message| message.body.as_str()).collect();
        assert_eq!(
            thread,
            ["It is a bug: the router repeats the scheduler's decision.", "Fix it in the router."]
        );
        assert_eq!(finding.thread[1].author, FindingMessageAuthor::Reviewer);
    }

    /// Anything that already asks for work keeps its severity, and an empty direction is refused.
    /// Only asking for a fix promotes a question: a plain reply goes on the thread, reopens it,
    /// and leaves the question a question.
    #[test]
    fn a_plain_reply_leaves_a_question_a_question() {
        let (mut session, id) = session_with(FindingSeverity::Question, FindingState::Resolved);
        session.append_reviewer_message(&id, "Why here?".into(), 3).unwrap();
        let finding = session.findings.iter().find(|finding| finding.id == id).unwrap();
        assert_eq!(finding.severity, FindingSeverity::Question);
        assert_eq!(finding.state, FindingState::Open);
        assert_eq!(finding.thread.last().map(|m| m.body.as_str()), Some("Why here?"));
    }

    #[test]
    fn a_comment_keeps_its_severity_and_an_empty_direction_is_refused() {
        let (mut session, id) = session_with(FindingSeverity::Suggestion, FindingState::Open);
        session.ask_for_fix(&id, "Go ahead.".into(), 3).unwrap();
        assert_eq!(session.findings[0].severity, FindingSeverity::Suggestion);
        assert_eq!(
            session.ask_for_fix(&id, "  ".into(), 4),
            Err(ReviewSessionMutationError::EmptyReply)
        );
        assert_eq!(
            session.ask_for_fix(&FindingId::new(), "x".into(), 5),
            Err(ReviewSessionMutationError::FindingNotFound)
        );
    }
}
