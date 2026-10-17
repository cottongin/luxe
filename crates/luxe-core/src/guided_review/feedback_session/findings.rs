//! Findings — adding, replying to, resolving, importing and marking the comments of a review.

use super::*;

impl ReviewSession {
    /// Adds one validated draft or imported finding without changing durable revision metadata.
    ///
    /// Assigns the next `R<n>` label when the caller supplies none. Numbers are never reused, so
    /// deleting a draft cannot renumber the comments a reviewer has already talked about.
    pub fn add_finding(
        &mut self,
        mut finding: ReviewFinding,
    ) -> Result<(), ReviewSessionMutationError> {
        finding.validate()?;
        if self.findings.iter().any(|existing| existing.id == finding.id) {
            return Err(ReviewSessionMutationError::DuplicateFindingId);
        }
        if finding.label.trim().is_empty() {
            finding.label = self.next_finding_label();
        }
        self.findings.push(finding);
        Ok(())
    }

    /// The next unused `R<n>` reference for this session.
    pub fn next_finding_label(&self) -> String {
        let highest =
            self.findings.iter().filter_map(ReviewFinding::label_number).max().unwrap_or(0);
        format!("R{}", highest.saturating_add(1))
    }

    /// Fills in labels for findings persisted before they existed, in review order.
    pub fn backfill_finding_labels(&mut self) {
        for index in 0..self.findings.len() {
            if self.findings[index].label.trim().is_empty() {
                let label = self.next_finding_label();
                self.findings[index].label = label;
            }
        }
    }

    /// Appends a reviewer reply to a finding's thread.
    ///
    /// The thread already carried agent remediation notes but had no writer for the human side, so
    /// a reviewer could read "not fixed, because X" and had no way to answer. A reply reopens a
    /// resolved finding: saying something about it means it is live again.
    pub fn append_reviewer_message(
        &mut self,
        finding_id: &FindingId,
        body: String,
        created_at_ms: u64,
    ) -> Result<(), ReviewSessionMutationError> {
        self.push_reviewer_reply(finding_id, body, created_at_ms).map(|_| ())
    }

    /// The one checked append of a reviewer's words: refuses an empty reply, reopens a resolved
    /// finding, and answers the finding for a caller that changes more of it.
    pub(super) fn push_reviewer_reply(
        &mut self,
        finding_id: &FindingId,
        body: String,
        created_at_ms: u64,
    ) -> Result<&mut ReviewFinding, ReviewSessionMutationError> {
        if body.trim().is_empty() {
            return Err(ReviewSessionMutationError::EmptyReply);
        }
        let finding = self
            .findings
            .iter_mut()
            .find(|finding| &finding.id == finding_id)
            .ok_or(ReviewSessionMutationError::FindingNotFound)?;
        finding.thread.push(FindingMessage {
            author: FindingMessageAuthor::Reviewer,
            body,
            created_at_ms,
        });
        if finding.state == FindingState::Resolved {
            finding.state = FindingState::Open;
        }
        Ok(finding)
    }

    /// Marks a finding resolved without altering its immutable evidence or submitted history.
    pub fn resolve_finding(
        &mut self,
        finding_id: &FindingId,
    ) -> Result<(), ReviewSessionMutationError> {
        let finding = self
            .findings
            .iter_mut()
            .find(|finding| &finding.id == finding_id)
            .ok_or(ReviewSessionMutationError::FindingNotFound)?;
        finding.state = FindingState::Resolved;
        Ok(())
    }

    /// Reopens a resolved finding so it remains actionable in the current review.
    pub fn reopen_finding(
        &mut self,
        finding_id: &FindingId,
    ) -> Result<(), ReviewSessionMutationError> {
        let finding = self
            .findings
            .iter_mut()
            .find(|finding| &finding.id == finding_id)
            .ok_or(ReviewSessionMutationError::FindingNotFound)?;
        if finding.state == FindingState::Resolved {
            finding.state = FindingState::Open;
        }
        Ok(())
    }

    /// Reanchors a stale finding at an explicit reviewer-selected current location.
    pub fn reanchor_stale_finding(
        &mut self,
        finding_id: &FindingId,
        anchor: FindingAnchor,
    ) -> Result<(), ReviewSessionMutationError> {
        anchor.validate()?;
        let finding = self
            .findings
            .iter_mut()
            .find(|finding| &finding.id == finding_id)
            .ok_or(ReviewSessionMutationError::FindingNotFound)?;
        if finding.state != FindingState::Stale {
            return Err(ReviewSessionMutationError::FindingNotStale { state: finding.state });
        }
        finding.anchor = anchor;
        finding.state = FindingState::NeedsVerification;
        Ok(())
    }

    /// Records the reviewer's explicit local completion outcome for durable persistence.
    pub fn set_outcome(&mut self, outcome: ReviewOutcome) {
        self.outcome = Some(outcome);
    }

    /// Adds the findings for review threads this session does not have yet, answering how many.
    ///
    /// Matched on the thread's id, so loading and refreshing the same pull request again never
    /// doubles a thread, and one already worked through keeps everything the reader did to it.
    /// A finding that does not validate is passed over rather than failing the rest.
    pub fn adopt_github_threads(&mut self, findings: Vec<ReviewFinding>) -> usize {
        let mut adopted = 0;
        for finding in findings {
            let Some(id) = finding.github_thread.as_ref().map(|thread| thread.id.clone()) else {
                continue;
            };
            let known = self.findings.iter().any(|existing| {
                existing.github_thread.as_ref().is_some_and(|thread| thread.id == id)
            });
            if !known && self.add_finding(finding).is_ok() {
                adopted += 1;
            }
        }
        adopted
    }

    /// Adopts comments written in another review of the same change — reading a pull request, before
    /// it was checked out here — answering how many were added.
    ///
    /// Each is moved onto this review's source by `remap`: an anchor that still exists is kept, one
    /// that moved follows its code, and one with nowhere to go is kept as stale rather than
    /// dropped, since the reader wrote it. A comment already here is passed over, and a label this
    /// session already uses is given the next free one.
    pub fn adopt_carried_findings(
        &mut self,
        findings: Vec<ReviewFinding>,
        mut remap: impl FnMut(&FindingAnchor) -> FindingAnchorRefresh,
    ) -> usize {
        let mut adopted = 0;
        for mut finding in findings {
            if self.findings.iter().any(|existing| existing.id == finding.id) {
                continue;
            }
            match remap(&finding.anchor) {
                FindingAnchorRefresh::Exact => {}
                FindingAnchorRefresh::NeedsVerification(anchor) => finding.anchor = anchor,
                FindingAnchorRefresh::Stale => finding.state = FindingState::Stale,
            }
            if self.findings.iter().any(|existing| existing.label == finding.label) {
                finding.label = String::new();
            }
            if self.add_finding(finding).is_ok() {
                adopted += 1;
            }
        }
        adopted
    }

    /// Records what answering a thread did on GitHub.
    pub fn mark_thread_answered(
        &mut self,
        finding_id: &FindingId,
        reply_url: Option<String>,
        resolved: bool,
    ) {
        let thread = self
            .findings
            .iter_mut()
            .find(|finding| &finding.id == finding_id)
            .and_then(|finding| finding.github_thread.as_mut());
        if let Some(thread) = thread {
            if reply_url.is_some() {
                thread.reply_url = reply_url;
            }
            thread.resolved |= resolved;
        }
    }

    /// Marks the comments one GitHub review carried as posted, so a later post leaves them out.
    ///
    /// Posting already happened, so an id no longer in the session is passed over rather than
    /// refused: refusing would lose the marks on every comment that is still here.
    pub fn mark_posted(&mut self, finding_ids: &[FindingId], posted: &PostedComment) {
        for finding in &mut self.findings {
            if finding_ids.contains(&finding.id) {
                finding.posted = Some(posted.clone());
            }
        }
    }

    /// The comments not sent anywhere yet: written or open, and never posted to GitHub.
    pub fn unsent_finding_ids(&self) -> Vec<FindingId> {
        self.findings
            .iter()
            .filter(|finding| {
                matches!(finding.state, FindingState::Draft | FindingState::Open)
                    && finding.posted.is_none()
            })
            .map(|finding| finding.id.clone())
            .collect()
    }

    /// Updates mutable text for a local draft while retaining its immutable evidence and history.
    pub fn update_draft_finding(
        &mut self,
        finding_id: &FindingId,
        severity: FindingSeverity,
        title: String,
        body: String,
        proposed_fix: Option<String>,
    ) -> Result<(), ReviewSessionMutationError> {
        let finding = self
            .findings
            .iter_mut()
            .find(|finding| &finding.id == finding_id)
            .ok_or(ReviewSessionMutationError::FindingNotFound)?;
        if finding.state != FindingState::Draft {
            return Err(ReviewSessionMutationError::FindingNotDraft { state: finding.state });
        }
        if self.submissions.iter().any(|submission| {
            submission.findings.iter().any(|submitted| &submitted.finding_id == finding_id)
        }) {
            return Err(ReviewSessionMutationError::FindingAlreadySubmitted);
        }
        finding.severity = severity;
        finding.title = title;
        finding.body = body;
        finding.proposed_fix = proposed_fix;
        finding.validate()?;
        Ok(())
    }

    /// Deletes a local draft that has never been captured in immutable submission history.
    pub fn delete_draft_finding(
        &mut self,
        finding_id: &FindingId,
    ) -> Result<ReviewFinding, ReviewSessionMutationError> {
        let position = self
            .findings
            .iter()
            .position(|finding| &finding.id == finding_id)
            .ok_or(ReviewSessionMutationError::FindingNotFound)?;
        let finding = &self.findings[position];
        if finding.state != FindingState::Draft {
            return Err(ReviewSessionMutationError::FindingNotDraft { state: finding.state });
        }
        if self.submissions.iter().any(|submission| {
            submission.findings.iter().any(|submitted| &submitted.finding_id == finding_id)
        }) {
            return Err(ReviewSessionMutationError::FindingAlreadySubmitted);
        }
        Ok(self.findings.remove(position))
    }
}
