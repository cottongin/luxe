use super::*;

/// A checkout at a temp dir's root, with `files` written into it.
fn checkout(files: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir_all(dir.path().join(".git")).expect("git marker");
    for file in files {
        let path = dir.path().join(file);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("folders");
        std::fs::write(&path, format!("- rules from {file}\n")).expect("write");
    }
    dir
}

/// What [`nested_files`] names for `target`, from luxe launched at `launch`, both under `root`.
fn shown(root: &Path, launch: &str, target: &str) -> Vec<String> {
    let files = nested_files(&root.join(launch), &root.join(target));
    files.into_iter().map(|file| file.shown).collect()
}

/// The break this catches: reading only `path`, so a multi-file `read` loaded nothing.
#[test]
fn a_call_touches_its_path_and_each_of_its_paths() {
    let args = serde_json::json!({ "path": "a.rs", "paths": ["web/b.ts", 7, "", "c.md"] });
    assert_eq!(touched_paths(&args), ["a.rs", "web/b.ts", "c.md"]);
    assert!(touched_paths(&serde_json::json!({ "command": "cat web/x" })).is_empty());
    assert!(touched_paths(&serde_json::json!({ "path": 3 })).is_empty());
}

/// The break this catches: the walk started at the launch folder, so a file one folder over
/// from it was never found.
#[test]
fn a_folder_off_the_launch_chain_is_found() {
    let root = checkout(&["AGENTS.md", "web/AGENTS.md", "web/src/app.ts"]);
    assert_eq!(shown(root.path(), "", "web/src/app.ts"), ["web/AGENTS.md"]);
}

/// The break this catches: the walk ran nearest first, so the more general rules arrived last
/// and read as the ones that win.
#[test]
fn files_come_top_down() {
    let root = checkout(&["web/AGENTS.md", "web/src/AGENTS.md", "web/src/app.ts"]);
    let found = shown(root.path(), "", "web/src/app.ts");
    assert_eq!(found, ["web/AGENTS.md", "web/src/AGENTS.md"]);
}

/// The break this catches: folders on the launch chain were offered again, sending a file the
/// system prompt already carries.
#[test]
fn the_launch_chain_is_left_to_the_system_prompt() {
    let root = checkout(&["AGENTS.md", "web/AGENTS.md", "web/src/AGENTS.md", "web/src/app.ts"]);
    assert_eq!(shown(root.path(), "web", "web/src/app.ts"), ["web/src/AGENTS.md"]);
    assert!(shown(root.path(), "", "README.md").is_empty(), "the top is the prompt's");
}

/// The break this catches: no ignore check, so a dependency's own `AGENTS.md` under an ignored
/// folder instructed the model as if this project wrote it.
#[test]
fn an_ignored_folder_is_skipped_and_its_sibling_is_not() {
    let root = checkout(&["vendor/lib/AGENTS.md", "app/AGENTS.md"]);
    std::fs::write(root.path().join(".gitignore"), "vendor/\n").expect("gitignore");
    assert!(shown(root.path(), "", "vendor/lib/x.rs").is_empty());
    assert_eq!(shown(root.path(), "", "app/x.rs"), ["app/AGENTS.md"]);
}

/// The break this catches: no checkout boundary, so a path outside the project loaded the rules
/// of whoever owns that folder.
#[test]
fn a_path_outside_the_checkout_finds_nothing() {
    let root = checkout(&[]);
    let elsewhere = checkout(&["lib/AGENTS.md", "lib/x.rs"]);
    let files = nested_files(root.path(), &elsewhere.path().join("lib/x.rs"));
    assert!(files.is_empty(), "{files:?}");
}

/// The break this catches: a target that doesn't exist yet (a file about to be written) found
/// no folder and so no rules.
#[test]
fn a_file_not_written_yet_uses_the_nearest_folder() {
    let root = checkout(&["web/AGENTS.md"]);
    assert_eq!(shown(root.path(), "", "web/new/deep/page.ts"), ["web/AGENTS.md"]);
}

/// The break this catches: only `AGENTS.md` was looked for, so a folder that still carries a
/// `CLAUDE.md` was silent here while the launch chain read it.
#[test]
fn a_claude_md_counts_like_it_does_on_the_chain() {
    let root = checkout(&["web/CLAUDE.md"]);
    assert_eq!(shown(root.path(), "", "web/x.ts"), ["web/CLAUDE.md"]);
}

/// The break this catches: reading a note back stopped at the first `]`, so a path holding one
/// read as a different file and was sent again.
#[test]
fn a_note_reads_back_as_its_file() {
    let block = note("web/[id]/AGENTS.md", "- rules");
    assert_eq!(noted(&block), Some("web/[id]/AGENTS.md"));
    assert!(block.ends_with("\n\n- rules"), "{block}");
    assert_eq!(noted("[instructions: web/AGENTS.md"), None, "a fragment isn't a note");
    assert_eq!(noted("12: [instructions: x] y"), None, "nor is a line of a file that quotes one");
}
