//! Where a phase review's evidence comes from: the project's own diff and planning files.
//!
//! The change under review is everything since the phase began — the working tree against the phase
//! baseline the loop recorded, which covers work the agent has committed as well as work it has not.
//!
//! Not the working tree against `HEAD`: a loop's agent commits each phase as it finishes, so every
//! review would be handed an empty diff, decline to judge, and let the phases advance on their
//! acceptance commands with the reviewer contributing nothing at all.

use luxe_core::phase_gate::PhaseReviewContext;
use luxe_core::phase_review::{ReviewedFile, MAX_REVIEW_FILES_BYTES, MAX_REVIEW_FILE_BYTES};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Planning files a review reads, in the order they are laid out for the reviewer.
///
/// `PLAN.md` is the current slice and `AGENTS.md` the rules it has to hold to. Both are optional:
/// plenty of projects have neither, and a review without them still checks the diff against the
/// gate.
const PLANNING_FILES: [&str; 2] = ["PLAN.md", "AGENTS.md"];

/// Bytes of planning context handed to a reviewer.
///
/// `AGENTS.md` is a few kilobytes and `PLAN.md` can be long. The cap is per file so a long plan
/// cannot crowd out the rules entirely.
const MAX_PLANNING_BYTES: usize = 8 * 1024;

/// New files named before the list is summarized.
///
/// A phase that adds a vendored directory would otherwise bury the change it was asked about under
/// a few thousand filenames.
const MAX_UNTRACKED_LISTED: usize = 50;

/// How many files a review reads in full.
///
/// A phase that touches thirty files cannot be judged by reading all of them in one call, and the
/// ones worth reading are the first few: what the diff touched, then what the phase named.
const MAX_REVIEW_FILES: usize = 8;

/// Reads a phase review's evidence out of a git worktree.
pub(crate) struct WorktreeReviewContext {
    root: PathBuf,
    /// Supplies the commit the current phase started from.
    controller: Option<Arc<luxe_core::loop_controller::LoopController>>,
    /// Whether the reviewer reads the whole plan rather than one phase of it
    whole_plan: bool,
}

impl std::fmt::Debug for WorktreeReviewContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("WorktreeReviewContext").field("root", &self.root).finish()
    }
}

impl WorktreeReviewContext {
    /// Read evidence from the project rooted at `root`, diffing against `HEAD`.
    pub(crate) fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), controller: None, whole_plan: false }
    }

    /// Hand the reviewer the whole plan rather than the phase it is asked about.
    ///
    /// For a review somebody asked for, which judges the work in progress against everything the
    /// plan says rather than one phase a loop is on.
    pub(crate) fn whole_plan(mut self) -> Self {
        self.whole_plan = true;
        self
    }

    /// Diff against the phase baseline the loop records, rather than against `HEAD`.
    pub(crate) fn anchored_to(
        mut self,
        controller: Arc<luxe_core::loop_controller::LoopController>,
    ) -> Self {
        self.controller = Some(controller);
        self
    }

    /// What this phase's diff is taken against.
    ///
    /// The recorded baseline when there is one, else `HEAD` — which is right for a project with no
    /// baseline yet, including anything that does not run through the loop.
    async fn base(&self) -> String {
        match self.controller.as_ref() {
            Some(controller) => {
                controller.phase_baseline().await.unwrap_or_else(|| "HEAD".to_string())
            }
            None => "HEAD".to_string(),
        }
    }
}

#[async_trait::async_trait]
impl PhaseReviewContext for WorktreeReviewContext {
    async fn diff(&self) -> Result<String, String> {
        // `git diff <base>` compares the *working tree* to that commit, so one command covers what
        // the agent committed during this phase and what it has not committed yet.
        let base = self.base().await;
        let output = tokio::process::Command::new("git")
            .args(["diff", &base, "--no-color"])
            .current_dir(&self.root)
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|error| format!("could not run git diff: {error}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("git diff failed: {}", stderr.trim()));
        }
        let tracked = String::from_utf8_lossy(&output.stdout).into_owned();

        // A phase whose work is entirely new files would otherwise review as an empty diff, because
        // `git diff` says nothing about a file git has never seen.
        let untracked = self.untracked_summary().await;
        Ok(match untracked.is_empty() {
            true => tracked,
            false => format!("{tracked}\n{untracked}"),
        })
    }

    async fn planning(&self, phase: u32) -> String {
        let mut sections = Vec::new();
        for name in PLANNING_FILES {
            let Some(body) = read_bounded(&self.root.join(name)).await else { continue };
            // The plan is narrowed to the phase under review where it says which is which; the rules
            // (`AGENTS.md`) are not, because they apply to every phase.
            let body = if name == "PLAN.md" && !self.whole_plan {
                luxe_core::plan::phase_section(&body, phase).unwrap_or(body)
            } else {
                body
            };
            sections.push(format!("### {name}\n\n{body}"));
        }
        sections.join("\n\n")
    }

    async fn files(&self, diff: &str, planning: &str) -> Vec<ReviewedFile> {
        let mut wanted: Vec<String> = Vec::new();
        // The diff's files first: they are what the phase demonstrably touched.
        for path in paths_in_diff(diff) {
            if !wanted.contains(&path) {
                wanted.push(path);
            }
        }
        // Then whatever the phase's own text names, which is the only source left when the work
        // landed during an earlier phase and the diff has nothing to say.
        for path in paths_named_in(planning) {
            if !wanted.contains(&path) {
                wanted.push(path);
            }
        }

        let mut files = Vec::new();
        let mut budget = MAX_REVIEW_FILES_BYTES;
        for path in wanted {
            if files.len() >= MAX_REVIEW_FILES || budget == 0 {
                break;
            }
            let full = self.root.join(&path);
            // Confined to the project: a plan naming `/etc/passwd`, or `../elsewhere`, does not get
            // it read and handed to a model.
            if !full.starts_with(&self.root) {
                continue;
            }
            let Ok(body) = tokio::fs::read_to_string(&full).await else { continue };
            if body.trim().is_empty() {
                continue;
            }
            let cap = MAX_REVIEW_FILE_BYTES.min(budget);
            let (contents, truncated) = cut_to(&body, cap);
            budget = budget.saturating_sub(contents.len());
            files.push(ReviewedFile { path, contents, truncated });
        }
        files
    }
}

impl WorktreeReviewContext {
    /// New files named, not inlined.
    ///
    /// Their contents are already in the reviewer's reach if it needs them, and a phase that adds a
    /// vendored directory would otherwise bury the change it was asked to look at.
    async fn untracked_summary(&self) -> String {
        let output = tokio::process::Command::new("git")
            .args(["ls-files", "--others", "--exclude-standard"])
            .current_dir(&self.root)
            .kill_on_drop(true)
            .output()
            .await;
        let Ok(output) = output else { return String::new() };
        if !output.status.success() {
            return String::new();
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let files: Vec<&str> =
            stdout.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        if files.is_empty() {
            return String::new();
        }
        let listed: Vec<String> =
            files.iter().take(MAX_UNTRACKED_LISTED).map(|file| format!("- {file}")).collect();
        let more = files.len().saturating_sub(listed.len());
        let mut summary = format!("Untracked files added by this phase:\n{}", listed.join("\n"));
        if more > 0 {
            summary.push_str(&format!("\n- … and {more} more"));
        }
        summary
    }
}

/// Paths a unified diff names, project-relative.
fn paths_in_diff(diff: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in diff.lines() {
        // `+++ b/path` is the post-image, which is the file as it now stands — the one worth reading.
        let Some(rest) = line.strip_prefix("+++ ") else { continue };
        let rest = rest.trim();
        if rest == "/dev/null" {
            continue;
        }
        let path = rest.strip_prefix("b/").unwrap_or(rest);
        // Untracked files arrive as a plain list rather than a diff, so `- path` lines count too.
        if !path.is_empty() && !paths.contains(&path.to_string()) {
            paths.push(path.to_string());
        }
    }
    for line in diff.lines() {
        let Some(rest) = line.trim().strip_prefix("- ") else { continue };
        let candidate = rest.trim();
        if looks_like_a_path(candidate) && !paths.contains(&candidate.to_string()) {
            paths.push(candidate.to_string());
        }
    }
    paths
}

/// Paths a piece of prose names in backticks.
///
/// The plan is the only place a requirement about a file that has not changed can come from, and a
/// plan written for a person names its files in backticks. Anything that does not look like a path is
/// ignored, so prose about `rank()` does not become a filename.
fn paths_named_in(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for token in text.split('`').skip(1).step_by(2) {
        let candidate = token.trim();
        if looks_like_a_path(candidate) && !paths.contains(&candidate.to_string()) {
            paths.push(candidate.to_string());
        }
    }
    paths
}

/// Whether a token is plausibly a project-relative file path.
///
/// Deliberately narrow. It has to reject `rank()`, `--top`, `Hello, world!` and `greet(name)` while
/// accepting `wordfreq.py` and `src/main.rs`; an over-eager guess here means reading files nobody
/// asked about and spending a reviewer's context on them.
fn looks_like_a_path(token: &str) -> bool {
    if token.is_empty() || token.len() > 200 {
        return false;
    }
    if token.starts_with('/') || token.starts_with("..") || token.contains(' ') {
        return false;
    }
    if token.contains(|c: char| "()<>|\"*?$".contains(c)) {
        return false;
    }
    // A dotted extension, or a directory separator: `wordfreq.py`, `src/main.rs`, `docs/design/x.md`.
    let extension = std::path::Path::new(token).extension().is_some_and(|ext| {
        let ext = ext.to_string_lossy();
        (1..=5).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric())
    });
    extension || token.contains('/')
}

/// Cut `body` to `cap` bytes on a line boundary, reporting whether it was cut.
fn cut_to(body: &str, cap: usize) -> (String, bool) {
    if body.len() <= cap {
        return (body.to_string(), false);
    }
    let mut kept = String::with_capacity(cap);
    for line in body.lines() {
        if kept.len().saturating_add(line.len()).saturating_add(1) > cap {
            break;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    (kept, true)
}

/// Read a planning file, bounded and filtered, or `None` when it is absent or unreadable.
///
/// `PLAN.md` and `AGENTS.md` reach the phase reviewer as *instruction* — what the phase was for,
/// and the rules it had to hold to — so they go through the same filter as the ones that reach the
/// developer's system prompt. It matters more here than there: the reviewer runs unattended, and a
/// gate decided on text nobody reviewing the diff could see is the failure the filter exists for.
async fn read_bounded(path: &Path) -> Option<String> {
    let body = tokio::fs::read_to_string(path).await.ok()?;
    let body = luxe_core::instructions::sanitize(&body).text;
    if body.trim().is_empty() {
        return None;
    }
    if body.len() <= MAX_PLANNING_BYTES {
        return Some(body);
    }
    // Cut on a line boundary and say so, for the same reason the diff is: a document that stops
    // mid-sentence reads as corruption rather than as an excerpt.
    let mut kept = String::with_capacity(MAX_PLANNING_BYTES);
    for line in body.lines() {
        if kept.len().saturating_add(line.len()).saturating_add(1) > MAX_PLANNING_BYTES {
            break;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    kept.push_str("\n… excerpt ends here.");
    Some(kept)
}

#[cfg(test)]
mod tests;
