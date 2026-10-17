//! The source picker — choosing what a review covers, and the reference field it opens for a base
//! or a commit.

use super::{
    GuidedReviewWorkspace, ReviewSourcePickerSelection, ReviewSourcePickerState,
    ReviewSourceReferenceEntry, ReviewSourceReferenceKind,
};
use luxe_core::guided_review::ReviewTarget;

impl GuidedReviewWorkspace {
    /// Opens or closes the local source picker without mutating the pinned review.
    pub(crate) fn toggle_source_picker(&mut self) {
        self.source_picker = match self.source_picker {
            ReviewSourcePickerState::Closed => ReviewSourcePickerState::Open {
                selection: ReviewSourcePickerSelection::picker_order()[0],
            },
            ReviewSourcePickerState::Open { .. } => {
                self.source_reference_entry = None;
                ReviewSourcePickerState::Closed
            }
        };
    }

    /// Whether the source picker is open.
    pub(crate) fn source_picker_open(&self) -> bool {
        matches!(self.source_picker, ReviewSourcePickerState::Open { .. })
    }

    /// Whether the source reference field is open.
    pub(crate) fn source_reference_open(&self) -> bool {
        self.source_reference_entry.is_some()
    }

    pub(crate) fn selected_source_picker(&self) -> Option<ReviewSourcePickerSelection> {
        match self.source_picker {
            ReviewSourcePickerState::Closed => None,
            ReviewSourcePickerState::Open { selection } => Some(selection),
        }
    }

    pub(crate) fn begin_source_reference_entry(&mut self, kind: ReviewSourceReferenceKind) {
        self.source_reference_entry =
            Some(ReviewSourceReferenceEntry { kind, value: String::new(), error: None });
    }

    pub(crate) fn push_source_reference_char(&mut self, character: char) {
        if let Some(entry) = &mut self.source_reference_entry {
            entry.value.push(character);
            entry.error = None;
        }
    }

    pub(crate) fn pop_source_reference_char(&mut self) {
        if let Some(entry) = &mut self.source_reference_entry {
            entry.value.pop();
            entry.error = None;
        }
    }

    /// Returns the source picker to target selection without replacing the pinned review.
    pub(crate) fn cancel_source_reference_entry(&mut self) {
        self.source_reference_entry = None;
    }

    /// Validates a pending source reference and resolves it into an immutable review target.
    pub(crate) fn submit_source_reference(&mut self) -> Option<ReviewTarget> {
        let entry = self.source_reference_entry.as_mut()?;
        let reference = entry.value.trim();
        let target = match entry.kind {
            ReviewSourceReferenceKind::Base => {
                if reference.is_empty() {
                    entry.error = Some("Type a base reference.");
                    return None;
                }
                if reference.len() > 256 || reference.chars().any(char::is_control) {
                    entry.error = Some("Base reference is not valid.");
                    return None;
                }
                if reference.split_whitespace().count() != 1 {
                    entry.error = Some("Base reference must be one token.");
                    return None;
                }
                ReviewTarget::Base { reference: reference.to_owned() }
            }
            ReviewSourceReferenceKind::Commit => {
                if reference.is_empty() {
                    entry.error = Some("Type a commit object ID.");
                    return None;
                }
                if !(4..=64).contains(&reference.len())
                    || !reference.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    entry.error = Some("Commit ID must be 4–64 hexadecimal characters.");
                    return None;
                }
                ReviewTarget::Commit { oid: reference.to_owned() }
            }
        };
        entry.error = None;
        Some(target)
    }

    /// Moves the source-picker selection one row with wraparound when it is open.
    pub(crate) fn move_source_picker_selection(
        &mut self,
        forward: bool,
        last_turn_available: bool,
    ) {
        let ReviewSourcePickerState::Open { selection } = &mut self.source_picker else {
            return;
        };
        let options = ReviewSourcePickerSelection::picker_order();
        let index = options.iter().position(|candidate| candidate == selection).unwrap_or(0);
        for step in 1..=options.len() {
            let next = if forward {
                (index + step) % options.len()
            } else {
                (index + options.len() - step) % options.len()
            };
            let candidate = options[next];
            if candidate != ReviewSourcePickerSelection::LastTurn || last_turn_available {
                *selection = candidate;
                return;
            }
        }
    }
}
