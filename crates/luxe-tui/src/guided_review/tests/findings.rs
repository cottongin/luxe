//! Findings: the finish chooser, drafts, triage order, jumps and anchors.

use super::super::*;
use super::text_file;
use crate::fixture::{seeded_app, FixtureScenario};
use luxe_core::guided_review::{ReviewLine, ReviewLineKind, ReviewSessionValidationError};

#[test]
fn finish_review_chooser_cycles_and_cancels_without_mutating_review_state() {
    let mut workspace = GuidedReviewWorkspace::default();

    workspace.open_finish_review(false);
    assert_eq!(
        workspace.finish_review_choice,
        Some(FinishReviewChoice::Accept),
        "nothing to send, so nothing to ask fixes with"
    );
    workspace.open_finish_review(true);
    assert_eq!(workspace.finish_review_choice, Some(FinishReviewChoice::RequestFixes));
    workspace.move_finish_review_choice(true);
    assert_eq!(workspace.finish_review_choice, Some(FinishReviewChoice::CommentOnly));
    workspace.move_finish_review_choice(false);
    workspace.close_finish_review();
    assert_eq!(workspace.finish_review_choice, None);
}

#[test]
fn finding_draft_captures_the_selection_and_never_overwrites_unsaved_input() {
    let file = text_file("src/draft.rs");
    let mut workspace = GuidedReviewWorkspace::default();
    workspace.position.file_id = Some(file.id.clone());
    workspace.position.hunk_id = Some(file.hunks[0].id.clone());
    workspace.position.line_offset = 1;

    assert!(workspace.begin_finding_draft(std::slice::from_ref(&file)));
    let draft = workspace.finding_draft.as_mut().unwrap();
    assert_eq!(draft.severity, FindingSeverity::Issue);
    assert!(matches!(draft.anchor, FindingAnchor::Line { side: DiffSide::New, .. }));
    draft.set_body("keep this");

    workspace.position.line_offset = 0;
    assert!(!workspace.begin_finding_draft(&[file]));
    assert_eq!(workspace.finding_draft.as_ref().unwrap().body(), "keep this");

    workspace.discard_finding_draft();
    assert!(workspace.finding_draft.is_none());
}

#[test]
fn finding_draft_conversion_validates_without_consuming_unsaved_input() {
    let file = text_file("src/draft.rs");
    let mut workspace = GuidedReviewWorkspace::default();
    workspace.position.file_id = Some(file.id.clone());
    workspace.position.hunk_id = Some(file.hunks[0].id.clone());
    workspace.position.line_offset = 1;
    assert!(workspace.begin_finding_draft(&[file]));

    let draft = workspace.finding_draft.as_mut().expect("open draft");
    draft.set_body("This branch reorders externally visible messages.");
    let finding = draft.to_review_finding(FindingId::new()).expect("valid draft");

    assert_eq!(finding.state, FindingState::Draft);
    assert!(finding.proposed_fix.is_none(), "the composer has no proposed-fix field to fill");
    assert_eq!(finding.body, draft.body());

    // There is no title field: the durable record derives one from the comment's first line, so a
    // sentence of feedback saves and nothing echoes it back a line behind as you type.
    assert_eq!(finding.title, "This branch reorders externally visible messages.");

    draft.set_body("");
    assert_eq!(
        draft.to_review_finding(FindingId::new()),
        Err(ReviewSessionValidationError::EmptyFindingTitle),
        "with nothing to derive from, the empty draft is rejected"
    );

    // A long comment elides its derived title rather than carrying a paragraph as a summary.
    draft.set_body("x".repeat(200));
    let elided = draft.to_review_finding(FindingId::new()).expect("long body derives a title");
    assert_eq!(elided.title.chars().count(), 80);
    assert!(elided.title.ends_with('…'));
}

#[test]
fn draft_finding_editor_prefills_without_mutating_persisted_evidence() {
    let finding = ReviewFinding {
        posted: None,
        github_thread: None,
        id: FindingId::new(),
        label: String::new(),
        anchor: FindingAnchor::Review,
        severity: FindingSeverity::Suggestion,
        title: "Keep retry bounds".into(),
        body: "The retry path needs a bounded error.".into(),
        proposed_fix: Some("Return the typed timeout error.".into()),
        state: FindingState::Draft,
        thread: Vec::new(),
    };
    let mut workspace = GuidedReviewWorkspace::default();

    assert!(workspace.begin_edit_finding_draft(&finding));
    let draft = workspace.finding_draft.expect("editor draft");
    assert_eq!(workspace.editing_finding_id, Some(finding.id.clone()));
    assert_eq!(draft.anchor, finding.anchor);
    assert_eq!(draft.body(), finding.body);
    // Nothing can author a remediation any more, but a draft written by a build that could must
    // not lose it just because the reviewer reopened it to fix a typo.
    assert_eq!(
        draft.to_review_finding(finding.id.clone()).expect("valid draft").proposed_fix,
        finding.proposed_fix
    );
}

#[test]
fn finding_selection_uses_stable_actionable_triage_order() {
    let finding = |title: &str, state: FindingState, severity: FindingSeverity| ReviewFinding {
        posted: None,
        github_thread: None,
        id: FindingId::new(),
        label: String::new(),
        anchor: FindingAnchor::Review,
        severity,
        title: title.into(),
        body: "Fixture feedback.".into(),
        proposed_fix: None,
        state,
        thread: Vec::new(),
    };
    let findings = vec![
        finding("stale", FindingState::Stale, FindingSeverity::Note),
        finding("draft", FindingState::Draft, FindingSeverity::Suggestion),
        finding("open", FindingState::Open, FindingSeverity::Issue),
        finding("blocking", FindingState::Open, FindingSeverity::Blocking),
    ];
    let mut workspace = GuidedReviewWorkspace::default();

    assert!(workspace.move_finding_selection(&findings, true));
    assert_eq!(workspace.selected_finding_id, Some(findings[3].id.clone()));
    assert!(workspace.move_finding_selection(&findings, true));
    assert_eq!(workspace.selected_finding_id, Some(findings[2].id.clone()));
    assert!(workspace.move_finding_selection(&findings, false));
    assert_eq!(workspace.selected_finding_id, Some(findings[3].id.clone()));
}

#[test]
fn finding_jump_restores_an_exact_line_anchor() {
    let app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let review = app.guided_review.expect("fixture review");
    let file = &review.files[0];
    let hunk = &file.hunks[0];
    let finding = ReviewFinding {
        posted: None,
        github_thread: None,
        id: FindingId::new(),
        label: String::new(),
        anchor: FindingAnchor::Line {
            file_id: file.id.clone(),
            hunk_id: hunk.id.clone(),
            side: DiffSide::New,
            line: hunk.lines[0].new_line.expect("fixture new line"),
            context_hash: hunk.lines[0].context_hash(),
        },
        severity: FindingSeverity::Issue,
        title: "Jump target".into(),
        body: "Jump to this immutable line.".into(),
        proposed_fix: None,
        state: FindingState::Open,
        thread: Vec::new(),
    };
    let mut workspace = GuidedReviewWorkspace::default();

    assert!(workspace.jump_to_finding(&finding, &review));
    assert_eq!(workspace.position.file_id, Some(file.id.clone()));
    assert_eq!(workspace.position.hunk_id, Some(hunk.id.clone()));
    assert_eq!(workspace.position.line_offset, 1);
}

#[test]
fn selected_diff_row_maps_to_side_specific_line_anchor() {
    let mut file = text_file("src/anchor.rs");
    file.hunks[0].lines = vec![
        ReviewLine {
            old_line: Some(7),
            new_line: None,
            kind: ReviewLineKind::Deletion,
            text: "old value".into(),
        },
        ReviewLine {
            old_line: None,
            new_line: Some(7),
            kind: ReviewLineKind::Addition,
            text: "new value".into(),
        },
        ReviewLine {
            old_line: None,
            new_line: None,
            kind: ReviewLineKind::NoNewlineMarker,
            text: "\\ No newline at end of file".into(),
        },
    ];
    let hunk_id = file.hunks[0].id.clone();
    let expected_old_hash = file.hunks[0].lines[0].context_hash();
    let expected_new_hash = file.hunks[0].lines[1].context_hash();
    let mut workspace = GuidedReviewWorkspace::default();
    workspace.position.file_id = Some(file.id.clone());
    workspace.position.hunk_id = Some(hunk_id.clone());

    workspace.position.line_offset = 1;
    assert_eq!(
        workspace.selected_finding_anchor(&[file.clone()]),
        FindingAnchor::Line {
            file_id: file.id.clone(),
            hunk_id: hunk_id.clone(),
            side: DiffSide::Old,
            line: 7,
            context_hash: expected_old_hash,
        }
    );

    workspace.position.line_offset = 2;
    assert_eq!(
        workspace.selected_finding_anchor(&[file.clone()]),
        FindingAnchor::Line {
            file_id: file.id.clone(),
            hunk_id: hunk_id.clone(),
            side: DiffSide::New,
            line: 7,
            context_hash: expected_new_hash,
        }
    );

    workspace.position.line_offset = 3;
    assert_eq!(
        workspace.selected_finding_anchor(&[file.clone()]),
        FindingAnchor::Hunk { file_id: file.id.clone(), hunk_id: hunk_id.clone() }
    );

    workspace.position.line_offset = 0;
    assert_eq!(
        workspace.selected_finding_anchor(&[file]),
        FindingAnchor::Hunk { file_id: workspace.position.file_id.clone().unwrap(), hunk_id }
    );
}
