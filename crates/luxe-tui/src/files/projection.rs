//! File projection — the rows the tree shows, walked from its listings, with single-child folders
//! compacted.

use super::*;

impl FilesTree {
    pub(super) fn build(&self, changes: &[DockChangeItem]) -> Vec<TreeRow> {
        let changed: HashMap<&str, &DockChangeItem> =
            changes.iter().map(|item| (item.path.as_str(), item)).collect();
        let ancestors = ancestors(changes);
        let from_changes = self.changed_only || self.listed("").is_none();
        let mut rows = Vec::new();
        if from_changes && changes.is_empty() {
            if !self.changed_only {
                rows.push(self.note_for("", 0));
            }
            return rows;
        }
        self.walk("", 0, from_changes, &changed, &ancestors, &mut rows);
        rows
    }

    fn walk(
        &self,
        folder: &str,
        depth: usize,
        from_changes: bool,
        changed: &HashMap<&str, &DockChangeItem>,
        ancestors: &HashSet<String>,
        rows: &mut Vec<TreeRow>,
    ) {
        let (children, listing) = self.children(folder, from_changes, changed);
        // A folder with changes under it already has rows to show while it is read; a note above
        // them would only push them down for a moment and pull them back up
        if !from_changes && listing.is_none() && children.is_empty() {
            rows.push(self.note_for(folder, depth));
        }
        let prefix = if folder.is_empty() { String::new() } else { format!("{folder}/") };
        for (name, kind, ignored) in children {
            let path = format!("{prefix}{name}");
            match kind {
                Child::Folder => {
                    let (path, name, open) =
                        self.compacted(path, name, from_changes, changed, ancestors);
                    rows.push(TreeRow::Folder {
                        path: path.clone(),
                        name,
                        depth,
                        open,
                        holds_change: ancestors.contains(&path),
                        ignored,
                    });
                    if open {
                        self.walk(&path, depth + 1, from_changes, changed, ancestors, rows);
                    }
                }
                Child::File => {
                    let change = changed.get(path.as_str()).map(|item| (*item).clone());
                    rows.push(TreeRow::File { path, name, depth, change, ignored });
                }
            }
        }
        if let Some(omitted) = listing.map(|listing| listing.omitted).filter(|count| *count > 0) {
            rows.push(TreeRow::Note { depth, text: format!("… {omitted} more not listed") });
        }
    }

    /// One folder's children, folders first, then files, each in case-blind name order: what its
    /// listing holds, and what changed under it whether listed or not (a deleted file, or a folder
    /// not read yet).
    fn children(
        &self,
        folder: &str,
        from_changes: bool,
        changed: &HashMap<&str, &DockChangeItem>,
    ) -> (Vec<(String, Child, bool)>, Option<&DirectoryListing>) {
        let mut children: BTreeMap<(bool, String, String), (Child, bool)> = BTreeMap::new();
        let mut insert = |name: &str, kind: Child, ignored: bool| {
            // A case-blind name, then the name itself to keep `A` and `a` apart
            let key = (kind == Child::File, name.to_lowercase(), name.to_string());
            children.entry(key).or_insert((kind, ignored));
        };
        let listing = (!from_changes).then(|| self.listed(folder)).flatten();
        if let Some(listing) = listing {
            for entry in &listing.entries {
                let kind = match entry.kind {
                    EntryKind::Directory => Child::Folder,
                    EntryKind::File => Child::File,
                };
                insert(&entry.name, kind, entry.ignored);
            }
        }
        let prefix = if folder.is_empty() { String::new() } else { format!("{folder}/") };
        for path in changed.keys() {
            let Some(rest) = path.strip_prefix(prefix.as_str()) else { continue };
            match rest.split_once('/') {
                Some((name, _)) => insert(name, Child::Folder, false),
                None => insert(rest, Child::File, false),
            }
        }
        let children = children
            .into_iter()
            .map(|((_, _, name), (kind, ignored))| (name, kind, ignored))
            .collect();
        (children, listing)
    }

    /// A folder whose only entry is another folder, drawn as one row: `crates/luxe-core/src`.
    ///
    /// What VS Code calls compact folders. A rail forty cells wide cannot spend a row and two cells
    /// of indent on every `src` in a chain, and the chain is one decision for the reader anyway.
    /// Only while the folder is drawn open and its listing is known, so a chain never hides a file
    /// that has not been read yet; the row toggles the deepest folder in it, and says whether it is
    /// drawn open.
    ///
    /// A folder taken into the chain is open unless the reader closed it. Opening `a` and finding
    /// it holds only `b` is opening the row the reader is now looking at, `a/b`: drawing that row
    /// closed made the folder they had just opened snap shut the moment its listing landed.
    fn compacted(
        &self,
        mut path: String,
        mut name: String,
        from_changes: bool,
        changed: &HashMap<&str, &DockChangeItem>,
        ancestors: &HashSet<String>,
    ) -> (String, String, bool) {
        let mut open = self.is_open(&path, ancestors, from_changes);
        loop {
            if !open {
                return (path, name, false);
            }
            let (children, listing) = self.children(&path, from_changes, changed);
            let known = from_changes || listing.is_some_and(|listing| listing.omitted == 0);
            match children.as_slice() {
                [(only, Child::Folder, _)] if known => {
                    path = format!("{path}/{only}");
                    name = format!("{name}/{only}");
                    open = !self.closed.contains(&path);
                }
                _ => return (path, name, true),
            }
        }
    }
}
