//! Integration: a pull request loads into Guided Review through the real engine.
//!
//! A scripted GitHub double and a local bare remote keep it offline on every platform with git. It
//! holds the engine's half: the pull request is read through GitHub, pinned by git, described by
//! what its author wrote, and kept apart from the session that happened to open it.
//!
//! One binary for every workflow, since each top-level file under `tests/` links on its own. The
//! doubles live in [`support`]; the scenarios are grouped by what they do to a pull request.

#![allow(clippy::unwrap_used)]

mod github_surface;
mod loading;
mod opening;
mod posting;
mod restacking;
mod support;
mod threads;
