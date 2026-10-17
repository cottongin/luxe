//! Workspace data — project identity collection at the binary boundary.
//!
//! The collector owns project inspection so front-ends only reduce the resulting
//! [`luxe_core::WorkspaceSnapshot`].

use crate::git_probe;
use luxe_core::external_work::ExternalWork;
use luxe_core::lsp::{DiagnosticsStore, Severity};
use luxe_core::redact::redact_secrets;
use luxe_core::{
    DiagnosticsSummary, FileDiagnostics, ProjectIdentity, ReviewSummary, SessionSummary,
    WorkspaceScopeId, WorkspaceSnapshot,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

mod changes;
mod external;
mod todos;

use changes::collect_change_summary;
use external::collect_external_summary;
use todos::collect_todo_summary;

static NEXT_SCOPE: AtomicU64 = AtomicU64::new(1);

/// Collects revisioned workspace snapshots for one opened project.
pub(crate) struct WorkspaceCollector {
    /// The checkout the agent works in: changes, identity, diagnostics
    root: PathBuf,
    /// The project whose `.luxe/` holds the session's todos and transcript
    state_root: PathBuf,
    scope_id: WorkspaceScopeId,
    revision: u64,
    session: Option<SessionSummary>,
    /// Whether the session title has been derived from the first user message yet.
    session_title_resolved: bool,
    review: Option<ReviewSummary>,
    /// Shared cache of the latest per-file LSP diagnostics, published by the edit hook.
    diagnostics: DiagnosticsStore,
    /// Shared warm LSP service, read for running-server state (`/lsp`). `None` in tests.
    lsp_service: Option<crate::engine_setup::SharedLspService>,
    /// Shared warm DAP service, read for live debug sessions (`/debug`). `None` in tests.
    dap_service: Option<std::sync::Arc<luxe_dap::DapService>>,
    /// Shared record of what this session changed outside the project, written by the hook.
    external: ExternalWork,
    /// The branch's pull request, as the watch last reported it
    pull_request: luxe_core::workspace::PullRequestPresence,
}

/// Bounded number of offending files carried on the snapshot's diagnostics roll-up.
const DIAGNOSTICS_FILE_LIMIT: usize = 8;

impl WorkspaceCollector {
    /// Creates a collector for one project-open generation, reading the process-wide diagnostics
    /// cache the edit hook publishes to.
    pub(crate) fn new(root: PathBuf) -> Self {
        let mut collector =
            Self::with_diagnostics_store(root, crate::engine_setup::shared_diagnostics_store());
        collector.lsp_service = Some(crate::engine_setup::build_lsp_service());
        collector.dap_service = Some(crate::engine_setup::shared_dap_service());
        collector
    }

    fn with_diagnostics_store(root: PathBuf, diagnostics: DiagnosticsStore) -> Self {
        let generation = NEXT_SCOPE.fetch_add(1, Ordering::Relaxed);
        let scope_id = WorkspaceScopeId::new(format!("{}-{generation}", std::process::id()));
        Self {
            state_root: root.clone(),
            root,
            scope_id,
            revision: 0,
            session: None,
            session_title_resolved: false,
            review: None,
            diagnostics,
            lsp_service: None,
            dap_service: None,
            external: crate::engine_setup::shared_external_work(),
            pull_request: luxe_core::workspace::PullRequestPresence::default(),
        }
    }

    /// Reads the session's state from `state_root` rather than the checkout it works in.
    pub(crate) fn with_state_root(mut self, state_root: PathBuf) -> Self {
        self.state_root = state_root;
        self
    }

    /// Reads external work from an explicit record rather than the process-wide one.
    #[cfg(test)]
    fn with_external_work(mut self, external: ExternalWork) -> Self {
        self.external = external;
        self
    }

    /// Records the session that owns later snapshots. Its title starts empty and fills in from the
    /// first user message on the next [`WorkspaceCollector::collect`] — a brand-new session stays
    /// untitled (no session id shown). Its plan comes from the session's own checklist, empty for
    /// a fresh session, so the dock reads clean until work begins.
    pub(crate) fn set_session(&mut self, session_id: impl Into<String>) {
        let id = session_id.into();
        self.session = Some(SessionSummary { id, title: String::new(), updated_at_ms: now_ms() });
        self.session_title_resolved = false;
    }

    /// Records the immutable review currently selected by the workspace.
    ///
    /// The finding count comes from the feedback session, not from here: a freshly loaded review
    /// has none yet, and [`Self::set_review_findings`] keeps the dock honest as they accumulate.
    pub(crate) fn set_guided_review(&mut self, review: &luxe_core::guided_review::GuidedReview) {
        let unresolved_findings = self
            .review
            .as_ref()
            .filter(|summary| summary.id == review.id.as_str())
            .map_or(0, |summary| summary.unresolved_findings);
        self.review = Some(ReviewSummary {
            id: review.id.as_str().to_string(),
            title: review.context.title.clone(),
            source: luxe_core::guided_review::pull_request_label(review)
                .unwrap_or_else(|| review_source_label(&review.target)),
            unresolved_findings,
            completed_at_ms: review.generated_at_ms,
        });
    }

    /// Updates the dock's open-finding count from the persisted feedback session.
    ///
    /// It was hardcoded to zero, so the REVIEW section always claimed "0 open findings" no matter
    /// how many comments the reviewer had written.
    pub(crate) fn set_review_findings(
        &mut self,
        session: &luxe_core::guided_review::ReviewSession,
    ) {
        use luxe_core::guided_review::FindingState;
        let unresolved_findings = session
            .findings
            .iter()
            .filter(|finding| {
                matches!(
                    finding.state,
                    FindingState::Draft | FindingState::Open | FindingState::NeedsVerification
                )
            })
            .count();
        match self.review.as_mut() {
            Some(summary) if summary.id == session.id.as_str() => {
                summary.unresolved_findings = unresolved_findings;
            }
            _ => {}
        }
    }

    /// Collects the project identity section without blocking the runtime.
    pub(crate) async fn collect(&mut self) -> WorkspaceSnapshot {
        self.revision += 1;
        self.resolve_session_title().await;
        let todo_root = self.state_root.clone();
        let change_root = self.root.clone();
        // The plan follows the *session's* checklist (empty for a fresh session), so the dock never
        // carries an unrelated session's todos.
        let session_id = self.session.as_ref().map(|session| session.id.clone());
        let (project, todos, changes) = tokio::join!(
            project_identity(&self.root),
            collect_todo_summary(todo_root, session_id),
            collect_change_summary(change_root),
        );
        let mut snapshot = WorkspaceSnapshot::empty(self.scope_id.clone(), project);
        snapshot.revision = self.revision;
        snapshot.session = self.session.clone();
        snapshot.todos = todos;
        snapshot.changes = changes;
        snapshot.external = collect_external_summary(self.external.record(), &self.root).await;
        snapshot.review = self.review.clone();
        snapshot.diagnostics = self.diagnostics_summary();
        snapshot.tooling = self.tooling_summary().await;
        snapshot.pull_request = self.pull_request.clone();
        // Working in a worktree: it is still the project, named as such, and the snapshot says
        // which checkout it describes
        if self.state_root != self.root {
            snapshot.project.display_name = luxe_core::paths::dir_name(&self.state_root);
            let place = self.root.strip_prefix(&self.state_root).unwrap_or(&self.root);
            snapshot.worktree = Some(luxe_core::paths::display(place));
        }
        snapshot
    }

    /// Records what the pull request watch found, for the next snapshot to carry.
    pub(crate) fn set_pull_request(&mut self, presence: luxe_core::workspace::PullRequestPresence) {
        self.pull_request = presence;
    }

    /// Rebuild the LSP-server + debug-adapter status for `/lsp` and `/debug`. Requires the shared
    /// services (present in production via [`WorkspaceCollector::new`]); empty in service-less
    /// tests.
    async fn tooling_summary(&self) -> luxe_core::ToolingSummary {
        match (&self.lsp_service, &self.dap_service) {
            (Some(lsp), Some(dap)) => {
                crate::engine_setup::collect_tooling_summary(&self.root, lsp, dap).await
            }
            _ => luxe_core::ToolingSummary::default(),
        }
    }

    /// Reduces the shared diagnostics cache to a bounded, worst-first summary for the snapshot.
    ///
    /// Only files under the active project root are counted (so a prior project's diagnostics never
    /// leak after a switch), paths are made project-relative, and files sort by errors then
    /// warnings so the most broken ones surface first.
    fn diagnostics_summary(&self) -> DiagnosticsSummary {
        let mut files: Vec<FileDiagnostics> = self
            .diagnostics
            .snapshot()
            .into_iter()
            .filter_map(|(path, diagnostics)| {
                // Scope to the active project: a file outside the root (e.g. left over from a
                // prior project) is dropped rather than shown with an absolute path.
                let relative = path.strip_prefix(&self.root).ok()?;
                let mut file =
                    FileDiagnostics { path: relative.to_path_buf(), ..Default::default() };
                for diagnostic in &diagnostics {
                    match diagnostic.severity {
                        Some(Severity::Error) => file.errors += 1,
                        Some(Severity::Warning) => file.warnings += 1,
                        Some(Severity::Information) => file.infos += 1,
                        Some(Severity::Hint) => file.hints += 1,
                        None => {}
                    }
                }
                // Only files with an error or warning are listed; info/hint-only files are the
                // kind of advice nothing acts on and they would crowd out the ones that matter.
                (file.errors + file.warnings > 0).then_some(file)
            })
            .collect();

        // Worst-first: errors, then warnings, then a stable path tiebreak.
        files.sort_by(|left, right| {
            right
                .errors
                .cmp(&left.errors)
                .then(right.warnings.cmp(&left.warnings))
                .then(left.path.cmp(&right.path))
        });

        let mut summary = DiagnosticsSummary {
            total_files: files.len(),
            truncated: files.len() > DIAGNOSTICS_FILE_LIMIT,
            ..DiagnosticsSummary::default()
        };
        for file in &files {
            summary.errors += file.errors;
            summary.warnings += file.warnings;
            summary.infos += file.infos;
            summary.hints += file.hints;
        }
        files.truncate(DIAGNOSTICS_FILE_LIMIT);
        summary.files = files;
        summary
    }

    /// Derives the session title from its first user message (opencode-style), once available.
    ///
    /// Retries across collects until the first user message exists (a brand-new session has none
    /// yet), then caches so later refreshes don't re-read the transcript.
    async fn resolve_session_title(&mut self) {
        if self.session_title_resolved {
            return;
        }
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let (root, id) = (self.state_root.clone(), session.id.clone());
        let derived = tokio::task::spawn_blocking(move || derive_session_title(&root, &id))
            .await
            .ok()
            .flatten();
        if let Some(title) = derived {
            if let Some(session) = self.session.as_mut() {
                session.title = title;
            }
            self.session_title_resolved = true;
        }
    }
}

/// Reads a session's first user message and reduces it to a bounded, single-line title.
fn derive_session_title(root: &Path, session_id: &str) -> Option<String> {
    use luxe_core::session::{SessionId, SessionStore};
    let store = crate::engine_setup::open_session_store(root);
    let entries = store.load(&SessionId(session_id.to_string())).ok()?;
    crate::dashboard_data::first_user_text(&entries)
        .map(|text| bounded_title(&text))
        .filter(|title| !title.is_empty())
}

/// Collapses whitespace to single spaces and bounds the title for the identity row.
fn bounded_title(text: &str) -> String {
    bounded_text(&text.split_whitespace().collect::<Vec<_>>().join(" "), 60)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

fn bounded_text(text: &str, limit: usize) -> String {
    let redacted = redact_secrets(text, &[]);
    let mut value = redacted.chars().take(limit).collect::<String>();
    if redacted.chars().nth(limit).is_some() {
        value.push('…');
    }
    value
}

/// Luxe's own project directory, excluded from what it reports as *your* changes.
const LUXE_PROJECT_DIR: &str = ".luxe";

async fn project_identity(root: &Path) -> ProjectIdentity {
    let (git_branch, git_dirty, git_head) =
        tokio::join!(git_probe::branch(root), git_probe::is_dirty(root), git_probe::head(root));
    ProjectIdentity {
        root: root.to_path_buf(),
        display_name: luxe_core::paths::dir_name(root),
        display_path: luxe_core::paths::home_relative(root),
        git_branch,
        git_dirty,
        git_head,
    }
}

/// What a review compares against, for the dock's REVIEW line — the target the reviewer actually
/// chose, not always "last turn".
fn review_source_label(target: &luxe_core::guided_review::ReviewTarget) -> String {
    use luxe_core::guided_review::ReviewTarget;
    match target {
        ReviewTarget::WorkingTree => "working tree".into(),
        ReviewTarget::LastTurn { .. } => "last turn".into(),
        ReviewTarget::Base { reference } => format!("base {reference}"),
        ReviewTarget::Commit { oid } => {
            format!("commit {}", oid.chars().take(12).collect::<String>())
        }
        ReviewTarget::PullRequest { number, .. } => format!("pull request #{number}"),
        ReviewTarget::OwnPullRequest { number } => format!("pull request #{number}"),
    }
}

#[cfg(test)]
mod tests;
