//! Detection — which test command a project runs, and how its package manager says so.

use super::*;

/// What detection settled on.
#[derive(Debug)]
pub(super) struct Detected {
    pub(super) command: String,
    pub(super) framework: Framework,
}

impl Detected {
    fn new(command: impl Into<String>, framework: Framework) -> Self {
        Self { command: command.into(), framework }
    }
}

/// Detect the project's test command from marker files.
///
/// `Err` is a message for the model: either there is no project here to detect, or there is one
/// whose manifest declares no way to test it — and saying which beats running a command that was
/// never going to work.
pub(super) fn detect_command(cwd: &Path) -> Result<Detected, String> {
    detect_with(cwd, &|program| luxe_core::program::resolve(program).is_some())
}

pub(super) fn detect_with(cwd: &Path, available: Available) -> Result<Detected, String> {
    if cwd.join("Cargo.toml").exists() {
        let base = if available("cargo-nextest") { "cargo nextest run" } else { "cargo test" };
        return Ok(Detected::new(base, Framework::Cargo));
    }
    if cwd.join("go.mod").exists() {
        // `-v` because go otherwise prints nothing for a test that passed: a green suite came back
        // as `0 passed, 0 failed`, which this tool then reported as "no tests actually ran".
        return Ok(Detected::new("go test -v ./...", Framework::Go));
    }
    if ["pyproject.toml", "setup.py", "pytest.ini", "tox.ini", "setup.cfg"]
        .iter()
        .any(|f| cwd.join(f).exists())
    {
        return Ok(Detected::new(python_command(cwd, available), Framework::Pytest));
    }
    if cwd.join("package.json").exists() {
        return node_command(cwd, available);
    }
    Err("Could not detect a test command (no Cargo.toml / go.mod / pyproject.toml / \
         package.json). Pass `command` explicitly."
        .to_string())
}

/// How to run pytest for the project at `cwd`.
///
/// Python's convention is one environment per project, so a global `pytest` is not a slightly worse
/// choice than the project's — it is a *different* project's interpreter, with none of this one's
/// dependencies on its path, and the failure it produces blames the suite. The environment is
/// driven through its interpreter rather than its `pytest` shim because `-m` also puts `cwd` on
/// `sys.path`, which is what a project whose own package is not installed needs.
pub(super) fn python_command(cwd: &Path, available: Available) -> String {
    let venv = luxe_core::program::project_venv(cwd);
    let installed = venv
        .as_deref()
        .filter(|venv| luxe_core::program::venv_program(venv, "pytest").is_some())
        .and_then(|venv| venv_pytest(cwd, venv));
    if let Some(command) = installed {
        return command;
    }
    // Nothing to run in the environment yet, but the manager that owns it can install what its
    // lock file pins. A global pytest cannot.
    if let Some(runner) = python_manager(cwd, available) {
        return format!("{runner} run pytest");
    }
    if let Some(command) = venv.as_deref().and_then(|venv| venv_pytest(cwd, venv)) {
        // Still the project's interpreter: `no module named pytest` names the real problem, where a
        // global run reports a missing dependency and points at the wrong thing entirely.
        return command;
    }
    if available("pytest") {
        return "pytest".to_string();
    }
    let python = if available("python3") { "python3" } else { "python" };
    format!("{python} -m pytest")
}

/// `pytest` run through the interpreter of the environment at `venv`.
pub(super) fn venv_pytest(cwd: &Path, venv: &Path) -> Option<String> {
    let python = luxe_core::program::venv_program(venv, "python")?;
    Some(format!("{} -m pytest", shell_word(cwd, &python)))
}

/// The dependency manager that owns this project's environment, if it is installed.
///
/// A lock file is the marker rather than a `pyproject.toml` section, because the lock is what says
/// the manager really runs here — and `<manager> run` is what a human in this repository types.
pub(super) fn python_manager(cwd: &Path, available: Available) -> Option<&'static str> {
    [("uv.lock", "uv"), ("poetry.lock", "poetry"), ("Pipfile.lock", "pipenv")]
        .into_iter()
        .find(|(lock, runner)| cwd.join(lock).exists() && available(runner))
        .map(|(_, runner)| runner)
}

/// Lock file, program, and the words that run one of its scripts, most specific first.
pub(super) const NODE_MANAGERS: [(&str, &str, &str); 6] = [
    ("pnpm-lock.yaml", "pnpm", "pnpm test"),
    ("yarn.lock", "yarn", "yarn test"),
    ("bun.lockb", "bun", "bun run test"),
    ("bun.lock", "bun", "bun run test"),
    ("package-lock.json", "npm", "npm test"),
    ("npm-shrinkwrap.json", "npm", "npm test"),
];

/// How to run the tests of a JavaScript project.
///
/// The manifest is consulted rather than assumed: `npm test` against a project that declares no
/// `test` script fails with `Missing script: "test"`, which reads as a broken tool. A manifest we
/// cannot parse is npm's error to report, not ours, so that case still runs.
pub(super) fn node_command(cwd: &Path, available: Available) -> Result<Detected, String> {
    let manager = node_manager(cwd, available);
    let Some(manifest) = std::fs::read_to_string(cwd.join("package.json"))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
    else {
        return Ok(Detected::new(manager, Framework::Node));
    };
    let declared = manifest
        .get("scripts")
        .and_then(|scripts| scripts.get("test"))
        .and_then(|script| script.as_str())
        .is_some_and(|script| !script.trim().is_empty());
    if declared {
        return Ok(Detected::new(manager, Framework::Node));
    }
    Err(missing_test_script(&manifest, manager))
}

/// The package manager this project is installed with, as the words that run one of its scripts.
///
/// The lock file is the project's own statement of which manager it uses, and it decides more than
/// style: a pnpm workspace's dependencies are not linked the way npm expects to find them.
pub(super) fn node_manager(cwd: &Path, available: Available) -> &'static str {
    NODE_MANAGERS
        .iter()
        .find(|(lock, program, _)| cwd.join(lock).exists() && available(program))
        .map(|(_, _, run)| *run)
        .or_else(|| available("npm").then_some("npm test"))
        .or_else(|| {
            NODE_MANAGERS.iter().find(|(_, program, _)| available(program)).map(|(_, _, run)| *run)
        })
        .unwrap_or("npm test")
}

/// Refusal for a JavaScript project whose manifest declares no test script.
///
/// Naming the runner already in its dependencies turns a dead end into a command the caller can
/// pass. `npx` regardless of manager: it resolves `node_modules/.bin` the same way in all of them.
pub(super) fn missing_test_script(manifest: &serde_json::Value, manager: &str) -> String {
    let depends_on = |name: &str| {
        ["devDependencies", "dependencies"]
            .iter()
            .any(|section| manifest.get(section).and_then(|deps| deps.get(name)).is_some())
    };
    let suggestion = [("vitest", "npx vitest run"), ("jest", "npx jest"), ("mocha", "npx mocha")]
        .into_iter()
        .find(|(name, _)| depends_on(name));
    match suggestion {
        Some((name, command)) => format!(
            "package.json declares no `test` script, so `{manager}` would fail on that rather than \
             on your tests. This project depends on {name} — pass `command: \"{command}\"` (the \
             `run` matters: a bare watcher never exits), or add a `test` script."
        ),
        None => format!(
            "package.json declares no `test` script, so `{manager}` would fail on that rather than \
             on your tests. Add one, or pass `command` with the runner this project uses."
        ),
    }
}

/// A path as one shell word: relative to `cwd` when it is inside it, forward slashes, quoted only
/// when it has to be.
pub(super) fn shell_word(cwd: &Path, path: &Path) -> String {
    let rendered = luxe_core::paths::display(path.strip_prefix(cwd).unwrap_or(path));
    if rendered.contains(char::is_whitespace) {
        format!("\"{rendered}\"")
    } else {
        rendered
    }
}
