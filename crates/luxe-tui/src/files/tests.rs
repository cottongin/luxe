use super::*;
use luxe_core::project_files::DirectoryEntry;
use luxe_core::FileChangeStatus;

fn change(path: &str) -> DockChangeItem {
    DockChangeItem {
        path: path.into(),
        status: FileChangeStatus::Modified,
        additions: Some(1),
        deletions: Some(0),
    }
}

fn listing(path: &str, folders: &[&str], files: &[&str]) -> DirectoryListing {
    let entry = |name: &&str, kind| DirectoryEntry { name: (*name).into(), kind, ignored: false };
    DirectoryListing {
        path: path.into(),
        entries: folders
            .iter()
            .map(|name| entry(name, EntryKind::Directory))
            .chain(files.iter().map(|name| entry(name, EntryKind::File)))
            .collect(),
        omitted: 0,
    }
}

/// Answer the outstanding request for `folder` with `listed`.
fn answer(tree: &mut FilesTree, listed: DirectoryListing) {
    let request = ListingRequestId::new();
    tree.requested(&listed.path, request.clone());
    assert!(tree.accept(&request, listed));
}

fn drawn(rows: &[TreeRow]) -> Vec<String> {
    rows.iter()
        .map(|row| match row {
            TreeRow::Folder { name, depth, open, .. } => {
                format!("{}{}{name}/", "  ".repeat(*depth), if *open { "-" } else { "+" })
            }
            TreeRow::File { name, depth, change, .. } => {
                format!("{}{name}{}", "  ".repeat(*depth), if change.is_some() { " M" } else { "" })
            }
            TreeRow::Note { depth, text } => format!("{}({text})", "  ".repeat(*depth)),
        })
        .collect()
}

/// Closing a filter opens the way to the file it found, so the cursor is still on a row once
/// the whole tree is back.
#[test]
fn closing_a_filter_opens_the_way_to_the_cursor() {
    let mut tree = FilesTree::default();
    answer(&mut tree, listing("", &["src"], &["README.md"]));
    answer(&mut tree, listing("src", &["view"], &["app.rs"]));
    answer(&mut tree, listing("src/view", &[], &["dock.rs"]));
    tree.set_project_files(&["src/view/dock.rs".to_string(), "README.md".to_string()]);
    tree.open_filter();
    "dock".chars().for_each(|character| tree.edit_filter(Some(character)));
    assert_eq!(drawn(&tree.rows(&[])), ["-src/view/", "  dock.rs"]);
    tree.cursor = Some("src/view/dock.rs".into());
    tree.close_filter();
    assert_eq!(
        drawn(&tree.rows(&[])),
        ["-src/", "  -view/", "    dock.rs", "  app.rs", "README.md"]
    );
}

#[test]
fn before_anything_is_listed_the_changes_are_the_tree() {
    let tree = FilesTree::default();
    let changes = [change("src/view/dock.rs"), change("PLAN.md")];
    assert_eq!(drawn(&tree.rows(&changes)), ["-src/view/", "  dock.rs M", "PLAN.md M"]);
    assert_eq!(drawn(&tree.rows(&[])), ["(reading…)"], "a clean tree says it is on its way");
    assert_eq!(tree.wanted(&changes), [""], "and the root is what it asks for first");
}

#[test]
fn folders_on_the_way_to_a_change_open_and_the_rest_wait() {
    let mut tree = FilesTree::default();
    let changes = [change("src/view/dock.rs")];
    answer(&mut tree, listing("", &["docs", "src"], &["Cargo.toml", "PLAN.md"]));
    assert_eq!(
        drawn(&tree.rows(&changes)),
        ["+docs/", "-src/", "  -view/", "    dock.rs M", "Cargo.toml", "PLAN.md"]
    );
    assert_eq!(tree.wanted(&changes), ["src", "src/view"]);

    answer(&mut tree, listing("src", &["view"], &["lib.rs"]));
    answer(&mut tree, listing("src/view", &[], &["dock.rs", "input.rs"]));
    assert_eq!(
        drawn(&tree.rows(&changes)),
        [
            "+docs/",
            "-src/",
            "  -view/",
            "    dock.rs M",
            "    input.rs",
            "  lib.rs",
            "Cargo.toml",
            "PLAN.md"
        ]
    );
    assert!(tree.wanted(&changes).is_empty());
}

#[test]
fn the_readers_own_choice_outlives_the_changes() {
    let mut tree = FilesTree::default();
    let changes = [change("src/lib.rs")];
    answer(&mut tree, listing("", &["docs", "src"], &[]));
    answer(&mut tree, listing("src", &[], &["lib.rs"]));
    tree.toggle("src", &changes);
    assert_eq!(drawn(&tree.rows(&changes)), ["+docs/", "+src/"], "closed by hand stays closed");

    tree.toggle("docs", &changes);
    assert_eq!(tree.wanted(&changes), ["docs"]);
    answer(&mut tree, listing("docs", &[], &["guide.md"]));
    assert_eq!(drawn(&tree.rows(&[])), ["-docs/", "  guide.md", "+src/"], "and opened stays open");
}

#[test]
fn a_deleted_file_is_drawn_from_the_changes_alone() {
    let mut tree = FilesTree::default();
    answer(&mut tree, listing("", &[], &["kept.rs"]));
    let gone = DockChangeItem { status: FileChangeStatus::Deleted, ..change("gone.rs") };
    assert_eq!(drawn(&tree.rows(&[gone])), ["gone.rs M", "kept.rs"]);
}

#[test]
fn changed_only_is_the_changes_list_in_tree_form() {
    let mut tree = FilesTree { changed_only: true, ..FilesTree::default() };
    answer(&mut tree, listing("", &["docs", "src"], &["Cargo.toml"]));
    let changes = [change("src/lib.rs")];
    assert_eq!(drawn(&tree.rows(&changes)), ["-src/", "  lib.rs M"]);
    assert!(tree.wanted(&changes).is_empty(), "nothing to list for a list of changes");
    assert!(drawn(&tree.rows(&[])).is_empty());
}

#[test]
fn a_superseded_listing_is_discarded_and_a_new_change_re_asks() {
    let mut tree = FilesTree::default();
    let old = ListingRequestId::new();
    tree.requested("", old.clone());
    let newer = ListingRequestId::new();
    tree.requested("", newer.clone());
    assert!(!tree.accept(&old, listing("", &[], &["a.rs"])));
    assert!(tree.accept(&newer, listing("", &[], &["a.rs"])));
    assert!(tree.wanted(&[]).is_empty());

    assert!(tree.refresh_if_changed(&[change("b.rs")]));
    assert!(!tree.refresh_if_changed(&[change("b.rs")]), "the same changes are not news");
    assert_eq!(tree.wanted(&[change("b.rs")]), [""], "the root is asked again");
    assert_eq!(
        drawn(&tree.rows(&[change("b.rs")])),
        ["a.rs", "b.rs M"],
        "and stays drawn meanwhile"
    );
}

/// A chain of lone folders is one row, as long as each link is known to be alone.
#[test]
fn a_chain_of_lone_folders_is_drawn_as_one_row() {
    let mut tree = FilesTree::default();
    let changes = [change("crates/core/src/lib.rs")];
    answer(&mut tree, listing("", &["crates"], &[]));
    assert_eq!(
        drawn(&tree.rows(&changes)),
        ["-crates/", "  -core/", "    -src/", "      lib.rs M"],
        "nothing is merged before it is read"
    );
    answer(&mut tree, listing("crates", &["core"], &[]));
    answer(&mut tree, listing("crates/core", &["src"], &[]));
    answer(&mut tree, listing("crates/core/src", &[], &["lib.rs", "main.rs"]));
    assert_eq!(drawn(&tree.rows(&changes)), ["-crates/core/src/", "  lib.rs M", "  main.rs"]);

    // Closing the row closes the folder it ends on, and the chain stays one row
    tree.toggle("crates/core/src", &changes);
    assert_eq!(drawn(&tree.rows(&changes)), ["+crates/core/src/"]);
}

#[test]
fn revealing_a_file_opens_the_way_to_it() {
    let mut tree = FilesTree::default();
    answer(&mut tree, listing("", &["crates"], &[]));
    tree.reveal("crates/core/src/lib.rs");
    assert_eq!(tree.wanted(&[]), ["crates"]);
    answer(&mut tree, listing("crates", &["core"], &[]));
    assert_eq!(tree.wanted(&[]), ["crates/core"]);
}

/// An empty folder opens on the first click and stays open once its empty listing lands.
#[test]
fn an_empty_folder_opens_on_the_first_click() {
    let mut tree = FilesTree::default();
    answer(&mut tree, listing("", &["empty", "src"], &["a.txt"]));
    tree.toggle("empty", &[]);
    assert_eq!(tree.wanted(&[]), ["empty"]);
    answer(&mut tree, listing("empty", &[], &[]));
    assert_eq!(drawn(&tree.rows(&[]))[0], "-empty/", "{:?}", drawn(&tree.rows(&[])));
}

/// Opening a folder that turns out to hold only another folder opens the chain, rather than
/// folding it into a closed `a/b` row the moment the listing lands; closing that row closes it.
#[test]
fn a_folder_that_holds_one_folder_stays_open_when_its_listing_lands() {
    let mut tree = FilesTree::default();
    answer(&mut tree, listing("", &["a"], &[]));
    tree.toggle("a", &[]);
    answer(&mut tree, listing("a", &["b"], &[]));
    assert_eq!(drawn(&tree.rows(&[])), ["-a/b/", "  (reading…)"]);
    answer(&mut tree, listing("a/b", &[], &[]));
    assert_eq!(drawn(&tree.rows(&[])), ["-a/b/"]);

    tree.toggle("a/b", &[]);
    assert_eq!(drawn(&tree.rows(&[])), ["+a/b/"], "closed by hand stays closed");
    tree.toggle("a/b", &[]);
    assert_eq!(drawn(&tree.rows(&[])), ["-a/b/"]);
}
