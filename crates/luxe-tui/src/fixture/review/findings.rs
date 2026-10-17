//! Review triage fixtures — comments in each of their states, a thread, and a line comment on the
//! working-tree review.

use super::*;

/// The working-tree review the triage and stage scenarios share: loaded, laid out, with its
/// feedback window open where the scenario is about feedback, then each scenario's own layer.
pub(super) fn seed_working_tree_review(
    app: &mut App,
    scenario: FixtureScenario,
) -> Result<(), String> {
    app.current_view = ViewMode::GuidedReview;
    app.push_convo(ConvoBlock::Assistant(
        "Captured the current working tree for Guided Review.".into(),
    ));
    let request_id = ReviewLoadRequestId::new();
    app.active_guided_review_request = Some(request_id.clone());
    app.update(Msg::Engine(Event::GuidedReviewLoaded {
        request_id,
        review: Box::new(fixture_guided_review()),
    }));
    if scenario == FixtureScenario::GuidedReviewSplit {
        app.guided_review_workspace.layout = luxe_core::config::DiffLayout::Split;
    }
    if matches!(
        scenario,
        FixtureScenario::GuidedReviewFindingsEmpty
            | FixtureScenario::GuidedReviewFindings
            | FixtureScenario::GuidedReviewNeedsVerification
    ) {
        app.guided_review_workspace.findings_visible = true;
    }
    seed_review_findings(app, scenario)?;
    if scenario == FixtureScenario::GuidedReviewThread {
        seed_review_thread(app)?;
    }
    super::stages::seed_review_stage(app, scenario)?;
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
    Ok(())
}

/// Comments in each state for the triage scenarios, the one in need of verification selected
/// where that is the point, and a line comment under the wide review's code.
fn seed_review_findings(app: &mut App, scenario: FixtureScenario) -> Result<(), String> {
    if matches!(
        scenario,
        FixtureScenario::GuidedReviewFindings
            | FixtureScenario::GuidedReviewNeedsVerification
            | FixtureScenario::GuidedReviewSend
            | FixtureScenario::GuidedReviewCommit
            | FixtureScenario::GuidedReviewThread
            | FixtureScenario::GuidedReviewFinish
    ) {
        let session = app.review.session_mut().ok_or("missing fixture review session")?;
        for (title, state, severity) in [
            ("Retry path omits a bounded error", FindingState::Open, FindingSeverity::Blocking),
            (
                "Confirm the migration handles old data",
                FindingState::NeedsVerification,
                FindingSeverity::Issue,
            ),
            ("Clarify the cancellation behavior", FindingState::Draft, FindingSeverity::Suggestion),
            ("Legacy formatter note", FindingState::Resolved, FindingSeverity::Note),
            ("Original context no longer maps", FindingState::Stale, FindingSeverity::Note),
        ] {
            session
                .add_finding(ReviewFinding {
                    posted: None,
                    github_thread: None,
                    id: FindingId::new(),
                    label: String::new(),
                    anchor: FindingAnchor::Review,
                    severity,
                    title: title.into(),
                    body: "Fixture feedback retained for visual triage.".into(),
                    proposed_fix: None,
                    state,
                    thread: Vec::new(),
                })
                .map_err(|error| error.to_string())?;
        }
        let findings =
            app.review.session().ok_or("missing fixture review session")?.findings.clone();
        app.guided_review_workspace.move_finding_selection(&findings, true);
        if scenario == FixtureScenario::GuidedReviewNeedsVerification {
            let finding_id = findings
                .iter()
                .find(|finding| finding.state == FindingState::NeedsVerification)
                .map(|finding| finding.id.clone())
                .ok_or("missing verification fixture finding")?;
            app.guided_review_workspace.selected_finding_id = Some(finding_id);
        }
    }
    if scenario == FixtureScenario::GuidedReviewWide {
        // Anchored to a line that is on screen, because that is where a comment renders
        // now: under the code it is about.
        if let Some(anchor) = review_line_anchor(app.guided_review.as_deref()) {
            app.review
                .session_mut()
                .ok_or("missing fixture review session")?
                .add_finding(ReviewFinding {
                    posted: None,
                    github_thread: None,
                    id: FindingId::new(),
                    label: String::new(),
                    anchor,
                    severity: FindingSeverity::Issue,
                    title: "The borrow buys nothing here".into(),
                    body: "Category is Copy and label() returns 'static, so &self is \
                           doing no work \u{2014} take self by value?"
                        .into(),
                    proposed_fix: None,
                    state: FindingState::Draft,
                    thread: Vec::new(),
                })
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// A comment the agent claims to have fixed, and the reviewer's answer to it, open in its
/// details.
fn seed_review_thread(app: &mut App) -> Result<(), String> {
    let session = app.review.session_mut().ok_or("missing fixture review session")?;
    let finding_id = session
        .findings
        .iter()
        .find(|finding| finding.state == FindingState::NeedsVerification)
        .map(|finding| finding.id.clone())
        .ok_or("missing verification fixture finding")?;
    if let Some(finding) = session.findings.iter_mut().find(|finding| finding.id == finding_id) {
        finding.thread.push(luxe_core::guided_review::FindingMessage {
            author: luxe_core::guided_review::FindingMessageAuthor::Agent,
            body: "Fixed by bounding the retry loop and returning a typed timeout.".into(),
            created_at_ms: 1,
        });
    }
    session
        .append_reviewer_message(
            &finding_id,
            "The timeout is bounded now, but the caller still ignores it.".into(),
            2,
        )
        .map_err(|error| error.to_string())?;
    app.guided_review_workspace.selected_finding_id = Some(finding_id);
    app.guided_review_workspace.findings_visible = true;
    app.guided_review_workspace.toggle_finding_details();
    Ok(())
}

/// A line anchor on the first source line of the first hunk, so a fixture comment lands where the
/// reviewer is already looking.
pub(super) fn review_line_anchor(
    review: Option<&luxe_core::guided_review::GuidedReview>,
) -> Option<FindingAnchor> {
    let file = review?.files.first()?;
    let hunk = file.hunks.first()?;
    // An addition, so the comment sits under a line the change actually made.
    let line = hunk
        .lines
        .iter()
        .find(|line| line.kind == luxe_core::guided_review::ReviewLineKind::Addition)?;
    Some(FindingAnchor::Line {
        file_id: file.id.clone(),
        hunk_id: hunk.id.clone(),
        side: luxe_core::guided_review::DiffSide::New,
        line: line.new_line?,
        context_hash: line.context_hash(),
    })
}
