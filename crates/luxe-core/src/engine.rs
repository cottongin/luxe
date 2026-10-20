//! Agent loop engine — consumes `Action`s, drives the provider, dispatches tools, emits `Event`s.
//!
//! Unidirectional flow: `Engine::spawn()` returns `(ActionTx, EventRx)`. A background task
//! owns the session + provider; on `SubmitUserMessage` it runs a **turn**, which is an
//! inner agentic loop: append user msg → build request from the full active path → stream →
//! append the assistant reply (text + tool calls) → if it called tools, run them, append the
//! results, and loop so the model actually sees its tool output → stop when the model
//! answers with no tool calls (or `max_tool_iterations` trips the runaway guard).
//!
//! `AgentEnd` fires exactly once per turn, in `Engine::run`, so the loop hooks
//! (self-continuation, runaway guards, …) see one event per turn.

use futures::StreamExt;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, warn};

use crate::compaction::CompactionSettings;
use crate::context::{PromptContext, SystemPromptBuilder};
use crate::event::{EngineState, Event, GuidedReviewFailure, ModelRef, SessionMessage, StopReason};
use crate::guided_review::{
    ActiveReviewRecord, ActiveReviewRecordStore, AgentTurnId, GuideManifest, GuidedReview,
    GuidedReviewError, LastTurnRecord, LastTurnRecordStore, LocalGit, ReviewContextDraft, ReviewId,
    ReviewLoadRequestId, ReviewRevisionId, ReviewTarget, TurnBaselineArtifact, TurnBaselineId,
    UntrackedSourcePolicy,
};
use crate::hook::{HookCtx, HookEvent, HookOutcome, HookPoint, HookRegistry};
use crate::message::{ContentBlock, Message, Role};
use crate::permission::{
    AskResolution, Check, PermissionAction, PermissionReply, DOOM_LOOP, HOOK_ASK,
};
use crate::provider::{CompletionRequest, Provider, ProviderError, ProviderEvent};
use crate::session::{EntryId, EntryKind, SessionError, SessionId, SessionStore};
use crate::todo::TodoStore;
use crate::tool::{Tool, ToolContext, ToolRegistry, ToolResultData};
use crate::tool_pairing::Unfinished;

/// The agent loop engine.
pub struct Engine {
    config: EngineConfig,
    cancel_tx: watch::Sender<bool>,
    /// The mode the session is in, by name
    current_mode: tokio::sync::Mutex<String>,
    /// The mode the model was last told about, which is the one the transcript says it is in.
    ///
    /// A switch is announced on the next message rather than when it happens, so toggling twice
    /// before sending tells the model nothing — and a new or loaded transcript starts from the
    /// default, since nothing in it says otherwise.
    announced_mode: tokio::sync::Mutex<String>,
    /// Whether a `/review` is running, so a second request waits for the first
    review_running: std::sync::atomic::AtomicBool,
    /// The model this session is on, when it is not `config.model`.
    ///
    /// Set by `Action::SetModel` and by a mode's model. Always the *full* `ModelInfo`, so a
    /// switch to another model — or another provider — brings its own context window, thinking map
    /// and prices rather than borrowing the launch model's and reporting fiction.
    active_model_info: tokio::sync::Mutex<Option<crate::provider::ModelInfo>>,
    /// The reasoning level this session runs at — the level *requested*, which both read sites clamp
    /// to what the active model can honor. Starts from the launch model's own configured level.
    active_thinking: tokio::sync::Mutex<crate::provider::ThinkingLevel>,
    /// Reasoning level the user asked for on each model this session, keyed by `provider/id`.
    ///
    /// What makes a model's configured starting level (`models.json`'s `thinking`) a default rather
    /// than a ceiling: switching away and back restores the level you chose for that model, while a
    /// model you have not touched still starts where its config says. Only an explicit
    /// `SetThinkingLevel` writes here — an automatic application must not look like a decision.
    chosen_thinking: tokio::sync::Mutex<HashMap<String, crate::provider::ThinkingLevel>>,
    /// The model and thinking level a mode overrode, put back on the next mode switch.
    ///
    /// Without it a mode's override outlives the mode: plan on its own model, then build, leaves
    /// the build on the plan's model. `None` when no override is in force, and cleared by an
    /// explicit `SetModel` or `SetThinkingLevel`, since a choice made while in a mode is the user's
    /// and not the mode's.
    before_mode: tokio::sync::Mutex<Option<settings::ModeBaseline>>,
    /// The current leaf entry — user messages and turns chain from here so the full
    /// conversation (not just the latest message) reaches the provider. `None` until the
    /// first turn resolves it from the session's last entry.
    current_leaf: tokio::sync::Mutex<Option<EntryId>>,
    /// Mutable session id — updated when loading/resuming a session so the next turn chains
    /// from the resumed state, matching the `--resume` path.
    session_id: tokio::sync::Mutex<crate::session::SessionId>,
    /// This process's claim on `session_id`, which keeps a second Luxe from writing it too
    session_claim: std::sync::Mutex<Option<crate::session::SessionClaim>>,
    last_turn_baseline: tokio::sync::Mutex<Option<RetainedTurnBaseline>>,
    active_guided_review: Arc<tokio::sync::Mutex<ActiveGuidedReview>>,
    /// The one owner of the open review and its feedback session, running beside the engine
    review_desk: review_desk::ReviewDeskHandle,
    /// The desk's inbox, until `run` starts the desk with the event stream it reports on
    review_desk_inbox: std::sync::Mutex<Option<review_desk::DeskInbox>>,
    /// What each confirm confirms, under the request that prepared it
    previews: Arc<std::sync::Mutex<previews::Previews>>,
    accepted_feedback_submissions: HashSet<crate::guided_review::SubmissionId>,
    pending_feedback:
        HashMap<crate::guided_review::SubmissionId, review_feedback::PendingReviewFeedback>,
    /// Set when a session with prior messages is loaded; its next message carries the briefing
    resume_briefing_pending: AtomicBool,
    /// Context the session's first message carries, once: an issue taken up with `--issue`
    pending_brief: tokio::sync::Mutex<Option<String>>,
    /// The read-only turn running, if one is: while set, the dispatch choke point lets only
    /// reading through
    read_only: Option<read_only::ReadOnlyTurn>,
    /// Which line of the ledger the running turn bills to: the agent's, except while drafting a
    /// review's summary, which is part of what that review cost
    turn_source: crate::usage::UsageSource,
    /// The drafting turn's final answer, which carries the draft
    draft_answer: tokio::sync::Mutex<Option<String>>,
    /// The conversation entries drafts have already carried, so a later one sends only what is new
    sent_digests: tokio::sync::Mutex<drafting::SentDigests>,
    /// Live permission policy. Behind a mutex because a session "always" approval mutates it.
    permission: Arc<tokio::sync::Mutex<crate::permission::PermissionEngine>>,
    /// How an `ask` decision is resolved (prompt the UI, or auto-allow/deny when headless).
    ask_resolution: crate::permission::AskResolution,
    /// Bridges a blocked tool dispatch to its `RespondPermission` reply.
    permission_broker: Arc<crate::permission::PermissionBroker>,
    /// Bridges a blocked interactive `question` tool call to its `AnswerQuestion` reply.
    question_broker: Arc<crate::question::QuestionBroker>,
    /// Runtime auto-approve override: when set, `ask` collapses to allow regardless of
    /// `ask_resolution` (the `/permissions` toggle / walk-away mode).
    permission_auto_approve: Arc<AtomicBool>,
    /// Consecutive-identical-call counter (hash, count) backing the doom-loop guard.
    recent_tool_call: tokio::sync::Mutex<Option<(u64, u32)>>,
    /// `(consecutive failing test runs, already-nudged?)` backing the debugger nudge.
    test_failure_state: tokio::sync::Mutex<(u32, bool)>,
    /// Exact context occupancy (tokens) from the provider's most recent usage event; 0 until the
    /// first turn resolves it. Compaction prefers this over the char/4 estimate.
    last_context_tokens: AtomicU32,
    /// The system prompt this session sends, written on its first turn
    prompt_snapshot: std::sync::Mutex<Option<prompt_snapshot::PromptSnapshot>>,
    /// Estimated tokens the last request spent before any conversation — system prompt plus tool
    /// schemas. Recorded where the request is built, because that is the only place both halves
    /// are resolved, and read by compaction, which has to know how much of the window is already
    /// gone before it decides how much conversation may stay.
    last_prefix_tokens: AtomicU32,
    /// Consecutive auto-compaction failures. Trips a circuit breaker (see
    /// [`crate::compaction::compaction_circuit_open`]) so a persistently failing summarizer stops
    /// being retried every turn; a success resets it.
    compaction_failures: AtomicU32,
    /// Steering messages admitted mid-turn, waiting for the next inter-iteration checkpoint.
    ///
    /// A turn can't take a new message the instant it arrives — the provider call and its tool
    /// dispatch are already in flight — so a steer is buffered here and delivered as a real user
    /// message before the next provider call, in admission order.
    pending_steers: tokio::sync::Mutex<Vec<String>>,
    /// Actions that arrived during a turn and wait for it, in the order they arrived.
    ///
    /// Drained by the main loop ahead of the channel, because everything in here arrived before
    /// anything still in the channel. Sent back into that channel, the turn's own loop would read
    /// them straight out again — spinning for the length of the turn and reordering them, so a
    /// cancel sent after its submission could land before it and cancel nothing.
    deferred_actions: std::sync::Mutex<std::collections::VecDeque<Action>>,
    /// Actions the engine handed itself, lined up behind the channel by [`Engine::next_action`]
    continuations: std::sync::Mutex<std::collections::VecDeque<Action>>,
    /// The session's running spend. Every provider call in the process records here — the turn
    /// loop, the review's guide pass, and the compaction summarizer alike — so no UI has to
    /// reconstruct a total from the event stream, and no call site can spend unobserved.
    usage_ledger: usage::SessionLedgers,
    /// Keeps the last request's prompt cache alive through a long wait (`agent.cache_warming`).
    cache_warmer: cache_warmer::CacheWarmer,
}

impl Engine {
    /// The directory whose instruction chain applies to this session.
    ///
    /// One owner, because three places ask: the prompt that injects `AGENTS.md`, the notice that
    /// reports a file it had to truncate, and `/context`'s accounting of the same. See
    /// [`EngineConfig::invoked_from`] for why it is not simply the process's directory.
    pub(crate) fn instruction_dir(&self) -> std::path::PathBuf {
        instruction_dir_of(self.config.invoked_from.as_deref())
    }

    /// Raise or lower the turn's cancel flag.
    ///
    /// `watch::Sender::send` is a **no-op when nobody is subscribed** — it returns `Err` and
    /// leaves the value alone — and nobody is subscribed between turns, because every receiver
    /// here is a transient `subscribe()` taken by a provider stream or a tool dispatch. So the
    /// two things this flag must do, it did not do: a cancel raised during the gap was dropped,
    /// and the reset at the top of the next turn never lowered the flag a cancel had raised. That
    /// second one bricked sessions — the next turn's stream was born cancelled, produced no
    /// events, and committed an empty message the provider then rejected forever.
    pub(crate) fn set_cancelled(&self, cancelled: bool) {
        self.cancel_tx.send_replace(cancelled);
    }

    /// Whether the turn in flight has been told to stop.
    pub(crate) fn is_cancelled(&self) -> bool {
        *self.cancel_tx.borrow()
    }
}

pub use action::*;
pub use config::*;
pub use error::*;
pub(crate) use review_state::{ActiveGuidedReview, RetainedTurnBaseline};
pub(crate) use turn::{malformed_arguments, parse_tool_arguments, ToolCallInfo};
pub use types::*;

mod action;
mod agent_turn;
mod answering;
mod cache_warmer;
use agent_turn::TurnFlow;
mod compact;
mod composition;
mod config;
mod construction;
mod dispatch;
mod drafting;
mod error;
mod github_change;
mod github_list;
mod github_overview;
mod github_post;
mod guide_run;
mod inbox;
mod read_only;
mod requested_review;
mod review_feedback;
mod session_turns;
mod settings;
pub use settings::Carryover;
mod startup;

pub use drafting::{DraftRequest, DraftTarget, FindingEvidence, ReviewDraft};
pub use github_change::{PullRequestChange, PullRequestWrite};
pub use github_post::GithubPost;
pub use review_desk::{CommentDraft, ReviewEdit};
pub use review_feedback::PULL_REQUEST_FIXES_REFUSED;
mod guided_review;
mod lifecycle;
mod nested_instructions;
mod opening;
mod posting;
mod previews;
mod project_reads;
mod prompt;
mod prompt_snapshot;
mod restacking;
mod review_commit;
mod review_desk;
mod review_state;
mod run;
mod stack_opening;
mod turn;
mod types;
mod usage;

/// [`Engine::instruction_dir`] over the one field it reads, so the fallback is testable without
/// standing up an engine.
fn instruction_dir_of(invoked_from: Option<&std::path::Path>) -> std::path::PathBuf {
    invoked_from
        .map(std::path::Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

#[cfg(test)]
mod instruction_dir_tests {
    use super::instruction_dir_of;

    /// A monorepo keeps shared rules at the root and specific ones beside the code, and `luxe`
    /// launched in `packages/app` is meant to get both. Since the launch moves the process to the
    /// project root, the only thing that still remembers which package that was is this field —
    /// drop it and the nested file goes quiet, with nothing failing to say so.
    #[test]
    fn the_launch_directory_outranks_the_one_the_process_moved_to() {
        let launched_in = std::path::Path::new("/work/monorepo/packages/app");
        assert_eq!(instruction_dir_of(Some(launched_in)), launched_in);
    }

    /// An embedder never moved, so its own directory is the honest answer.
    #[test]
    fn nothing_recorded_falls_back_to_the_process() {
        let expected = std::env::current_dir().expect("a working directory");
        assert_eq!(instruction_dir_of(None), expected);
    }
}

#[cfg(test)]
mod tests;
