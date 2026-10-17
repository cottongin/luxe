//! Wire → runtime action translation.
//!
//! One exhaustive `match` from the published [`WireAction`] vocabulary to the engine's own
//! [`Action`]. It lives here rather than in `luxe-core` because two arms need the model catalog,
//! which is a `luxe-cli` concern — and a translator that punts on exactly the interesting cases is
//! how the previous half-bridge ended up with no callers.

use luxe_core::engine::Action;
use luxe_core::event::Action as WireAction;

/// Why an inbound frame produced nothing.
///
/// Carries the `kind` so the rejection sent back to the client can name what it refused, which is
/// the difference between a client author fixing their frame and guessing at it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rejected {
    /// The wire `kind` that was refused, when the frame parsed far enough to have one.
    pub(crate) kind: Option<String>,
    /// What was wrong with it, in the same voice as a permission denial: lowercase, no period.
    pub(crate) reason: String,
}

impl Rejected {
    /// A frame whose `kind` this build does not implement.
    fn unknown(kind: impl Into<String>) -> Self {
        let kind = kind.into();
        Self { reason: format!("no such action `{kind}`"), kind: Some(kind) }
    }
}

/// What this session lets a client do, decided once at launch.
///
/// The translator is the one choke point every inbound frame passes, so the RPC mode's own rules
/// live here rather than in the engine: the TUI's GitHub writes are confirmed by a person in a
/// preview, and nothing about them changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Policy {
    /// Whether actions that publish to GitHub are accepted
    pub(crate) github_writes: GithubWrites,
    /// The `/github` sections a bare `list_github` runs: the user's configured ones
    pub(crate) github_sections: Vec<luxe_core::config::GithubSection>,
}

/// Whether a client may publish to GitHub, and if not, why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GithubWrites {
    /// Launched with `--github-writes`
    Allowed,
    /// The default: writes need the flag
    NotEnabled,
    /// A read-only session refuses them, flag or no flag
    ReadOnly,
}

impl GithubWrites {
    /// The answer for a launch: read-only wins over the flag, so turning on writes can never
    /// loosen a session someone made read-only.
    pub(crate) fn for_launch(flag: bool, mode: luxe_core::permission::PermissionMode) -> Self {
        match (mode, flag) {
            (luxe_core::permission::PermissionMode::ReadOnly, _) => Self::ReadOnly,
            (_, true) => Self::Allowed,
            (_, false) => Self::NotEnabled,
        }
    }

    /// Whether writes are accepted, for the `ready` frame.
    pub(crate) fn allowed(self) -> bool {
        self == Self::Allowed
    }

    /// Why `kind` was refused, when it was.
    fn refusal(self, kind: &str) -> Option<String> {
        match self {
            Self::Allowed => None,
            Self::NotEnabled => Some(format!(
                "{kind} publishes to GitHub, which this session does not allow: start Luxe with \
                 --github-writes to let a client do that"
            )),
            Self::ReadOnly => Some(format!(
                "{kind} publishes to GitHub, and this session is read-only, which --github-writes \
                 does not override"
            )),
        }
    }
}

/// Resolves a model reference to the full metadata the engine needs.
///
/// Injected rather than called directly because the real one reads the user's `models.json`, and a
/// test whose outcome depends on the developer's config is not a test.
pub(crate) trait ModelResolver {
    /// The catalog's answer for `provider`/`id`.
    ///
    /// Total: an id no layer recognizes still resolves, to provider-aware defaults. That is the
    /// same contract `--model` has always had, and it is what makes a local or proxied model
    /// usable without an entry in anyone's catalog.
    fn resolve(&self, provider: &str, id: &str) -> luxe_core::ModelInfo;
}

/// The real catalog: configured models first, models.dev filling the gaps, defaults under that.
pub(crate) struct CatalogResolver;

impl ModelResolver for CatalogResolver {
    fn resolve(&self, provider: &str, id: &str) -> luxe_core::ModelInfo {
        let config = crate::engine_setup::load_luxe_config();
        crate::model_catalog::resolve_model(&config, provider, id).0
    }
}

/// Translate one wire action into the engine action it stands for.
///
/// Every arm is a total mapping or a named refusal — there is no silent drop, because a front-end
/// whose frame vanished has no way to tell that from an engine that is merely slow.
pub(crate) fn to_engine_action(
    wire: WireAction,
    models: &dyn ModelResolver,
    policy: &Policy,
) -> Result<Action, Rejected> {
    if wire.writes_to_github() {
        if let Some(reason) = policy.github_writes.refusal(wire.kind()) {
            return Err(Rejected { kind: Some(wire.kind().to_string()), reason });
        }
    }
    if wire.writes_the_repository() && policy.github_writes == GithubWrites::ReadOnly {
        let kind = wire.kind();
        let reason = format!("{kind} writes to the repository, and this session is read-only");
        return Err(Rejected { kind: Some(kind.to_string()), reason });
    }
    Ok(match wire {
        WireAction::SubmitUserMessage { text, images } => {
            Action::SubmitUserMessage { content: text, images, remediation: None }
        }
        WireAction::SteerTurn { text } => Action::SteerTurn { content: text },
        WireAction::WithdrawSteering => Action::WithdrawSteering,
        WireAction::Cancel => Action::Cancel,
        WireAction::EditResend { text } => Action::EditResend { content: text },
        WireAction::Retry => Action::Retry,
        WireAction::Regenerate => Action::Regenerate,
        WireAction::RespondPermission { id, reply } => Action::RespondPermission { id, reply },
        WireAction::AnswerQuestion { id, reply } => Action::AnswerQuestion { id, reply },
        WireAction::SetPermissionAutoApprove(on) => Action::SetPermissionAutoApprove(on),
        WireAction::SelectModel(model) => {
            Action::SetModel(models.resolve(&model.provider, &model.model))
        }
        WireAction::SetThinkingLevel(level) => Action::SetThinkingLevel(level),
        // The mode's model and thinking level come from config, so a client names the mode alone
        WireAction::SetMode(mode) => Action::SetMode(mode),
        WireAction::RequestReview { focus } => Action::RequestReview { focus },
        WireAction::NewSession => Action::NewSession,
        WireAction::LoadSession(id) => Action::LoadSession(id),
        WireAction::RefreshSessions => Action::RefreshSessions,
        WireAction::RefreshWorkspace => Action::RefreshWorkspace,
        WireAction::RequestContextComposition => Action::RequestContextComposition,
        WireAction::CompactNow { focus } => Action::CompactNow { focus },
        WireAction::LoadGuidedReview { request_id, target } => {
            Action::LoadGuidedReview { request_id, target }
        }
        WireAction::EditReview { request_id, review_id, edit } => {
            Action::EditReview { request_id, review_id, edit }
        }
        WireAction::RecordReviewProgress { review_id, walked_chapter_ids } => {
            Action::RecordReviewProgress {
                review_id,
                progress: luxe_core::guided_review::ReviewProgress {
                    walked_chapter_ids,
                    ..Default::default()
                },
            }
        }
        WireAction::RecordReviewSeen { review_id } => Action::RecordReviewSeen { review_id },
        WireAction::RefreshGuidedReview { request_id, review_id, revision_id } => {
            Action::RefreshGuidedReview { request_id, review_id, expected_revision: revision_id }
        }
        WireAction::CommitReviewChapters {
            request_id,
            review_id,
            revision_id,
            chapter_ids,
            squash,
        } => Action::CommitReviewChapters {
            request_id,
            review_id,
            expected_revision: revision_id,
            chapter_ids,
            squash,
        },
        WireAction::PreparePullRequest { request_id, review_id } => {
            Action::PreparePullRequest { request_id, review_id }
        }
        WireAction::OpenPullRequest { request_id, title, body, draft } => {
            Action::OpenPullRequest { request_id, title, body, draft }
        }
        WireAction::PrepareStack { request_id, review_id } => {
            Action::PrepareStack { request_id, review_id }
        }
        WireAction::OpenStack { request_id, draft } => Action::OpenStack { request_id, draft },
        WireAction::PreparePullRequestPost { request_id, review_id } => {
            Action::PreparePullRequestPost { request_id, review_id }
        }
        WireAction::PostPullRequestReview { request_id, verdict, session_revision } => {
            Action::PostPullRequestReview { request_id, verdict, session_revision }
        }
        WireAction::PrepareThreadAnswers { request_id, review_id } => {
            Action::PrepareThreadAnswers { request_id, review_id }
        }
        WireAction::AnswerThreads { request_id, review_id, answers } => {
            Action::AnswerThreads { request_id, review_id, answers }
        }
        WireAction::ListGithub { sections } => Action::ListGithub {
            sections: sections.unwrap_or_else(|| policy.github_sections.clone()),
        },
        WireAction::SearchGithub { request, section, after } => {
            Action::SearchGithub { request, section, after }
        }
        WireAction::ReadGithubOverview { repository, number, pull_request } => {
            Action::ReadGithubOverview { repository, number, pull_request }
        }
        WireAction::ReadGithubConversation { repository, number, pull_request } => {
            Action::ReadGithubConversation { repository, number, pull_request }
        }
        WireAction::DraftGithubReply(request) => Action::DraftGithubReply(request),
        WireAction::CheckPullRequestFix { number, repository } => {
            Action::CheckPullRequestFix { number, repository }
        }
        WireAction::PlanRestack { request_id } => Action::PlanRestack { request_id },
        WireAction::CheckWorktreeRemoval => Action::CheckWorktreeRemoval,
        WireAction::LoadFileDiff { request_id, path } => Action::LoadFileDiff { request_id, path },
        WireAction::ListDirectory { request_id, path, include_ignored } => {
            Action::ListDirectory { request_id, path, include_ignored }
        }
        WireAction::ReadProjectFile { request_id, path } => {
            Action::ReadProjectFile { request_id, path }
        }
        WireAction::SetFocus { focused } => Action::FocusChanged { focused },
        WireAction::LinkPullRequest { number, url } => Action::LinkPullRequest { number, url },
        WireAction::PostGithubComment(post) => Action::PostGithubComment(post),
        WireAction::ChangePullRequest { repository, number, write } => {
            Action::ChangePullRequest { repository, number, write }
        }
        WireAction::Restack { request_id } => Action::Restack { request_id },
        WireAction::Close => Action::Close,
        WireAction::Other { kind, .. } => return Err(Rejected::unknown(kind)),
        // `WireAction` is `#[non_exhaustive]`, so this crate cannot match it exhaustively even
        // while it does cover every variant. A variant published upstream without an arm here
        // lands on this line and is refused by name rather than silently dropped;
        // `the_translator_covers_the_whole_published_vocabulary` is what stops it shipping that
        // way in the first place.
        future => return Err(Rejected::unknown(future.kind())),
    })
}

#[cfg(test)]
mod tests;
