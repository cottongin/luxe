//! Navigation: lines, pages, hunks, files and chapters, folds, and the walkthrough's order.

use super::super::*;
use super::{review_of, text_file};
use crate::fixture::{seeded_app, FixtureScenario};
use luxe_core::guided_review::{
    LineRange, NoiseKind, NonTextChange, NonTextChangeKind, ReviewHunk, ReviewLine, ReviewLineKind,
};

#[test]
fn line_navigation_stays_within_selected_file_rows() {
    let file = text_file("src/lines.rs");
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: None,
            file_id: Some(file.id.clone()),
            hunk_id: Some(file.hunks[0].id.clone()),
            line_offset: 0,
        },
        ..Default::default()
    };

    workspace.next_line(&review_of(std::slice::from_ref(&file)));
    workspace.next_line(&review_of(std::slice::from_ref(&file)));
    assert_eq!(workspace.position.line_offset, 1);

    workspace.previous_line(&review_of(std::slice::from_ref(&file)));
    workspace.previous_line(&review_of(std::slice::from_ref(&file)));
    assert_eq!(workspace.position.line_offset, 0);
}

#[test]
fn chapter_navigation_moves_the_position_to_the_next_section() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let mut review = app.guided_review.take().expect("fixture review");
    let mut supporting = review.chapters[0].clone();
    supporting.id = ChapterId::from_item_keys("supporting", &["fixture-supporting"]);
    supporting.title = "Supporting changes".into();
    review.chapters.push(supporting.clone());
    let mut workspace = app.guided_review_workspace;

    workspace.next_chapter(&review);

    assert_eq!(workspace.position.chapter_id, Some(supporting.id));
}

#[test]
fn chapter_navigation_selects_a_referenced_non_text_file() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let mut review = app.guided_review.take().expect("fixture review");
    let mut non_text = text_file("assets/chapter.bin");
    non_text.hunks.clear();
    non_text.binary = true;
    non_text.non_text = Some(NonTextChange {
        kind: NonTextChangeKind::Binary,
        summary: "Binary files differ".into(),
    });
    let mut chapter = review.chapters[0].clone();
    chapter.id = ChapterId::from_item_keys("binary", &[non_text.id.as_str()]);
    chapter.items = vec![ReviewItemRef::File(non_text.id.clone())];
    let first_chapter_id = review.chapters[0].id.clone();
    review.files.push(non_text.clone());
    review.chapters.push(chapter);
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: Some(first_chapter_id),
            file_id: Some(review.files[0].id.clone()),
            hunk_id: Some(review.files[0].hunks[0].id.clone()),
            line_offset: 0,
        },
        ..Default::default()
    };

    workspace.next_chapter(&review);

    assert_eq!(workspace.position.file_id, Some(non_text.id));
    assert_eq!(workspace.position.hunk_id, None);
}

#[test]
fn file_navigation_selects_text_files_and_sensible_hunk_anchors() {
    let first = text_file("src/first.rs");
    let mut second = text_file("src/second.rs");
    let mut second_hunk = second.hunks[0].clone();
    second_hunk.id = HunkId::from_content(&second.id, "second", "let next = 2;");
    second.hunks.push(second_hunk.clone());
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: None,
            file_id: Some(first.id.clone()),
            hunk_id: Some(first.hunks[0].id.clone()),
            line_offset: 0,
        },
        ..Default::default()
    };
    let files = vec![first.clone(), second.clone()];

    workspace.next_chapter_file(&review_of(&files));
    assert_eq!(workspace.position.file_id, Some(second.id.clone()));
    assert_eq!(workspace.position.hunk_id, Some(second.hunks[0].id.clone()));
    assert_eq!(workspace.position.line_offset, 0);
    workspace.previous_chapter_file(&review_of(&files));
    assert_eq!(workspace.position.file_id, Some(first.id));
    assert_eq!(workspace.position.hunk_id, Some(first.hunks[0].id.clone()));
}

#[test]
fn file_navigation_selects_non_text_files_without_a_hunk_anchor() {
    let first = text_file("src/first.rs");
    let mut non_text = text_file("assets/icon.bin");
    non_text.hunks.clear();
    non_text.binary = true;
    non_text.non_text = Some(NonTextChange {
        kind: NonTextChangeKind::Binary,
        summary: "Binary files differ".into(),
    });
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: None,
            file_id: Some(first.id.clone()),
            hunk_id: Some(first.hunks[0].id.clone()),
            line_offset: 0,
        },
        ..Default::default()
    };
    let files = vec![first.clone(), non_text.clone()];

    workspace.next_chapter_file(&review_of(&files));
    assert_eq!(workspace.position.file_id, Some(non_text.id));
    assert_eq!(workspace.position.hunk_id, None);
    workspace.previous_chapter_file(&review_of(&files));
    assert_eq!(workspace.position.file_id, Some(first.id));
    assert_eq!(workspace.position.hunk_id, Some(first.hunks[0].id.clone()));
}

#[test]
fn chapter_file_navigation_selects_a_referenced_non_text_file() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let mut review = app.guided_review.take().expect("fixture review");
    let first = review.files[0].clone();
    let mut non_text = text_file("assets/chapter-file.bin");
    non_text.hunks.clear();
    non_text.binary = true;
    non_text.non_text = Some(NonTextChange {
        kind: NonTextChangeKind::Binary,
        summary: "Binary files differ".into(),
    });
    review.chapters[0].items.push(ReviewItemRef::File(non_text.id.clone()));
    review.files.push(non_text.clone());
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: Some(review.chapters[0].id.clone()),
            file_id: Some(first.id.clone()),
            hunk_id: Some(first.hunks[0].id.clone()),
            line_offset: 0,
        },
        ..Default::default()
    };

    workspace.next_chapter_file(&review);

    assert_eq!(workspace.position.file_id, Some(non_text.id));
    assert_eq!(workspace.position.hunk_id, None);
}

#[test]
fn guide_file_navigation_stays_within_active_chapter() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let mut review = app.guided_review.take().expect("fixture review");
    let first = review.files[0].clone();
    let second = text_file("src/chapter_second.rs");
    let unrelated = text_file("src/unrelated.rs");
    review.chapters[0].items.push(ReviewItemRef::Hunk(second.hunks[0].id.clone()));
    review.files.extend([second.clone(), unrelated]);
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: Some(review.chapters[0].id.clone()),
            file_id: Some(first.id.clone()),
            hunk_id: Some(first.hunks[0].id.clone()),
            line_offset: 0,
        },
        ..Default::default()
    };

    workspace.next_chapter_file(&review);
    assert_eq!(workspace.position.file_id, Some(second.id.clone()));
    workspace.next_chapter_file(&review);
    assert_eq!(workspace.position.file_id, Some(second.id));
    workspace.previous_chapter_file(&review);
    assert_eq!(workspace.position.file_id, Some(first.id));
}

#[test]
fn noise_fold_preserves_a_selected_non_text_item() {
    let mut text = text_file("src/format.rs");
    text.hunks[0].noise = NoiseKind::FormattingOnly;
    let mut non_text = text_file("assets/icon.bin");
    non_text.hunks.clear();
    non_text.binary = true;
    non_text.non_text = Some(NonTextChange {
        kind: NonTextChangeKind::Binary,
        summary: "Binary files differ".into(),
    });
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: None,
            file_id: Some(non_text.id.clone()),
            hunk_id: None,
            line_offset: 0,
        },
        ..Default::default()
    };
    let files = vec![text, non_text.clone()];

    workspace.toggle_noise(&files);
    workspace.toggle_noise(&files);

    assert_eq!(workspace.position.file_id, Some(non_text.id));
    assert_eq!(workspace.position.hunk_id, None);
}

#[test]
fn hunk_navigation_continues_across_reviewable_file_boundaries() {
    let first = text_file("src/first.rs");
    let second = text_file("src/second.rs");
    let files = vec![first.clone(), second.clone()];
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: None,
            file_id: Some(first.id.clone()),
            hunk_id: Some(first.hunks[0].id.clone()),
            line_offset: 0,
        },
        ..Default::default()
    };

    workspace.next_hunk(&review_of(&files));
    assert_eq!(workspace.position.file_id, Some(second.id.clone()));
    assert_eq!(workspace.position.hunk_id, Some(second.hunks[0].id.clone()));
    workspace.previous_hunk(&review_of(&files));
    assert_eq!(workspace.position.file_id, Some(first.id));
    assert_eq!(workspace.position.hunk_id, Some(first.hunks[0].id.clone()));
}

/// Scrolling used to roll into the neighboring file at either end. The pane draws one file at a
/// time, so a single row too many put the reviewer in a different diff with nothing on screen
/// saying so; moving between files belongs to the keys that also carry the narration.
#[test]
fn scrolling_stops_at_the_files_edges_instead_of_changing_file() {
    let first = text_file("src/first.rs");
    let second = text_file("src/second.rs");
    let files = vec![first.clone(), second.clone()];
    let review = review_of(&files);
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: None,
            file_id: Some(second.id.clone()),
            hunk_id: Some(second.hunks[0].id.clone()),
            line_offset: 0,
        },
        ..Default::default()
    };

    for _ in 0..20 {
        workspace.next_line(&review);
    }
    assert_eq!(workspace.position.file_id, Some(second.id.clone()), "forward stays put");

    for _ in 0..20 {
        workspace.previous_line(&review);
    }
    assert_eq!(workspace.position.file_id, Some(second.id.clone()), "and so does backward");
    assert_eq!(workspace.position.line_offset, 0, "clamped, not wrapped");

    // Paging obeys the same edge, so the two vertical keys cannot disagree about what a boundary
    // means.
    workspace.previous_page(&review);
    assert_eq!(workspace.position.file_id, Some(second.id), "paging stops there too");
}

#[test]
fn hunk_navigation_selects_stable_ids_and_header_offsets() {
    let mut file = text_file("src/hunks.rs");
    let mut second = file.hunks[0].clone();
    second.id = HunkId::from_content(&file.id, "second", "let next = 2;");
    second.lines[0].old_line = Some(2);
    second.lines[0].new_line = Some(2);
    file.hunks.push(second.clone());
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: None,
            file_id: Some(file.id.clone()),
            hunk_id: Some(file.hunks[0].id.clone()),
            line_offset: 1,
        },
        ..Default::default()
    };

    workspace.next_hunk(&review_of(std::slice::from_ref(&file)));
    assert_eq!(workspace.position.hunk_id, Some(second.id.clone()));
    assert_eq!(workspace.position.line_offset, 2);
    workspace.previous_hunk(&review_of(std::slice::from_ref(&file)));
    assert_eq!(workspace.position.hunk_id, Some(file.hunks[0].id.clone()));
    assert_eq!(workspace.position.line_offset, 0);
}

#[test]
fn page_navigation_uses_the_rendered_diff_viewport() {
    let mut file = text_file("src/page.rs");
    file.hunks[0].lines.extend((2..=10).map(|number| ReviewLine {
        old_line: Some(number),
        new_line: Some(number),
        kind: ReviewLineKind::Context,
        text: format!("let value_{number} = {number};"),
    }));
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: None,
            file_id: Some(file.id.clone()),
            hunk_id: Some(file.hunks[0].id.clone()),
            line_offset: 0,
        },
        ..Default::default()
    };
    workspace.set_terminal_size(80, 7);

    workspace.next_page(&review_of(std::slice::from_ref(&file)));
    assert_eq!(workspace.position.line_offset, 4);
    workspace.previous_page(&review_of(std::slice::from_ref(&file)));
    assert_eq!(workspace.position.line_offset, 0);
}

#[test]
fn noise_fold_reports_exact_hunk_and_line_counts_until_expanded() {
    let mut file = text_file("src/noise.rs");
    let mut noise = file.hunks[0].clone();
    noise.id = HunkId::from_content(&file.id, "format", "let value=1;");
    noise.lines.push(ReviewLine {
        old_line: Some(2),
        new_line: Some(2),
        kind: ReviewLineKind::Context,
        text: "let unchanged = true;".into(),
    });
    noise.noise = NoiseKind::FormattingOnly;
    file.hunks.push(noise.clone());
    let mut workspace = GuidedReviewWorkspace {
        position: ReviewPosition {
            chapter_id: None,
            file_id: Some(file.id.clone()),
            hunk_id: Some(noise.id.clone()),
            line_offset: 2,
        },
        ..Default::default()
    };

    assert!(workspace.is_hunk_hidden(&file, &noise));
    assert!(!workspace.noise_details_visible);
    workspace.toggle_noise_details();
    assert!(workspace.noise_details_visible);
    assert_eq!(workspace.hidden_counts(&[file.clone()]), (1, 2));
    workspace.toggle_noise(&[file.clone()]);
    assert!(!workspace.is_hunk_hidden(&file, &noise));
    assert_eq!(workspace.hidden_counts(&[file.clone()]), (0, 0));
    workspace.toggle_noise(&[file.clone()]);
    assert_eq!(workspace.position.hunk_id, Some(file.hunks[0].id.clone()));
    assert_eq!(workspace.position.line_offset, 0);
    // Two source rows for the open hunk, then the collapsed run's own row — scrolling reaches it
    // rather than stopping short of content the renderer draws.
    workspace.next_line(&review_of(std::slice::from_ref(&file)));
    workspace.next_line(&review_of(std::slice::from_ref(&file)));
    assert_eq!(workspace.position.line_offset, 2);
    assert!(workspace.position.hunk_id.is_none(), "the cursor sits on the collapsed run");
}

/// Regression: the diff renderer lays out only *visible* hunks, so a selection offset counted over
/// every hunk dropped the cursor rows deep inside the target whenever a folded hunk preceded it.
#[test]
fn hunk_selection_lands_on_the_header_with_folded_noise_ahead_of_it() {
    let mut file = text_file("src/folded.rs");
    let formatting = ReviewHunk {
        id: HunkId::from_content(&file.id, "formatting", "use std::fmt;"),
        old_range: LineRange::new(1, 1),
        new_range: LineRange::new(1, 1),
        heading: Some("imports".into()),
        symbol: None,
        lines: vec![
            ReviewLine {
                old_line: Some(1),
                new_line: None,
                kind: ReviewLineKind::Deletion,
                text: "use std::{fmt,io};".into(),
            },
            ReviewLine {
                old_line: None,
                new_line: Some(1),
                kind: ReviewLineKind::Addition,
                text: "use std::{fmt, io};".into(),
            },
        ],
        noise: NoiseKind::FormattingOnly,
    };
    let semantic = file.hunks[0].clone();
    file.hunks = vec![formatting.clone(), semantic.clone()];
    let review = review_of(std::slice::from_ref(&file));

    let mut workspace = GuidedReviewWorkspace::default();
    workspace.position.file_id = Some(file.id.clone());
    workspace.position.hunk_id = Some(formatting.id.clone());
    assert!(workspace.is_hunk_hidden(&file, &file.hunks[0]), "formatting folds by default");

    workspace.next_hunk(&review);

    assert_eq!(workspace.position.hunk_id.as_ref(), Some(&semantic.id));
    assert_eq!(
        workspace.position.line_offset, 1,
        "the collapsed run holds row 0, so the semantic hunk's header is row 1"
    );
    let groups = crate::guided_review::visible_groups(&file, &workspace.fold);
    let all = crate::guided_review::diff::visible_rows(&groups, 0, 8);
    assert!(
        matches!(all.first(), Some(crate::guided_review::diff::UnifiedDiffRow::Folded(run)) if run.hunks == 1),
        "the collapsed formatting hunk still says it is there: {all:?}"
    );
    let rows = crate::guided_review::diff::visible_rows(&groups, workspace.position.line_offset, 4);
    assert!(
        matches!(rows.first(), Some(crate::guided_review::diff::UnifiedDiffRow::HunkHeader { .. })),
        "the first rendered row is the selected hunk's header"
    );
}

/// Regression: hunk stepping used to roll into the next file in canonical *source* order without
/// touching the chapter, so the rail narrated one chapter beside another chapter's diff.
#[test]
fn hunk_navigation_walks_sections_in_order_and_keeps_the_narrative_with_it() {
    let core = text_file("src/core.rs");
    let test = text_file("tests/core.rs");
    let files = vec![core.clone(), test.clone()];
    let review = review_of(&files);
    assert_eq!(review.chapters.len(), 2, "path evidence splits core from tests");

    let mut workspace = GuidedReviewWorkspace::default();
    workspace.position.chapter_id = Some(review.chapters[0].id.clone());
    workspace.position.file_id = Some(core.id.clone());
    workspace.position.hunk_id = Some(core.hunks[0].id.clone());

    assert!(workspace.at_scope_end(&review), "the core chapter owns exactly one hunk");

    // Stepping carries on into the next section and takes the narrative with it, so the prose on
    // screen always owns the diff on screen. A boundary that ate the keystroke made the reviewer
    // discover `]` in order to keep reading, which is not a walkthrough.
    workspace.next_hunk(&review);
    assert_eq!(workspace.position.chapter_id.as_ref(), Some(&review.chapters[1].id));
    assert_eq!(workspace.position.file_id.as_ref(), Some(&test.id));

    // `]` remains the explicit way across.
    workspace.position.chapter_id = Some(review.chapters[0].id.clone());
    workspace.position.file_id = Some(core.id.clone());
    workspace.position.hunk_id = Some(core.hunks[0].id.clone());
    workspace.next_chapter(&review);
    assert_eq!(workspace.position.chapter_id.as_ref(), Some(&review.chapters[1].id));
    assert_eq!(workspace.position.file_id.as_ref(), Some(&test.id));
    assert_eq!(workspace.position.hunk_id.as_ref(), Some(&test.hunks[0].id));

    // Outside the walkthrough there is no chapter to stay inside, so traversal is unscoped and
    // walks the whole review in canonical order.
    workspace.position.chapter_id = Some(review.chapters[0].id.clone());
    workspace.position.file_id = Some(core.id.clone());
    workspace.position.hunk_id = Some(core.hunks[0].id.clone());
    workspace.next_hunk(&review);
    assert_eq!(workspace.position.file_id.as_ref(), Some(&test.id));
}

/// An explicit jump (a rail click, next-unreviewed, a finding anchor) may legitimately leave the
/// current chapter — so it re-points the narrative rather than stranding it.
#[test]
fn explicit_jumps_repoint_the_narrative_at_the_owning_chapter() {
    let core = text_file("src/core.rs");
    let test = text_file("tests/core.rs");
    let files = vec![core.clone(), test.clone()];
    let review = review_of(&files);

    let mut workspace = GuidedReviewWorkspace::default();
    workspace.position.chapter_id = Some(review.chapters[0].id.clone());
    workspace.position.file_id = Some(core.id.clone());
    workspace.position.hunk_id = Some(core.hunks[0].id.clone());

    workspace.select_file_by_id(&review, &test.id);
    assert_eq!(workspace.position.file_id.as_ref(), Some(&test.id));
    assert_eq!(
        workspace.position.chapter_id.as_ref(),
        Some(&review.chapters[1].id),
        "clicking a file outside the chapter moves the narrative with it"
    );
}

/// The walkthrough follows the order the guide chose, not the filesystem's.
///
/// A section lists its items in the order the work was reasoned in — the schema before the logic
/// that consumes it, the call sites before the tests. Walking `review.files` and merely *filtering*
/// by membership threw that away and served the same alphabetical order a plain diff would.
#[test]
fn stepping_a_section_follows_the_order_the_guide_chose() {
    use luxe_core::guided_review::{ChapterId, ChapterKind, ReviewChapter, ReviewItemRef};

    let ui = text_file("src/ui.rs");
    let model = text_file("src/model.rs");
    let tests = text_file("tests/model.rs");
    let files = vec![ui.clone(), model.clone(), tests.clone()];
    let mut review = review_of(&files);
    // The guide's order: the model first, then the UI that renders it, then the tests.
    let items = vec![
        ReviewItemRef::Hunk(model.hunks[0].id.clone()),
        ReviewItemRef::Hunk(ui.hunks[0].id.clone()),
        ReviewItemRef::Hunk(tests.hunks[0].id.clone()),
    ];
    review.chapters = vec![ReviewChapter {
        id: ChapterId::from_items("core", &[]),
        title: "The category becomes a type".into(),
        rationale: "Model first, then its readers.".into(),
        impact: None,
        kind: ChapterKind::Core,
        items,
        notes: Vec::new(),
    }];

    let mut workspace = GuidedReviewWorkspace::default();
    workspace.position.chapter_id = Some(review.chapters[0].id.clone());
    workspace.position.file_id = Some(model.id.clone());
    workspace.position.hunk_id = Some(model.hunks[0].id.clone());

    workspace.next_hunk(&review);
    assert_eq!(
        workspace.position.file_id.as_ref(),
        Some(&ui.id),
        "the guide put the UI second; source order would have gone to tests/model.rs"
    );
    workspace.next_hunk(&review);
    assert_eq!(workspace.position.file_id.as_ref(), Some(&tests.id));
    assert!(workspace.at_scope_end(&review), "three items, three stops");

    workspace.previous_hunk(&review);
    assert_eq!(workspace.position.file_id.as_ref(), Some(&ui.id), "and back again in that order");
}

/// Outside a section the order is the source's, because there is no narrative to follow.
#[test]
fn all_files_walks_canonical_source_order() {
    let ui = text_file("src/ui.rs");
    let model = text_file("src/model.rs");
    let files = vec![ui.clone(), model.clone()];
    let review = review_of(&files);

    let mut workspace = GuidedReviewWorkspace::default();
    workspace.position.file_id = Some(ui.id.clone());
    workspace.position.hunk_id = Some(ui.hunks[0].id.clone());

    workspace.next_hunk(&review);

    assert_eq!(workspace.position.file_id.as_ref(), Some(&model.id));
}

/// `]` enters a generated chapter, and walking off the end of the chapter before it does not.
///
/// The break this catches: the two callers that find a neighboring chapter share a lookup and
/// differ only in that refusal, so folding them together can quietly give the walkthrough `]`'s
/// policy, or `]` the walkthrough's.
#[test]
fn explicit_chapter_step_enters_generated_but_walkthrough_boundary_stops() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("fixture app");
    let mut review = app.guided_review.take().expect("fixture review");
    let mut generated = review.chapters[0].clone();
    generated.id = ChapterId::from_item_keys("generated", &["fixture-generated"]);
    generated.kind = luxe_core::guided_review::ChapterKind::Generated;
    review.chapters.push(generated.clone());
    let first = review.chapters[0].id.clone();

    let mut walking = app.guided_review_workspace.clone();
    walking.position.chapter_id = Some(first.clone());
    for _ in 0..64 {
        walking.next_hunk(&review);
    }
    assert_eq!(walking.position.chapter_id, Some(first.clone()), "the walk stops at the boundary");

    let mut jumping = app.guided_review_workspace;
    jumping.position.chapter_id = Some(first);
    jumping.next_chapter(&review);
    assert_eq!(jumping.position.chapter_id, Some(generated.id), "`]` opens it");
}
