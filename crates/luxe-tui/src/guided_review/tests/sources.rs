//! Sources: the picker, and a load that fails.

use super::super::*;
use luxe_core::event::GuidedReviewFailureKind;

#[test]
fn failed_refresh_keeps_typed_target_and_error() {
    let mut workspace = GuidedReviewWorkspace::default();
    workspace.begin_loading(ReviewTarget::WorkingTree);
    workspace.mark_failed(
        ReviewTarget::WorkingTree,
        GuidedReviewFailure {
            kind: GuidedReviewFailureKind::Capture,
            message: "source changed while capturing".into(),
        },
    );

    assert_eq!(
        workspace.load_state,
        GuidedReviewLoadState::Failed {
            target: ReviewTarget::WorkingTree,
            error: GuidedReviewFailure {
                kind: GuidedReviewFailureKind::Capture,
                message: "source changed while capturing".into(),
            },
        }
    );
}

#[test]
fn source_picker_opens_at_the_working_tree_and_closes_locally() {
    let mut workspace = GuidedReviewWorkspace::default();

    workspace.toggle_source_picker();
    assert_eq!(
        workspace.source_picker,
        ReviewSourcePickerState::Open { selection: ReviewSourcePickerSelection::WorkingTree }
    );
    workspace.toggle_source_picker();
    assert_eq!(workspace.source_picker, ReviewSourcePickerState::Closed);
}

#[test]
fn source_picker_recovers_from_a_failed_review_without_replacing_pinned_state() {
    let mut workspace = GuidedReviewWorkspace::default();
    workspace.mark_failed(
        ReviewTarget::WorkingTree,
        GuidedReviewFailure {
            kind: GuidedReviewFailureKind::Capture,
            message: "capture failed".into(),
        },
    );

    workspace.toggle_source_picker();

    assert_eq!(
        workspace.source_picker,
        ReviewSourcePickerState::Open { selection: ReviewSourcePickerSelection::WorkingTree }
    );
    assert!(matches!(workspace.load_state, GuidedReviewLoadState::Failed { .. }));
}

#[test]
fn source_picker_skips_unavailable_last_turn_when_navigating() {
    let mut workspace = GuidedReviewWorkspace::default();
    workspace.toggle_source_picker();

    workspace.move_source_picker_selection(true, false);
    assert_eq!(
        workspace.source_picker,
        ReviewSourcePickerState::Open { selection: ReviewSourcePickerSelection::Base }
    );
    workspace.move_source_picker_selection(false, false);
    assert_eq!(
        workspace.source_picker,
        ReviewSourcePickerState::Open { selection: ReviewSourcePickerSelection::WorkingTree }
    );
}

#[test]
fn source_picker_selection_wraps_in_presentation_order() {
    let mut workspace = GuidedReviewWorkspace::default();
    workspace.toggle_source_picker();

    workspace.move_source_picker_selection(false, true);
    assert_eq!(
        workspace.source_picker,
        ReviewSourcePickerState::Open { selection: ReviewSourcePickerSelection::Commit }
    );
    workspace.move_source_picker_selection(true, true);
    workspace.move_source_picker_selection(true, true);
    assert_eq!(
        workspace.source_picker,
        ReviewSourcePickerState::Open { selection: ReviewSourcePickerSelection::LastTurn }
    );

    workspace.begin_source_reference_entry(ReviewSourceReferenceKind::Base);
    assert_eq!(
        workspace.source_reference_entry,
        Some(ReviewSourceReferenceEntry {
            kind: ReviewSourceReferenceKind::Base,
            value: String::new(),
            error: None,
        })
    );

    assert_eq!(workspace.submit_source_reference(), None);
    assert_eq!(
        workspace.source_reference_entry.as_ref().and_then(|entry| entry.error),
        Some("Type a base reference.")
    );
    workspace.push_source_reference_char('m');
    workspace.push_source_reference_char('a');
    workspace.pop_source_reference_char();
    assert_eq!(
        workspace.source_reference_entry.as_ref().map(|entry| entry.value.as_str()),
        Some("m")
    );
    assert_eq!(
        workspace.submit_source_reference(),
        Some(ReviewTarget::Base { reference: "m".into() })
    );

    workspace.begin_source_reference_entry(ReviewSourceReferenceKind::Commit);
    assert_eq!(workspace.submit_source_reference(), None);
    assert_eq!(
        workspace.source_reference_entry.as_ref().and_then(|entry| entry.error),
        Some("Type a commit object ID.")
    );
    for character in "zzzz".chars() {
        workspace.push_source_reference_char(character);
    }
    assert_eq!(workspace.submit_source_reference(), None);
    assert_eq!(
        workspace.source_reference_entry.as_ref().and_then(|entry| entry.error),
        Some("Commit ID must be 4–64 hexadecimal characters.")
    );
    workspace.source_reference_entry.as_mut().expect("commit entry").value = "a1b2c3d4".into();
    assert_eq!(
        workspace.submit_source_reference(),
        Some(ReviewTarget::Commit { oid: "a1b2c3d4".into() })
    );

    workspace.begin_loading(ReviewTarget::WorkingTree);
    assert_eq!(workspace.source_picker, ReviewSourcePickerState::Closed);
    assert_eq!(workspace.source_reference_entry, None);
}
