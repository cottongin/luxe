use super::*;
use crate::fixture::{seeded_app, FixtureScenario};
use crossterm::event::{KeyCode, KeyModifiers};

/// A review that failed to capture offers `r`, and `r` has to reach the retry.
fn failed_review() -> App {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("review");
    app.guided_review_workspace.mark_failed(
        luxe_core::guided_review::ReviewTarget::WorkingTree,
        luxe_core::event::GuidedReviewFailure {
            kind: luxe_core::event::GuidedReviewFailureKind::Capture,
            message: "capture failed".into(),
        },
    );
    app
}

#[test]
fn failed_review_claims_r_for_retry() {
    let mut app = failed_review();

    let claimed = claim(&mut app, &KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));

    assert!(matches!(claimed, Some(Msg::GuidedReviewRetry)));
}

/// ...unless a field is open, in which case `r` is a letter someone is typing.
#[test]
fn a_typed_source_reference_outranks_the_retry() {
    let mut app = failed_review();
    app.guided_review_workspace
        .begin_source_reference_entry(crate::guided_review::ReviewSourceReferenceKind::Base);

    let claimed = claim(&mut app, &KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));

    assert!(matches!(claimed, Some(Msg::GuidedReviewSourceReferenceInsert('r'))));
}

/// The two arms that mean "not mine". Everything else the surface swallows, deliberately —
/// a plain letter must never leak into the composer from a full-screen review — so these are
/// the only paths by which a key reaches the rungs below, and a regression here is silent.
#[test]
fn the_composer_and_bound_review_chords_fall_through() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("review");
    let files = app.guided_review.as_ref().expect("a loaded review").files.clone();
    assert!(app.guided_review_workspace.begin_finding_draft(&files), "the composer opened");

    assert!(
        claim(&mut app, &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)).is_none(),
        "a letter typed into the comment composer belongs to the composer"
    );
}

/// vim's half-page moves reach the diff, and `Ctrl+D` is not read as a bare `d`, which
/// toggles the noise details and used to take the chord too.
#[test]
fn ctrl_d_and_ctrl_u_move_half_a_page() {
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("review");
    let ctrl = |code| KeyEvent::new(code, KeyModifiers::CONTROL);
    assert!(matches!(
        claim(&mut app, &ctrl(KeyCode::Char('d'))),
        Some(Msg::GuidedReviewHalfPage(true))
    ));
    assert!(matches!(
        claim(&mut app, &ctrl(KeyCode::Char('u'))),
        Some(Msg::GuidedReviewHalfPage(false))
    ));
    assert!(matches!(
        claim(&mut app, &KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)),
        Some(Msg::GuidedReviewToggleNoiseDetails)
    ));
}

/// What each key claims with the review's own overlays open together, the help sheet over a
/// search and the reference field over the picker it came from. Esc closes what is on top; typing
/// goes to the field that takes it; and a space in the reference field still walks on.
#[test]
fn overlays_open_together_take_keys_in_their_order() {
    let plain = |code| KeyEvent::new(code, KeyModifiers::NONE);
    let claimed = |app: &mut App, key| format!("{:?}", claim(app, &key).expect("claimed"));
    let mut app = seeded_app("nord", FixtureScenario::GuidedReviewReady).expect("review");
    let cases = [
        (plain(KeyCode::Esc), "ExitGuidedReview"),
        (plain(KeyCode::Char('/')), "GuidedReviewBeginSearch"),
    ];
    for (key, want) in cases {
        assert_eq!(claimed(&mut app, key), want, "nothing open: {key:?}");
    }

    app.guided_review_workspace.toggle_help();
    app.guided_review_workspace.begin_search();
    let cases = [
        (plain(KeyCode::Esc), "GuidedReviewToggleHelp"),
        (plain(KeyCode::Char('?')), "GuidedReviewSearchInput('?')"),
        (plain(KeyCode::Enter), "GuidedReviewSearchNext"),
    ];
    for (key, want) in cases {
        assert_eq!(claimed(&mut app, key), want, "help over a search: {key:?}");
    }

    app.guided_review_workspace.toggle_help();
    app.guided_review_workspace.toggle_source_picker();
    let cases = [
        (plain(KeyCode::Esc), "GuidedReviewToggleSourcePicker"),
        (plain(KeyCode::Down), "GuidedReviewSearchNext"),
        (KeyEvent::new(KeyCode::Char('/'), KeyModifiers::CONTROL), "Tick"),
    ];
    for (key, want) in cases {
        assert_eq!(claimed(&mut app, key), want, "a search over the picker: {key:?}");
    }

    app.guided_review_workspace.cancel_search();
    let cases = [
        (plain(KeyCode::Enter), "GuidedReviewSelectSourcePicker"),
        (KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL), "GuidedReviewSourcePickerNext"),
        (plain(KeyCode::Char('?')), "Tick"),
    ];
    for (key, want) in cases {
        assert_eq!(claimed(&mut app, key), want, "the picker alone: {key:?}");
    }

    app.guided_review_workspace
        .begin_source_reference_entry(crate::guided_review::ReviewSourceReferenceKind::Base);
    let cases = [
        (plain(KeyCode::Esc), "GuidedReviewCancelSourceReference"),
        (plain(KeyCode::Char('/')), "GuidedReviewSourceReferenceInsert('/')"),
        // a space is text here, refused at submit as more than one token, never a step underneath
        (plain(KeyCode::Char(' ')), "GuidedReviewSourceReferenceInsert(' ')"),
        (plain(KeyCode::Up), "GuidedReviewSourcePickerPrevious"),
        (plain(KeyCode::Enter), "GuidedReviewSubmitSourceReference"),
        (KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL), "GuidedReviewHalfPage(true)"),
    ];
    for (key, want) in cases {
        assert_eq!(claimed(&mut app, key), want, "the reference field over the picker: {key:?}");
    }
}
