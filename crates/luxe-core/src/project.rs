//! project — where a project begins.
//!
//! [`discover`] is the one owner. The CLI, [`crate::instructions`] and [`crate::external_work`] all
//! ask it, so `luxe` launched one level down, from `crates/luxe-core`, still reads the repository's
//! instructions and writes its sessions in the repository's `.luxe/` rather than a second, empty
//! one.
//!
//! # What identifies a project
//!
//! The **path**, and nothing else. A root is a directory, sessions live under it, and no part of
//! that survives a `git` that stopped working.
//!
//! This is deliberate, and it is the half of VCS-awareness worth having. Deriving an identity from
//! a repository — a root-commit hash, a remote URL — reads as the more principled choice and is a
//! trap: the fact is mutable and the storage key is not supposed to be. `git init` in a directory
//! that already has history, a `rebase --root`, a linked worktree, a checkout with no remote yet —
//! each one silently renames the project, and a rename with nothing to migrate is a user watching
//! their sessions disappear. A broken `git` binary does it to every project at once.
//!
//! So Git is consulted for **where the project starts**, never for **what it is called**, and even
//! that is answered by [`std::path::Path::exists`] rather than by running anything.
//!
//! # The walk
//!
//! Up from the starting directory, as far as the nearest [`VCS_MARKERS`] entry:
//!
//! 1. The nearest directory on the way holding `.luxe/` is the root. An existing Luxe project
//!    outranks the repository it sits in, which makes `git init` in a directory Luxe has already
//!    worked in a no-op rather than an amnesia event, and lets someone scope Luxe to one package of
//!    a monorepo on purpose by creating the directory there.
//! 2. Otherwise the top of that working copy is the root.
//! 3. With no working copy at all, the starting directory, unchanged.
//!
//! A `.luxe/` claims nothing outside its own checkout: every session writes one, so honoring the
//! nearest anywhere would make launching Luxe once in `~/.config` or `/tmp` turn every directory
//! under it into that project. A checkout has a top every tool agrees on; a loose directory does
//! not, and the only honest answer for one is where you stood, as Codex and Claude Code say.

use std::path::{Path, PathBuf};

/// The directory Luxe keeps a project's own state in.
pub const PROJECT_DIR: &str = ".luxe";

/// Entries that mark the top of a working copy.
///
/// Matched as *entries*, not directories: a linked worktree and a submodule both carry a `.git`
/// **file** pointing elsewhere, and reading those as "not a repository" is how a checkout's worth
/// of work gets filed as loose files.
///
/// Subversion is absent on purpose. Its pre-1.7 layout puts `.svn` in *every* directory, so the
/// walk would stop at whichever one it started in and confidently report a leaf as the root.
pub const VCS_MARKERS: [&str; 3] = [".git", ".jj", ".hg"];

/// The project root for work starting at `start`.
///
/// Never fails and never runs a program: an unreadable parent, a repository with no commits and a
/// `git` that does not execute all land on the same answer as a directory with no markers at all —
/// `start` itself.
pub fn discover(start: &Path) -> PathBuf {
    discover_within(start, crate::user_dirs::home_dir().as_deref())
}

/// [`discover`] against an explicit home, so the guard is testable without moving the real one.
pub fn discover_within(start: &Path, home: Option<&Path>) -> PathBuf {
    let mut scoped = None;
    for directory in ancestors(start, home) {
        if scoped.is_none() && directory.join(PROJECT_DIR).is_dir() {
            scoped = Some(directory.clone());
        }
        if is_working_copy(&directory) {
            return scoped.unwrap_or(directory);
        }
    }
    start.to_path_buf()
}

/// The top of the working copy `start` sits in, if it sits in one, home guarded like the rest.
///
/// At or above [`discover`]'s root, never below it: a package scoped with its own `.luxe/` is still
/// part of its repository, and the repository's word reaches it.
fn checkout_top(start: &Path, home: Option<&Path>) -> Option<PathBuf> {
    ancestors(start, home).find(|directory| is_working_copy(directory))
}

/// The directories from the top of `start`'s checkout down to `start`, top first.
///
/// Just `start` when it is in no checkout. This is the whole of what the repository gets a say in:
/// its `AGENTS.md` files and its `.agents/skills` are read along this chain and nowhere above it,
/// because above it the directories belong to whoever owns the machine.
pub fn chain_from_top(start: &Path) -> Vec<PathBuf> {
    chain_from_top_within(start, crate::user_dirs::home_dir().as_deref())
}

/// [`chain_from_top`] against an explicit home, so it is testable without moving the real one.
pub fn chain_from_top_within(start: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let top = checkout_top(start, home);
    let mut chain = Vec::new();
    for directory in start.ancestors() {
        chain.push(directory.to_path_buf());
        if top.as_deref().is_none_or(|top| directory == top) {
            break;
        }
    }
    chain.reverse();
    chain
}

/// Whether `directory` is the top of a working copy.
pub fn is_working_copy(directory: &Path) -> bool {
    VCS_MARKERS.iter().any(|marker| directory.join(marker).exists())
}

/// Where the shell was standing when Luxe started, if that was below the project root.
static LAUNCHED_FROM: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Records where the shell was standing, before the process moves to the project root.
///
/// A fact about the process rather than about any one run, so it is set once and every front-end
/// reads the same answer: the one thing the launch directory still decides after the move is which
/// instruction files apply, and the prompt, the dashboard and the context list must agree on that.
pub fn note_launched_from(directory: &Path) {
    let _ = LAUNCHED_FROM.set(directory.to_path_buf());
}

/// Where the shell was standing, when [`note_launched_from`] recorded it.
pub fn launched_from() -> Option<PathBuf> {
    LAUNCHED_FROM.get().cloned()
}

/// The directory whose instruction chain applies to a session rooted at `root`: where Luxe was
/// launched, else `root` itself.
///
/// What the engine reads for the prompt, so a surface listing those files asks this rather than
/// walking from the root and coming up one `packages/app/AGENTS.md` short.
pub fn instruction_dir(root: &Path) -> PathBuf {
    launched_from().unwrap_or_else(|| root.to_path_buf())
}

/// The directories a walk from `start` may adopt, nearest first.
///
/// Home is where it stops. A great many people keep their dotfiles in a repository checked out at
/// `~`, and without the guard a `luxe` opened in `~/Downloads/scratch` adopts the entire home
/// directory as its project: every file indexed, every dotfile in the diff, `.luxe/` written beside
/// `.bashrc`. Home is still a perfectly good project when it is the directory you actually opened,
/// so the guard is about *inheriting* it, not about naming it.
fn ancestors<'a>(start: &'a Path, home: Option<&'a Path>) -> impl Iterator<Item = PathBuf> + 'a {
    let mut next = Some(start.to_path_buf());
    std::iter::from_fn(move || {
        let directory = next.take()?;
        if home.is_some_and(|home| directory == home) {
            // Nothing above home belongs to this project either way, so the walk ends here — and
            // home itself is only adopted when it is the directory that was opened.
            return (directory == start).then_some(directory);
        }
        next = directory.parent().map(Path::to_path_buf);
        Some(directory)
    })
}

#[cfg(test)]
mod tests;
