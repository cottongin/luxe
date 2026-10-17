//! Tree filter — what `/` in FILES narrows the tree to.
//!
//! The tree reads one folder at a time, so filtering what it has read would miss most of a large
//! project. The filter matches against the whole project's file list instead, the one the `@`
//! picker reads, with the same matcher, and draws the matches as a tree of their own: each file
//! under the folders that hold it, every folder open. Only the best matches are drawn, in tree
//! order, since a tree sorted by score would not read as a tree.
//!
//! The list leaves out what `.gitignore` ignores, as the `@` picker does, so a filter never finds
//! an ignored file even while the tree is showing them.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::TreeRow;
use crate::workspace_dock::DockChangeItem;

/// The most matches the tree draws; past this, typing more is the way to the rest.
pub(crate) const SHOWN: usize = 100;

/// A filter being typed, or kept after typing stopped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TreeFilter {
    /// What was typed
    pub query: String,
    /// Whether keys type into the query rather than move through the matches
    pub typing: bool,
}

/// What a filter matched, beside the rows it drew.
#[derive(Debug, Clone, Default)]
pub struct FilterHits {
    /// Character positions in each drawn path that the query matched, by path
    pub positions: HashMap<String, Vec<usize>>,
    /// Every file that matched, drawn or not
    pub total: usize,
    /// The best match, where the cursor goes as the query changes
    pub best: Option<String>,
}

/// A folder of the matches' tree: what it holds, each keyed by its case-blind name and then its
/// name, so `A` and `a` stay apart and the order is the tree's own.
#[derive(Default)]
struct Node {
    folders: BTreeMap<(String, String), Node>,
    files: BTreeMap<(String, String), String>,
}

impl Node {
    fn insert(&mut self, path: &str) {
        let mut node = self;
        let mut parts = path.split('/').peekable();
        while let Some(part) = parts.next() {
            let key = (part.to_lowercase(), part.to_string());
            if parts.peek().is_none() {
                node.files.insert(key, path.to_string());
            } else {
                node = node.folders.entry(key).or_default();
            }
        }
    }
}

/// The rows and hits for `query` over `files` and the changes.
///
/// Args: `(query, files, changes, ancestors)` — `ancestors` is every folder on the way to a
/// change, so a folder still says it holds one.
pub(crate) fn build(
    query: &str,
    files: &[String],
    changes: &[DockChangeItem],
    ancestors: &HashSet<String>,
) -> (Vec<TreeRow>, FilterHits) {
    // A deleted file or one created since the list was read is still a change worth finding
    let listed: HashSet<&str> = files.iter().map(String::as_str).collect();
    let candidates: Vec<&str> = files
        .iter()
        .map(String::as_str)
        .chain(changes.iter().map(|item| item.path.as_str()).filter(|path| !listed.contains(path)))
        .collect();
    if candidates.is_empty() {
        let note = TreeRow::Note { depth: 0, text: "reading the project…".into() };
        return (vec![note], FilterHits::default());
    }
    let matches = crate::fuzzy::rank_paths(query, &candidates);
    let mut hits = FilterHits {
        total: matches.len(),
        best: matches.first().map(|hit| candidates[hit.idx].to_string()),
        ..FilterHits::default()
    };
    let mut root = Node::default();
    for hit in matches.iter().take(SHOWN) {
        let path = candidates[hit.idx];
        root.insert(path);
        hits.positions.insert(path.to_string(), hit.positions.clone());
        // Each folder on the way lights the matched characters that fall inside it
        for (end, _) in path.match_indices('/') {
            let length = path[..end].chars().count();
            let inside = hit.positions.iter().copied().filter(|at| *at < length);
            hits.positions.entry(path[..end].to_string()).or_default().extend(inside);
        }
    }
    let changed: HashMap<&str, &DockChangeItem> =
        changes.iter().map(|item| (item.path.as_str(), item)).collect();
    let mut rows = Vec::new();
    emit(&root, "", 0, &changed, ancestors, &mut rows);
    match hits.total {
        0 => rows.push(TreeRow::Note { depth: 0, text: "no file matches".into() }),
        total if total > SHOWN => rows.push(TreeRow::Note {
            depth: 0,
            text: format!("… {} more, keep typing", total - SHOWN),
        }),
        _ => {}
    }
    (rows, hits)
}

/// One folder's rows, folders first, a chain of lone folders drawn as one row the way the tree
/// compacts them.
fn emit(
    node: &Node,
    prefix: &str,
    depth: usize,
    changed: &HashMap<&str, &DockChangeItem>,
    ancestors: &HashSet<String>,
    rows: &mut Vec<TreeRow>,
) {
    for ((_, name), child) in &node.folders {
        let (mut path, mut name, mut child) = (format!("{prefix}{name}"), name.clone(), child);
        while child.files.is_empty() && child.folders.len() == 1 {
            let Some(((_, only), next)) = child.folders.iter().next() else { break };
            path = format!("{path}/{only}");
            name = format!("{name}/{only}");
            child = next;
        }
        rows.push(TreeRow::Folder {
            holds_change: ancestors.contains(&path),
            path: path.clone(),
            name,
            depth,
            open: true,
            ignored: false,
        });
        emit(child, &format!("{path}/"), depth + 1, changed, ancestors, rows);
    }
    for ((_, name), path) in &node.files {
        rows.push(TreeRow::File {
            path: path.clone(),
            name: name.clone(),
            depth,
            change: changed.get(path.as_str()).map(|item| (*item).clone()),
            ignored: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luxe_core::FileChangeStatus;

    fn files(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|path| (*path).to_string()).collect()
    }

    fn drawn(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                TreeRow::Folder { name, depth, .. } => format!("{}{name}/", "  ".repeat(*depth)),
                TreeRow::File { name, depth, change, .. } => {
                    let mark = if change.is_some() { " M" } else { "" };
                    format!("{}{name}{mark}", "  ".repeat(*depth))
                }
                TreeRow::Note { text, .. } => format!("({text})"),
            })
            .collect()
    }

    /// The matches are a tree in the tree's own order, whatever order they scored in, each under
    /// the folders that hold it, and a lone chain of folders is one row.
    #[test]
    fn matches_are_drawn_as_a_tree() {
        let project = files(&[
            "Cargo.toml",
            "crates/luxe-tui/src/view/dock.rs",
            "crates/luxe-tui/src/workspace_dock.rs",
            "docs/guide/interface.md",
        ]);
        let (rows, hits) = build("dock", &project, &[], &HashSet::new());
        assert_eq!(
            drawn(&rows),
            ["crates/luxe-tui/src/", "  view/", "    dock.rs", "  workspace_dock.rs"]
        );
        assert_eq!(hits.total, 2);
        assert!(hits.best.as_deref().is_some_and(|best| best.ends_with("dock.rs")));
    }

    /// The whole path is matched, so a folder's name finds what is in it, and the folder lights
    /// the characters that fell inside it.
    #[test]
    fn a_folder_name_finds_its_files_and_lights_up() {
        let project = files(&["src/view/dock.rs", "src/app.rs"]);
        let (rows, hits) = build("view", &project, &[], &HashSet::new());
        assert_eq!(drawn(&rows), ["src/view/", "  dock.rs"]);
        assert_eq!(hits.positions["src/view"], [4, 5, 6, 7]);
    }

    /// A change the list does not hold yet, a file created since it was read, is still found, and
    /// a changed file keeps its mark.
    #[test]
    fn a_change_is_found_whether_listed_or_not() {
        let change = DockChangeItem {
            path: "src/new.rs".into(),
            status: FileChangeStatus::Added,
            additions: Some(3),
            deletions: Some(0),
        };
        let (rows, _) = build("new", &files(&["src/old.rs"]), &[change], &HashSet::new());
        assert_eq!(drawn(&rows), ["src/", "  new.rs M"]);
    }

    /// The tree draws the best few, and says how many more there are and how to reach them; with
    /// nothing matched, or nothing read yet, it says so rather than going blank.
    #[test]
    fn the_rest_and_the_nothing_are_said() {
        let many: Vec<String> = (0..SHOWN + 5).map(|n| format!("file{n:03}.rs")).collect();
        let (rows, hits) = build("file", &many, &[], &HashSet::new());
        assert_eq!(rows.len(), SHOWN + 1);
        assert_eq!(drawn(&rows).last().map(String::as_str), Some("(… 5 more, keep typing)"));
        assert_eq!(hits.total, SHOWN + 5);

        let (rows, _) = build("zzz", &many, &[], &HashSet::new());
        assert_eq!(drawn(&rows), ["(no file matches)"]);
        let (rows, _) = build("a", &[], &[], &HashSet::new());
        assert_eq!(drawn(&rows), ["(reading the project…)"]);
    }
}
