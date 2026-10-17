//! luxe-tools — tool implementations.
//!
//! Provides the standard tool set (read, write, edit, bash, grep, glob, etc.)
//! as `Tool` trait impls. Depends on `luxe-core` for the `Tool` trait.

pub mod activate;
pub mod astgrep;
pub mod bash;
pub mod brave;
pub mod diff_preview;
pub mod edit;
pub mod exa;
pub mod exa_mcp;
pub mod firecrawl;
mod git;
pub mod glob;
pub mod grep;
mod line_endings;
pub mod loop_control;
mod output_limit;
mod process;
mod question;
pub mod read;
pub mod screenshot;
mod skill;
mod ssrf;
pub mod tavily;
mod test_baseline;
mod test_runner;
pub mod todo;
pub(crate) use luxe_core::walk;

/// The read policy a tool runs under outside the engine, where none was given: every file allowed.
///
/// `glob` and `grep` have one implementation, the guarded one, so a direct call goes through it
/// too rather than through a second path that could drift.
pub(crate) struct NoReadPolicy;

impl luxe_core::tool::FileReadHook for NoReadPolicy {
    fn decision(&self, _: &std::path::Path) -> luxe_core::permission::PermissionAction {
        luxe_core::permission::PermissionAction::Allow
    }
}
pub mod web;
pub mod write;

/// Register all built-in tools in the given registry.
///
/// The `todo` tool requires a store root (e.g. `.luxe/todos`). Pass `None` to skip registering it.
/// `web` configures `web_search` and `web_fetch`; [`web::WebConfig::from_env`] is the usual start.
///
/// Returns the shared [`luxe_core::read_tracker::ReadTracker`] backing the read-before-edit /
/// staleness / blind-write guards, so the caller can also wire it into the LSP funnel reads
/// (`read_symbol`, `module_report`) that see file content but live in a crate that can't depend
/// on `luxe-tools`.
pub fn register_builtins(
    registry: &mut luxe_core::ToolRegistry,
    todo_store_root: Option<std::path::PathBuf>,
    web: web::WebConfig,
) -> std::sync::Arc<luxe_core::read_tracker::ReadTracker> {
    use std::sync::Arc;
    // One shared read tracker backs the read-before-edit + staleness + blind-write guards across
    // the read/edit/write trio (and the LSP funnel reads, wired by the caller).
    let read_tracker = Arc::new(luxe_core::read_tracker::ReadTracker::new());
    // Both of these need a program that may simply not be here, and a tool that cannot work in
    // this environment should not cost a schema in every request — the same gate `debug` uses.
    // Where they do work, they cover ground nothing else does, so the gate is on capability, not
    // on how often they happen to get called.
    if astgrep::is_available() {
        registry.register(Arc::new(astgrep::AstGrepTool::new().with_tracker(read_tracker.clone())));
    }
    if screenshot::is_available() {
        registry.register(Arc::new(screenshot::ScreenshotTool::new()));
    }
    registry.register(Arc::new(read::ReadTool::new().with_tracker(read_tracker.clone())));
    registry.register(Arc::new(write::WriteTool::new().with_tracker(read_tracker.clone())));
    registry.register(Arc::new(edit::EditTool::new().with_tracker(read_tracker.clone())));
    registry.register(Arc::new(bash::BashTool::new().with_tracker(read_tracker.clone())));
    registry.register(Arc::new(grep::GrepTool::new().with_tracker(read_tracker.clone())));
    registry.register(Arc::new(glob::GlobTool::new()));
    registry.register(Arc::new(question::QuestionTool::new(question::QuestionConfig::default())));
    registry.register(Arc::new(skill::SkillTool::new(skill::SkillConfig)));
    registry.register(Arc::new(git::GitTool::new()));
    registry.register(Arc::new(test_runner::TestTool::new()));

    // Web: both tools are always available and neither needs a key. Search falls back to Exa's
    // keyless endpoint, so research is a capability a fresh install has rather than one it has to
    // be configured into. A configured `BRAVE_API_KEY`/`TAVILY_API_KEY`/`EXA_API_KEY` still wins.
    let web = web::WebTools::new(web);
    registry.register(Arc::new(web.web_fetch()));
    registry.register(Arc::new(web.web_search()));

    if let Some(root) = todo_store_root {
        registry.register(Arc::new(todo::TodoTool::new(root)));
    }

    read_tracker
}
