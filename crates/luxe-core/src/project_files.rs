//! Project files — one folder's listing and one file's text, for a reader browsing the project.
//!
//! The Context Dock's FILES tree and its preview ask through here, `Action::ListDirectory` and
//! `Action::ReadProjectFile` in, one event back, so the UI never walks the disk itself. Both are
//! bounded and both stay inside the project: a path that climbs out of it, or a symlink that
//! leads out, is refused rather than followed.
//!
//! What a listing leaves out by default is what the agent's own `glob` leaves out: whatever
//! `.gitignore` ignores, and `.git`. A tree that opened `target/` to a hundred thousand rows would be
//! a tree nobody could use. Dotfiles are listed, since `.github` and `.gitignore` are as much the
//! project as `src`. A reader can ask for the ignored entries too, and gets them marked as such;
//! `.git` is never listed.

use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

/// Most entries one folder's listing carries.
///
/// A folder past this is a generated one (vendored dependencies, fixtures by the thousand), and the
/// listing says how many it left out rather than drawing them all.
pub const MAX_ENTRIES: usize = 2_000;

/// Bytes past which a file is previewed only as far as this, and says it was cut.
pub const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;

/// Bytes sniffed for a NUL to tell a binary file from text.
const BINARY_SNIFF: usize = 8 * 1024;

/// Identifies one listing request, so a superseded answer can be discarded.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ListingRequestId(String);

impl ListingRequestId {
    /// A distinct request id.
    pub fn new() -> Self {
        Self(format!("listing:{}", Uuid::new_v4()))
    }
}

impl Default for ListingRequestId {
    fn default() -> Self {
        Self::new()
    }
}

/// Identifies one file-read request.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FileReadRequestId(String);

impl FileReadRequestId {
    /// A distinct request id.
    pub fn new() -> Self {
        Self(format!("file-read:{}", Uuid::new_v4()))
    }
}

impl Default for FileReadRequestId {
    fn default() -> Self {
        Self::new()
    }
}

/// What a folder entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// A folder that can be listed in turn
    Directory,
    /// Anything that can be read: a file, or a symlink to one inside the project
    File,
}

/// One entry of a folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryEntry {
    /// The entry's own name, not its path
    pub name: String,
    /// Folder or file
    pub kind: EntryKind,
    /// Whether `.gitignore` ignores it, which a listing only carries when asked for
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ignored: bool,
}

/// One folder's entries: folders first, then files, each in case-blind name order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectoryListing {
    /// Project-relative folder, with forward slashes; empty for the project root.
    pub path: String,
    /// What the folder holds.
    pub entries: Vec<DirectoryEntry>,
    /// Entries left out past [`MAX_ENTRIES`].
    #[serde(default)]
    pub omitted: usize,
}

/// One file's text, as the preview shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectFile {
    /// Project-relative path, with forward slashes.
    pub path: String,
    /// The text, up to [`MAX_FILE_BYTES`]; empty for a binary file.
    pub text: String,
    /// Size on disk, in bytes.
    pub bytes: u64,
    /// Whether the text stops short of the file.
    #[serde(default)]
    pub truncated: bool,
    /// Whether the file is not text, and so has no lines to show.
    #[serde(default)]
    pub binary: bool,
}

/// Why a folder or a file could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
#[serde(rename_all = "snake_case")]
pub enum ProjectFileFailure {
    /// The path is not inside the project.
    #[error("that is not a path inside this project")]
    OutsideProject,
    /// Nothing is there any more.
    #[error("nothing is there any more")]
    NotFound,
    /// A folder was asked for as a file, or a file as a folder.
    #[error("that is not what it was asked for as")]
    WrongKind,
    /// The filesystem refused.
    #[error("it could not be read: {0}")]
    Unreadable(String),
}

/// List one project-relative folder. `""` is the project root. With `include_ignored`, what
/// `.gitignore` ignores is listed too, each such entry marked.
pub fn list_directory(
    root: &Path,
    relative: &str,
    include_ignored: bool,
) -> Result<DirectoryListing, ProjectFileFailure> {
    let folder = resolve(root, relative)?;
    if !folder.is_dir() {
        return Err(ProjectFileFailure::WrongKind);
    }
    let kept: std::collections::HashSet<std::ffi::OsString> = walk(&folder, true)
        .filter(|entry| entry.depth() == 1)
        .map(|entry| entry.file_name().to_os_string())
        .collect();
    let mut entries = Vec::new();
    for entry in walk(&folder, !include_ignored).filter(|entry| entry.depth() == 1) {
        let ignored = !kept.contains(entry.file_name());
        let name = entry.file_name().to_string_lossy().into_owned();
        let kind = match entry.file_type() {
            Some(kind) if kind.is_dir() => EntryKind::Directory,
            Some(kind) if kind.is_symlink() => match symlink_target_kind(root, entry.path()) {
                Some(kind) => kind,
                None => continue,
            },
            _ => EntryKind::File,
        };
        entries.push(DirectoryEntry { name, kind, ignored });
    }
    entries.sort_by(|a, b| {
        (a.kind != EntryKind::Directory)
            .cmp(&(b.kind != EntryKind::Directory))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    let omitted = entries.len().saturating_sub(MAX_ENTRIES);
    entries.truncate(MAX_ENTRIES);
    Ok(DirectoryListing { path: normalized(relative), entries, omitted })
}

/// One level of `folder`, honoring the ignore files when `ignoring`, never `.git`.
fn walk(folder: &Path, ignoring: bool) -> impl Iterator<Item = ignore::DirEntry> {
    ignore::WalkBuilder::new(folder)
        .max_depth(Some(1))
        .hidden(false)
        .ignore(ignoring)
        .git_ignore(ignoring)
        .git_exclude(ignoring)
        .git_global(ignoring)
        .parents(ignoring)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build()
        .flatten()
}

/// Read one project-relative file for the preview.
pub fn read_file(root: &Path, relative: &str) -> Result<ProjectFile, ProjectFileFailure> {
    use std::io::Read;

    let path = resolve(root, relative)?;
    if !path.is_file() {
        return Err(ProjectFileFailure::WrongKind);
    }
    let bytes = std::fs::metadata(&path).map(|meta| meta.len()).map_err(unreadable)?;
    let mut content = Vec::new();
    std::fs::File::open(&path)
        .map_err(unreadable)?
        .take(MAX_FILE_BYTES as u64)
        .read_to_end(&mut content)
        .map_err(unreadable)?;
    let binary = content.iter().take(BINARY_SNIFF).any(|byte| *byte == 0)
        || std::str::from_utf8(&content).is_err_and(|error| error.error_len().is_some());
    let truncated = bytes > content.len() as u64;
    let text = match binary {
        true => String::new(),
        // A cut can land inside a character; drop the partial one rather than the whole file
        false => match String::from_utf8(content) {
            Ok(text) => text,
            Err(error) => {
                let valid = error.utf8_error().valid_up_to();
                let mut bytes = error.into_bytes();
                bytes.truncate(valid);
                String::from_utf8(bytes).unwrap_or_default()
            }
        },
    };
    Ok(ProjectFile { path: normalized(relative), text, bytes, truncated, binary })
}

/// `relative` under `root`, refused when it climbs out lexically or through a symlink.
fn resolve(root: &Path, relative: &str) -> Result<PathBuf, ProjectFileFailure> {
    let relative = Path::new(relative.trim_end_matches('/'));
    let lexical = relative
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));
    if relative.is_absolute() || !lexical {
        return Err(ProjectFileFailure::OutsideProject);
    }
    let joined = root.join(relative);
    let canonical_root = root.canonicalize().map_err(unreadable)?;
    let canonical = joined.canonicalize().map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => ProjectFileFailure::NotFound,
        _ => unreadable(error),
    })?;
    if !canonical.starts_with(&canonical_root) {
        return Err(ProjectFileFailure::OutsideProject);
    }
    Ok(canonical)
}

/// What a symlink inside the project points at, or `None` when it leads out of it or nowhere.
fn symlink_target_kind(root: &Path, link: &Path) -> Option<EntryKind> {
    let target = link.canonicalize().ok()?;
    if !target.starts_with(root.canonicalize().ok()?) {
        return None;
    }
    Some(if target.is_dir() { EntryKind::Directory } else { EntryKind::File })
}

/// A path as the rest of Luxe spells it: forward slashes, no leading or trailing one.
fn normalized(relative: &str) -> String {
    crate::paths::slashify(relative.trim_matches('/'))
}

fn unreadable(error: std::io::Error) -> ProjectFileFailure {
    ProjectFileFailure::Unreadable(error.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/nested")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n*.log\n").unwrap();
        std::fs::write(root.join("README.md"), "# hi\n").unwrap();
        std::fs::write(root.join("build.log"), "noise").unwrap();
        std::fs::write(root.join("b.rs"), "fn b() {}\n").unwrap();
        std::fs::write(root.join("Alpha.rs"), "fn a() {}\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn x() {}\n").unwrap();
        dir
    }

    fn names(listing: &DirectoryListing) -> Vec<&str> {
        listing.entries.iter().map(|entry| entry.name.as_str()).collect()
    }

    #[test]
    fn a_listing_is_folders_first_and_leaves_out_what_git_ignores() {
        let dir = project();
        let listing = list_directory(dir.path(), "", false).unwrap();
        assert_eq!(names(&listing), ["src", ".gitignore", "Alpha.rs", "b.rs", "README.md"]);
        assert_eq!(listing.entries[0].kind, EntryKind::Directory);
        assert_eq!(listing.path, "");

        let nested = list_directory(dir.path(), "src/", false).unwrap();
        assert_eq!(nested.path, "src");
        assert_eq!(names(&nested), ["nested", "lib.rs"]);
    }

    /// Asked for, the ignored entries come too, each marked; `.git` never does.
    #[test]
    fn ignored_entries_are_listed_and_marked_when_asked_for() {
        let dir = project();
        let listing = list_directory(dir.path(), "", true).unwrap();
        let marked: Vec<(&str, bool)> =
            listing.entries.iter().map(|entry| (entry.name.as_str(), entry.ignored)).collect();
        assert_eq!(
            marked,
            [
                ("src", false),
                ("target", true),
                (".gitignore", false),
                ("Alpha.rs", false),
                ("b.rs", false),
                ("build.log", true),
                ("README.md", false)
            ]
        );
    }

    #[test]
    fn nothing_outside_the_project_is_reachable() {
        let dir = project();
        for path in ["../", "../etc", "/etc", "src/../../x"] {
            assert_eq!(
                list_directory(dir.path(), path, false),
                Err(ProjectFileFailure::OutsideProject),
                "{path}"
            );
        }
        assert_eq!(read_file(dir.path(), "../x"), Err(ProjectFileFailure::OutsideProject));
        assert_eq!(read_file(dir.path(), "gone.rs"), Err(ProjectFileFailure::NotFound));
        assert_eq!(read_file(dir.path(), "src"), Err(ProjectFileFailure::WrongKind));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_project_is_neither_listed_nor_read() {
        let dir = project();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret"), "no").unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret"), dir.path().join("leak")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("src/lib.rs"), dir.path().join("inside"))
            .unwrap();

        let listing = list_directory(dir.path(), "", false).unwrap();
        assert!(!names(&listing).contains(&"leak"));
        assert!(names(&listing).contains(&"inside"), "a link that stays in is a file like any");
        assert_eq!(read_file(dir.path(), "leak"), Err(ProjectFileFailure::OutsideProject));
        assert_eq!(read_file(dir.path(), "inside").unwrap().text, "pub fn x() {}\n");
    }

    #[test]
    fn a_file_reads_as_text_or_says_it_is_not() {
        let dir = project();
        let readme = read_file(dir.path(), "README.md").unwrap();
        assert_eq!((readme.text.as_str(), readme.bytes, readme.binary), ("# hi\n", 5, false));

        std::fs::write(dir.path().join("blob.bin"), [0u8, 159, 146, 150]).unwrap();
        let blob = read_file(dir.path(), "blob.bin").unwrap();
        assert!(blob.binary && blob.text.is_empty());
    }

    #[test]
    fn a_large_file_is_cut_on_a_character_and_says_so() {
        let dir = project();
        let mut big = "é".repeat(MAX_FILE_BYTES / 2);
        big.push_str("tail");
        std::fs::write(dir.path().join("big.txt"), &big).unwrap();
        let file = read_file(dir.path(), "big.txt").unwrap();
        assert!(file.truncated && !file.binary);
        assert!(file.text.len() <= MAX_FILE_BYTES);
        assert!(file.text.chars().all(|c| c == 'é'));
    }
}
