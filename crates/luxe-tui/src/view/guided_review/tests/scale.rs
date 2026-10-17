//! Scale — tests moved out of the module they cover.

use crate::app::Msg;
use crate::fixture::{seeded_app, FixtureScenario};
use luxe_core::keymap::UiAction;
use std::collections::HashSet;

/// Every hunk stays reachable at scale. Chapter containment keeps the narrative honest, so the
/// walk has to prove it does not also strand part of the diff.
#[test]
fn every_hunk_in_a_large_review_is_reachable() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewLarge).expect("fixture app");
    app.update(Msg::TerminalResized { width: 160, height: 44 });
    // Folded noise counts too: the fold promises reachability, not omission.
    app.update(Msg::DispatchUiAction(UiAction::ReviewToggleNoise));

    let review = app.guided_review.clone().expect("large review");
    let total: usize = review.files.iter().map(|file| file.hunks.len()).sum();
    let mut seen: HashSet<String> = HashSet::new();

    for _ in 0..review.chapters.len() {
        for _ in 0..=total {
            if let Some(hunk) = app.guided_review_workspace.position.hunk_id.as_ref() {
                seen.insert(hunk.as_str().to_owned());
            }
            app.update(Msg::DispatchUiAction(UiAction::ReviewHunkNext));
        }
        app.update(Msg::DispatchUiAction(UiAction::ReviewChapterNext));
    }

    assert_eq!(seen.len(), total, "reached {} of {total} hunks", seen.len());
}

/// Non-text items are reachable too — a rename and a binary asset are review items, not gaps.
#[test]
fn non_text_items_are_reachable_at_scale() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewLarge).expect("fixture app");
    app.update(Msg::TerminalResized { width: 160, height: 44 });
    let review = app.guided_review.clone().expect("large review");
    let non_text: Vec<_> = review
        .files
        .iter()
        .filter(|file| file.non_text.is_some())
        .map(|file| file.id.clone())
        .collect();
    assert!(non_text.len() >= 2, "the fixture carries a rename and a binary");

    // Every non-text item stays reachable by walking chapters and stepping inside them: fold
    // state and chapter containment must not strand part of the diff.
    app.update(Msg::DispatchUiAction(UiAction::ReviewToggleNoise));
    let mut seen = HashSet::new();
    let items: usize = review
        .files
        .iter()
        .map(|file| file.hunks.len() + usize::from(file.non_text.is_some()))
        .sum();
    for _ in 0..review.chapters.len() {
        for _ in 0..=items {
            if let Some(file) = app.guided_review_workspace.position.file_id.as_ref() {
                seen.insert(file.clone());
            }
            app.update(Msg::DispatchUiAction(UiAction::ReviewHunkNext));
        }
        app.update(Msg::DispatchUiAction(UiAction::ReviewChapterNext));
    }
    for id in non_text {
        assert!(seen.contains(&id), "walking the guide reaches every non-text item");
    }
}
