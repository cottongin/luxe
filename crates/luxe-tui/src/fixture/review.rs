//! Guided Review fixtures — the review surface at each width and stage.
//!
//! Every scenario here builds on the same pinned review source and then varies one thing: the
//! width, the mode, which findings exist, or whether a refresh is in flight. Sharing the source is
//! what makes two review screenshots comparable.

mod findings;
mod stages;

use super::*;

/// Seeds `app` for a review scenario, reporting whether this family owns it.
///
/// `Ok(false)` means another family does — the caller tries the next one. An `Err` is a fixture
/// that claimed the scenario and then could not build it, which is a bug in the fixture.
pub(super) fn seed(app: &mut App, scenario: FixtureScenario) -> Result<bool, String> {
    match scenario {
        FixtureScenario::GuidedReviewError => seed_error(app),
        FixtureScenario::GuidedReviewReady
        | FixtureScenario::GuidedReviewBrief
        | FixtureScenario::GuidedReviewSend
        | FixtureScenario::GuidedReviewCommit
        | FixtureScenario::GuidedReviewOpenPullRequest
        | FixtureScenario::GuidedReviewOpenStack
        | FixtureScenario::GuidedReviewThread
        | FixtureScenario::GuidedReviewWide
        | FixtureScenario::GuidedReviewFindingsEmpty
        | FixtureScenario::GuidedReviewFindings
        | FixtureScenario::GuidedReviewNeedsVerification
        | FixtureScenario::GuidedReviewFinish
        | FixtureScenario::GuidedReviewMedium
        | FixtureScenario::GuidedReviewNarrow
        | FixtureScenario::GuidedReviewSplit => findings::seed_working_tree_review(app, scenario)?,
        FixtureScenario::GuidedReviewDemo | FixtureScenario::GuidedReviewGuideHidden => {
            seed_demo(app, scenario)
        }
        FixtureScenario::GuidedReviewPullRequest
        | FixtureScenario::GuidedReviewPullRequestPost
        | FixtureScenario::GuidedReviewFixPullRequest => stages::seed_pull_request(app, scenario)?,
        FixtureScenario::GuidedReviewOwnPullRequest => stages::seed_own_pull_request(app)?,
        FixtureScenario::GuidedReviewLarge => seed_large(app),
        FixtureScenario::GuidedReviewNoise => seed_noise(app),
        FixtureScenario::GuidedReviewRefreshing => seed_refreshing(app),
        _ => return Ok(false),
    }
    Ok(true)
}

/// Guided Review source failure rendered through the production event reducer.
fn seed_error(app: &mut App) {
    app.current_view = ViewMode::GuidedReview;
    app.push_convo(ConvoBlock::Assistant(
        "Capturing the current working tree for Guided Review.".into(),
    ));
    let request_id = ReviewLoadRequestId::new();
    app.active_guided_review_request = Some(request_id.clone());
    app.update(Msg::Engine(Event::GuidedReviewFailed {
        request_id,
        target: ReviewTarget::WorkingTree,
        error: GuidedReviewFailure {
            kind: GuidedReviewFailureKind::Capture,
            message: "Could not capture a complete immutable working-tree source.".into(),
        },
    }));
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
}

/// The `GuidedReviewDemo` and `GuidedReviewGuideHidden` scenarios.
fn seed_demo(app: &mut App, scenario: FixtureScenario) {
    app.current_view = ViewMode::GuidedReview;
    let request_id = ReviewLoadRequestId::new();
    app.active_guided_review_request = Some(request_id.clone());
    app.update(Msg::Engine(Event::GuidedReviewLoaded {
        request_id,
        review: Box::new(super::fixture_demo_guided_review()),
    }));
    app.guided_review_workspace.guide_visible =
        scenario != FixtureScenario::GuidedReviewGuideHidden;
    // The demo's chapter prose and brief are model-shaped, so the surface has to say the
    // model wrote them. Left unset it would claim to still be writing, over prose that is
    // already there.
    app.review.provenance = Some(luxe_core::guided_review::GuideProvenance::Model { batches: 1 });
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
}

fn seed_large(app: &mut App) {
    app.current_view = ViewMode::GuidedReview;
    let request_id = ReviewLoadRequestId::new();
    app.active_guided_review_request = Some(request_id.clone());
    app.update(Msg::Engine(Event::GuidedReviewLoaded {
        request_id,
        review: Box::new(fixture_large_guided_review()),
    }));
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
}

/// Guided Review with an initially folded formatting hunk.
fn seed_noise(app: &mut App) {
    app.current_view = ViewMode::GuidedReview;
    let request_id = ReviewLoadRequestId::new();
    app.active_guided_review_request = Some(request_id.clone());
    app.update(Msg::Engine(Event::GuidedReviewLoaded {
        request_id,
        review: Box::new(fixture_guided_review_with_noise()),
    }));
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
}

/// In-flight Guided Review refresh rendered through the production event reducer.
fn seed_refreshing(app: &mut App) {
    app.current_view = ViewMode::GuidedReview;
    app.push_convo(ConvoBlock::Assistant(
        "Captured the current working tree for Guided Review.".into(),
    ));
    let ready_request = ReviewLoadRequestId::new();
    app.active_guided_review_request = Some(ready_request.clone());
    app.update(Msg::Engine(Event::GuidedReviewLoaded {
        request_id: ready_request,
        review: Box::new(fixture_guided_review()),
    }));
    let refresh_request = ReviewLoadRequestId::new();
    app.active_guided_review_request = Some(refresh_request.clone());
    app.update(Msg::Engine(Event::GuidedReviewLoading {
        request_id: refresh_request,
        target: ReviewTarget::WorkingTree,
    }));
    // The refresh fixture is about the diff staying visible, so it opens on it.
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
}
