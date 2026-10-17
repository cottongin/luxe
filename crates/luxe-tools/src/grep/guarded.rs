//! Guarded grep — classify candidate paths before reading content in a bounded worker.

use super::*;
use crate::walk::{StopOnDrop, Walked};
use luxe_core::permission::PermissionAction;
use luxe_core::tool::FileReadHook;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub(super) fn parse_search(args: &serde_json::Value) -> Result<Search<'_>, String> {
    let pattern = args
        .get("pattern")
        .and_then(|value| value.as_str())
        .ok_or("Error: 'pattern' argument is required")?;
    // a model under a strict schema sends every property, so "no path" arrives as `""`
    let path = args
        .get("path")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .unwrap_or(".");
    // a negative count is a mistake, not a smaller window
    let count = |key: &str| {
        args.get(key).and_then(|value| value.as_i64()).filter(|value| *value > 0).unwrap_or(0)
            as usize
    };
    // a zero on one side is the strict schema's default, not a choice
    let side = |key: &str| match count(key) {
        0 => count("context"),
        asked => asked,
    };
    // `limit` only tightens: a cap the caller can raise is not a bound
    let limit = args
        .get("limit")
        .and_then(|value| value.as_i64())
        .and_then(|value| usize::try_from(value).ok())
        .filter(|value| *value > 0)
        .unwrap_or(GREP_MAX_LINES)
        .min(GREP_MAX_LINES);
    Ok(Search {
        pattern,
        path: Path::new(path),
        glob: args.get("glob").and_then(|value| value.as_str()),
        before: side("before"),
        after: side("after"),
        all: args.get("all").and_then(|value| value.as_bool()).unwrap_or(false),
        invert: args.get("invert").and_then(|value| value.as_bool()).unwrap_or(false),
        // camelCase is advertised; snake_case is what models reach for, and a silently ignored
        // mode returns the expensive answer
        mode: OutputMode::parse(
            luxe_core::tool::arg(args, &["outputMode", "output_mode"])
                .and_then(|value| value.as_str()),
        ),
        limit,
    })
}

struct ReadPlan {
    allowed: Vec<PathBuf>,
    denied: usize,
    asks: usize,
}

fn classify(
    paths: Vec<PathBuf>,
    glob: Option<&str>,
    hook: &dyn FileReadHook,
) -> Result<ReadPlan, String> {
    let filter = glob.map(crate::walk::matcher).transpose().map_err(|error| error.to_string())?;
    let mut plan = ReadPlan { allowed: Vec::new(), denied: 0, asks: 0 };
    for path in paths {
        if filter.as_ref().is_some_and(|filter| !filter.is_match(&path)) {
            continue;
        }
        match hook.decision(&path) {
            PermissionAction::Allow => plan.allowed.push(path),
            PermissionAction::Deny => plan.denied += 1,
            PermissionAction::Ask => plan.asks += 1,
        }
    }
    Ok(plan)
}

async fn candidates(
    cx: &ToolContext,
    path: &Path,
    all: bool,
    stopped: Arc<AtomicBool>,
) -> Result<Walked, String> {
    let cwd = cx.cwd.clone();
    let path = path.to_path_buf();
    let cancelled = cx.cancelled.clone();
    tokio::task::spawn_blocking(move || {
        crate::walk::files_until(&cwd, &path, all, None, move || {
            stopped.load(Ordering::Relaxed) || *cancelled.borrow()
        })
    })
    .await
    .map_err(|error| format!("grep candidate walk failed: {error}"))
}

async fn scan(
    args: serde_json::Value,
    cx: &ToolContext,
    paths: Vec<PathBuf>,
    stopped: Arc<AtomicBool>,
) -> Result<String, String> {
    let cwd = cx.cwd.clone();
    let cancelled = cx.cancelled.clone();
    tokio::task::spawn_blocking(move || {
        let search = parse_search(&args)?;
        super::native_search_files(&cwd, &search, paths, || {
            stopped.load(Ordering::Relaxed) || *cancelled.borrow()
        })
        .map(|rows| rows.join("\n"))
    })
    .await
    .map_err(|error| format!("grep scan worker failed: {error}"))?
}

pub(super) async fn run(
    tool: &GrepTool,
    args: serde_json::Value,
    cx: &ToolContext,
    hook: &dyn FileReadHook,
) -> ToolResultData {
    let search = match parse_search(&args) {
        Ok(search) => search,
        Err(error) => return ToolResultData::error(error),
    };
    let stopped = Arc::new(AtomicBool::new(false));
    let _stop_on_drop = StopOnDrop(stopped.clone());
    let walked = match candidates(cx, search.path, search.all, stopped.clone()).await {
        Ok(walked) => walked,
        Err(error) => return ToolResultData::error(error),
    };
    let plan = match classify(walked.files, search.glob, hook) {
        Ok(plan) => plan,
        Err(error) => return ToolResultData::error(error),
    };
    let found = scan(args.clone(), cx, plan.allowed, stopped).await;
    let mut result = finish(tool, cx, &search, found);
    if plan.denied + plan.asks > 0 {
        result.content.push(ContentBlock::Text(format!(
            "skipped {} files denied by read policy; {} files require approval and were not read",
            plan.denied, plan.asks
        )));
    }
    if walked.capped {
        result.content.push(ContentBlock::Text(crate::walk::capped_notice()));
    }
    result
}

pub(super) fn finish(
    tool: &GrepTool,
    cx: &ToolContext,
    search: &Search<'_>,
    found: Result<String, String>,
) -> ToolResultData {
    match found {
        Ok(matches) if matches.trim().is_empty() => ToolResultData::text("No matches found"),
        Ok(matches) => {
            let bounded = crate::output_limit::cap_lines_within(
                &matches,
                search.limit,
                GREP_MAX_CHARS,
                search.mode.unit(),
                search.mode.hint(),
            );
            record_matches(tool, cx, search, &bounded);
            ToolResultData::text(bounded)
        }
        Err(problem) => {
            warn!(pattern = search.pattern, error = %problem, "grep failed");
            ToolResultData::error(format!("Grep failed: {problem}"))
        }
    }
}

fn record_matches(tool: &GrepTool, cx: &ToolContext, search: &Search<'_>, bounded: &str) {
    let (Some(tracker), OutputMode::Content) = (&tool.tracker, search.mode) else { return };
    let searched = cx.cwd.join(search.path);
    let shown = if searched.is_file() {
        vec![searched]
    } else {
        crate::bash::seen::files_in_rows(bounded, &cx.cwd)
    };
    for path in shown {
        tracker.record(&cx.session_id, &path);
    }
}
