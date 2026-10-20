//! luxe-core — the engine every front-end drives.
//!
//! Defines the `Provider`, `Tool` and `Hook` traits, the session model, the agent loop, context
//! assembly and compaction, the permission engine, the Guided Review model and its feedback
//! sessions, the configuration and keymap, and the `Action`/`Event` protocol front-ends speak.
//! Everything else plugs into these. The hard boundary is **no UI and no HTTP/provider deps**;
//! local process spawning for hook-/tool-driven work (review gate, command hooks, formatters, git,
//! MCP stdio) is allowed.

pub mod activity;
pub mod auth;
pub mod briefs;
pub mod cache_warming;
pub mod command_hooks;
pub mod compaction;
pub mod compression;
pub mod config;
pub(crate) mod context;
pub mod context_report;
pub mod dashboard;
pub(crate) mod empty_blocks;
pub mod engine;
pub(crate) mod error_cause;
pub mod event;
pub mod external_work;
pub mod fetched;
pub mod file_diff;
pub mod formatter;
pub mod fs_atomic;
pub mod github;
pub mod guided_review;
pub mod hook;
pub mod hooks;
pub mod instructions;
pub mod intraline;
pub mod isolation;
pub mod issue;
pub mod json_file;
pub mod jsonrpc_frame;
pub mod keymap;
pub mod local_endpoint;
pub mod loop_controller;
pub mod loop_hooks;
pub(crate) mod loop_state;
pub mod lsp;
pub mod mcp_client;
pub mod mcp_config;
pub mod mcp_stdio;
pub mod mention_source;
pub mod message;
pub mod mode;
pub mod oauth_loopback;
pub mod omarchy;
pub mod output_limit;
pub mod paths;
pub mod permission;
pub mod phase_gate;
pub mod phase_review;
pub mod plan;
pub mod program;
pub mod project;
pub mod project_files;
pub mod provider;
pub mod question;
pub mod read_tracker;
pub mod redact;
pub mod restack;
pub(crate) mod review;
pub mod runtime_state;
pub mod secret_command;
pub mod session;
pub mod settings;
pub mod skills;
pub mod theme;
pub mod todo;
pub mod tool;
pub mod tool_activation;
pub mod tool_catalog;
pub(crate) mod tool_pairing;
pub mod update;
pub mod usage;
pub mod user_dirs;
pub mod walk;
pub mod workspace;
pub mod worktree;
pub mod version {
    /// Semantic version string, kept in sync with Cargo.toml.
    pub const VERSION: &str = env!("CARGO_PKG_VERSION");
}

// Re-exports for convenience — callers don't need to reach into submodules.
pub use activity::{
    describe_call, describe_partial_call, lookup_runs, partial_card_body, tool_card_body,
    web_search_queries, ToolActivity, ToolActivityState, ToolBatch, ToolCardBody, ToolRun,
};
pub use config::Config;
pub use context::{GitStatus, PromptContext, SystemPromptBuilder};
pub use engine::{Action, Engine, EngineConfig};
pub use event::{EngineState, Event, ModelRef, StopReason, PROTOCOL_VERSION};
pub use guided_review::{
    ChapterId, FileId, HunkId, ReviewId, ReviewLoadRequestId, ReviewRevisionId,
};
pub use hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint, HookRegistry};
pub use message::{ContentBlock, Message, Role, ToolOutput};
pub use provider::{
    CompletionRequest, Cost, CostTier, Modality, ModelInfo, Provider, ProviderError,
    ProviderErrorKind, ProviderEvent, ThinkingLevel, ToolSchema, Usage,
};
pub use read_tracker::{ReadStatus, ReadTracker};
pub use session::{Entry, EntryId, EntryKind, SessionError, SessionId, SessionStore};
pub use todo::{Todo, TodoError, TodoStatus, TodoStore};
pub use tool::{Tool, ToolContext, ToolRegistry, ToolResultData};
pub use usage::{RecordedCall, SessionUsage, SourceUsage, UsageLedger, UsageSource, UsageTotals};
pub use workspace::{
    DebugAdapterStatus, DebugSessionStatus, DiagnosticsSummary, ExternalChangesSummary,
    ExternalRootSummary, FileChangeStatus, FileChangeSummary, FileDiagnostics,
    LanguageServerStatus, McpServerStatus, ProjectIdentity, ReviewSummary, SessionSummary,
    TodoSummary, TodoSummaryItem, TodoSummaryStatus, ToolAvailability, ToolStatus, ToolingSummary,
    WorkspaceChangesSummary, WorkspaceScopeId, WorkspaceSnapshot,
};

#[cfg(test)]
mod tests {
    use crate::version::VERSION;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty(), "version should not be empty");
    }
}
