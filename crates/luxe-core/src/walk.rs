//! Walk — the ignore-aware file walk every file-finding tool and the symbol index share.
//!
//! One walk so one question gets one answer: built from [`ignore`] and `globset`, the crates `fd`
//! and `rg` are made of, it honors the same ignore files and speaks the same glob dialect on every
//! platform, with no subprocess. `glob`, `grep` and `ast_grep` filter what it finds through read
//! policy before reading anything, which only an in-process walk can do, and the symbol index
//! reads the same tree the search tools do.

use globset::{Glob, GlobMatcher};
use ignore::{WalkBuilder, WalkState};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A hard ceiling on how many paths a single walk will hold.
///
/// The search tools cap their *output* far below this, so for them the limit is what keeps a walk
/// rooted at `/` on a mistyped path from filling memory first; for a [`listing`] it is the cap.
const MAX_WALKED_FILES: usize = 100_000;

/// Whether a search rooted at `search_path` stops reading ignore files at its own root.
///
/// True inside a fetched repository. `.luxe/.gitignore` is `*`, and with `--no-require-git` (which
/// the tools need, so a plain directory still hides its build output) fd, rg and this walk all
/// apply it to every file below, the clone's own `.git` notwithstanding: `web_fetch` pointed the
/// agent at a tree the same tools then said was empty. The clone's own ignore files still apply,
/// so its `node_modules/` stays out.
pub fn searches_own_root(search_path: &Path) -> bool {
    crate::fetched::contains(search_path)
}

/// Files under `root`, project-relative and sorted, honoring ignore files unless `all`.
///
/// `.git/` is always excluded — the tool descriptions promise it, and nobody has ever meant to glob
/// their own object store. Hidden files are *not* excluded: `.github/`, `.luxe/` and `.env.example`
/// are ordinary things to look for.
pub fn files(cwd: &Path, search_path: &Path, all: bool) -> Vec<PathBuf> {
    files_until(cwd, search_path, all, None, || false).files
}

/// What a stoppable walk found, and whether it gave up at `MAX_WALKED_FILES`.
pub struct Walked {
    pub files: Vec<PathBuf>,
    pub capped: bool,
}

/// The note a search adds when its walk hit the ceiling, so a partial answer says it is partial.
pub fn capped_notice() -> String {
    format!(
        "stopped after {MAX_WALKED_FILES} files, so later files were not searched; narrow `path`"
    )
}

/// Raised when a guarded search's future is dropped, so its blocking walk or scan stops early.
pub struct StopOnDrop(pub Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// [`files`], stoppable between entries, with an extra ignore-file name honored unless `all`.
pub fn files_until(
    cwd: &Path,
    search_path: &Path,
    all: bool,
    extra_ignore: Option<&str>,
    stopped: impl Fn() -> bool + Send + Sync + 'static,
) -> Walked {
    walk_within(cwd, search_path, (all, extra_ignore), stopped, MAX_WALKED_FILES)
}

/// Files under `root` that `keep` accepts, relative and sorted, at most `limit` of them, for an
/// index that must read the same files every time.
///
/// Sequential and in name order, so which files a capped walk kept is decided by their names, not
/// by thread timing. Never enters a hidden directory or one named in `skip_dirs`, ignore files or
/// not: the vendored and generated trees (`.yarn/releases`, `node_modules`) a project without a
/// `.gitignore` still has, and which would otherwise sort first and spend the budget.
pub fn index_files(
    root: &Path,
    skip_dirs: &'static [&'static str],
    keep: impl Fn(&Path) -> bool,
    limit: usize,
) -> Walked {
    let mut builder = configured(root, false, None, move |entry| {
        let is_dir = entry.depth() > 0 && entry.file_type().is_some_and(|kind| kind.is_dir());
        let name = entry.file_name().to_string_lossy();
        !(is_dir && (name.starts_with('.') || skip_dirs.contains(&name.as_ref())))
    });
    builder.sort_by_file_name(|a, b| a.cmp(b));
    let (mut files, mut capped) = (Vec::new(), false);
    for entry in builder.build().flatten() {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let relative = entry.path().strip_prefix(root).unwrap_or(entry.path());
        if !keep(relative) {
            continue;
        }
        if files.len() == limit {
            capped = true;
            break;
        }
        files.push(relative.to_path_buf());
    }
    files.sort();
    Walked { files, capped }
}

/// Every file under `root` a person picks from: ignore files honored, hidden files kept, never
/// entering a directory named in `skip_dirs`, relative and sorted, at most `MAX_WALKED_FILES`.
///
/// What the `@` picker's Files tab lists. The folders in `skip_dirs` are skipped whether ignored or
/// not, since a project without a `.gitignore` still has build output nobody means to reference.
pub fn listing(root: &Path, skip_dirs: &'static [&'static str]) -> Walked {
    listing_within(root, skip_dirs, MAX_WALKED_FILES)
}

fn listing_within(root: &Path, skip_dirs: &'static [&'static str], limit: usize) -> Walked {
    let mut builder = configured(root, false, None, move |entry| {
        entry.depth() == 0
            || entry.file_name().to_str().is_none_or(|name| !skip_dirs.contains(&name))
    });
    // In name order, so which files a capped listing keeps is decided by their names
    builder.sort_by_file_name(|a, b| a.cmp(b));
    let (mut files, mut capped) = (Vec::new(), false);
    for entry in builder.build().flatten() {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        if files.len() == limit {
            capped = true;
            break;
        }
        files.push(entry.path().strip_prefix(root).unwrap_or(entry.path()).to_path_buf());
    }
    files.sort();
    Walked { files, capped }
}

/// A walk of `root` with the rules every walk here shares, and `admit` deciding what else it
/// enters.
fn configured(
    root: &Path,
    all: bool,
    extra_ignore: Option<&str>,
    admit: impl Fn(&ignore::DirEntry) -> bool + Send + Sync + 'static,
) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .parents(!all && !searches_own_root(root))
        .ignore(!all)
        .git_ignore(!all)
        .git_global(!all)
        .git_exclude(!all)
        // Without this, ignore files apply only inside a git repository, so a plain project
        // directory would hand back its own build output. (`fd --no-require-git`, same reason.)
        .require_git(false)
        .filter_entry(move |entry| entry.file_name() != ".git" && admit(entry));
    if let Some(ignore_file) = extra_ignore.filter(|_| !all) {
        builder.add_custom_ignore_filename(ignore_file);
    }
    builder
}

fn walk_within(
    cwd: &Path,
    search_path: &Path,
    (all, extra_ignore): (bool, Option<&str>),
    stopped: impl Fn() -> bool + Send + Sync + 'static,
    limit: usize,
) -> Walked {
    let stopped = Arc::new(stopped);
    let stop_walk = stopped.clone();
    let root =
        if search_path.is_absolute() { search_path.to_path_buf() } else { cwd.join(search_path) };
    let mut builder = configured(&root, all, extra_ignore, move |_| !stop_walk());
    // Walked on every core, as `rg` and `fd` walk: on a large tree the walk is most of a search.
    // Sorted afterwards, so a complete answer does not depend on which thread got where first; a
    // capped one does, which is why the index walks with `index_files` instead.
    let found = std::sync::Mutex::new(Vec::new());
    let capped = AtomicBool::new(false);
    builder.threads(walk_threads()).build_parallel().run(|| {
        Box::new(|entry| {
            let Ok(entry) = entry else { return WalkState::Continue };
            if stopped() || capped.load(Ordering::Relaxed) {
                return WalkState::Quit;
            }
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                return WalkState::Continue;
            }
            let path = entry.path();
            // Relative to the *working directory*, not the search root: an absolute prefix on
            // every row is a tax the model pays to read and pays again to strip.
            let relative = path.strip_prefix(cwd).unwrap_or(path).to_path_buf();
            let Ok(mut found) = found.lock() else { return WalkState::Quit };
            if found.len() >= limit {
                capped.store(true, Ordering::Relaxed);
                return WalkState::Quit;
            }
            found.push(relative);
            WalkState::Continue
        })
    });
    let mut found = found.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
    found.sort();
    Walked { files: found, capped: capped.into_inner() }
}

/// Threads for one walk: every core, within reason.
fn walk_threads() -> usize {
    std::thread::available_parallelism().map_or(1, usize::from).min(12)
}

/// Compile a glob the way `fd --glob` reads it, matching against whichever part of the path applies.
///
/// A pattern with no `/` describes a *file name* (`*.rs`); one with a `/` describes a path
/// (`src/**/*.ts`) and is anchored so it matches at any depth. `**` crosses directory boundaries and
/// `*` does not, which is the dialect every tool description in this repo promises.
pub fn matcher(pattern: &str) -> Result<PathMatcher, globset::Error> {
    let spans_directories = pattern.contains('/');
    let compiled =
        if spans_directories { Glob::new(&anchor(pattern))? } else { Glob::new(pattern)? };
    Ok(PathMatcher {
        matcher: compiled.compile_matcher(),
        // Compiled with `literal_separator` off in both cases, because `*.rs` has to match a bare
        // file name and `**/src/*.rs` needs `**` to cross separators.
        against_name_only: !spans_directories,
    })
}

/// A compiled glob plus what it should be matched against.
pub struct PathMatcher {
    matcher: GlobMatcher,
    against_name_only: bool,
}

impl PathMatcher {
    /// Whether `path` (project-relative) matches.
    pub fn is_match(&self, path: &Path) -> bool {
        if self.against_name_only {
            path.file_name().is_some_and(|name| self.matcher.is_match(Path::new(name)))
        } else {
            // Globs are written with `/` on every platform, and on Windows a path is rendered with
            // `\` — so the comparison happens in the glob's own alphabet.
            self.matcher.is_match(crate::paths::display(path))
        }
    }
}

/// Anchor a directory-spanning pattern so it matches at any depth: `src/**/*.ts` → `**/src/**/*.ts`.
///
/// Already-anchored and absolute patterns are left alone.
fn anchor(pattern: &str) -> String {
    if pattern.starts_with("**/") || pattern.starts_with('/') {
        pattern.to_string()
    } else {
        format!("**/{pattern}")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// A walk that stops at its ceiling must say so, or a search over a huge tree reads as complete.
    #[test]
    fn a_walk_that_hits_its_ceiling_says_so() {
        let dir = repo();
        let walk = |limit| walk_within(dir.path(), Path::new("."), (true, None), || false, limit);
        let walked = walk(2);
        assert_eq!(walked.files.len(), 2);
        assert!(walked.capped);
        let whole = walk(100);
        assert!(!whole.capped, "{:?}", whole.files);
    }

    /// An index walk that stops at its limit keeps the same files every time: the first by name.
    #[test]
    fn a_capped_index_walk_keeps_the_first_files_by_name() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..40 {
            let sub = dir.path().join(format!("d{}", index % 4));
            std::fs::create_dir_all(&sub).unwrap();
            std::fs::write(sub.join(format!("f{index:02}.rs")), "").unwrap();
        }
        std::fs::create_dir_all(dir.path().join(".hidden")).unwrap();
        std::fs::write(dir.path().join(".hidden/a.rs"), "").unwrap();
        let walk = || index_files(dir.path(), &[], |path| path.extension().is_some(), 10);
        let first = walk();
        assert!(first.capped);
        assert_eq!(first.files.len(), 10);
        assert!(first.files.iter().all(|path| path.starts_with("d0") || path.starts_with("d1")));
        assert_eq!(first.files, walk().files, "the same ten each time");
        assert!(!first.files.iter().any(|path| path.starts_with(".hidden")));
    }

    /// A listing keeps hidden files, honors the ignore file, and never enters a skipped folder,
    /// ignored or not.
    #[test]
    fn a_listing_keeps_dotfiles_and_skips_the_named_folders() {
        let dir = repo();
        std::fs::create_dir_all(dir.path().join(".github")).unwrap();
        std::fs::write(dir.path().join(".github/ci.yml"), "").unwrap();
        std::fs::create_dir_all(dir.path().join("vendor")).unwrap();
        std::fs::write(dir.path().join("vendor/lib.rs"), "").unwrap();
        let listed = listing(dir.path(), &["vendor"]);
        assert!(!listed.capped);
        let dotfile = Path::new(".github").join("ci.yml");
        assert!(listed.files.contains(&dotfile), "{:?}", listed.files);
        assert!(listed.files.iter().all(|path| !path.starts_with("vendor")));
        assert!(listed.files.iter().all(|path| !path.starts_with("target")));
    }

    /// A listing that stops at its limit keeps the same files every time: the first by name.
    #[test]
    fn a_capped_listing_keeps_the_first_files_by_name() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..40 {
            let sub = dir.path().join(format!("d{}", index % 4));
            std::fs::create_dir_all(&sub).unwrap();
            std::fs::write(sub.join(format!("f{index:02}.rs")), "").unwrap();
        }
        let first = listing_within(dir.path(), &[], 10);
        assert!(first.capped);
        assert_eq!(first.files.len(), 10);
        assert!(first.files.iter().all(|path| path.starts_with("d0")), "{:?}", first.files);
        assert_eq!(first.files, listing_within(dir.path(), &[], 10).files);
    }

    /// A repo with one real source file and a build directory full of look-alikes.
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        std::fs::create_dir_all(dir.path().join("src").join("deep")).unwrap();
        std::fs::write(dir.path().join("src").join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.path().join("src").join("deep").join("nested.rs"), "").unwrap();
        std::fs::create_dir_all(dir.path().join("target").join("debug")).unwrap();
        std::fs::write(dir.path().join("target").join("debug").join("gen.rs"), "").unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git").join("config"), "").unwrap();
        dir
    }

    fn names(paths: &[PathBuf]) -> Vec<String> {
        paths.iter().map(|path| crate::paths::display(path)).collect()
    }

    #[test]
    fn the_walk_honors_ignore_files_and_never_enters_dot_git() {
        let dir = repo();
        let found = names(&files(dir.path(), Path::new("."), false));
        assert!(found.contains(&"src/main.rs".to_string()), "{found:?}");
        assert!(found.contains(&"src/deep/nested.rs".to_string()), "{found:?}");
        assert!(!found.iter().any(|path| path.contains("target/")), "ignored: {found:?}");
        assert!(
            !found.iter().any(|path| path.contains(".git/")),
            "never the object store: {found:?}"
        );
    }

    #[test]
    fn all_reaches_ignored_files_but_still_not_dot_git() {
        let dir = repo();
        let found = names(&files(dir.path(), Path::new("."), true));
        assert!(found.contains(&"target/debug/gen.rs".to_string()), "{found:?}");
        assert!(!found.iter().any(|path| path.contains(".git/")), "{found:?}");
    }

    /// The bug the `find` fallback shipped with: `**/` means *zero or more* directories, so
    /// `src/**/*.rs` has to match `src/main.rs` — a file sitting directly under the globbed
    /// directory, which is the common case.
    #[test]
    fn a_directory_spanning_glob_matches_zero_intermediate_directories() {
        let glob = matcher("src/**/*.rs").unwrap();
        assert!(glob.is_match(Path::new("src/main.rs")), "zero directories in between");
        assert!(glob.is_match(Path::new("src/deep/nested.rs")), "and any number of them");
        assert!(glob.is_match(Path::new("crates/x/src/main.rs")), "anchored to match at any depth");
        assert!(!glob.is_match(Path::new("tests/main.rs")));
    }

    #[test]
    fn a_bare_pattern_matches_the_file_name_at_any_depth() {
        let glob = matcher("*.rs").unwrap();
        assert!(glob.is_match(Path::new("main.rs")));
        assert!(glob.is_match(Path::new("src/deep/nested.rs")));
        assert!(!glob.is_match(Path::new("src/main.py")));
    }

    #[test]
    fn a_malformed_glob_is_an_error_rather_than_a_silent_no_match() {
        assert!(matcher("src/[").is_err());
    }
}
