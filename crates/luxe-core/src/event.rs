//! Core↔UI contract — `Action` (UI→core intents), `Event` (core→UI stream),
//! and supporting types (`EngineState`, `StopReason`, `ModelRef`).
//!
//! Both enums are `#[non_exhaustive]` so later phases can extend them without
//! breaking UIs.

mod action;
mod decode;
mod events;
mod types;

pub use action::Action;
pub use events::Event;
pub use types::{
    EngineState, GuidedReviewFailure, GuidedReviewFailureKind, ModelChoice, ModelRef, OpenReview,
    RequestedReviewVerdict, SessionMessage, StopReason,
};

/// Version of the `Action`/`Event` wire protocol, announced in [`Event::Ready`].
///
/// Bumped only by a change a correct client cannot absorb — a removed or retagged variant, a field
/// that changed meaning. Additions are not among them: an unknown tag degrades to `Other` and a new
/// optional field defaults, so bumping for those would train clients to ignore the number.
pub const PROTOCOL_VERSION: u32 = 2;

#[cfg(test)]
mod tests;
