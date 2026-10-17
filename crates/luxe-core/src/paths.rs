//! paths — how a path becomes a string, and how two paths are compared.
//!
//! A `Path` is bytes to the filesystem, but the moment it becomes a *string* it is data: a line the
//! model reads and writes back, a permission pattern the glob matcher evaluates, a label the sort
//! order splits on `/`. Windows renders that string with `\`, and every one of those consumers
//! speaks `/` — the model was trained on it, our globs are built on it, git reports it, and the
//! tools' own descriptions promise it.
//!
//! So the invariant is: **`\` never leaves the filesystem layer.** [`display`] is the one way a path
//! becomes user- or model-facing text, and it answers with forward slashes on every platform.
//! Windows accepts `/` back in essentially every API, so nothing is lost by round-tripping.
//!
//! The comparison half is the same idea from the other end. `starts_with` is case-sensitive and
//! Windows filesystems are not, so `C:\Users\Dev\project` "escapes" `c:\users\dev\project` — which
//! in [`crate::permission`] means the external-directory guard fires on the user's own project.
//! [`contains`] compares the way the platform's filesystem does.
//!
//! The third Windows surprise has no function of its own, because the rule is easier than a
//! helper: `fs::canonicalize` hands back a *verbatim* path, `\\?\C:\project`, and one of those
//! must never become user- or model-facing text. It is a correct path and almost nothing else
//! likes it — it reaches the model as `//?/C:/project`, and a program handed it as a working
//! directory may not parse it at all. So a canonical path is for *comparing and opening*, which
//! [`contains`] and the filesystem both do correctly with the prefix still attached, and the
//! spelling that reaches a reader comes from the path as it was configured. Strip the prefix only
//! if a canonical path ever has to escape those two uses.
//!
//! Logging is deliberately not a consumer of any of this: a `tracing` field is for the operator
//! standing in front of the machine, and there the native spelling is the honest one.

use std::ffi::OsStr;
use std::path::{Component, Path, Prefix};

/// Render `path` as text, with forward slashes on every platform.
///
/// Use this anywhere a path becomes something other than a path: tool output, a prompt, a TUI label,
/// a permission pattern, a key that will later be split on `/`.
pub fn display(path: &Path) -> String {
    slashify(&path.to_string_lossy())
}

/// Render `path` with the user's home directory collapsed to `~`.
///
/// Shorter to read, and it keeps the user's name off the screen and out of their screenshots.
/// `$HOME` is not set on Windows, so this asks [`crate::user_dirs`] rather than the environment.
pub fn home_relative(path: &Path) -> String {
    match crate::user_dirs::home_dir() {
        Some(home) => home_relative_to(path, &home),
        None => display(path),
    }
}

/// [`home_relative`] against an explicit home, for the caller that is scanning one it was handed.
pub fn home_relative_to(path: &Path, home: &Path) -> String {
    if path == home {
        return "~".to_string();
    }
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", display(rest)),
        Err(_) => display(path),
    }
}

/// The name a directory shows under: its own last component.
///
/// A path with no nameable component — a filesystem root, a bare drive prefix — falls back to the
/// whole rendered path, because "" names nothing and a project has to be called something.
pub fn dir_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| display(path))
}

/// Rewrite an already-rendered path string to forward slashes.
///
/// For the cases where the string arrived as a string. On unix this is the identity function on
/// purpose: `\` is a legal character in a unix filename, so rewriting it there would corrupt a real
/// (if unusual) name rather than normalize a separator.
pub fn slashify(rendered: &str) -> String {
    slashify_on(rendered, cfg!(windows))
}

/// Whether `path` is inside `root` (or is `root`), compared the way the platform compares filenames.
///
/// Purely lexical — neither side is resolved — so it answers for paths that do not exist yet, which
/// is what a write target is. Component-wise rather than string-prefix, so `project-two` is not
/// inside `project`. Case-insensitive on Windows, and indifferent to whether one side came back
/// verbatim from `canonicalize` and the other did not.
pub fn contains(root: &Path, path: &Path) -> bool {
    let mut root_parts = root.components().map(comparable);
    let mut path_parts = path.components().map(comparable);
    loop {
        let Some(wanted) = root_parts.next() else { return true };
        let Some(actual) = path_parts.next() else { return false };
        if actual != wanted {
            return false;
        }
    }
}

/// One path component reduced to something two spellings of the same name compare equal on.
///
/// Windows is case-insensitive and has two spellings of a drive prefix (`C:` and the verbatim
/// `\\?\C:`); unix has neither, and the prefix arm is dead code there.
fn comparable(component: Component<'_>) -> String {
    let raw: &OsStr = match component {
        Component::Prefix(prefix) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                return (letter as char).to_ascii_uppercase().to_string()
            }
            _ => prefix.as_os_str(),
        },
        other => other.as_os_str(),
    };
    let rendered = raw.to_string_lossy();
    if cfg!(windows) {
        rendered.to_lowercase()
    } else {
        rendered.into_owned()
    }
}

/// [`slashify`] with the platform passed in, so Windows behavior is testable from anywhere.
fn slashify_on(rendered: &str, windows: bool) -> String {
    if windows {
        rendered.replace('\\', "/")
    } else {
        rendered.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn windows_separators_become_forward_slashes() {
        assert_eq!(slashify_on(r"C:\Users\dev\src\main.rs", true), "C:/Users/dev/src/main.rs");
        assert_eq!(slashify_on("src/main.rs", true), "src/main.rs", "already-slashed is untouched");
        assert_eq!(slashify_on(r"src\a/b\c.rs", true), "src/a/b/c.rs", "a mixed path is settled");
    }

    /// The reason this is not a blanket `replace`: on unix a backslash is an ordinary character in a
    /// filename, and rewriting it would rename the file rather than normalize a separator.
    #[test]
    fn a_backslash_in_a_unix_filename_survives() {
        assert_eq!(slashify_on(r"weird\name.txt", false), r"weird\name.txt");
    }

    #[test]
    fn display_renders_through_the_platform_rule() {
        let path: PathBuf = ["src", "main.rs"].iter().collect();
        assert_eq!(display(&path), "src/main.rs");
    }

    #[test]
    fn a_path_under_home_reads_as_a_tilde() {
        let home = crate::user_dirs::home_dir().expect("a home directory");
        assert_eq!(home_relative(&home), "~");
        assert_eq!(home_relative(&home.join("code").join("luxe")), "~/code/luxe");

        let elsewhere = if cfg!(windows) { r"C:\opt\luxe" } else { "/opt/luxe" };
        assert_eq!(home_relative(Path::new(elsewhere)), slashify(elsewhere), "outside home, whole");
    }

    #[test]
    fn containment_is_component_wise_not_a_string_prefix() {
        let root: PathBuf = ["home", "dev", "project"].iter().collect();
        assert!(contains(&root, &root), "a root contains itself");
        assert!(contains(&root, &root.join("src").join("a.rs")));
        assert!(
            !contains(&root, Path::new("home").join("dev").join("project-two").as_path()),
            "a sibling whose name merely starts the same is outside"
        );
        assert!(
            !contains(&root, Path::new("home").join("dev").as_path()),
            "a parent is not inside"
        );
    }

    /// Windows filesystems are case-insensitive, so a case-sensitive comparison reports the user's
    /// own project as somebody else's directory. Written in the platform's own path syntax because
    /// `Path::components` only splits on the separator the host uses.
    #[cfg(windows)]
    #[test]
    fn windows_containment_ignores_case_and_the_verbatim_prefix() {
        let root = Path::new(r"C:\Users\Dev\project");
        assert!(contains(root, Path::new(r"c:\users\dev\PROJECT\src\main.rs")));
        assert!(
            contains(Path::new(r"\\?\C:\Users\Dev\project"), Path::new(r"C:\Users\Dev\project\a")),
            "a canonicalized root still contains an ordinary path"
        );
        assert!(!contains(root, Path::new(r"C:\Users\Dev\other")));
    }
}
