//! Review stage fixtures — the post, opening, stack, fix and answers dialogs, and the pull requests
//! they act on.

use super::findings::review_line_anchor;
use super::*;

/// The dialog a stage scenario is about: the commit, the opening, the stack, the send, or the
/// finish with its blocking confirmation armed.
pub(super) fn seed_review_stage(app: &mut App, scenario: FixtureScenario) -> Result<(), String> {
    if scenario == FixtureScenario::GuidedReviewCommit {
        app.guided_review_workspace.open_commit_preview();
    }
    if scenario == FixtureScenario::GuidedReviewOpenPullRequest {
        open_pull_request(app)?;
    }
    if scenario == FixtureScenario::GuidedReviewOpenStack {
        let layer = |head: &str, base: &str, title: &str| luxe_core::github::NewPullRequest {
            head: head.into(),
            base: base.into(),
            title: title.into(),
            body: String::new(),
            draft: false,
        };
        app.guided_review_workspace.stack_opening = Some(crate::guided_review::StackOpening {
            request_id: luxe_core::guided_review::PullRequestOpenId::new(),
            layers: Some(vec![
                layer("123-category-enum-part-1", "main", "Shared notification model"),
                layer("123-category-enum-part-2", "123-category-enum-part-1", "Route by category"),
                layer("123-category-enum", "123-category-enum-part-2", "Reject unknown categories"),
            ]),
            draft: false,
            opening: false,
            error: None,
        });
    }
    if scenario == FixtureScenario::GuidedReviewSend {
        app.guided_review_workspace.open_send_preview(None, None);
    }
    if scenario == FixtureScenario::GuidedReviewFinish {
        app.guided_review_workspace.open_finish_review(true);
        // Land on Accept (Request fixes → Accept and commit → Accept) with the blocking
        // confirmation armed, which is the state worth screenshotting.
        app.guided_review_workspace.move_finish_review_choice(false);
        app.guided_review_workspace.move_finish_review_choice(false);
        app.guided_review_workspace.confirm_finish_review_accept();
    }
    Ok(())
}

/// The `GuidedReviewPullRequest`, `GuidedReviewPullRequestPost` and `GuidedReviewFixPullRequest`
/// scenarios.
pub(super) fn seed_pull_request(app: &mut App, scenario: FixtureScenario) -> Result<(), String> {
    app.current_view = ViewMode::GuidedReview;
    let request_id = ReviewLoadRequestId::new();
    app.active_guided_review_request = Some(request_id.clone());
    app.update(Msg::Engine(Event::GuidedReviewLoaded {
        request_id,
        review: Box::new(super::fixture_pull_request_review()),
    }));
    app.review.provenance = Some(luxe_core::guided_review::GuideProvenance::Model { batches: 1 });
    if scenario == FixtureScenario::GuidedReviewPullRequestPost {
        pull_request_post(app)?;
    }
    if scenario == FixtureScenario::GuidedReviewFixPullRequest {
        // The comments of the post scenario, offered to the agent in a worktree instead
        pull_request_post(app)?;
        app.guided_review_workspace.github_post = None;
        let comments = app.review.session().map_or(0, |s| s.findings.len());
        app.guided_review_workspace.pull_request_fix = Some(crate::guided_review::PullRequestFix {
            number: 42,
            branch: Some("fix-redirects".into()),
            refusal: None,
            comments,
        });
    }
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
    Ok(())
}

/// Your own pull request's review, answering its threads on GitHub.
pub(super) fn seed_own_pull_request(app: &mut App) -> Result<(), String> {
    app.current_view = ViewMode::GuidedReview;
    let mut review = super::fixture_pull_request_review();
    // Your own: the same pull request, compared to the working tree it is fixed in
    review.target = ReviewTarget::OwnPullRequest { number: 42 };
    review.source.base = luxe_core::guided_review::ReviewBase::MergeBase {
        reference: "main".into(),
        oid: "4f1c2d9a7e6b5c4d3e2f1a0b9c8d7e6f5a4b3c2d".into(),
    };
    let request_id = ReviewLoadRequestId::new();
    app.active_guided_review_request = Some(request_id.clone());
    app.update(Msg::Engine(Event::GuidedReviewLoaded {
        request_id,
        review: Box::new(review.clone()),
    }));
    bind_session(app, &review);
    app.review.provenance = Some(luxe_core::guided_review::GuideProvenance::Model { batches: 1 });
    let session = app.review.session_mut().ok_or("missing fixture review session")?;
    if let Some(first) = session.findings.iter_mut().find(|f| f.github_thread.is_some()) {
        first.state = FindingState::Resolved;
    }
    let findings = session.findings.clone();
    let mut answers = crate::guided_review::ThreadAnswers::new(
        luxe_core::guided_review::ThreadAnswersId::new(),
        &findings,
    );
    answers.prepared = Some((false, false));
    if let Some(row) = answers.rows.first_mut() {
        row.reply = "Only the webhook built it from a string, and it parses first now.".into();
    }
    app.guided_review_workspace.thread_answers = Some(answers);
    app.status.model = "qwen3.8-27b".into();
    app.status.provider = "lmstudio".into();
    Ok(())
}

/// Three comments on the pull request and the post preview GitHub's answer opens: one on the review
/// as a whole, one on the line that carries the new type, and a question the agent has already
/// answered, which is why it is not in the preview.
///
/// Titles are the bodies' first lines, which is all a title ever is: there is no field for one.
fn pull_request_post(app: &mut App) -> Result<(), String> {
    let review = app.guided_review.as_deref().ok_or("missing fixture review")?.clone();
    let line = review_line_anchor(Some(&review)).ok_or("missing fixture line")?;
    let session = app.review.session_mut().ok_or("missing fixture review session")?;
    let answer = luxe_core::guided_review::FindingMessage {
        author: luxe_core::guided_review::FindingMessageAuthor::Agent,
        body: "So the webhook can answer an unknown category with a 422 that names it. A serde \
               failure would reject the whole payload with a generic error."
            .into(),
        created_at_ms: 1,
    };
    for (anchor, severity, body, state, thread) in [
        (
            FindingAnchor::Review,
            FindingSeverity::Issue,
            "An unknown category now fails the whole webhook with a `422`, and older senders still \
             post `alert`.\n\nCan we log and drop those for one release before rejecting them \
             outright?",
            FindingState::Draft,
            Vec::new(),
        ),
        (
            line,
            FindingSeverity::Suggestion,
            "Worth a doc comment here: this is parsed once, in the webhook, so nothing downstream \
             should build a category from a raw string again.",
            FindingState::Draft,
            Vec::new(),
        ),
        (
            FindingAnchor::Review,
            FindingSeverity::Question,
            "Why `FromStr` here rather than serde's `rename_all` on the enum?",
            FindingState::Open,
            vec![answer],
        ),
    ] {
        let first = body.lines().next().unwrap_or(body);
        let title = if first.chars().count() <= 80 {
            first.to_owned()
        } else {
            format!("{}…", first.chars().take(79).collect::<String>().trim_end())
        };
        session
            .add_finding(ReviewFinding {
                posted: None,
                github_thread: None,
                id: FindingId::new(),
                label: String::new(),
                anchor,
                severity,
                title,
                body: body.into(),
                proposed_fix: None,
                state,
                thread,
            })
            .map_err(|error| error.to_string())?;
    }
    // GitHub's patch shows every line the capture does, which is the common case
    let patches = review
        .files
        .iter()
        .filter_map(|file| {
            let path = file.new_path.as_ref().or(file.old_path.as_ref())?;
            let mut commentable = luxe_core::github::CommentableLines::default();
            for line in file.hunks.iter().flat_map(|hunk| &hunk.lines) {
                commentable.left.extend(line.old_line);
                commentable.right.extend(line.new_line);
            }
            Some(luxe_core::github::FilePatch {
                path: luxe_core::paths::display(path),
                previous_path: None,
                commentable,
            })
        })
        .collect();
    app.guided_review_workspace.github_post = Some(crate::guided_review::GithubPost {
        request_id: luxe_core::guided_review::PullRequestPostId::new(),
        verdict: luxe_core::github::ReviewEvent::RequestChanges,
        prepared: Some(crate::guided_review::PreparedPost {
            patches,
            head_moved: false,
            own_pull_request: false,
        }),
        posting: false,
        error: None,
        selected: 0,
        scroll: 0,
        drafting: None,
        drafting_since: None,
        draft_note: None,
    });
    Ok(())
}

/// The preview of a branch's pull request, drafted from its review for issue #123: its work not
/// committed yet, and the branch never pushed.
fn open_pull_request(app: &mut App) -> Result<(), String> {
    let review = app.guided_review.as_ref().ok_or("missing fixture review")?;
    let issue = luxe_core::guided_review::ClosedIssue { number: 123, title: None };
    let draft = luxe_core::guided_review::pull_request_draft(review, Some(&issue));
    let commits = luxe_core::guided_review::chapter_commits(review, &[])
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|commit| commit.subject)
        .collect();
    app.guided_review_workspace.pull_request_opening =
        Some(crate::guided_review::PullRequestOpening {
            request_id: luxe_core::guided_review::PullRequestOpenId::new(),
            prepared: Some(crate::guided_review::PreparedOpening {
                pull_request: luxe_core::github::NewPullRequest {
                    head: "123-category-enum".into(),
                    base: "main".into(),
                    title: draft.title,
                    body: draft.body,
                    draft: false,
                },
                publication: luxe_core::github::Publication::Unpublished,
                uncommitted: 4,
                commits,
            }),
            editing: None,
            body_scroll: 0,
            body_room: std::cell::Cell::new(0),
            opening: false,
            error: None,
        });
    Ok(())
}

/// Answers a loaded review with the session the engine's review desk would bind to it: new, with
/// the open threads of your own pull request already adopted as findings.
fn bind_session(app: &mut App, review: &luxe_core::guided_review::GuidedReview) {
    let mut session = luxe_core::guided_review::ReviewSession::new(
        review.id.clone(),
        review.target.clone(),
        review.source.clone(),
        review.generated_at_ms,
    );
    if matches!(review.target, ReviewTarget::OwnPullRequest { .. }) {
        session.adopt_github_threads(luxe_core::guided_review::thread_findings(review));
    }
    app.update(Msg::Engine(Event::ReviewSessionRestored { session: Box::new(session) }));
}
