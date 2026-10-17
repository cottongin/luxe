use super::*;
use crate::app::{QueuedReview, QueuedReviewState};
use luxe_core::{config::Config, keymap::KeyMap};

fn app() -> App {
    let (tx, _rx) = tokio::sync::mpsc::channel(4);
    App::new(tx, KeyMap::defaults(), Config { theme: Some("nord".into()), ..Config::default() })
}

fn ready_review() -> QueuedReview {
    QueuedReview {
        target: luxe_core::guided_review::ReviewTarget::WorkingTree,
        state: QueuedReviewState::Ready,
    }
}

fn theme() -> ThemeStyles {
    let registry = luxe_core::theme::ThemeRegistry::new();
    let resolved = registry.resolve("nord").expect("nord resolves");
    ThemeStyles::from_resolved(&resolved, "nord")
}

fn held(delivery: MidTurnDelivery, display: &str) -> PendingSubmission {
    PendingSubmission {
        display: display.to_string(),
        chips: Vec::new(),
        attached: Vec::new(),
        content: display.to_string(),
        draft: display.to_string(),
        images: Vec::new(),
        delivery,
    }
}

#[test]
fn a_multiline_draft_still_occupies_one_row() {
    let submission = held(MidTurnDelivery::Steer, "first line\nsecond line\nthird line");
    let line = submission_line(&submission, 0, 80, &theme());
    assert!(!line.to_string().contains('\n'));
    assert!(line.width() <= 80);
    assert!(line.to_string().contains("first line second line"));
}

/// The strip carries two unrelated things, and the offer to edit belongs to only one of them.
/// A ready review is not a message: it cannot be recalled into the composer, so offering to
/// edit "them" over a row reporting that a capture finished names an action that does nothing.
#[test]
fn a_queued_review_alone_offers_nothing_to_edit() {
    let mut app = app();
    app.review.queued = Some(ready_review());
    assert_eq!(recall_hint(&app), None, "nothing was typed, so there is nothing to recall");

    app.pending_submissions.push(held(MidTurnDelivery::Steer, "one moment"));
    assert!(
        recall_hint(&app).is_some_and(|hint| hint.contains("to edit it here")),
        "a held message does earn the offer",
    );
}

/// Capturing is the one state on this row where work is happening, so it animates. A pause
/// glyph over a running job says the opposite of what is true, and a spinner whose frame never
/// advances is worse than no spinner — it reads as a hang.
#[test]
fn a_capturing_review_spins() {
    let mut app = app();
    app.review.queued = Some(QueuedReview {
        target: luxe_core::guided_review::ReviewTarget::WorkingTree,
        state: QueuedReviewState::Capturing,
    });
    // Stepped a whole frame at a time: the sweep is deliberately slower than the render tick,
    // so consecutive ticks hold the same glyph on purpose.
    let frames: Vec<String> = (0..3)
        .map(|step| {
            app.status.spinner_frame = step * crate::glyphs::spinner_ticks_per_frame();
            queued_review_line(&app, app.review.queued.as_ref().expect("set"), &theme()).to_string()
        })
        .collect();

    assert!(frames[0].contains("capturing"), "{}", frames[0]);
    assert!(
        frames[0].contains(crate::statusline::spinner(0)),
        "the running state wears the same spinner as everything else: {}",
        frames[0],
    );
    assert_ne!(frames[0], frames[1], "the frame advances");
    assert_ne!(frames[1], frames[2]);

    // And the tick that drives it keeps running while a capture is in flight, which is after
    // the turn that would otherwise have been advancing it.
    app.status.animating = true;
    let before = app.status.spinner_frame;
    app.status.tick(false);
    assert_ne!(app.status.spinner_frame, before);
}

/// Each capture says what it is capturing, and a pull request says it is fetching.
#[test]
fn a_capture_names_its_source() {
    use luxe_core::guided_review::ReviewTarget;
    for (target, says) in [
        (ReviewTarget::WorkingTree, "capturing the working tree"),
        (
            ReviewTarget::LastTurn { session_id: "s".into(), turn_id: "t".into() },
            "capturing the last turn",
        ),
        (ReviewTarget::Base { reference: "main".into() }, "capturing the change since main"),
        (
            ReviewTarget::Commit { oid: "a9d9d84ee3e848f0e1ea97eaef837126d3a7d983".into() },
            "capturing commit a9d9d84ee3e8",
        ),
        (ReviewTarget::PullRequest { number: 96, repository: None }, "fetching pull request #96"),
    ] {
        assert_eq!(capture_label(&target), says);
    }
}

/// The long wait names who is doing the work and counts, and still spins.
#[test]
fn a_guide_being_written_names_the_model_and_counts() {
    let mut app = app();
    app.status.model = "Claude Opus 5".into();
    let since = app.frame_now() - std::time::Duration::from_secs(34);
    app.review.queued = Some(QueuedReview {
        target: luxe_core::guided_review::ReviewTarget::PullRequest {
            number: 96,
            repository: None,
        },
        state: QueuedReviewState::Writing { since },
    });
    let text =
        queued_review_line(&app, app.review.queued.as_ref().expect("set"), &theme()).to_string();
    assert!(text.contains("Claude Opus 5 is writing the guide"), "{text}");
    assert!(text.contains("34s"), "it counts: {text}");
    assert!(text.contains(crate::statusline::spinner(app.status.spinner_frame)), "{text}");

    app.status.model.clear();
    let text =
        queued_review_line(&app, app.review.queued.as_ref().expect("set"), &theme()).to_string();
    assert!(text.contains("the model is writing the guide"), "{text}");
}

/// The row already says the review is ready, and opening it is the only thing anyone does with
/// one — so the chord stands alone rather than being narrated.
#[test]
fn a_ready_review_names_the_chord_and_stops_there() {
    let mut app = app();
    app.review.queued = Some(ready_review());
    let line = queued_review_line(&app, app.review.queued.as_ref().expect("set"), &theme());
    let text = line.to_string();
    assert!(text.contains("review ready"), "{text}");
    assert!(!text.contains("opens it"), "the chord speaks for itself: {text}");
}

#[test]
fn a_long_draft_is_cut_to_the_available_room() {
    let submission = held(MidTurnDelivery::Queue, &"word ".repeat(60));
    let line = submission_line(&submission, 2, 40, &theme());
    assert!(line.width() <= 40, "rendered {} cells wide", line.width());
    assert!(line.to_string().contains("queued #2"));
    assert!(line.to_string().ends_with('\u{2026}'));
}
