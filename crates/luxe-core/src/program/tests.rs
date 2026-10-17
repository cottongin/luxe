use super::*;

/// Whether `pid` is still running, an exit nobody has reaped yet counting as gone: a test waiting
/// for a kill to land must not be held up by a zombie its parent never collected.
#[cfg(unix)]
pub(crate) fn still_running(pid: u32) -> bool {
    let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit(')').next().is_some_and(|rest| rest.trim_start().starts_with('Z'))
    });
    process_is_alive(pid) && !zombie
}

/// Write a runnable fixture called `name` into `dir`, and return the file that was created.
///
/// Not the same file on both platforms, on purpose. npm's Windows shim for a dev dependency is
/// `name.cmd`, and a bare `name` is not runnable there at all — which is precisely the case
/// [`runnable`] exists to cover, so a fixture that wrote one file everywhere would test the
/// suffix walk on neither.
fn write_executable(dir: &Path, name: &str) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write");
        let mut perms = std::fs::metadata(&path).expect("metadata").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).expect("chmod");
        path
    }
    #[cfg(not(unix))]
    {
        let path = dir.join(format!("{name}.cmd"));
        std::fs::write(&path, "@echo off\r\nexit /b 0\r\n").expect("write");
        path
    }
}

/// Whether a resolved path is the fixture `name` sitting in `dir`.
///
/// Compared by directory and stem rather than by equality: on Windows the suffix comes back
/// from `PATHEXT` in its own case (`.CMD`), and the fixture wrote `.cmd`.
fn is_fixture(resolved: &Path, dir: &Path, name: &str) -> bool {
    resolved.parent() == Some(dir)
        && resolved.file_stem().is_some_and(|stem| stem.eq_ignore_ascii_case(name))
}

/// A program every platform has, so the resolver is exercised rather than the fixture.
fn certainly_installed() -> &'static str {
    if cfg!(windows) {
        "cmd"
    } else {
        "sh"
    }
}

#[test]
fn a_program_on_path_resolves() {
    let known = certainly_installed();
    assert!(is_available(known), "{known} is on PATH everywhere this runs");
    assert!(resolve(known).is_some_and(|path| path.is_absolute()));
}

/// The shell a script tool goes through has to be a real, resolved program — and on unix it takes
/// `-c`, which is what every call site passes the script with.
#[test]
fn the_shell_resolves_to_something_runnable() {
    let (program, flag) = shell();
    assert!(program.is_absolute() || program.file_name().is_some(), "{program:?}");
    assert!(matches!(flag, "-c" | "/C"), "unexpected shell flag: {flag}");
    if !cfg!(windows) {
        assert_eq!(flag, "-c", "there is always a posix shell on unix");
    }
    assert!(is_available(&program.to_string_lossy()), "the shell must resolve: {program:?}");
}

#[test]
fn a_program_that_does_not_exist_stays_unresolved() {
    assert!(resolve("luxe-definitely-not-a-real-program").is_none());
}

/// The case that started this: `cargo install` writes to `~/.cargo/bin`, which is frequently
/// not on `PATH`, and the tool was reported missing while sitting on disk.
#[cfg(unix)]
#[test]
fn a_program_installed_off_path_is_still_found() {
    static HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());

    let home = tempfile::tempdir().expect("temp home");
    let cargo_bin = home.path().join(".cargo").join("bin");
    std::fs::create_dir_all(&cargo_bin).expect("cargo bin");
    write_executable(&cargo_bin, "luxe-test-tool");

    let previous = std::env::var("HOME").ok();
    std::env::set_var("HOME", home.path());
    let found = resolve("luxe-test-tool");
    match previous {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }

    assert_eq!(found.as_deref(), Some(cargo_bin.join("luxe-test-tool").as_path()));
}

#[cfg(unix)]
#[test]
fn a_path_with_a_separator_is_taken_as_given() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = write_executable(dir.path(), "tool");
    assert_eq!(resolve(path.to_str().expect("utf8")).as_deref(), Some(path.as_path()));
    assert!(resolve(dir.path().join("missing").to_str().expect("utf8")).is_none());
}

#[cfg(unix)]
#[test]
fn a_non_executable_file_is_not_a_program() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("data.txt");
    std::fs::write(&path, "not a program").expect("write");
    assert!(resolve(path.to_str().expect("utf8")).is_none());
}

#[test]
fn resolve_any_reports_which_name_won() {
    let known = certainly_installed();
    let (name, path) =
        resolve_any(&["luxe-definitely-not-real", known]).expect("the known program resolves");
    assert_eq!(name, known);
    assert!(path.is_absolute());
    assert!(resolve_any(&["luxe-definitely-not-real"]).is_none());
}

/// A Windows layout spelled in the host's own path syntax, so the *rule* is exercised
/// everywhere. `Path::components` only splits on the separator the host uses, so a literal
/// `C:\...` would collapse to a single component on unix and test nothing.
fn windows_layout() -> (PathBuf, PathBuf, PathBuf) {
    let root = if cfg!(windows) { r"C:\Windows" } else { "/Windows" };
    let git = if cfg!(windows) { r"C:\Program Files\Git\bin" } else { "/Program Files/Git/bin" };
    let root = PathBuf::from(root);
    let wsl = root.join("System32").join("bash.exe");
    (root, wsl, PathBuf::from(git).join("bash.exe"))
}

/// The trap that would have shipped: with WSL installed, `C:\Windows\System32\bash.exe` is on
/// `PATH` before anything Git for Windows put there — and it is not a shell, it is a launcher
/// that runs the command inside a Linux VM with a different filesystem. Every path the agent
/// then used would be wrong, and every error would describe a machine the user cannot see.
#[test]
fn the_wsl_launcher_is_never_mistaken_for_a_posix_shell() {
    let (root, wsl, git) = windows_layout();
    let system_root = || Some(root.to_string_lossy().into_owned());

    let wsl_only = |name: &str| (name == "bash").then(|| wsl.clone());
    assert_eq!(posix_shell_on_windows(wsl_only, system_root), None, "falls through to cmd");

    // ...and with a real one also present, that is the one taken.
    let both = |name: &str| match name {
        "bash" => Some(wsl.clone()),
        "sh" => Some(git.clone()),
        _ => None,
    };
    assert_eq!(posix_shell_on_windows(both, system_root), Some(git));
}

/// Git for Windows' default install puts only `Git\cmd` on `PATH`, which holds `git.exe` and
/// essentially nothing else — so the shell that *is* installed has to be found by looking.
#[test]
fn a_shell_installed_off_path_still_counts() {
    let (root, _, git) = windows_layout();
    let off_path = |name: &str| (name == "bash").then(|| git.clone());
    assert_eq!(
        posix_shell_on_windows(off_path, || Some(root.to_string_lossy().into_owned())),
        Some(git)
    );
}

/// The shell answer and the "is there one" answer must not be able to disagree.
#[test]
fn the_posix_shell_report_matches_the_shell_that_was_chosen() {
    assert_eq!(has_posix_shell(), shell().1 == "-c");
    if !cfg!(windows) {
        assert!(has_posix_shell(), "there is always a posix shell on unix");
    }
}

/// A repository's own binary is found, and running it is a decision the caller has to make.
///
/// The gate is the point of the function: with it off the project copy must be invisible even
/// though it is sitting right there, because that is the difference between Luxe choosing a
/// program and a repository choosing one for it.
#[test]
fn a_project_binary_is_found_only_when_it_is_allowed() {
    let project = tempfile::tempdir().expect("tempdir");
    let bin = project.path().join("node_modules").join(".bin");
    std::fs::create_dir_all(&bin).expect("mkdir");
    write_executable(&bin, "luxe-fixture-server");

    let found = project_local(project.path(), "luxe-fixture-server").expect("the fixture");
    assert!(is_fixture(&found, &bin, "luxe-fixture-server"), "found {}", found.display());
    assert!(
        resolve_in(project.path(), "luxe-fixture-server", true).is_some_and(|path| is_fixture(
            &path,
            &bin,
            "luxe-fixture-server"
        )),
        "allowed, the project's copy is preferred"
    );
    assert_eq!(
        resolve_in(project.path(), "luxe-fixture-server", false),
        None,
        "denied, it is as if it were not there"
    );
}

/// A package deep in a workspace finds the binary its monorepo hoisted to the top.
#[test]
fn a_hoisted_binary_is_found_from_a_nested_package() {
    let repo = tempfile::tempdir().expect("tempdir");
    let bin = repo.path().join("node_modules").join(".bin");
    std::fs::create_dir_all(&bin).expect("mkdir");
    write_executable(&bin, "luxe-fixture-hoisted");

    let package = repo.path().join("packages").join("web");
    std::fs::create_dir_all(&package).expect("mkdir");
    let found = project_local(&package, "luxe-fixture-hoisted").expect("the hoisted fixture");
    assert!(is_fixture(&found, &bin, "luxe-fixture-hoisted"), "found {}", found.display());
}

/// Allowing project binaries does not mean "look everywhere": a program the project does not
/// carry still comes from `PATH`, and one that exists nowhere is still `None`.
#[test]
fn allowing_project_binaries_falls_through_to_the_ordinary_resolver() {
    let empty = tempfile::tempdir().expect("tempdir");
    let known = certainly_installed();
    assert!(resolve_in(empty.path(), known, true).is_some(), "{known} is still on PATH");
    assert_eq!(resolve_in(empty.path(), "luxe-no-such-program-anywhere", true), None);
}

/// A path is a path. Splicing one into `node_modules/.bin` would be nonsense at best and a way
/// out of the directory at worst.
#[test]
fn a_name_with_a_separator_is_never_treated_as_a_project_binary() {
    let project = tempfile::tempdir().expect("tempdir");
    assert_eq!(project_local(project.path(), "../../../bin/sh"), None);

    // Traversals that *would* resolve if the guard were not there, in both spellings: Windows
    // accepts either separator, so a guard checking only one is a way out of the directory on
    // the platform it forgot.
    std::fs::create_dir_all(project.path().join("node_modules").join(".bin")).expect("mkdir");
    let tools = project.path().join("tools");
    std::fs::create_dir_all(&tools).expect("mkdir");
    write_executable(&tools, "luxe-fixture-rogue");
    for escape in ["../../tools/luxe-fixture-rogue", "..\\..\\tools\\luxe-fixture-rogue"] {
        assert_eq!(project_local(project.path(), escape), None, "{escape} is a path");
    }

    let venv = write_venv(project.path(), ".venv", &["python"]);
    let bin = if cfg!(windows) { "Scripts" } else { "bin" };
    for escape in [format!("../{bin}/python"), format!("..\\{bin}\\python")] {
        assert_eq!(venv_program(&venv, &escape), None, "{escape} is a path, not a program");
    }
}

/// A virtualenv laid out the way the platform lays one out, so fixture and code cannot drift.
fn write_venv(root: &Path, name: &str, programs: &[&str]) -> PathBuf {
    let venv = root.join(name);
    let bin = venv.join(if cfg!(windows) { "Scripts" } else { "bin" });
    std::fs::create_dir_all(&bin).expect("mkdir");
    for program in programs {
        write_executable(&bin, program);
    }
    venv
}

/// `.venv` is the convention, and a directory that merely has the *name* is not an environment.
#[test]
fn a_projects_virtualenv_is_the_one_with_an_interpreter_in_it() {
    let project = tempfile::tempdir().expect("tempdir");
    assert_eq!(project_venv(project.path()), None, "no environment at all");

    std::fs::create_dir_all(project.path().join(".venv")).expect("mkdir");
    let fallback = write_venv(project.path(), "venv", &["python"]);
    assert_eq!(
        project_venv(project.path()),
        Some(fallback),
        "an empty `.venv` directory is not an environment"
    );

    let idiomatic = write_venv(project.path(), ".venv", &["python"]);
    assert_eq!(project_venv(project.path()), Some(idiomatic), "`.venv` outranks `venv`");
}

/// An environment's own executables are what the project pinned; `PATH` knows nothing of them.
#[test]
fn a_virtualenvs_programs_are_found_by_layout_not_by_path() {
    let project = tempfile::tempdir().expect("tempdir");
    let venv = write_venv(project.path(), ".venv", &["python", "pytest"]);
    assert!(venv_program(&venv, "python").is_some());
    assert!(venv_program(&venv, "pytest").is_some());
    assert_eq!(venv_program(&venv, "ruff"), None, "not installed is not installed");
    assert_eq!(venv_program(project.path(), "python"), None, "a project root is not a venv");
}

/// An activated environment answers for the project it is in, and for no other one.
///
/// The containment check is the load-bearing half: a shell that activated one project's venv
/// and then opened another must not run the second project's tests against the first's
/// dependencies.
#[test]
fn an_activated_environment_answers_only_inside_its_own_tree() {
    let project = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    let foreign = write_venv(elsewhere.path(), ".venv", &["python"]);
    let previous = std::env::var_os("VIRTUAL_ENV");

    std::env::set_var("VIRTUAL_ENV", &foreign);
    assert_eq!(project_venv(project.path()), None, "another project's environment");

    let named = write_venv(project.path(), "odd-name", &["python"]);
    std::env::set_var("VIRTUAL_ENV", &named);
    assert_eq!(
        project_venv(project.path()),
        Some(named),
        "an activated environment need not be conventionally named"
    );

    match previous {
        Some(value) => std::env::set_var("VIRTUAL_ENV", value),
        None => std::env::remove_var("VIRTUAL_ENV"),
    }
}
