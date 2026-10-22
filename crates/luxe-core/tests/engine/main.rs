//! Engine scenarios — the turn loop, interruption, models, review restore, remediation and
//! compaction, driven end to end against doubles.
//!
//! One binary for the lot, since every top-level file under `tests/` links on its own. The shared
//! doubles and the bounded event waiters live in [`fixtures`].

#![allow(clippy::unwrap_used)]

// Backdating a directory's modified time takes opening it, which only unix allows
#[cfg(unix)]
mod baseline_retention;
mod carryover;
mod compaction;
mod fixtures;
mod interruption;
mod models;
mod nested_instructions;
mod project_reads;
mod remediation;
mod review_restore;
mod turns;
