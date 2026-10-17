//! Integration: the permission engine gates tool dispatch end-to-end.
//!
//! Drives the real engine with a scripted provider that calls a `bash`-named tool, and asserts:
//! deny blocks the tool (model sees a denial), and the ask round-trip honors reject / once /
//! always (session-persisted) — exercising `Event::PermissionRequested` +
//! `Action::RespondPermission` and the `select!`-serviced turn.
//!
//! It also covers the half of the chain that runs *above* the engine: a `BeforeToolUse` hook's
//! rewrite and escalation, which reach the gate as arguments and as one more check rather than as
//! a second decision path.

#![allow(clippy::unwrap_used)]

mod ask_resolution;
mod fixtures;
mod hooks_and_guardrails;
mod rules_and_modes;
