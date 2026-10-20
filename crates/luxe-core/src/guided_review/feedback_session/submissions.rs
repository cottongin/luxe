//! Submissions — the immutable snapshots of comments sent to the agent, and their dispatch.

use super::*;

impl ReviewSession {
    /// Queues immutable snapshots of selected findings for exactly one later agent dispatch.
    pub fn queue_submission(
        &mut self,
        submission_id: SubmissionId,
        mode: ReviewFeedbackMode,
        finding_ids: &[FindingId],
        files: &[ReviewFile],
        created_at_ms: u64,
    ) -> Result<ReviewSubmission, ReviewSessionMutationError> {
        if finding_ids.is_empty() {
            return Err(ReviewSessionMutationError::EmptySubmission);
        }
        if self.submissions.iter().any(|submission| submission.id == submission_id) {
            return Err(ReviewSessionMutationError::DuplicateSubmissionId);
        }
        let findings = finding_ids
            .iter()
            .map(|finding_id| {
                self.findings
                    .iter()
                    .find(|finding| &finding.id == finding_id)
                    .map(|finding| SubmittedFinding {
                        finding_id: finding.id.clone(),
                        label: finding.label.clone(),
                        anchor: finding.anchor.clone(),
                        // Resolved now, against the revision under review: an agent handed only
                        // `file:<sha256>` cannot find the code, and a later refresh makes the IDs
                        // unresolvable even in principle.
                        evidence: FindingEvidence::from_anchor(&finding.anchor, files),
                        severity: finding.severity,
                        title: finding.title.clone(),
                        body: finding.body.clone(),
                        proposed_fix: finding.proposed_fix.clone(),
                        thread: finding.thread.clone(),
                        github_author: finding
                            .github_thread
                            .as_ref()
                            .map(|thread| thread.author.clone()),
                    })
                    .ok_or(ReviewSessionMutationError::FindingNotFound)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let submission = ReviewSubmission {
            id: submission_id,
            revision_id: self.source.content_digest.clone(),
            mode,
            findings,
            dispatch: SubmissionDispatch::Queued,
            created_at_ms,
        };
        submission.validate()?;
        self.submissions.push(submission.clone());
        for finding in &mut self.findings {
            if finding.state == FindingState::Draft
                && finding_ids.iter().any(|finding_id| finding_id == &finding.id)
            {
                finding.state = FindingState::Open;
            }
        }
        Ok(submission)
    }

    /// Marks a queued or retryable submission as accepted at an agent turn boundary.
    pub fn dispatch_submission(
        &mut self,
        submission_id: &SubmissionId,
    ) -> Result<(), ReviewSessionMutationError> {
        let submission = self.submission_mut(submission_id)?;
        match submission.dispatch {
            SubmissionDispatch::Queued | SubmissionDispatch::FailedRetryable => {
                submission.dispatch = SubmissionDispatch::Dispatched;
                Ok(())
            }
            state => Err(ReviewSessionMutationError::SubmissionNotDispatchable { state }),
        }
    }

    /// Makes a dispatched submission available for a same-ID retry after a failed turn.
    pub fn fail_submission(
        &mut self,
        submission_id: &SubmissionId,
    ) -> Result<(), ReviewSessionMutationError> {
        let submission = self.submission_mut(submission_id)?;
        if submission.dispatch != SubmissionDispatch::Dispatched {
            return Err(ReviewSessionMutationError::SubmissionNotDispatched {
                state: submission.dispatch,
            });
        }
        submission.dispatch = SubmissionDispatch::FailedRetryable;
        Ok(())
    }

    /// Marks a dispatched submission complete after its resulting agent turn ends.
    pub fn complete_submission(
        &mut self,
        submission_id: &SubmissionId,
    ) -> Result<(), ReviewSessionMutationError> {
        let submission = self.submission_mut(submission_id)?;
        if submission.dispatch != SubmissionDispatch::Dispatched {
            return Err(ReviewSessionMutationError::SubmissionNotDispatched {
                state: submission.dispatch,
            });
        }
        submission.dispatch = SubmissionDispatch::Completed;
        Ok(())
    }

    /// Removes a submission before the engine accepts it, retaining the review findings.
    pub fn cancel_queued_submission(
        &mut self,
        submission_id: &SubmissionId,
    ) -> Result<ReviewSubmission, ReviewSessionMutationError> {
        let index = self
            .submissions
            .iter()
            .position(|submission| &submission.id == submission_id)
            .ok_or(ReviewSessionMutationError::SubmissionNotFound)?;
        if self.submissions[index].dispatch != SubmissionDispatch::Queued {
            return Err(ReviewSessionMutationError::SubmissionNotQueued {
                state: self.submissions[index].dispatch,
            });
        }
        Ok(self.submissions.remove(index))
    }

    fn submission_mut(
        &mut self,
        submission_id: &SubmissionId,
    ) -> Result<&mut ReviewSubmission, ReviewSessionMutationError> {
        self.submissions
            .iter_mut()
            .find(|submission| &submission.id == submission_id)
            .ok_or(ReviewSessionMutationError::SubmissionNotFound)
    }

    /// Appends validated agent evidence without treating an agent claim as reviewer resolution.
    pub fn record_remediation_report(
        &mut self,
        report: &RemediationReport,
        created_at_ms: u64,
    ) -> Result<(), ReviewSessionMutationError> {
        if report.review_id != self.id {
            return Err(ReviewSessionMutationError::RemediationReportWrongReview);
        }
        let submitted = self.submissions.iter().any(|submission| {
            submission.revision_id == report.revision_id
                && report.findings.iter().all(|result| {
                    submission
                        .findings
                        .iter()
                        .any(|finding| finding.finding_id == result.finding_id)
                })
        });
        if !submitted {
            return Err(ReviewSessionMutationError::RemediationReportNotSubmitted);
        }
        for result in &report.findings {
            let finding = self
                .findings
                .iter_mut()
                .find(|finding| finding.id == result.finding_id)
                .ok_or(ReviewSessionMutationError::FindingNotFound)?;
            let status = match result.status {
                RemediationStatus::Fixed => "fixed",
                RemediationStatus::NotFixed => "not fixed",
                RemediationStatus::NeedsClarification => "needs clarification",
            };
            let body = result.note.as_ref().map_or_else(
                || format!("Agent reported: {status}."),
                |note| format!("Agent reported: {status}. {note}"),
            );
            finding.thread.push(FindingMessage {
                author: FindingMessageAuthor::Agent,
                body,
                created_at_ms,
            });
            if result.status == RemediationStatus::Fixed && finding.state != FindingState::Resolved
            {
                finding.state = FindingState::NeedsVerification;
            }
        }
        Ok(())
    }

    /// Notes, in luxe's own name, that a fix turn ended with no word from the agent on these
    /// comments, and why.
    ///
    /// Without it a comment the agent was asked to fix looks exactly as if it had been ignored. The
    /// state stays where it was, since only the reviewer moves a comment, and a comment no longer
    /// in the session is passed over, as in [`Self::mark_posted`].
    ///
    /// Args: `(submission_id, finding_ids, reason, created_at_ms)` — `reason` finishes the
    /// sentence "No word from the agent on this comment: …", so it reads as a clause.
    pub fn note_unreported(
        &mut self,
        submission_id: &SubmissionId,
        finding_ids: &[FindingId],
        reason: &str,
        created_at_ms: u64,
    ) -> Result<(), ReviewSessionMutationError> {
        if !self.submissions.iter().any(|submission| &submission.id == submission_id) {
            return Err(ReviewSessionMutationError::SubmissionNotFound);
        }
        let body = format!(
            "No word from the agent on this comment: {reason}. Refresh the review to see the code \
             as it is now."
        );
        for finding in self.findings.iter_mut().filter(|finding| finding_ids.contains(&finding.id))
        {
            finding.thread.push(FindingMessage {
                author: FindingMessageAuthor::System,
                body: body.clone(),
                created_at_ms,
            });
        }
        Ok(())
    }
}

/// Submission mode for structured agent feedback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewFeedbackMode {
    /// Ask the active agent to remediate actionable findings.
    RequestFixes,
    /// Send findings as comments without requesting edits.
    CommentOnly,
    /// Ask the agent to answer, explicitly forbidding edits.
    Question,
}

impl ReviewFeedbackMode {
    /// What sending findings of these severities asks for: the strongest intent among them.
    ///
    /// Any comment that asks for work makes it a request for fixes; otherwise a question makes it
    /// a question; otherwise it is comments. Each finding still travels with its own severity, so
    /// nothing is reinterpreted on the way out.
    pub fn asked_by(severities: impl IntoIterator<Item = FindingSeverity>) -> Self {
        let mut asks_a_question = false;
        for severity in severities {
            match severity {
                FindingSeverity::Blocking
                | FindingSeverity::Issue
                | FindingSeverity::Suggestion => {
                    return Self::RequestFixes;
                }
                FindingSeverity::Question => asks_a_question = true,
                FindingSeverity::Note => {}
            }
        }
        if asks_a_question {
            Self::Question
        } else {
            Self::CommentOnly
        }
    }
}

/// Dispatch lifecycle for one unique persisted submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmissionDispatch {
    /// Persisted before the engine accepts it at a turn boundary.
    Queued,
    /// The engine has accepted the durable submission ID.
    Dispatched,
    /// Provider or runtime failure allows a same-ID retry.
    FailedRetryable,
    /// The resulting agent turn has ended.
    Completed,
}

/// Immutable copy of a finding sent with one agent submission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmittedFinding {
    /// Original durable finding identity.
    pub finding_id: FindingId,
    /// Stable short reference shown to the reviewer and sent to the agent.
    #[serde(default)]
    pub label: String,
    /// Exact anchor at submission time.
    pub anchor: FindingAnchor,
    /// Path/line/symbol/source resolved against the reviewed revision.
    #[serde(default)]
    pub evidence: FindingEvidence,
    /// Severity at submission time.
    pub severity: FindingSeverity,
    /// Title at submission time.
    pub title: String,
    /// Body at submission time.
    pub body: String,
    /// Proposed narrow remediation at submission time.
    pub proposed_fix: Option<String>,
    /// The conversation so far, so a re-sent finding carries the reviewer's replies with it.
    #[serde(default)]
    pub thread: Vec<FindingMessage>,
    /// Who opened the GitHub review thread this came from, when it did: its words are theirs,
    /// and the agent is told so
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_author: Option<String>,
}

impl SubmittedFinding {
    pub(super) fn validate(&self) -> Result<(), ReviewSessionValidationError> {
        if self.title.trim().is_empty() {
            return Err(ReviewSessionValidationError::EmptyFindingTitle);
        }
        if self.body.trim().is_empty() {
            return Err(ReviewSessionValidationError::EmptyFindingBody);
        }
        self.anchor.validate()
    }
}

/// Immutable agent-feedback record for one pinned source revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewSubmission {
    /// Unique durable submission identity used for idempotent dispatch.
    pub id: SubmissionId,
    /// Exact immutable source revision reviewed by this submission.
    pub revision_id: ReviewRevisionId,
    /// Requested agent behavior.
    pub mode: ReviewFeedbackMode,
    /// Immutable submitted finding snapshots.
    pub findings: Vec<SubmittedFinding>,
    /// Delivery state retained across restart.
    pub dispatch: SubmissionDispatch,
    /// Creation time in milliseconds since Unix epoch.
    pub created_at_ms: u64,
}

impl ReviewSubmission {
    pub(super) fn validate(&self) -> Result<(), ReviewSessionValidationError> {
        ensure_unique(
            self.findings.iter().map(|finding| &finding.finding_id),
            "submitted finding",
        )?;
        for finding in &self.findings {
            finding.validate()?;
        }
        Ok(())
    }
}
