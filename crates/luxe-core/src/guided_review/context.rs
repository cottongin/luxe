//! Review targets and bounded project context.
//!
//! These values describe where a pinned review came from and why the current project needs it.
//! They remain data-only so source loading and guide generation can run without UI dependencies.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

/// Maximum title length retained in review context.
pub const MAX_CONTEXT_TITLE_CHARS: usize = 160;

/// What a review is called when nothing names it: no goal, no commits, no pull request.
pub const UNTITLED_REVIEW: &str = "Guided Review";
/// Maximum length retained for each review-context excerpt.
pub const MAX_CONTEXT_EXCERPT_CHARS: usize = 4_000;

/// Project-relative planning files a review reads when they exist, most specific first.
const DEFAULT_PLANNING_FILES: [&str; 2] = ["PLAN.md", "AGENTS.md"];

/// Selects a locally resolved source comparison for a Guided Review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ReviewTarget {
    /// Compare captured `HEAD` state with the captured working tree.
    WorkingTree,
    /// Compare a pre-turn baseline with the captured state after that turn.
    LastTurn {
        /// Session that owns the requested turn.
        session_id: String,
        /// Turn identifier within the session.
        turn_id: String,
    },
    /// Compare a resolved merge-base for a local reference with the captured working tree.
    Base {
        /// Local Git reference requested by the user.
        reference: String,
    },
    /// Compare a commit with its first parent, or the empty tree for a root commit.
    Commit {
        /// Full or abbreviated commit object identifier.
        oid: String,
    },
    /// A pull request, from where it branched off its base to its head.
    PullRequest {
        /// Number within its repository.
        number: u64,
        /// `owner/name` when the pull request is on a repository other than the checkout's own, such
        /// as the upstream of a fork; absent for the checkout's repository.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repository: Option<String>,
    },
    /// The checked-out branch's own pull request, from where it branched off its base to the
    /// working tree: where a fix to one of its threads lands, and is verified.
    OwnPullRequest {
        /// Number within the checkout's repository.
        number: u64,
    },
}

/// Identifies the context category a project-local text source contributes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewContextSourceKind {
    /// Planning, task intent, or phase instructions.
    Planning,
    /// Acceptance-command or reviewer evidence.
    Acceptance,
}

/// Errors raised while reading configured project-local review context.
#[derive(Debug, Error)]
pub enum ReviewContextSourceError {
    /// The configured source escaped the opened project root.
    #[error("review context source must be a normal project-relative path: {0}")]
    UnsafePath(PathBuf),
    /// A configured source could not be read as UTF-8 text.
    #[error("could not read review context source {path}: {error}")]
    Read {
        /// Project-relative source path.
        path: PathBuf,
        /// Underlying filesystem failure.
        error: std::io::Error,
    },
}

/// Errors raised while assembling project-local review context.
#[derive(Debug, Error)]
pub enum ReviewContextAssemblyError {
    /// A configured project-local context source could not be read safely.
    #[error(transparent)]
    Source(#[from] ReviewContextSourceError),
}

/// Project-local inputs collected before a bounded review context is assembled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewContextDraft {
    /// User-facing review title.
    pub title: String,
    /// User's stated goal, when available.
    pub goal: Option<String>,
    /// Current project phase index, when available.
    pub phase_index: Option<u32>,
    /// Current project phase title, when available.
    pub phase_title: Option<String>,
    /// Ordered planning sources, such as the project's own plan files.
    pub planning_sources: Vec<String>,
    /// Ordered acceptance sources, such as review-gate and reviewer evidence.
    pub acceptance_sources: Vec<String>,
}

impl ReviewContextDraft {
    /// Collects plan context from one project's own artifacts.
    ///
    /// `extra_planning` names additional project-relative planning files (from `review.context_files`);
    /// each is optional and read only when it resolves inside the opened project. The defaults are
    /// deliberately Luxe's own: a file belonging to the harness Luxe is *developed* with is excluded
    /// from release archives, so in any shipped Luxe it would never exist.
    pub fn from_project_local_sources(
        title: impl Into<String>,
        project_root: &Path,
        extra_planning: &[String],
    ) -> Result<Self, ReviewContextAssemblyError> {
        let mut draft = Self { title: title.into(), ..Self::default() };
        for relative in DEFAULT_PLANNING_FILES.iter().map(Path::new) {
            draft.append_optional_project_file(
                project_root,
                relative,
                ReviewContextSourceKind::Planning,
            )?;
        }
        for relative in extra_planning {
            draft.append_optional_project_file(
                project_root,
                Path::new(relative),
                ReviewContextSourceKind::Planning,
            )?;
        }
        Ok(draft)
    }

    /// Appends an optional UTF-8 file rooted under the opened project.
    pub fn append_optional_project_file(
        &mut self,
        project_root: &Path,
        relative_path: impl AsRef<Path>,
        kind: ReviewContextSourceKind,
    ) -> Result<(), ReviewContextSourceError> {
        let relative_path = relative_path.as_ref();
        if !is_normal_relative_path(relative_path) {
            return Err(ReviewContextSourceError::UnsafePath(relative_path.to_owned()));
        }
        let path = project_root.join(relative_path);
        if !path.exists() {
            return Ok(());
        }
        let canonical_root = fs::canonicalize(project_root).map_err(|source| {
            ReviewContextSourceError::Read { path: PathBuf::from("."), error: source }
        })?;
        let canonical_path = fs::canonicalize(&path).map_err(|source| {
            ReviewContextSourceError::Read { path: relative_path.to_owned(), error: source }
        })?;
        // `paths::contains`, not `starts_with`: this is an escape check, and a case-sensitive string
        // prefix rejects the project's own files on a case-insensitive filesystem.
        if !crate::paths::contains(&canonical_root, &canonical_path) {
            return Err(ReviewContextSourceError::UnsafePath(relative_path.to_owned()));
        }
        let source = fs::read_to_string(&canonical_path).map_err(|source| {
            ReviewContextSourceError::Read { path: relative_path.to_owned(), error: source }
        })?;
        // Filtered on the way in, like every other project file that reaches a model as guidance
        // rather than as data. A plan is read *as instruction* — it tells the reviewer what the
        // change was for — so an HTML comment in it is text the author's own diff view renders to
        // nothing while the reviewer acts on it. The files under review are deliberately not
        // filtered: a reviewer's job is to see exactly what is there.
        let source = crate::instructions::sanitize(&source).text;
        if !source.trim().is_empty() {
            match kind {
                ReviewContextSourceKind::Planning => self.planning_sources.push(source),
                ReviewContextSourceKind::Acceptance => self.acceptance_sources.push(source),
            }
        }
        Ok(())
    }

    /// Bounds and joins project-local sources into UI-neutral review context.
    ///
    /// The title falls back through what the project actually knows — the user's stated goal —
    /// before the generic label, so the review header says something rather than repeating the word
    /// "review" back at the reviewer.
    pub fn build(self) -> ReviewContext {
        let title =
            self.goal.as_deref().and_then(first_sentence).unwrap_or_else(|| self.title.clone());
        ReviewContext::bounded(
            title,
            self.goal,
            self.phase_index,
            self.phase_title,
            joined_sources(self.planning_sources),
            joined_sources(self.acceptance_sources),
        )
    }
}

/// Bounded project intent paired with one review source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewContext {
    /// User-facing review title.
    pub title: String,
    /// User's stated goal, when available.
    pub goal: Option<String>,
    /// Current project phase index, when available.
    pub phase_index: Option<u32>,
    /// Current project phase title, when available.
    pub phase_title: Option<String>,
    /// Bounded project planning excerpt.
    pub planning_excerpt: Option<String>,
    /// Bounded acceptance or reviewer evidence excerpt.
    pub acceptance_excerpt: Option<String>,
    /// What the reviewed source says about itself — commit subjects and body for a `base`/`commit`
    /// review, absent for a working-tree or last-turn one where the session is the better witness.
    #[serde(default)]
    pub source_intent: Option<String>,
    /// How many commits `source_intent` was read from, so a reader can see it covers the reviewed
    /// range and not the project's history
    #[serde(default)]
    pub source_commits: usize,
    /// The pull request under review, as GitHub describes it; absent for every other source
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_request: Option<super::PullRequestContext>,
}

impl ReviewContext {
    /// Creates bounded context from project-local plan and phase data.
    pub fn bounded(
        title: impl AsRef<str>,
        goal: Option<impl AsRef<str>>,
        phase_index: Option<u32>,
        phase_title: Option<impl AsRef<str>>,
        planning_excerpt: Option<impl AsRef<str>>,
        acceptance_excerpt: Option<impl AsRef<str>>,
    ) -> Self {
        Self {
            title: bounded_text(title.as_ref(), MAX_CONTEXT_TITLE_CHARS),
            goal: bounded_option(goal, MAX_CONTEXT_EXCERPT_CHARS),
            phase_index,
            phase_title: bounded_option(phase_title, MAX_CONTEXT_TITLE_CHARS),
            planning_excerpt: bounded_option(planning_excerpt, MAX_CONTEXT_EXCERPT_CHARS),
            acceptance_excerpt: bounded_option(acceptance_excerpt, MAX_CONTEXT_EXCERPT_CHARS),
            source_intent: None,
            source_commits: 0,
            pull_request: None,
        }
    }

    /// Attaches what the reviewed commits say, naming the review after them when nothing better is
    /// known.
    ///
    /// A stated goal wins — it describes the work the reviewer asked for, and it is what
    /// [`ReviewContextDraft::build`] titles the review from. Without one, the newest commit
    /// subject beats reading the words "Guided Review" back at them.
    pub fn with_source_intent(
        mut self,
        title: impl AsRef<str>,
        summary: impl AsRef<str>,
        commits: usize,
    ) -> Self {
        // Commit messages are whoever wrote the commits' words, which for a fetched pull request
        // is a stranger: scrubbed like any other text headed for the guide's prompt
        let scrub = |text: &str| crate::instructions::sanitize(text).text;
        // A pull request names itself; its commits are a second witness, not a better title
        if self.goal.as_deref().and_then(first_sentence).is_none() && self.pull_request.is_none() {
            let title = bounded_text(&scrub(title.as_ref()), MAX_CONTEXT_TITLE_CHARS);
            if !title.is_empty() {
                self.title = title;
            }
        }
        self.source_intent =
            bounded_option(Some(scrub(summary.as_ref())), MAX_CONTEXT_EXCERPT_CHARS);
        self.source_commits = commits;
        self
    }
}

fn is_normal_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path.components().all(|component| matches!(component, Component::Normal(_)))
}

/// The opening sentence (or first line) of a stated goal, as a review title.
fn first_sentence(goal: &str) -> Option<String> {
    let line = goal.lines().map(str::trim).find(|line| !line.is_empty())?;
    let sentence = line.split_once(". ").map_or(line, |(head, _)| head);
    let trimmed = sentence.trim_end_matches('.').trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn joined_sources(sources: Vec<String>) -> Option<String> {
    let joined = sources
        .into_iter()
        .filter(|source| !source.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!joined.is_empty()).then_some(joined)
}

fn bounded_option(value: Option<impl AsRef<str>>, max_chars: usize) -> Option<String> {
    value.and_then(|text| {
        let text = bounded_text(text.as_ref(), max_chars);
        (!text.is_empty()).then_some(text)
    })
}

fn bounded_text(value: &str, max_chars: usize) -> String {
    value
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests;
