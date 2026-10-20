//! The Files tab's tree, drawn as the TUI's dock draws FILES.
//!
//! One row a line: a `│` guide for each level down, a chevron on a folder, the folder or language
//! mark in its hue, then the name, with a changed file's counts and letter at the right edge and a
//! dot on a closed folder that holds a change. The app decides which folders are open and sends
//! the rows it can see, so this draws and keeps nothing.

use super::workspace::{change_status, counts};
use super::{escape, Renderer};
use luxe_core::workspace::{FileChangeSummary, WorkspaceChangesSummary};

/// The mark a file wears in the tree, and the theme role it's colored by.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileMark {
    /// The glyph, as plain text; empty for none
    pub glyph: String,
    /// What colors it
    pub hue: MarkHue,
}

/// The theme roles a language's mark is colored by, each a variable in the app's stylesheet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MarkHue {
    /// The theme's warning hue
    Warning,
    /// The theme's info hue
    Info,
    /// The theme's success hue
    Success,
    /// The theme's error hue
    Error,
    /// The deepest thinking level's hue
    ThinkingUltra,
    /// Body text
    Text,
    /// Muted text, for a mark with no known hue
    #[default]
    Muted,
}

impl MarkHue {
    fn class(self) -> &'static str {
        match self {
            Self::Warning => "hue-warning",
            Self::Info => "hue-info",
            Self::Success => "hue-success",
            Self::Error => "hue-error",
            Self::ThinkingUltra => "hue-thinking-ultra",
            Self::Text => "hue-text",
            Self::Muted => "hue-muted",
        }
    }
}

/// One row the app can see, as it sends it.
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum TreeRow {
    /// A folder, open or closed
    Folder(Entry),
    /// A file
    File(Entry),
    /// Words where entries would be: loading, empty, or why not
    Note { depth: usize, text: String },
}

/// A folder or file in the tree.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct Entry {
    /// From the project's root, `/` between folders
    path: String,
    name: String,
    /// Folders above it, below the root
    depth: usize,
    /// Whether a folder shows what's in it
    #[serde(default)]
    open: bool,
}

/// Every row, marked from `changes` where a file in it changed.
pub(super) fn tree(
    renderer: &dyn Renderer,
    rows: &[TreeRow],
    changes: Option<&WorkspaceChangesSummary>,
) -> String {
    let changed = changes.map_or(&[][..], |changes| changes.files.as_slice());
    let rows: String = rows.iter().map(|row| tree_row(renderer, row, changed)).collect();
    format!("<div class=\"tree\">{rows}</div>")
}

/// The project's name behind the folder mark, then its branch, as the TUI's project section has
/// them: the branch quieter, with ` *` when the tree has changes.
pub(super) fn project_name(
    renderer: &dyn Renderer,
    name: &str,
    branch: Option<&str>,
    dirty: bool,
) -> String {
    let folder = escape(&renderer.folder_mark());
    let mut out = format!("<span class=\"project-mark\">{folder}</span>{}", escape(name));
    if let Some(branch) = branch {
        let dirty = if dirty { " *" } else { "" };
        out.push_str(&format!(
            "<span class=\"project-branch\"><span class=\"project-mark\">{}</span>{}{dirty}</span>",
            escape(&renderer.branch_mark()),
            escape(branch)
        ));
    }
    out
}

fn tree_row(renderer: &dyn Renderer, row: &TreeRow, changed: &[FileChangeSummary]) -> String {
    match row {
        TreeRow::Folder(folder) => {
            folder_row(renderer, folder, holds_change(changed, &folder.path))
        }
        TreeRow::File(file) => {
            file_row(renderer, file, changed.iter().find(|change| display(change) == file.path))
        }
        TreeRow::Note { depth, text } => {
            format!("<p class=\"tree-row tree-note\">{}{}</p>", guides(*depth), escape(text))
        }
    }
}

fn folder_row(renderer: &dyn Renderer, folder: &Entry, holds_change: bool) -> String {
    let chevron = if folder.open { "\u{25be}" } else { "\u{25b8}" };
    // only a closed folder hides the change, so only a closed one says it's there
    let dot = match holds_change && !folder.open {
        true => "<span class=\"tree-right held\">\u{2022}</span>",
        false => "",
    };
    format!(
        "<button type=\"button\" class=\"tree-row\" data-folder=\"{}\" aria-expanded=\"{}\">\
         {}<span class=\"chevron\">{chevron}</span>{}<span class=\"tree-name folder\">{}</span>\
         {dot}</button>",
        escape(&folder.path),
        folder.open,
        guides(folder.depth),
        mark(&renderer.folder_mark(), "hue-secondary"),
        escape(&folder.name)
    )
}

fn file_row(renderer: &dyn Renderer, file: &Entry, change: Option<&FileChangeSummary>) -> String {
    let file_mark = renderer.file_mark(&file.name);
    let (name_class, right) = match change {
        Some(change) => {
            (format!(" change-{}", change_status(change.status)), change_cells(renderer, change))
        }
        None => (String::new(), String::new()),
    };
    format!(
        "<button type=\"button\" class=\"tree-row\" data-file=\"{}\">{}<span class=\"chevron\">\
         </span>{}<span class=\"tree-name{name_class}\">{}</span>{right}</button>",
        escape(&file.path),
        guides(file.depth),
        mark(&file_mark.glyph, file_mark.hue.class()),
        escape(&file.name)
    )
}

/// ` +6 −0 M`, as the dock's change rows end.
fn change_cells(renderer: &dyn Renderer, change: &FileChangeSummary) -> String {
    format!(
        "<span class=\"tree-right\">{} <span class=\"change-{}\">{}</span></span>",
        counts(change.additions, change.deletions),
        change_status(change.status),
        escape(&renderer.change_mark(change.status))
    )
}

fn guides(depth: usize) -> String {
    "<span class=\"guide\" aria-hidden=\"true\"></span>".repeat(depth)
}

/// The mark's cells, kept even when there's no glyph so names line up with their neighbors'.
fn mark(glyph: &str, hue: &str) -> String {
    format!("<span class=\"mark {hue}\" aria-hidden=\"true\">{}</span>", escape(glyph))
}

fn holds_change(changed: &[FileChangeSummary], folder: &str) -> bool {
    changed
        .iter()
        .any(|file| display(file).strip_prefix(folder).is_some_and(|rest| rest.starts_with('/')))
}

fn display(file: &FileChangeSummary) -> String {
    luxe_core::paths::display(&file.path)
}

#[cfg(test)]
mod tests;
