use super::*;
use crate::app::ViewMode;

/// No still may show a state the runtime cannot reach.
///
/// Dispatch runs a message's calls strictly in order, one at a time, and a card appears when
/// its call *starts* — so at any instant exactly one card is in flight and it is the last one
/// on screen. Two fixtures had drifted into showing a spinner with finished work below it,
/// which is a lifecycle no session produces; one of them is a published site asset, which made
/// it a picture of a build nobody can run.
#[test]
fn a_fixture_never_shows_work_that_could_not_have_happened() {
    for scenario in FixtureScenario::ALL {
        let app = seeded_app("luxe", *scenario).expect("every scenario seeds");
        let in_flight = app
            .convo
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                matches!(item.block(), ConvoBlock::Tool(tool)
                    if tool.activity.state == ToolActivityState::Running)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        assert!(
            in_flight.len() <= 1,
            "{scenario:?} has {} calls in flight at once, and dispatch runs them one at a time",
            in_flight.len()
        );
        let trailing_cards = app
            .convo
            .iter()
            .skip(in_flight.first().map_or(usize::MAX, |index| index + 1))
            .filter(|item| matches!(item.block(), ConvoBlock::Tool(_)))
            .count();
        assert_eq!(
            trailing_cards, 0,
            "{scenario:?} draws a card after the call in flight, which would mean that call \
                 had already finished"
        );
    }
}

#[test]
fn seeds_stable_dashboard_and_chat_states() {
    let dashboard = seeded_app("nord", FixtureScenario::Dashboard).unwrap();
    assert_eq!(dashboard.current_view, ViewMode::Dashboard);
    let resume = dashboard.dashboard.as_ref().unwrap().resume.as_ref().unwrap();
    assert_eq!(resume.age, "12m ago");

    let chat = seeded_app("catppuccin", FixtureScenario::ChatIdle).unwrap();
    assert_eq!(chat.current_view, ViewMode::Chat);
    assert_eq!(chat.convo.len(), 2);
    assert_eq!(chat.status.model, "qwen3.8-27b");

    let wide = seeded_app("nord", FixtureScenario::ChatWideProse).unwrap();
    assert_eq!(wide.convo.len(), 1);
    assert!(matches!(wide.convo[0].block(), ConvoBlock::Assistant(_)));

    let tool_error = seeded_app("catppuccin", FixtureScenario::ChatToolError).unwrap();
    assert_eq!(tool_error.convo.len(), 3, "prose plus both rejection cards");
    assert!(matches!(tool_error.convo[1].block(), ConvoBlock::Tool(_)));
    assert!(matches!(tool_error.convo[2].block(), ConvoBlock::Tool(_)));

    let guided_review_error = seeded_app("dark", FixtureScenario::GuidedReviewError).unwrap();
    assert!(guided_review_error.active_guided_review_request.is_none());
    assert!(matches!(
        guided_review_error.convo.last().map(|item| item.block()),
        Some(ConvoBlock::System(message)) if message.starts_with("[review error]")
    ));

    let guided_review_ready = seeded_app("dark", FixtureScenario::GuidedReviewReady).unwrap();
    assert!(guided_review_ready.active_guided_review_request.is_none());
    assert!(guided_review_ready.guided_review.is_some());
    assert_eq!(guided_review_ready.guided_review.as_ref().unwrap().files.len(), 1);
    assert_eq!(
        guided_review_ready.review.session().map(|session| &session.id),
        guided_review_ready.guided_review.as_ref().map(|review| &review.id)
    );
    // Nothing about the review reaches the transcript any more: it was a status line for a
    // surface the reader was already standing on by the time they could read it.
    assert!(
        !guided_review_ready
            .convo
            .iter()
            .any(|item| matches!(item.block(), ConvoBlock::System(message)
                if message.starts_with("[review "))),
        "the review leaves the transcript alone"
    );

    for (name, scenario) in [
        ("guided-review-wide", FixtureScenario::GuidedReviewWide),
        ("guided-review-brief", FixtureScenario::GuidedReviewBrief),
        ("guided-review-send", FixtureScenario::GuidedReviewSend),
        ("guided-review-commit", FixtureScenario::GuidedReviewCommit),
        ("guided-review-large", FixtureScenario::GuidedReviewLarge),
        ("guided-review-pull-request", FixtureScenario::GuidedReviewPullRequest),
        ("guided-review-pull-request-post", FixtureScenario::GuidedReviewPullRequestPost),
        ("guided-review-own-pull-request", FixtureScenario::GuidedReviewOwnPullRequest),
        ("guided-review-open-pull-request", FixtureScenario::GuidedReviewOpenPullRequest),
        ("guided-review-fix-pull-request", FixtureScenario::GuidedReviewFixPullRequest),
        ("guided-review-open-stack", FixtureScenario::GuidedReviewOpenStack),
        ("guided-review-guide-hidden", FixtureScenario::GuidedReviewGuideHidden),
        ("guided-review-thread", FixtureScenario::GuidedReviewThread),
        ("guided-review-findings-empty", FixtureScenario::GuidedReviewFindingsEmpty),
        ("guided-review-findings", FixtureScenario::GuidedReviewFindings),
        ("guided-review-finish", FixtureScenario::GuidedReviewFinish),
        ("guided-review-medium", FixtureScenario::GuidedReviewMedium),
        ("guided-review-narrow", FixtureScenario::GuidedReviewNarrow),
    ] {
        assert_eq!(name.parse::<FixtureScenario>().unwrap(), scenario);
        let responsive_review = seeded_app("dark", scenario).unwrap();
        assert!(responsive_review.guided_review.is_some());
        // `auto` is the default layout, so a review opens side by side and falls back to unified
        // only where the pane is too narrow for two readable columns.
        assert_eq!(
            responsive_review.guided_review_workspace.layout,
            luxe_core::config::DiffLayout::Auto
        );
    }

    let guided_review_needs_verification =
        seeded_app("dark", FixtureScenario::GuidedReviewNeedsVerification).unwrap();
    let selected_id = guided_review_needs_verification
        .guided_review_workspace
        .selected_finding_id
        .as_ref()
        .expect("selected verification fixture finding");
    assert_eq!(
        guided_review_needs_verification
            .review
            .session()
            .and_then(|session| session.findings.iter().find(|finding| &finding.id == selected_id))
            .map(|finding| finding.state),
        Some(FindingState::NeedsVerification)
    );

    let guided_review_finish = seeded_app("dark", FixtureScenario::GuidedReviewFinish).unwrap();
    assert_eq!(
        guided_review_finish.guided_review_workspace.finish_review_choice,
        Some(crate::guided_review::FinishReviewChoice::Accept)
    );
    assert!(guided_review_finish.guided_review_workspace.finish_review_accept_confirmed);

    let guided_review_split = seeded_app("dark", FixtureScenario::GuidedReviewSplit).unwrap();
    assert!(guided_review_split.guided_review.is_some());
    assert_eq!(
        guided_review_split.guided_review_workspace.layout,
        luxe_core::config::DiffLayout::Split
    );
    assert_eq!(
        "guided-review-split".parse::<FixtureScenario>().unwrap(),
        FixtureScenario::GuidedReviewSplit
    );

    let guided_review_refreshing =
        seeded_app("dark", FixtureScenario::GuidedReviewRefreshing).unwrap();
    assert!(guided_review_refreshing.active_guided_review_request.is_some());
    assert!(guided_review_refreshing.guided_review.is_some());
    assert_eq!(
        guided_review_refreshing.guided_review_workspace.load_state,
        crate::guided_review::GuidedReviewLoadState::Loading {
            target: luxe_core::guided_review::ReviewTarget::WorkingTree
        },
        "a refresh in flight is state on the surface, not a line in the transcript"
    );

    for scenario in
        [FixtureScenario::DockWide, FixtureScenario::DockOverlay, FixtureScenario::DockCompact]
    {
        let dock = seeded_app("nord", scenario).unwrap();
        assert!(dock.workspace.is_some());
        assert_eq!(dock.convo.len(), 1);
        assert_eq!(dock.dock.runtime_override, crate::workspace_dock::DockRuntimeOverride::Shown);
    }
}

#[test]
fn rejects_unknown_theme_and_scenario() {
    assert!(seeded_app("missing", FixtureScenario::Dashboard).is_err());
    assert!("missing".parse::<FixtureScenario>().is_err());
}

/// What the capture scripts and the site pass, in [`FixtureScenario::ALL`]'s order. A literal, so
/// renaming a scenario has to change it here as well as in the scripts that would stop working.
const SCRIPT_NAMES: [&str; 60] = [
    "dashboard",
    "dashboard-context-files",
    "dashboard-typing",
    "dashboard-minimal",
    "configuration",
    "setup",
    "setup-tools",
    "settings",
    "settings-tools",
    "chat-idle",
    "chat-thinking",
    "chat-streaming",
    "chat-tools",
    "chat-card-bodies",
    "chat-wide-prose",
    "chat-tool-error",
    "chat-tool-run",
    "chat-question",
    "chat-plan",
    "chat-plan-ready",
    "chat-turn",
    "chat-permission",
    "chat-palette",
    "chat-screenshot",
    "guided-review-error",
    "guided-review-ready",
    "guided-review-wide",
    "guided-review-demo",
    "guided-review-guide-hidden",
    "guided-review-brief",
    "guided-review-send",
    "guided-review-commit",
    "guided-review-large",
    "guided-review-pull-request",
    "guided-review-pull-request-post",
    "guided-review-own-pull-request",
    "guided-review-open-pull-request",
    "guided-review-fix-pull-request",
    "guided-review-open-stack",
    "guided-review-thread",
    "guided-review-findings-empty",
    "guided-review-findings",
    "guided-review-needs-verification",
    "guided-review-finish",
    "guided-review-medium",
    "guided-review-narrow",
    "guided-review-split",
    "guided-review-noise",
    "guided-review-refreshing",
    "session-picker",
    "chat-pending",
    "chat-paste",
    "dock-wide",
    "dock-pull-request",
    "dock-overlay",
    "dock-compact",
    "changes-wide",
    "changes-narrow",
    "changes-failed",
    "changes-external",
];

/// Every scenario parses from the name the scripts use, and that name is the one it reports.
///
/// The break this catches: a scenario renamed or added in one of the places a name is spelled and
/// not the others, which used to be three: the parser, the runner's title, and its usage line.
#[test]
fn every_scenario_round_trips_through_its_script_name() {
    assert_eq!(FixtureScenario::ALL.len(), SCRIPT_NAMES.len());
    for (scenario, name) in FixtureScenario::ALL.iter().zip(SCRIPT_NAMES) {
        assert_eq!(name.parse::<FixtureScenario>(), Ok(*scenario), "{name}");
        assert_eq!(scenario.name(), name);
    }
}
