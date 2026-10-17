//! Key labels — every key the TUI names is written in lower case.
//!
//! `ctrl+p` and `enter`, the way a keybindings file spells them, so a capital only ever means
//! Shift: `G` is Shift+G, and `ctrl+p` can't be misread as Ctrl+Shift+P the way `Ctrl+P` could.
//! A label from the keymap gets this from `KeyChord`'s `Display`; the ones written by hand in hint
//! rows and fallbacks are what this holds to it, since nothing else would notice one slip back.

use std::path::{Path, PathBuf};

/// Hand-written spellings that break the rule, as they would sit in a string literal.
const CAPITALIZED: &[&str] = &[
    "Ctrl+",
    "Alt+",
    "Shift+",
    "Option+",
    "\"Enter ",
    "\"Esc ",
    "\"Tab ",
    "\"Space ",
    "· Enter ",
    "· Esc ",
    "· Tab ",
    "· Space ",
    "\" Enter ",
    "\" Esc ",
    "\"Enter\"",
    "\"Esc\"",
    "\"Tab\"",
    "\"Space\"",
    "PgUp",
    "PgDn",
];

fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("a source folder").flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

/// The lines of `text` that ship: no comments, nothing from a test module onward, and none of a
/// file that is only tests.
fn shipped(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let only_tests = text.lines().take(40).any(|line| line.starts_with("use super::*"))
        && !text.contains("#[cfg(test)]");
    text.lines()
        .enumerate()
        .take_while(move |(_, line)| !only_tests && !line.trim_start().starts_with("#[cfg(test)]"))
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
}

#[test]
fn every_key_the_tui_names_is_lower_case() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files);
    sources(&root.join("../luxe-core/src/keymap"), &mut files);
    let mut capitalized = Vec::new();
    for file in files {
        let name = file.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        if name == "tests.rs" || name.ends_with("_keys.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&file).expect("readable source");
        for (index, line) in shipped(&text) {
            if let Some(spelling) = CAPITALIZED.iter().find(|spelling| line.contains(*spelling)) {
                capitalized.push(format!("{}:{}: {spelling}", file.display(), index + 1));
            }
        }
    }
    assert!(capitalized.is_empty(), "write keys in lower case:\n{}", capitalized.join("\n"));
}
