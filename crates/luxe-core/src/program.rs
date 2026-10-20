//! Program resolution — find an external tool that is installed but not on `PATH`.
//!
//! luxe drives a lot of external programs: language servers, debug adapters, formatters, `git`,
//! `ast-grep`. Resolving them through `PATH` alone reports "not installed" for a tool that is
//! sitting on disk, because a package manager's bin directory and the launching process's `PATH`
//! are different things. `cargo install` writes to `~/.cargo/bin` whether or not that directory was
//! ever added to a shell profile; `pipx`, `npm -g`, `go install`, and Homebrew each have their own.
//! An app also inherits whatever environment launched it, which for a desktop entry, a systemd
//! unit, or an editor's integrated terminal is often not the user's login shell.
//!
//! So: try `PATH` first (it is the user's stated preference), then the well-known install
//! directories. A tool that is genuinely absent still reports absent, with its install hint intact.

use std::path::{Path, PathBuf};

mod bounded;
mod nesting;
mod opener;
mod process;

pub use bounded::{run_bounded, run_bounded_with_input, BoundedRunError};
pub use nesting::{
    check_nesting, claim_nesting_slot, NestingRefused, DEFAULT_MAX_NESTING_DEPTH,
    MAX_NESTING_DEPTH_VAR, NESTING_DEPTH_VAR,
};
pub use opener::{
    desktop_opener, editor, editor_file_args, file_manager, file_manager_from, parse_editor,
    EditorError,
};
pub use process::{
    captured_command, detach_from_terminal, group_members, hostname, kill_tree, process_is_alive,
    take_stdin, GroupMember, TreeGuard,
};

/// Directories a package manager drops executables into, checked after `PATH`.
///
/// Ordered by how likely the entry is to be the one a developer meant: language toolchains first,
/// then the generic user bin directory, then system-wide installs.
fn well_known_bin_dirs() -> Vec<PathBuf> {
    let home = crate::user_dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let mut dirs = vec![
        home.join(".cargo").join("bin"),
        home.join(".local").join("bin"),
        home.join("go").join("bin"),
        home.join(".bun").join("bin"),
        home.join(".deno").join("bin"),
        home.join(".npm-global").join("bin"),
    ];
    if cfg!(windows) {
        dirs.extend(windows_bin_dirs());
    } else {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
    }
    dirs
}

/// Where Windows keeps installed programs that its installers did not put on `PATH`.
///
/// Scoop and winget shim their own directories onto `PATH`, so the interesting entries are Git for
/// Windows and Chocolatey. Git matters most: its default install adds only `Git\cmd` (which holds
/// `git.exe` and little else), so the unix tools a coding agent leans on sit on disk, one directory
/// away, invisible to a `PATH` lookup.
fn windows_bin_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let roots = ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .chain(std::env::var_os("LOCALAPPDATA").map(|local| PathBuf::from(local).join("Programs")));
    for root in roots {
        dirs.push(root.join("Git").join("bin"));
        dirs.push(root.join("Git").join("usr").join("bin"));
        dirs.push(root.join("Git").join("cmd"));
    }
    if let Some(data) = std::env::var_os("ProgramData") {
        dirs.push(PathBuf::from(data).join("chocolatey").join("bin"));
    }
    dirs
}

/// Whether `path` is a file we could execute.
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Resolve `program` to a runnable path: `PATH` first, then the well-known install directories.
///
/// An absolute or relative path (anything containing a separator) is honored as given. Returns
/// `None` only when the program really cannot be found.
pub fn resolve(program: &str) -> Option<PathBuf> {
    let candidate = Path::new(program);
    // Windows accepts both separators, so checking only `MAIN_SEPARATOR` would treat `tools/rg` as a
    // bare program name and go looking for it on `PATH`.
    if candidate.is_absolute() || program.contains('/') || program.contains('\\') {
        return runnable(candidate).map(|_| candidate.to_path_buf());
    }

    let dirs = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    dirs.into_iter().chain(well_known_bin_dirs()).find_map(|dir| runnable(&dir.join(program)))
}

/// The runnable file at `base`, trying the platform's executable suffixes.
fn runnable(base: &Path) -> Option<PathBuf> {
    runnable_as(base, cfg!(windows), &executable_suffixes(), is_executable)
}

/// The runnable file at `base` when `windows` decides by extension, and `runs` says whether a
/// file exists to be run.
///
/// On unix a program is its own filename. On Windows the name on disk carries an extension from
/// `PATHEXT` — `rg` is `rg.exe`, a shim may be `rg.cmd` — and only those run. A bare name is taken
/// as given only when it already carries one: npm installs an extensionless shell script beside
/// every `.cmd` shim, and taking that file fails with "not a valid Win32 application".
fn runnable_as(
    base: &Path,
    windows: bool,
    suffixes: &[String],
    runs: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if !windows {
        return runs(base).then(|| base.to_path_buf());
    }
    if has_suffix(base, suffixes) && runs(base) {
        return Some(base.to_path_buf());
    }
    suffixes.iter().find_map(|suffix| {
        let mut name = base.as_os_str().to_os_string();
        name.push(suffix);
        let candidate = PathBuf::from(name);
        runs(&candidate).then_some(candidate)
    })
}

/// Whether `path` already ends in one of `suffixes`, `.CMD` matching `.cmd`.
fn has_suffix(path: &Path, suffixes: &[String]) -> bool {
    let extension = path.extension().and_then(|extension| extension.to_str());
    extension.is_some_and(|extension| {
        suffixes.iter().any(|suffix| suffix.trim_start_matches('.').eq_ignore_ascii_case(extension))
    })
}

/// The Python 3 that runs here, as the words that start it: `["py", "-3"]`, `["python3"]`.
///
/// On Windows `python3` and `python` can be the Microsoft Store's stand-in, a file that only prints
/// "Python was not found", so there each candidate is asked for its version before it counts, the
/// `py` launcher first; elsewhere finding the program is enough. Cached, since the answer holds for
/// the life of the process and asking costs a process start.
pub fn python() -> Option<Vec<String>> {
    static FOUND: std::sync::OnceLock<Option<Vec<String>>> = std::sync::OnceLock::new();
    FOUND
        .get_or_init(|| {
            python_candidates(cfg!(windows)).into_iter().find(|argv| match cfg!(windows) {
                true => answers_as_python3(argv),
                false => resolve(&argv[0]).is_some(),
            })
        })
        .clone()
}

/// The ways to start Python 3, best first: Windows' `py` launcher is installed with Python itself,
/// never as a stand-in, while elsewhere `python` can still be Python 2.
fn python_candidates(windows: bool) -> Vec<Vec<String>> {
    let argv = |words: &[&str]| words.iter().map(|word| word.to_string()).collect::<Vec<_>>();
    match windows {
        true => vec![argv(&["py", "-3"]), argv(&["python"]), argv(&["python3"])],
        false => vec![argv(&["python3"]), argv(&["python"])],
    }
}

/// Whether `argv` starts a working Python 3, by asking it, for at most two seconds.
fn answers_as_python3(argv: &[String]) -> bool {
    use std::process::{Command, Stdio};
    let Some(program) = resolve(&argv[0]) else { return false };
    let child = Command::new(program)
        .args(&argv[1..])
        .args(["-c", "import sys; print(sys.version_info[0])"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    let Ok(child) = child else { return false };
    wait_briefly(child, std::time::Duration::from_secs(2)).is_some_and(|out| {
        out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "3"
    })
}

/// `child`'s output once it exits, or `None` when it has not within `limit`, killed then.
fn wait_briefly(
    mut child: std::process::Child,
    limit: std::time::Duration,
) -> Option<std::process::Output> {
    let deadline = std::time::Instant::now() + limit;
    while child.try_wait().ok()?.is_none() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    child.wait_with_output().ok()
}

/// Windows executable suffixes, from `PATHEXT` when it is set.
fn executable_suffixes() -> Vec<String> {
    std::env::var("PATHEXT")
        .ok()
        .map(|list| list.split(';').filter(|ext| !ext.is_empty()).map(str::to_string).collect())
        .unwrap_or_else(|| {
            [".COM", ".EXE", ".BAT", ".CMD"].iter().map(|ext| (*ext).to_string()).collect()
        })
}

/// The shell a script-running tool should go through, as `(program, flag)`.
///
/// The `bash` tool hands whole command lines to a shell, and the model writes those in POSIX: pipes,
/// `&&`, globs, quoting. So a POSIX shell is preferred wherever one exists — on Windows that means
/// the one Git for Windows ships. `cmd` is the last resort rather than the default, because a command
/// written for `sh` mostly fails in it, and failing *differently* is worse than failing clearly.
///
/// Returns `None` for the shell only if the platform has nothing at all, which cannot happen: unix
/// has `sh` and Windows has `cmd`.
pub fn shell() -> (PathBuf, &'static str) {
    if !cfg!(windows) {
        return (resolve("sh").unwrap_or_else(|| PathBuf::from("sh")), "-c");
    }
    match posix_shell_on_windows(resolve, || std::env::var("SystemRoot").ok()) {
        Some(path) => (path, "-c"),
        None => (resolve("cmd").unwrap_or_else(|| PathBuf::from("cmd")), "/C"),
    }
}

/// Whether [`shell`] found a real POSIX shell, or fell back to `cmd`.
///
/// The `bash` tool uses this to explain a failure that is really "this box has no shell that speaks
/// your command" rather than a problem with the command.
pub fn has_posix_shell() -> bool {
    shell().1 == "-c"
}

/// Find a POSIX shell on Windows, refusing the one that is not what it says.
///
/// `C:\Windows\System32\bash.exe` is the WSL launcher. It is on `PATH` before anything else the
/// moment WSL is installed, and running an agent's commands through it would execute them inside a
/// Linux VM with a different filesystem — `cd` into the project would fail, and every subsequent
/// error would describe a machine the user is not looking at. So a candidate sitting in the Windows
/// system directory is skipped rather than taken.
///
/// Args: `(program lookup, `%SystemRoot%` lookup)` — both injected so the rule is testable from any
/// platform.
fn posix_shell_on_windows(
    lookup: impl Fn(&str) -> Option<PathBuf>,
    system_root: impl Fn() -> Option<String>,
) -> Option<PathBuf> {
    let system = system_root()
        .map(|root| PathBuf::from(root).join("System32"))
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows\System32"));
    ["bash", "sh"]
        .iter()
        .filter_map(|name| lookup(name))
        .find(|path| path.parent().is_none_or(|parent| !crate::paths::contains(&system, parent)))
}

/// Whether `program` can be run at all — `PATH` or a well-known install directory.
pub fn is_available(program: &str) -> bool {
    resolve(program).is_some()
}

/// The first of `programs` that resolves, as `(requested name, runnable path)`.
///
/// For a tool that ships under more than one name (`fd`/`fdfind`, `ast-grep`/`sg`).
pub fn resolve_any<'a>(programs: &[&'a str]) -> Option<(&'a str, PathBuf)> {
    programs.iter().find_map(|name| resolve(name).map(|path| (*name, path)))
}

/// The copy of `program` a project under `root` supplies for itself, if any.
///
/// Two conventions, one question. npm links a dev dependency's executable into
/// `node_modules/.bin`, and Python installs everything into a virtualenv — in both cases the
/// version a repository pins is invisible to [`resolve`], which answers with whatever global copy
/// shares the name. The environment is checked first because it belongs to `root` itself, then
/// `node_modules/.bin` walking upward, because a workspace hoists: a package in a monorepo has its
/// own or shares the root's, and which one depends on the package manager rather than on anything
/// the caller can see.
///
/// **This returns a path to a binary the repository supplied**, so whether to run it is the
/// caller's decision and not this module's. Home is skipped on the way up: a stray `~/node_modules`
/// belongs to no project and would otherwise answer for every one of them.
pub fn project_local(root: &Path, program: &str) -> Option<PathBuf> {
    // A name with a separator is a path, not something to go looking for.
    if program.contains('/') || program.contains('\\') {
        return None;
    }
    if let Some(installed) = project_venv(root).and_then(|venv| venv_program(&venv, program)) {
        return Some(installed);
    }
    let home = crate::user_dirs::home_dir();
    std::iter::successors(Some(root), |dir| dir.parent())
        .filter(|dir| home.as_deref() != Some(*dir))
        .find_map(|dir| runnable(&dir.join("node_modules").join(".bin").join(program)))
}

/// Directory names a Python virtualenv conventionally lives under, most idiomatic first.
const VENV_DIR_NAMES: &[&str] = &[".venv", "venv", "env"];

/// The copy of `program` installed into the virtualenv at `venv`, if both exist.
///
/// A virtualenv's executables are the ones a Python project actually pinned, and they are never on
/// `PATH` unless somebody activated it — so [`resolve`] answers with a global interpreter that
/// shares nothing with the project but a version number. The layout differs by platform (`bin` on
/// unix, `Scripts` on Windows) and that is the whole reason this lives here: a call site that
/// hardcodes `bin/python` works everywhere except Windows, silently.
pub fn venv_program(venv: &Path, program: &str) -> Option<PathBuf> {
    if program.contains('/') || program.contains('\\') {
        return None;
    }
    let bin = if cfg!(windows) { "Scripts" } else { "bin" };
    runnable(&venv.join(bin).join(program))
}

/// The virtualenv a project under `root` carries, if any.
///
/// An activated `VIRTUAL_ENV` inside the tree wins, because it is the user's own answer and it may
/// be named something no list would guess (`.direnv/python-3.12`). One *outside* the tree is
/// ignored on purpose: it belongs to another project, and if it should win it is already on `PATH`.
///
/// Only `root` itself is searched. Walking upward would let a parent directory's environment answer
/// for a project that has none, which is the failure this exists to prevent.
pub fn project_venv(root: &Path) -> Option<PathBuf> {
    let active = std::env::var_os("VIRTUAL_ENV").map(PathBuf::from).filter(|venv| {
        crate::paths::contains(root, venv) && venv_program(venv, "python").is_some()
    });
    active.or_else(|| {
        VENV_DIR_NAMES
            .iter()
            .map(|name| root.join(name))
            .find(|venv| venv_program(venv, "python").is_some())
    })
}

/// Resolve `program` for work rooted at `root`, preferring what the project installed for itself.
///
/// `allow_project_local` is the whole gate, and it is the caller's answer rather than this
/// module's: a binary under `root` got there from the repository, so who is watching and what the
/// user configured decide it, and neither is knowable here. Denied, this is exactly [`resolve`].
pub fn resolve_in(root: &Path, program: &str, allow_project_local: bool) -> Option<PathBuf> {
    if allow_project_local {
        if let Some(local) = project_local(root, program) {
            return Some(local);
        }
    }
    resolve(program)
}

#[cfg(test)]
pub(crate) mod tests;
