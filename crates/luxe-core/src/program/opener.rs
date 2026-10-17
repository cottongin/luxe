//! Opener — the editor, file manager or desktop handler a file is handed to.

use super::{is_available, resolve};
use std::path::{Path, PathBuf};

/// The program that hands a file to whatever the desktop opens that kind of file with, as
/// `(program, leading arguments)` — the caller appends the path.
///
/// Returns `None` on a machine with no desktop at all, which is a real answer: a CI runner and an
/// ssh session into a headless box both have nowhere to open anything, and a caller that assumed
/// otherwise would report a failure the user cannot act on.
///
/// `xdg-open` first on unix because it is the freedesktop contract every distribution implements;
/// `gio open` is the GNOME path for a box that never installed xdg-utils, and `wslview` is how a
/// Linux process reaches the Windows shell it is really running under.
pub fn desktop_opener() -> Option<(PathBuf, &'static [&'static str])> {
    if cfg!(target_os = "macos") {
        return resolve("open").map(|path| (path, &[] as &[&str]));
    }
    if cfg!(windows) {
        // `start` is a `cmd` builtin rather than a program, and its first quoted argument is the
        // window *title* — omitting the empty one makes `start` treat the path as a title and open
        // nothing at all.
        return resolve("cmd").map(|path| (path, &["/C", "start", ""] as &[&str]));
    }
    for (program, leading) in [("xdg-open", &[] as &[&str]), ("wslview", &[]), ("gio", &["open"])] {
        if let Some(path) = resolve(program) {
            return Some((path, leading));
        }
    }
    None
}

/// The reader's editor, as `(program, leading arguments)` — the caller appends the file.
///
/// `LUXE_EDITOR`, then `VISUAL`, then `EDITOR`, each a command line that may carry arguments
/// (`code --wait`); failing all three, the first of `nvim`, `vim` and `vi` that exists, or
/// `notepad` on Windows.
pub fn editor() -> Result<(String, Vec<String>), EditorError> {
    for variable in ["LUXE_EDITOR", "VISUAL", "EDITOR"] {
        if let Ok(value) = std::env::var(variable) {
            if !value.trim().is_empty() {
                return parse_editor(&value).ok_or(EditorError::Unparsable { variable });
            }
        }
    }
    let fallbacks: &[&str] = if cfg!(windows) { &["notepad"] } else { &["nvim", "vim", "vi"] };
    fallbacks
        .iter()
        .find(|program| is_available(program))
        .map(|program| ((*program).to_string(), Vec::new()))
        .ok_or(EditorError::NotFound)
}

/// An editor command line split into its program and arguments, or `None` for an empty one or
/// one with unbalanced quotes.
pub fn parse_editor(value: &str) -> Option<(String, Vec<String>)> {
    let mut parts = shlex::split(value)?;
    if parts.is_empty() {
        return None;
    }
    let program = parts.remove(0);
    Some((program, parts))
}

/// The arguments that open `path` in `program`, at `line` where the editor has a way to say so.
///
/// Editors disagree: the vi family, nano, emacs and kakoune take `+N` before the file; helix, zed,
/// sublime and micro read `file:N`; VS Code and its forks want `--goto file:N`. An editor not on
/// the list gets the file alone, which opens it at the top rather than handing it an argument it
/// may read as a second file. Matched on the program's own name, so `/usr/bin/nvim` and `nvim.exe`
/// count.
///
/// Args: `(program, path, line)`.
pub fn editor_file_args(program: &str, path: &Path, line: Option<u32>) -> Vec<std::ffi::OsString> {
    let file = path.as_os_str().to_owned();
    let Some(line) = line.filter(|line| *line > 0) else {
        return vec![file];
    };
    let name = Path::new(program)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let at_line = || {
        let mut joined = file.clone();
        joined.push(format!(":{line}"));
        joined
    };
    match name.as_str() {
        "vi" | "vim" | "nvim" | "gvim" | "mvim" | "nano" | "emacs" | "emacsclient" | "kak"
        | "joe" | "mg" | "ne" => vec![format!("+{line}").into(), file],
        "hx" | "helix" | "zed" | "zeditor" | "subl" | "sublime_text" | "micro" => vec![at_line()],
        "code" | "code-insiders" | "codium" | "cursor" | "windsurf" | "positron" => {
            vec!["--goto".into(), at_line()]
        }
        _ => vec![file],
    }
}

/// The terminal file managers tried, in order, when no variable names one.
const FILE_MANAGERS: &[&str] = &["yazi", "lf", "ranger", "nnn"];

/// The reader's terminal file manager, as `(program, leading arguments)` — the caller appends the
/// folder.
///
/// `LUXE_FILE_MANAGER`, then `FILE_MANAGER`, each a command line that may carry arguments; failing
/// both, the first of yazi, lf, ranger and nnn that is installed. `None` means there is no terminal
/// one to hand the screen to, and the desktop's own file manager is the way left
/// ([`desktop_opener`]). A variable that does not parse counts as unset rather than an error, since
/// the desktop is still a fine answer.
pub fn file_manager() -> Option<(String, Vec<String>)> {
    file_manager_from(|variable| std::env::var(variable).ok(), is_available)
}

/// [`file_manager`] over a given environment and `PATH`, so it can be tested without either.
///
/// Args: `(variable lookup, whether a program is installed)`.
pub fn file_manager_from(
    variable: impl Fn(&str) -> Option<String>,
    installed: impl Fn(&str) -> bool,
) -> Option<(String, Vec<String>)> {
    ["LUXE_FILE_MANAGER", "FILE_MANAGER"]
        .into_iter()
        .filter_map(&variable)
        .find_map(|value| parse_editor(&value))
        .or_else(|| {
            FILE_MANAGERS
                .iter()
                .find(|program| installed(program))
                .map(|program| ((*program).to_string(), Vec::new()))
        })
}

/// Why no editor could be found.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EditorError {
    /// A variable named one, and it could not be split into a command
    #[error("${variable} is empty or has unbalanced quotes")]
    Unparsable {
        /// The variable that named it
        variable: &'static str,
    },
    /// Nothing named one, and none of the usual ones is installed
    #[error("no editor found; set LUXE_EDITOR, VISUAL, or EDITOR")]
    NotFound,
}

#[cfg(test)]
mod tests;
