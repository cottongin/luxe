//! Refresh: what the position keeps, and where it falls back to, when the review is recaptured.

use super::super::*;
use super::{review_of, text_file};
use luxe_core::guided_review::{NonTextChange, NonTextChangeKind};

#[test]
fn accepting_a_refreshed_review_closes_noise_details() {
    let mut workspace = GuidedReviewWorkspace::default();
    workspace.toggle_noise_details();

    workspace.mark_ready();

    assert!(!workspace.noise_details_visible);
}

#[test]
fn refresh_retains_selected_non_text_file_without_a_hunk_anchor() {
    let mut file = text_file("assets/selected.bin");
    file.hunks.clear();
    file.binary = true;
    file.non_text = Some(NonTextChange {
        kind: NonTextChangeKind::Binary,
        summary: "Binary files differ".into(),
    });
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: None,
            file_id: Some(file.id.clone()),
            hunk_id: None,
            line_offset: 0,
        },
        ..Default::default()
    };

    workspace.reconcile_position(&[file.clone()]);

    assert_eq!(workspace.position.file_id, Some(file.id));
    assert_eq!(workspace.position.hunk_id, None);
}

#[test]
fn position_reconciliation_falls_back_to_first_non_text_file() {
    let mut first = text_file("assets/first.bin");
    first.hunks.clear();
    first.binary = true;
    first.non_text = Some(NonTextChange {
        kind: NonTextChangeKind::Binary,
        summary: "Binary files differ".into(),
    });
    let mut second = text_file("assets/second.bin");
    second.hunks.clear();
    second.binary = true;
    second.non_text = Some(NonTextChange {
        kind: NonTextChangeKind::Binary,
        summary: "Binary files differ".into(),
    });
    let mut workspace = GuidedReviewWorkspace::default();

    workspace.reconcile_position(&[first.clone(), second]);

    assert_eq!(workspace.position.file_id, Some(first.id));
    assert_eq!(workspace.position.hunk_id, None);
}

#[test]
fn refresh_falls_back_when_selected_non_text_file_disappears() {
    let mut retained = text_file("assets/retained.bin");
    retained.hunks.clear();
    retained.binary = true;
    retained.non_text = Some(NonTextChange {
        kind: NonTextChangeKind::Binary,
        summary: "Binary files differ".into(),
    });
    let mut removed = text_file("assets/removed.bin");
    removed.hunks.clear();
    removed.binary = true;
    removed.non_text = Some(NonTextChange {
        kind: NonTextChangeKind::Binary,
        summary: "Binary files differ".into(),
    });
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: None,
            file_id: Some(removed.id),
            hunk_id: None,
            line_offset: 9,
        },
        ..Default::default()
    };

    workspace.reconcile_position(&[retained.clone()]);

    assert_eq!(workspace.position.file_id, Some(retained.id));
    assert_eq!(workspace.position.hunk_id, None);
    assert_eq!(workspace.position.line_offset, 0);
}

#[test]
fn refresh_falls_back_to_a_retained_file_when_its_selected_hunk_changes() {
    let file = text_file("src/retained.rs");
    let mut refreshed = file.clone();
    refreshed.hunks[0].id = HunkId::from_content(&refreshed.id, "changed", "let replacement = 2;");
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: None,
            file_id: Some(file.id.clone()),
            hunk_id: Some(file.hunks[0].id.clone()),
            line_offset: 7,
        },
        ..Default::default()
    };

    workspace.reconcile_position(std::slice::from_ref(&refreshed));

    assert_eq!(workspace.position.file_id, Some(file.id));
    assert_eq!(workspace.position.hunk_id, Some(refreshed.hunks[0].id.clone()));
    assert_eq!(workspace.position.line_offset, 0);
}

#[test]
fn changed_hunk_fallback_keeps_navigation_in_the_retained_file() {
    let file = text_file("src/retained-navigation.rs");
    let mut refreshed = file.clone();
    refreshed.hunks[0].id = HunkId::from_content(&refreshed.id, "changed", "let replacement = 2;");
    let mut following = refreshed.hunks[0].clone();
    following.id = HunkId::from_content(&refreshed.id, "following", "let following = 3;");
    refreshed.hunks.push(following.clone());
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: None,
            file_id: Some(file.id.clone()),
            hunk_id: Some(file.hunks[0].id.clone()),
            line_offset: 7,
        },
        ..Default::default()
    };

    workspace.reconcile_position(std::slice::from_ref(&refreshed));
    workspace.next_hunk(&review_of(std::slice::from_ref(&refreshed)));

    assert_eq!(workspace.position.file_id, Some(file.id));
    assert_eq!(workspace.position.hunk_id, Some(following.id));
}

#[test]
fn refresh_keeps_stable_position_and_resets_only_when_it_disappears() {
    let first = text_file("src/first.rs");
    let replacement = text_file("src/replacement.rs");
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: None,
            file_id: Some(first.id.clone()),
            hunk_id: Some(first.hunks[0].id.clone()),
            line_offset: 7,
        },
        ..Default::default()
    };

    workspace.reconcile_position(std::slice::from_ref(&first));
    assert_eq!(workspace.position.line_offset, 7);
    assert_eq!(workspace.position.file_id, Some(first.id));

    workspace.reconcile_position(std::slice::from_ref(&replacement));
    assert_eq!(workspace.position.line_offset, 0);
    assert_eq!(workspace.position.file_id, Some(replacement.id));
    assert_eq!(workspace.position.hunk_id, Some(replacement.hunks[0].id.clone()));
}
