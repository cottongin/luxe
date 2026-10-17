//! Answers — what a question's turn said, put on the questions it was asked about.
//!
//! A fix comes back as a remediation report naming each comment; a question carries no report, so
//! the turn's final reply is the answer, recorded on every question in the submission. That is what
//! lets the card show it, a fix request carry it, and the question stop "waiting on the agent".

use super::{
    FindingMessage, FindingMessageAuthor, FindingSeverity, ReviewSession,
    ReviewSessionMutationError, SubmissionId,
};

impl ReviewSession {
    /// Records `answer` as the agent's reply on each question `submission_id` sent.
    pub fn record_answer(
        &mut self,
        submission_id: &SubmissionId,
        answer: &str,
        created_at_ms: u64,
    ) -> Result<(), ReviewSessionMutationError> {
        let asked: Vec<_> = self
            .submissions
            .iter()
            .find(|submission| &submission.id == submission_id)
            .ok_or(ReviewSessionMutationError::SubmissionNotFound)?
            .findings
            .iter()
            .map(|finding| finding.finding_id.clone())
            .collect();
        for finding in self.findings.iter_mut().filter(|finding| {
            asked.contains(&finding.id) && finding.severity == FindingSeverity::Question
        }) {
            finding.thread.push(FindingMessage {
                author: FindingMessageAuthor::Agent,
                body: answer.to_string(),
                created_at_ms,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::guided_review::{
        FindingAnchor, FindingId, FindingMessageAuthor, FindingSeverity, FindingState, ReviewBase,
        ReviewFeedbackMode, ReviewFinding, ReviewId, ReviewRevisionId, ReviewSessionMutationError,
        ReviewSource, ReviewTarget, SubmissionId,
    };

    fn finding(severity: FindingSeverity) -> ReviewFinding {
        ReviewFinding {
            posted: None,
            github_thread: None,
            id: FindingId::new(),
            label: String::new(),
            anchor: FindingAnchor::Review,
            severity,
            title: "Why?".into(),
            body: "Why take one more?".into(),
            proposed_fix: None,
            state: FindingState::Draft,
            thread: Vec::new(),
        }
    }

    /// The answer lands on the submission's questions, and on nothing else.
    #[test]
    fn an_answer_lands_on_the_questions_it_answers() {
        let source = ReviewSource {
            base: ReviewBase::EmptyTree,
            content_digest: ReviewRevisionId::from_canonical_bytes(b"diff"),
            includes_staged: false,
            includes_unstaged: true,
            included_untracked_files: 0,
            captured_at_ms: 1,
            uncovered_paths: Vec::new(),
        };
        let mut session =
            super::ReviewSession::new(ReviewId::new(), ReviewTarget::WorkingTree, source, 1);
        let (asked, other) =
            (finding(FindingSeverity::Question), finding(FindingSeverity::Question));
        let ids = [asked.id.clone()];
        session.add_finding(asked).unwrap();
        session.add_finding(other).unwrap();
        let submission = SubmissionId::new();
        session
            .queue_submission(submission.clone(), ReviewFeedbackMode::Question, &ids, &[], 2)
            .unwrap();

        session.record_answer(&submission, "To give trim_end a character.", 3).unwrap();
        let thread = |index: usize| &session.findings[index].thread;
        assert_eq!(
            thread(0).last().map(|message| message.author),
            Some(FindingMessageAuthor::Agent)
        );
        assert_eq!(
            thread(0).last().map(|message| message.body.as_str()),
            Some("To give trim_end a character.")
        );
        assert!(thread(1).is_empty(), "a question nobody sent is not answered");
        assert_eq!(
            session.record_answer(&SubmissionId::new(), "x", 4),
            Err(ReviewSessionMutationError::SubmissionNotFound)
        );
    }
}
