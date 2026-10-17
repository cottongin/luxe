//! Frames — when the loop paints, defers or skips one.

use super::*;

/// The frame gate's whole point: an idle session paints nothing.
#[test]
fn an_idle_tick_paints_nothing() {
    assert_eq!(
        frame_decision(Wake::Tick, false, false, Duration::from_secs(9), FRAME_BUDGET),
        FrameDecision::Skip
    );
}

/// ...and its whole risk: a real change must never be dropped, only deferred onto a later tick.
#[test]
fn a_deferred_change_is_paid_back_by_the_next_tick() {
    // Too soon after the last frame: held, not dropped.
    assert_eq!(
        frame_decision(Wake::Event, false, false, Duration::ZERO, FRAME_BUDGET),
        FrameDecision::Defer
    );
    // The tick that follows draws it even though nothing is animating.
    assert_eq!(
        frame_decision(Wake::Tick, false, true, FRAME_BUDGET, FRAME_BUDGET),
        FrameDecision::Draw
    );
}

#[test]
fn input_is_never_made_to_wait_for_a_frame() {
    assert_eq!(
        frame_decision(Wake::Input, false, false, Duration::ZERO, FRAME_BUDGET),
        FrameDecision::Draw
    );
}

#[test]
fn an_animation_keeps_painting_at_the_frame_budget() {
    assert_eq!(
        frame_decision(Wake::Tick, true, false, FRAME_BUDGET, FRAME_BUDGET),
        FrameDecision::Draw
    );
    // Inside the budget it waits for the next tick rather than drawing twice in a frame.
    assert_eq!(
        frame_decision(Wake::Tick, true, false, Duration::ZERO, FRAME_BUDGET),
        FrameDecision::Defer
    );
}

#[test]
fn pointer_motion_over_dead_space_never_costs_a_frame() {
    for animating in [false, true] {
        for pending in [false, true] {
            assert_eq!(
                frame_decision(
                    Wake::IdleMotion,
                    animating,
                    pending,
                    Duration::from_secs(9),
                    FRAME_BUDGET,
                ),
                FrameDecision::Skip
            );
        }
    }
}

/// The reveal's clock is armed only while it owes text, and it never waits a whole tick.
#[test]
fn the_reveal_clock_only_runs_while_text_is_owed() {
    assert_eq!(reveal_wait(false, Duration::ZERO), None, "idle arms nothing");
    assert_eq!(reveal_wait(true, Duration::ZERO), Some(REVEAL_FRAME));
    // A frame already overdue fires at once rather than waiting out the gap again.
    assert_eq!(reveal_wait(true, Duration::from_millis(50)), Some(Duration::ZERO));
    assert!(REVEAL_FRAME < FRAME_BUDGET, "the reveal paints faster than the animation tick");
}

/// Mid-reveal, a frame must not be held for a whole tick: that is what would put the cursor back
/// to eight-character steps, which is the lumpiness the reveal exists to remove.
#[test]
fn a_reveal_frame_is_not_deferred_for_a_whole_tick() {
    assert_eq!(
        frame_decision(Wake::Tick, true, false, REVEAL_FRAME, REVEAL_FRAME),
        FrameDecision::Draw
    );
    assert_eq!(
        frame_decision(Wake::Tick, true, false, REVEAL_FRAME, FRAME_BUDGET),
        FrameDecision::Defer,
        "and on the slower budget the same frame would have waited"
    );
}

#[test]
fn animating_tracks_the_states_that_move_on_their_own() {
    let mut app = seeded_app("dark", FixtureScenario::ChatIdle).expect("fixture");
    app.status.state = crate::app::AppState::Idle;
    app.status.turn_started = None;
    assert!(!app.animating(), "an idle transcript is a still image");

    app.status.state = crate::app::AppState::Streaming;
    assert!(app.animating(), "the spinner and the activity bar are moving");

    app.status.state = crate::app::AppState::Compacting;
    assert!(app.animating());

    // A turn that ended clears both, and nothing moves again until the next one.
    app.status.state = crate::app::AppState::Idle;
    app.status.turn_started = Some(std::time::Instant::now());
    assert!(app.animating(), "the elapsed readout still ticks");
    app.status.turn_started = None;
    assert!(!app.animating());

    // A review capture runs after the turn is over, so nothing above is true while it works.
    // Left out, the loop skips every frame and the tray's spinner freezes on one character.
    app.status.animating = true;
    assert!(app.animating(), "a capture in flight still earns frames");
    app.status.animating = false;
    assert!(!app.animating());
}

/// A modal waiting on GitHub turns its spinner while the app is otherwise idle, and stops
/// earning frames once the answer is in.
#[test]
fn a_modal_waiting_on_github_earns_frames() {
    let mut app =
        seeded_app("dark", FixtureScenario::GuidedReviewOpenPullRequest).expect("fixture");
    app.status.state = crate::app::AppState::Idle;
    app.status.turn_started = None;
    app.status.animating = false;
    assert!(!app.animating(), "a drafted preview is a still image");
    app.guided_review_workspace.pull_request_opening.as_mut().expect("preview").opening = true;
    assert!(app.animating(), "opening it on GitHub spins");
    app.current_view = crate::app::ViewMode::Chat;
    assert!(!app.animating(), "not drawn, so not animated");
    app.current_view = crate::app::ViewMode::GuidedReview;
    let workspace = &mut app.guided_review_workspace;
    workspace.pull_request_opening = None;
    workspace.pull_request_fix = Some(crate::guided_review::PullRequestFix {
        number: 42,
        branch: None,
        refusal: None,
        comments: 0,
    });
    assert!(app.animating(), "asking whether a fix can land spins");
    app.guided_review_workspace.pull_request_fix.as_mut().unwrap().refusal = Some("No.".into());
    assert!(!app.animating());
}

/// `/github` fetching and a restack being planned or pushed earn frames; answered, they don't.
#[test]
fn a_panel_waiting_on_github_earns_frames() {
    let mut app = seeded_app("dark", FixtureScenario::ChatIdle).expect("fixture");
    app.status.state = crate::app::AppState::Idle;
    app.status.turn_started = None;
    assert!(!app.animating());
    app.overlays.github = Some(crate::github_panel::GithubPanel::default());
    assert!(app.animating(), "fetching");
    app.overlays.github.as_mut().unwrap().sections = Some(Vec::new());
    assert!(!app.animating(), "answered");
    app.overlays.github = None;
    for (panel, moving) in [
        (crate::restack_panel::RestackPanel::Planning, true),
        (crate::restack_panel::RestackPanel::Pushing, true),
        (crate::restack_panel::RestackPanel::Said("Done.".into()), false),
    ] {
        app.overlays.restack = Some(panel);
        assert_eq!(app.animating(), moving);
    }
}

/// Each modal waits while it is sent and unanswered, and while it is asking and neither an
/// answer nor a failure has come; an answer or a failure ends the wait.
#[test]
fn every_modal_waits_exactly_while_something_is_out() {
    use FixtureScenario as F;
    let waits = |app: &crate::app::App| app.guided_review_workspace.waiting();

    let mut app = seeded_app("dark", F::GuidedReviewPullRequestPost).expect("fixture");
    assert!(!waits(&app), "prepared");
    let post = app.guided_review_workspace.github_post.as_mut().unwrap();
    post.posting = true;
    assert!(waits(&app), "posting");
    let post = app.guided_review_workspace.github_post.as_mut().unwrap();
    (post.posting, post.prepared) = (false, None);
    assert!(waits(&app), "asking");
    app.guided_review_workspace.github_post.as_mut().unwrap().error = Some("No.".into());
    assert!(!waits(&app), "failed");

    let mut app = seeded_app("dark", F::GuidedReviewOwnPullRequest).expect("fixture");
    let answers = app.guided_review_workspace.thread_answers.as_mut().unwrap();
    answers.prepared = Some((true, false));
    assert!(!waits(&app), "prepared");
    app.guided_review_workspace.thread_answers.as_mut().unwrap().sending = true;
    assert!(waits(&app), "sending");
    let answers = app.guided_review_workspace.thread_answers.as_mut().unwrap();
    (answers.sending, answers.prepared) = (false, None);
    assert!(waits(&app), "asking");
    app.guided_review_workspace.thread_answers.as_mut().unwrap().error = Some("No.".into());
    assert!(!waits(&app), "failed");

    let mut app = seeded_app("dark", F::GuidedReviewOpenPullRequest).expect("fixture");
    let opening = app.guided_review_workspace.pull_request_opening.as_mut().unwrap();
    opening.prepared = None;
    assert!(waits(&app), "drafting");
    app.guided_review_workspace.pull_request_opening.as_mut().unwrap().error = Some("No.".into());
    assert!(!waits(&app), "failed");

    let mut app = seeded_app("dark", F::GuidedReviewOpenStack).expect("fixture");
    assert!(!waits(&app), "drafted");
    app.guided_review_workspace.stack_opening.as_mut().unwrap().opening = true;
    assert!(waits(&app), "opening");
    let stack = app.guided_review_workspace.stack_opening.as_mut().unwrap();
    (stack.opening, stack.layers) = (false, None);
    assert!(waits(&app), "drafting");
    app.guided_review_workspace.stack_opening.as_mut().unwrap().error = Some("No.".into());
    assert!(!waits(&app), "failed");

    let mut app = seeded_app("dark", F::GuidedReviewFixPullRequest).expect("fixture");
    assert!(!waits(&app), "answered with a branch");
    let fix = app.guided_review_workspace.pull_request_fix.as_mut().unwrap();
    fix.branch = None;
    assert!(waits(&app), "asking");
}

/// The logotype's own frames. Both surfaces that draw it have to claim them — a clock that
/// advances while nothing asks for a frame is a mark that only moves when you touch the
/// keyboard.
#[test]
fn the_wordmark_earns_frames_on_every_surface_that_draws_it() {
    let mut app = seeded_app("dark", FixtureScenario::Dashboard).expect("fixture");
    app.banner = crate::widgets::BannerClock::default();
    assert!(app.animating(), "the dashboard's mark is mid-pass at tick 0");

    app.open_configuration();
    assert!(app.animating(), "and setup draws the same mark on the same clock");

    app.banner = crate::widgets::BannerClock::new(false);
    assert!(!app.animating(), "banner_animation = false stops asking, on both");
}
