//! Guarded structural search — approve concrete read paths before invoking the matcher.
//!
//! The matcher gets the approved files themselves, never a directory it could read past the
//! policy in, in batches under the platform's command-line limit. It runs in the session's working
//! directory, the one the walk listed them from, so they go as the walk wrote them.

use super::*;
use crate::walk::{StopOnDrop, Walked};
use luxe_core::permission::PermissionAction;
use luxe_core::tool::FileReadHook;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Bytes of path arguments one invocation carries. Windows caps a whole command line at 32K
/// characters; unix allows far more, and fewer batches mean fewer process starts.
#[cfg(windows)]
const ARGUMENT_BUDGET: usize = 24 * 1024;
#[cfg(not(windows))]
const ARGUMENT_BUDGET: usize = 256 * 1024;

pub(super) enum Call {
    Search(AstGrepSearchParams),
    Replace(AstGrepReplaceParams),
}

impl Call {
    pub(super) fn parse(params: serde_json::Value) -> Result<Self, String> {
        let action = params.get("action").and_then(|value| value.as_str()).unwrap_or("search");
        match action {
            "search" => serde_json::from_value(params).map(Self::Search),
            "replace" => serde_json::from_value(params).map(Self::Replace),
            other => return Err(format!("Unknown action: {other}")),
        }
        .map_err(|error| format!("Invalid params: {error}"))
    }

    fn roots(&self) -> Vec<String> {
        let paths = match self {
            Self::Search(search) => &search.paths,
            Self::Replace(replace) => &replace.paths,
        };
        if paths.is_empty() {
            vec![".".into()]
        } else {
            paths.clone()
        }
    }

    /// Run the matcher once per batch and report every batch's matches as one result.
    pub(super) async fn over(
        mut self,
        tool: &AstGrepTool,
        batches: Vec<Vec<String>>,
        cx: &ToolContext,
    ) -> luxe_core::ToolResultData {
        let mut matches = Vec::new();
        for batch in batches {
            let found = match &mut self {
                Self::Search(search) => {
                    search.paths = batch;
                    AstGrepTool::search(search, &cx.cwd).await
                }
                Self::Replace(replace) => {
                    replace.paths = batch;
                    AstGrepTool::replace(replace, &cx.cwd).await
                }
            };
            if let Err(error) = found.map(|found| matches.extend(found)) {
                return self.failed_after(tool, matches, error, cx).await;
            }
        }
        self.outcome(tool, Ok(matches), cx).await
    }

    /// A later batch failed: an apply says what the earlier batches already rewrote.
    async fn failed_after(
        &self,
        tool: &AstGrepTool,
        matches: Vec<AstGrepMatch>,
        error: AstGrepError,
        cx: &ToolContext,
    ) -> luxe_core::ToolResultData {
        match self {
            Self::Replace(replace) if replace.apply && !matches.is_empty() => {
                let mut result = super::render::finish_replace(tool, Ok(matches), true, cx).await;
                result.content.push(luxe_core::ContentBlock::Text(format!(
                    "then ast-grep replace error: {error}; the rewrites above are on disk"
                )));
                result.is_error = true;
                result
            }
            _ => self.outcome(tool, Err(error), cx).await,
        }
    }

    async fn outcome(
        &self,
        tool: &AstGrepTool,
        found: Result<Vec<AstGrepMatch>, AstGrepError>,
        cx: &ToolContext,
    ) -> luxe_core::ToolResultData {
        match self {
            Self::Search(_) => search_outcome(found),
            Self::Replace(replace) => {
                super::render::finish_replace(tool, found, replace.apply, cx).await
            }
        }
    }
}

/// Split `paths` into runs whose argument bytes stay within `budget`; one oversized path still
/// gets a batch of its own rather than being dropped.
fn batches(paths: Vec<String>, budget: usize) -> Vec<Vec<String>> {
    let (mut batches, mut current, mut used) = (Vec::new(), Vec::new(), 0);
    for path in paths {
        let cost = path.len() + 1;
        if !current.is_empty() && used + cost > budget {
            batches.push(std::mem::take(&mut current));
            used = 0;
        }
        used += cost;
        current.push(path);
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

async fn candidates(
    cx: &ToolContext,
    roots: Vec<String>,
    stopped: Arc<AtomicBool>,
) -> Result<Walked, String> {
    let cwd = cx.cwd.clone();
    let cancelled = cx.cancelled.clone();
    tokio::task::spawn_blocking(move || {
        let mut all = Walked { files: Vec::new(), capped: false };
        for root in roots {
            let stop_walk = stopped.clone();
            let cancel_walk = cancelled.clone();
            let walked = crate::walk::files_until(&cwd, Path::new(&root), false, None, move || {
                stop_walk.load(Ordering::Relaxed) || *cancel_walk.borrow()
            });
            all.files.extend(walked.files);
            all.capped |= walked.capped;
            if stopped.load(Ordering::Relaxed) || *cancelled.borrow() {
                break;
            }
        }
        all.files.sort();
        all.files.dedup();
        all
    })
    .await
    .map_err(|error| format!("ast-grep candidate walk failed: {error}"))
}

struct ReadPlan {
    allowed: Vec<String>,
    denied: usize,
    asks: usize,
}

fn classify(files: Vec<PathBuf>, hook: &dyn FileReadHook) -> ReadPlan {
    let mut plan = ReadPlan { allowed: Vec::new(), denied: 0, asks: 0 };
    for file in files {
        match hook.decision(&file) {
            PermissionAction::Allow => plan.allowed.push(luxe_core::paths::display(&file)),
            PermissionAction::Deny => plan.denied += 1,
            PermissionAction::Ask => plan.asks += 1,
        }
    }
    plan
}

pub(super) async fn run(
    tool: &AstGrepTool,
    params: serde_json::Value,
    cx: &ToolContext,
    hook: &dyn FileReadHook,
) -> luxe_core::ToolResultData {
    let call = match Call::parse(params) {
        Ok(call) => call,
        Err(error) => return ToolResultData::error(error),
    };
    let stopped = Arc::new(AtomicBool::new(false));
    let _stop_on_drop = StopOnDrop(stopped.clone());
    let walked = match candidates(cx, call.roots(), stopped).await {
        Ok(walked) => walked,
        Err(error) => return ToolResultData::error(error),
    };
    let plan = classify(walked.files, hook);
    let mut result = if plan.allowed.is_empty() {
        ToolResultData::text("no policy-approved files to search")
    } else {
        call.over(tool, batches(plan.allowed, ARGUMENT_BUDGET), cx).await
    };
    if plan.denied + plan.asks > 0 {
        result.content.push(luxe_core::ContentBlock::Text(format!(
            "skipped {} files denied by read policy; {} files require approval and were not read",
            plan.denied, plan.asks
        )));
    }
    if walked.capped {
        result.content.push(luxe_core::ContentBlock::Text(crate::walk::capped_notice()));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use luxe_core::permission::{PermissionEngine, PermissionMode, Rule};

    /// Candidate classification must preserve Ask and user denials even when default guards opt out.
    #[test]
    fn candidate_reads_honor_yolo_denials_and_approval_requirements() {
        let policy = PermissionEngine::assemble(
            PermissionMode::Yolo,
            vec![
                Rule {
                    permission: "read".into(),
                    pattern: "blocked.rs".into(),
                    action: PermissionAction::Deny,
                },
                Rule {
                    permission: "read".into(),
                    pattern: "review.rs".into(),
                    action: PermissionAction::Ask,
                },
            ],
            vec![],
            false,
        );
        let files = [".env", "blocked.rs", "review.rs"].map(PathBuf::from).to_vec();
        let plan = classify(files, &policy);
        assert_eq!(plan.allowed, [".env"]);
        assert_eq!(plan.denied, 1);
        assert_eq!(plan.asks, 1);
    }

    /// One invocation per project file list would overflow a command line on a large tree.
    #[test]
    fn approved_files_are_batched_within_the_argument_budget() {
        let paths: Vec<String> = (0..10).map(|n| format!("/project/src/file_{n}.rs")).collect();
        let batched = batches(paths.clone(), 60);
        assert!(batched.len() > 1, "{batched:?}");
        assert!(batched.iter().all(|batch| batch.iter().map(|p| p.len() + 1).sum::<usize>() <= 60));
        assert_eq!(batched.concat(), paths);
        let oversized = vec!["x".repeat(100), "y".into()];
        assert_eq!(batches(oversized.clone(), 60), [vec![oversized[0].clone()], vec!["y".into()]]);
    }
}
