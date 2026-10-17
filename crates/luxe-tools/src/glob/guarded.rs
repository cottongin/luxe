//! Guarded glob — classify actual metadata paths before returning a bounded listing.

use super::*;
use crate::walk::{StopOnDrop, Walked};
use luxe_core::permission::PermissionAction;
use luxe_core::tool::FileReadHook;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

async fn candidates(
    cx: &ToolContext,
    path: PathBuf,
    all: bool,
    stopped: Arc<AtomicBool>,
) -> Result<Walked, String> {
    let cwd = cx.cwd.clone();
    let cancelled = cx.cancelled.clone();
    tokio::task::spawn_blocking(move || {
        crate::walk::files_until(&cwd, &path, all, None, move || {
            stopped.load(Ordering::Relaxed) || *cancelled.borrow()
        })
    })
    .await
    .map_err(|error| format!("glob candidate walk failed: {error}"))
}

struct Listing {
    allowed: Vec<String>,
    denied: usize,
    asks: usize,
}

fn classify(
    files: Vec<PathBuf>,
    pattern: &str,
    hook: &dyn FileReadHook,
) -> Result<Listing, String> {
    let matcher = crate::walk::matcher(pattern)
        .map_err(|error| format!("'{pattern}' is not a valid glob: {error}"))?;
    let mut listing = Listing { allowed: Vec::new(), denied: 0, asks: 0 };
    for file in files {
        if !matcher.is_match(&file) {
            continue;
        }
        match hook.decision(&file) {
            PermissionAction::Allow => listing.allowed.push(luxe_core::paths::display(&file)),
            PermissionAction::Deny => listing.denied += 1,
            PermissionAction::Ask => listing.asks += 1,
        }
    }
    Ok(listing)
}

fn render(listing: Listing, all: bool, capped: bool) -> ToolResultData {
    let body = if listing.allowed.is_empty() {
        if all {
            "No files found".into()
        } else {
            "No files found (ignored paths were skipped; pass all:true to include build output and vendored files)".into()
        }
    } else {
        crate::output_limit::cap_lines(
            &listing.allowed.join("\n"),
            GLOB_MAX_FILES,
            "files",
            "Narrow the pattern or search a subdirectory to see the rest.",
        )
    };
    let mut content = vec![ContentBlock::Text(body)];
    if listing.denied + listing.asks > 0 {
        content.push(ContentBlock::Text(format!("skipped {} metadata paths denied by read policy; {} require approval and were not returned", listing.denied, listing.asks)));
    }
    if capped {
        content.push(ContentBlock::Text(crate::walk::capped_notice()));
    }
    ToolResultData { content, is_error: false }
}

pub(super) async fn run(
    args: serde_json::Value,
    cx: &ToolContext,
    hook: &dyn FileReadHook,
) -> ToolResultData {
    let Some(pattern) = args.get("pattern").and_then(|value| value.as_str()) else {
        return ToolResultData::error("Error: 'pattern' argument is required");
    };
    let path = args
        .get("path")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .unwrap_or(".");
    let all = args.get("all").and_then(|value| value.as_bool()).unwrap_or(false);
    let stopped = Arc::new(AtomicBool::new(false));
    let _stop_on_drop = StopOnDrop(stopped.clone());
    let found = candidates(cx, PathBuf::from(path), all, stopped)
        .await
        .and_then(|walked| Ok((classify(walked.files, pattern, hook)?, walked.capped)));
    match found {
        Ok((listing, capped)) => render(listing, all, capped),
        Err(problem) => ToolResultData::error(format!("Glob failed: {problem}")),
    }
}
