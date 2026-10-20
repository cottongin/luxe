//! Nested instructions — the `AGENTS.md` files off the launch chain, read when work reaches them.
//!
//! The system prompt carries the files from the top of the checkout down to where Luxe started,
//! and it's written once per session. A file in a folder off that chain (`web/AGENTS.md` when Luxe
//! started at the root) can't join it without rewriting the prompt every later request is cached
//! against. So it rides on the result of the first tool call that reaches its folder: an append,
//! like any other result. Whether the model already has one is read off the conversation by the
//! note that opens it, so a rewind or a compaction that took it away lets it be sent again.

use std::path::{Path, PathBuf};

/// Opens the block that carries a nested file. Its path follows, up to [`NOTE_REST`].
const NOTE_OPEN: &str = "[instructions: ";

/// What follows the path, which is also where reading one back stops: a path may hold a `]`.
const NOTE_REST: &str = "] Instructions for work in this folder, from the file there. They add \
                         to the project's.";

/// Whether nested files are read at all. `LUXE_NESTED_INSTRUCTIONS=0` turns them off.
pub(crate) fn enabled() -> bool {
    std::env::var("LUXE_NESTED_INSTRUCTIONS").as_deref() != Ok("0")
}

/// The paths a call's arguments name: `path`, and each string in `paths`.
///
/// Those two keys cover the tools that open, write or search a place in the tree. A shell command
/// isn't parsed: guessing at the paths in one would load rules for folders the call never reached.
pub(crate) fn touched_paths(args: &serde_json::Value) -> Vec<&str> {
    let single = args.get("path").and_then(serde_json::Value::as_str);
    let many = args.get("paths").and_then(serde_json::Value::as_array).into_iter().flatten();
    single
        .into_iter()
        .chain(many.filter_map(serde_json::Value::as_str))
        .filter(|path| !path.trim().is_empty())
        .collect()
}

/// One nested instruction file a call reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NestedFile {
    /// The file, canonical
    pub(crate) path: PathBuf,
    /// Its path from the top of the checkout, with forward slashes: what the note names
    pub(crate) shown: String,
}

/// The instruction files that apply to `target` but weren't read at session start, top first.
///
/// Only folders inside the checkout `launch` sits in, off its chain, and not ignored: a file under
/// `node_modules` was written by a dependency's authors, not this project's. `target` may not exist
/// yet (a file about to be written), so the nearest folder that does stands in.
pub(crate) fn nested_files(launch: &Path, target: &Path) -> Vec<NestedFile> {
    let chain: Vec<PathBuf> = crate::project::chain_from_top(launch)
        .iter()
        .filter_map(|folder| folder.canonicalize().ok())
        .collect();
    let (Some(top), Some(folder)) = (chain.first(), existing_folder(target)) else {
        return Vec::new();
    };
    folders_below(top, &folder)
        .into_iter()
        .filter(|dir| !chain.iter().any(|on_chain| on_chain == dir))
        .filter_map(|dir| Some((dir, instruction_file(dir)?)))
        .filter(|(dir, _)| !crate::project_files::is_ignored(top, dir))
        .filter_map(|(_, file)| nested_file(top, file))
        .collect()
}

/// The folders from just below `top` down to `folder`, top first; none when `folder` is outside.
fn folders_below<'a>(top: &Path, folder: &'a Path) -> Vec<&'a Path> {
    if !folder.starts_with(top) {
        return Vec::new();
    }
    let mut below: Vec<&Path> = folder.ancestors().take_while(|dir| *dir != top).collect();
    below.reverse();
    below
}

/// The nearest folder at or above `target` that exists, canonical.
fn existing_folder(target: &Path) -> Option<PathBuf> {
    target.ancestors().find(|path| path.is_dir())?.canonicalize().ok()
}

/// The instruction file `dir` holds, by the same names and order the launch chain uses.
fn instruction_file(dir: &Path) -> Option<PathBuf> {
    super::INSTRUCTION_FILES.iter().map(|name| dir.join(name)).find(|path| path.is_file())
}

/// `file` as a [`NestedFile`], named from `top` by where it sits rather than where a link points.
fn nested_file(top: &Path, file: PathBuf) -> Option<NestedFile> {
    let shown = crate::paths::display(file.strip_prefix(top).ok()?);
    Some(NestedFile { path: file.canonicalize().ok()?, shown })
}

/// The block that carries `shown`'s text to the model.
pub(crate) fn note(shown: &str, text: &str) -> String {
    format!("{NOTE_OPEN}{shown}{NOTE_REST}\n\n{text}")
}

/// The file a block carries, when `text` is one.
pub(crate) fn noted(text: &str) -> Option<&str> {
    text.strip_prefix(NOTE_OPEN)?.split_once(NOTE_REST).map(|(shown, _)| shown)
}

#[cfg(test)]
mod tests;
