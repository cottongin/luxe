//! Review findings — the feedback window and a finding's details, drafts and their anchors.

use super::*;

impl GuidedReviewWorkspace {
    /// Toggles the local inspector for persisted review findings.
    pub(crate) fn toggle_findings(&mut self) {
        self.findings_visible = !self.findings_visible;
        if self.findings_visible {
            self.help_visible = false;
        } else {
            self.finding_details_visible = false;
        }
    }

    /// Closes the feedback window and the details and fix views drawn over it, which answer to its
    /// keys and would be left with none.
    pub(crate) fn close_feedback_window(&mut self) {
        self.findings_visible = false;
        self.finding_details_visible = false;
        self.finding_fix_visible = false;
    }

    /// Toggles the selected finding's immutable evidence summary.
    pub(crate) fn toggle_finding_details(&mut self) {
        self.finding_details_visible = !self.finding_details_visible;
    }

    /// Toggles the before/after view of what changed at the selected comment's anchor.
    pub(crate) fn toggle_finding_fix(&mut self) {
        self.finding_fix_visible = !self.finding_fix_visible;
        if self.finding_fix_visible {
            self.finding_details_visible = false;
        }
    }

    /// Moves the persisted-finding selection through its stable triage order.
    pub(crate) fn move_finding_selection(
        &mut self,
        findings: &[ReviewFinding],
        forward: bool,
    ) -> bool {
        self.feedback_scroll = 0;
        let findings = ordered_findings(findings);
        let Some(_first) = findings.first() else {
            return false;
        };
        let next = self
            .selected_finding_id
            .as_ref()
            .and_then(|selected| findings.iter().position(|finding| &finding.id == selected))
            .map_or_else(
                || if forward { 0 } else { findings.len().saturating_sub(1) },
                |index| {
                    if forward {
                        (index + 1) % findings.len()
                    } else {
                        index.checked_sub(1).unwrap_or(findings.len().saturating_sub(1))
                    }
                },
            );
        self.selected_finding_id = Some(findings[next].id.clone());
        true
    }

    /// Selects a persisted finding's immutable anchor when it remains in the pinned review.
    pub(crate) fn jump_to_finding(
        &mut self,
        finding: &ReviewFinding,
        review: &GuidedReview,
    ) -> bool {
        match &finding.anchor {
            FindingAnchor::Line { file_id, hunk_id, side, line, .. } => {
                let Some(file) = review.files.iter().find(|file| &file.id == file_id) else {
                    return false;
                };
                let Some((hunk_index, hunk)) =
                    file.hunks.iter().enumerate().find(|(_, hunk)| &hunk.id == hunk_id)
                else {
                    return false;
                };
                let Some(line_index) = hunk.lines.iter().position(|source| match side {
                    DiffSide::Old => source.old_line == Some(*line),
                    DiffSide::New => source.new_line == Some(*line),
                }) else {
                    return false;
                };
                self.select_finding_hunk(file, hunk_index, Some(line_index));
                true
            }
            FindingAnchor::Hunk { file_id, hunk_id } => {
                let Some(file) = review.files.iter().find(|file| &file.id == file_id) else {
                    return false;
                };
                let Some(hunk_index) = file.hunks.iter().position(|hunk| &hunk.id == hunk_id)
                else {
                    return false;
                };
                self.select_finding_hunk(file, hunk_index, None);
                true
            }
            FindingAnchor::Chapter { chapter_id } => {
                let known = review.chapters.iter().any(|chapter| &chapter.id == chapter_id);
                if known {
                    self.position.chapter_id = Some(chapter_id.clone());
                }
                known
            }
            // Anchored to the review rather than to any line in it, so there is nowhere to go.
            FindingAnchor::Review => true,
        }
    }

    /// Opens one unsaved finding draft at the current review selection.
    pub(crate) fn begin_finding_draft(&mut self, files: &[ReviewFile]) -> bool {
        if self.finding_draft.is_some() {
            return false;
        }
        self.finding_draft = Some(FindingDraft::new(
            self.selected_finding_anchor(files),
            FindingSeverity::Issue,
            false,
            None,
        ));
        true
    }

    /// Opens a comment on the review as a whole, which posts as its summary.
    ///
    /// The cursor always sits somewhere in the diff, so `c` never reaches the review itself; the
    /// post preview is where a summary is wanted, and where Request changes needs one.
    pub(crate) fn begin_review_summary(&mut self) -> bool {
        if self.finding_draft.is_some() {
            return false;
        }
        self.finding_draft =
            Some(FindingDraft::new(FindingAnchor::Review, FindingSeverity::Issue, false, None));
        true
    }

    /// Opens a question at the current selection, dispatched the moment it is saved.
    ///
    /// Asking is how a reviewer builds a theory of code they did not write, so it is a verb of its
    /// own rather than a comment severity buried in a form.
    pub(crate) fn begin_question(&mut self, files: &[ReviewFile]) -> bool {
        if self.finding_draft.is_some() {
            return false;
        }
        self.finding_draft = Some(FindingDraft::new(
            self.selected_finding_anchor(files),
            FindingSeverity::Question,
            true,
            None,
        ));
        true
    }

    /// Opens a reply to an existing comment, continuing its thread.
    ///
    /// The reply's `severity` is what it asks of the agent: the finding's own to continue it, or an
    /// issue to turn a question into a fix request.
    pub(crate) fn begin_reply(
        &mut self,
        finding: &ReviewFinding,
        severity: FindingSeverity,
    ) -> bool {
        if self.finding_draft.is_some() {
            return false;
        }
        let draft =
            FindingDraft::new(finding.anchor.clone(), severity, true, Some(finding.id.clone()));
        self.finding_draft = Some(draft);
        self.close_feedback_window();
        true
    }

    /// Opens a local editor prefilled from an unsubmitted persisted finding draft.
    pub(crate) fn begin_edit_finding_draft(&mut self, finding: &ReviewFinding) -> bool {
        if self.finding_draft.is_some() || finding.state != FindingState::Draft {
            return false;
        }
        let mut draft = FindingDraft::new(finding.anchor.clone(), finding.severity, false, None);
        draft.carry_fix(finding.proposed_fix.clone());
        draft.set_body(finding.body.clone());
        self.finding_draft = Some(draft);
        self.editing_finding_id = Some(finding.id.clone());
        self.close_feedback_window();
        true
    }

    /// Discards the unsaved finding draft without changing the underlying review session.
    pub(crate) fn discard_finding_draft(&mut self) {
        self.finding_draft = None;
        self.editing_finding_id = None;
    }

    /// Scrolls the feedback window's reading pane by `rows`, down when positive, within what it
    /// was last drawn with.
    pub(crate) fn scroll_feedback(&mut self, rows: i32) {
        let room = i32::from(self.feedback_room.get());
        let at = i32::from(self.feedback_scroll).saturating_add(rows).clamp(0, room);
        self.feedback_scroll = u16::try_from(at).unwrap_or(0);
    }

    /// Maps the focused unified-diff row to the narrowest honest finding anchor.
    pub(crate) fn selected_finding_anchor(&self, files: &[ReviewFile]) -> FindingAnchor {
        let Some(file_id) = self.position.file_id.as_ref() else {
            return self.chapter_or_review_anchor();
        };
        let Some(file) = files.iter().find(|file| &file.id == file_id) else {
            return self.chapter_or_review_anchor();
        };

        let mut row = 0usize;
        for group in visible_groups(file, &self.fold) {
            let VisibleGroup::Hunk(hunk) = group else {
                // A collapsed run is one row, and comments do not anchor to it.
                row = row.saturating_add(1);
                continue;
            };
            if row == self.position.line_offset {
                return FindingAnchor::Hunk { file_id: file.id.clone(), hunk_id: hunk.id.clone() };
            }
            row = row.saturating_add(1);

            for line in &hunk.lines {
                if row == self.position.line_offset {
                    return finding_anchor_for_line(file, hunk, line).unwrap_or_else(|| {
                        FindingAnchor::Hunk { file_id: file.id.clone(), hunk_id: hunk.id.clone() }
                    });
                }
                row = row.saturating_add(1);
            }
        }

        self.position.hunk_id.clone().map_or_else(
            || self.chapter_or_review_anchor(),
            |hunk_id| FindingAnchor::Hunk { file_id: file.id.clone(), hunk_id },
        )
    }

    /// The file the cursor is in and the line of the *new* file it is on, for opening it in an
    /// editor: a hunk's header is its first line, a removed line is the line it was removed
    /// before, and a folded run of churn names no line at all.
    pub(crate) fn selected_new_line<'a>(
        &self,
        files: &'a [ReviewFile],
    ) -> Option<(&'a ReviewFile, Option<u32>)> {
        let file = self.selected_file(files)?;
        let mut row = 0usize;
        for group in visible_groups(file, &self.fold) {
            let VisibleGroup::Hunk(hunk) = group else {
                if row == self.position.line_offset {
                    return Some((file, None));
                }
                row = row.saturating_add(1);
                continue;
            };
            if row == self.position.line_offset {
                return Some((file, (hunk.new_range.start > 0).then_some(hunk.new_range.start)));
            }
            row = row.saturating_add(1);
            for index in 0..hunk.lines.len() {
                if row == self.position.line_offset {
                    return Some((file, new_side_line(hunk, index)));
                }
                row = row.saturating_add(1);
            }
        }
        Some((file, None))
    }

    fn chapter_or_review_anchor(&self) -> FindingAnchor {
        self.position
            .chapter_id
            .clone()
            .map_or(FindingAnchor::Review, |chapter_id| FindingAnchor::Chapter { chapter_id })
    }
}
