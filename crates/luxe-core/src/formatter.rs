//! Formatter registry — language-specific code formatters.
//!
//! Maps file extensions to formatter commands (rustfmt, biome, ruff format, etc.).
//! The `file_changed` hook uses this to run format-on-save for the changed file's language.
//!
//! A registry built with [`FormatterRegistry::for_project`] answers for one project: it prefers the
//! copy of each formatter that project installed (its virtualenv, its `node_modules/.bin`) when the
//! run allows repository-supplied binaries, and it formats Rust against the edition the project
//! declares. A formatter is only useful if it agrees with the repository's own settings — a
//! different `ruff` reformats to a different style, and rustfmt under the wrong edition can refuse
//! to parse valid code.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command as TokioCommand;

/// A formatter specification: which command to run and how.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatterSpec {
    /// The binary to invoke (e.g., "rustfmt", "biome", "ruff").
    pub command: String,
    /// Arguments passed to the command.
    pub args: Vec<String>,
    /// If true, pass file contents via stdin and expect formatted output on stdout.
    /// If false, the formatter operates on the file directly.
    pub via_stdin: bool,
}

/// One built-in formatter: `(language, extensions, command, args, via_stdin)`.
type DefaultFormatter =
    (&'static str, &'static [&'static str], &'static str, &'static [&'static str], bool);

/// The built-in formatters. Rust's edition is a fallback that [`FormatterRegistry::for_project`]
/// replaces with the project's own.
const DEFAULTS: &[DefaultFormatter] = &[
    ("rust", &["rs"], "rustfmt", &["--edition", "2021"], false),
    ("python", &["py"], "ruff", &["format"], false),
    ("typescript", &["ts", "tsx"], "biome", &["format", "--write"], false),
    ("javascript", &["js", "jsx"], "biome", &["format", "--write"], false),
    ("go", &["go"], "gofmt", &["-w"], false),
    ("toml", &["toml"], "taplo", &["format"], true),
];

/// Registry mapping language names to formatter specs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatterRegistry {
    /// language name (e.g., "rust", "python") → formatter spec.
    formatters: HashMap<String, FormatterSpec>,
    /// file extension (without dot) → language name.
    extensions: HashMap<String, String>,
    /// The project these formatters are for, empty when the registry is not bound to one.
    ///
    /// Not configuration — it is where [`run_formatter`] looks for the formatter itself, so it
    /// travels with the registry rather than through every call site. A registry that came back
    /// from disk has no project, which reads as "resolve the way everything else does".
    #[serde(skip)]
    root: PathBuf,
    /// Whether the copy this project installed for itself may run.
    ///
    /// The caller's answer, never this module's: the same question `lsp.project_servers` settles
    /// for language servers, and for the same reason — the binary came from the repository.
    #[serde(skip)]
    project_local: bool,
}

impl Default for FormatterRegistry {
    fn default() -> Self {
        let mut registry = Self {
            formatters: HashMap::new(),
            extensions: HashMap::new(),
            root: PathBuf::new(),
            project_local: false,
        };
        registry.add_defaults();
        registry
    }
}

impl FormatterRegistry {
    /// Create a new registry with sensible defaults (rustfmt for Rust).
    pub fn new() -> Self {
        Self::default()
    }

    /// The defaults, bound to the project at `root`.
    ///
    /// Two things change: Rust is formatted against the edition this project declares rather than a
    /// constant, and — when `project_local` says the run may run a binary the repository supplied —
    /// each formatter resolves to the project's own copy first. Both are the same point: the
    /// formatter that matters is the one the repository would have used itself.
    pub fn for_project(root: impl Into<PathBuf>, project_local: bool) -> Self {
        let root = root.into();
        let mut registry = Self { root, project_local, ..Default::default() };
        let edition = rust_edition(&registry.root);
        registry.formatters.insert(
            "rust".to_string(),
            FormatterSpec {
                command: "rustfmt".to_string(),
                args: vec!["--edition".to_string(), edition],
                via_stdin: false,
            },
        );
        registry
    }

    /// Add the default formatter entries, from [`DEFAULTS`].
    fn add_defaults(&mut self) {
        for (language, extensions, command, args, via_stdin) in DEFAULTS {
            let args = args.iter().map(|arg| arg.to_string()).collect();
            let spec = FormatterSpec { command: command.to_string(), args, via_stdin: *via_stdin };
            self.formatters.insert(language.to_string(), spec);
            for extension in *extensions {
                self.extensions.insert(extension.to_string(), language.to_string());
            }
        }
    }

    /// The language whose formatter should run on `path`, if any.
    ///
    /// rustfmt and gofmt run on every file: each is its language's own standard, and a project that
    /// formats Rust or Go some other way is vanishingly rare. The rest run only where the project
    /// chose them, by a config file in the edited file's directory or one above it, up to the
    /// project root. Before that, biome rewrote every TypeScript file an agent touched in a project
    /// formatted by Prettier, whenever biome happened to be installed: a whole-file diff nobody
    /// asked for.
    pub fn formatter_for(&self, path: &Path) -> Option<String> {
        let language = self.language_for_path(path)?;
        let markers = opt_in_markers(&language);
        if markers.is_empty() || self.project_chose(path, markers) {
            Some(language)
        } else {
            None
        }
    }

    /// Whether a config file in `markers` sits beside `path` or above it, within the project.
    fn project_chose(&self, path: &Path, markers: &[ConfigMarker]) -> bool {
        let bounded = !self.root.as_os_str().is_empty();
        path.ancestors()
            .skip(1)
            .take_while(|dir| !bounded || dir.starts_with(&self.root))
            .any(|dir| markers.iter().any(|marker| marker.found_in(dir)))
    }

    /// Register a custom formatter for a language.
    ///
    /// Also maps `<language>` (lowercased) as a fallback extension if no explicit
    /// extension mapping exists — e.g. `register("zig", ...)` makes `.zig` work.
    pub fn register(&mut self, language: &str, spec: FormatterSpec) {
        self.formatters.insert(language.to_string(), spec);
        // Auto-map the language name as a fallback extension (e.g., "zig" → .zig).
        self.extensions.entry(language.to_lowercase()).or_insert_with(|| language.to_string());
    }

    /// Map a file extension to a language name.
    pub fn language_for_extension(&self, ext: &str) -> Option<&str> {
        self.extensions.get(ext).map(|s| s.as_str())
    }

    /// Get the formatter spec for a language, if registered.
    pub fn get(&self, language: &str) -> Option<&FormatterSpec> {
        self.formatters.get(language)
    }

    /// Detect the language for a file path based on its extension.
    pub fn language_for_path(&self, path: &Path) -> Option<String> {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(|ext| self.language_for_extension(ext))
            .map(|s| s.to_string())
    }

    /// List all registered language names.
    pub fn languages(&self) -> Vec<&str> {
        self.formatters.keys().map(|s| s.as_str()).collect()
    }
}

/// A config file that opts a project into a formatter, and the text it must contain when the file
/// is shared with other tools.
struct ConfigMarker(&'static str, Option<&'static str>);

impl ConfigMarker {
    fn found_in(&self, dir: &Path) -> bool {
        let file = dir.join(self.0);
        match self.1 {
            None => file.is_file(),
            Some(needle) => std::fs::read_to_string(file).is_ok_and(|text| text.contains(needle)),
        }
    }
}

/// The config files that opt a project into `language`'s formatter; none means it always runs.
fn opt_in_markers(language: &str) -> &'static [ConfigMarker] {
    match language {
        "python" => &[
            ConfigMarker("ruff.toml", None),
            ConfigMarker(".ruff.toml", None),
            ConfigMarker("pyproject.toml", Some("[tool.ruff")),
        ],
        "typescript" | "javascript" => {
            &[ConfigMarker("biome.json", None), ConfigMarker("biome.jsonc", None)]
        }
        "toml" => &[ConfigMarker("taplo.toml", None), ConfigMarker(".taplo.toml", None)],
        _ => &[],
    }
}

/// Error from running a formatter.
#[derive(Debug, thiserror::Error)]
pub enum FormatterError {
    #[error("formatter '{command}' not found")]
    NotFound { command: String },
    #[error("formatter exited with status {status}: {stderr}")]
    Failed { status: String, stderr: String, command: String },
    #[error("no formatter registered for {language}")]
    NoFormatter { language: String },
    /// Reading, writing or running failed, which is not the formatter being missing
    #[error("formatter '{command}' could not {action}: {error}")]
    Io { command: String, action: &'static str, error: std::io::Error },
    /// It answered a file that was not empty with nothing, so the file was left as it was
    #[error(
        "formatter '{command}' printed nothing for a non-empty file, so the file was left alone"
    )]
    EmptyOutput { command: String },
    /// The file changed while it ran, so its output, made from the old text, was not written
    #[error("the file changed while formatter '{command}' ran, so its output was not written")]
    Changed { command: String },
}

/// Run a formatter on a file. Returns Ok(()) on success, or an error describing what went wrong.
///
/// If the formatter uses stdin mode, reads the file, passes it through, and writes back.
/// If via_stdin is false, the formatter operates on the file directly (e.g., rustfmt --edition 2021 file.rs).
pub async fn run_formatter(
    path: &Path,
    registry: &FormatterRegistry,
) -> Result<(), FormatterError> {
    let lang = registry.language_for_path(path).ok_or_else(|| FormatterError::NoFormatter {
        language: path.extension().and_then(|e| e.to_str()).unwrap_or("unknown").to_string(),
    })?;

    // a registry read back from disk can map an extension to a language it has no spec for
    let spec = registry
        .get(&lang)
        .cloned()
        .ok_or_else(|| FormatterError::NoFormatter { language: lang.clone() })?;

    // Resolve before running: `cargo install` / `pipx` / `npm -g` bin directories are frequently
    // absent from the `PATH` an app inherits, and reporting "not found" for a formatter that is
    // installed silently turns format-on-save off. Where the run allows it, the copy this project
    // installed for itself wins — that is the one whose version agrees with the repository.
    //
    // No liveness probe: `<formatter> --version` was one, and `gofmt` has no such flag, so every Go
    // file failed a check it could not pass while the formatter itself worked perfectly. A program
    // that cannot start still fails at spawn, with its own stderr instead of a guess.
    let project_copy = registry
        .project_local
        .then(|| crate::program::project_local(&registry.root, &spec.command))
        .flatten();
    let Some(program) = project_copy.clone().or_else(|| crate::program::resolve(&spec.command))
    else {
        return Err(FormatterError::NotFound { command: spec.command });
    };
    if project_copy.is_some() {
        // Worth an `info`: the program that just rewrote a file came from the tree being edited
        // rather than from the machine, and that should be findable in a normal run.
        tracing::info!(
            language = lang,
            formatter = %crate::paths::display(&program),
            "using the formatter this project installed"
        );
    }
    let spec = FormatterSpec { command: program.to_string_lossy().into_owned(), ..spec };

    if spec.via_stdin {
        run_via_stdin(&spec, path).await
    } else {
        run_in_place(&spec, path).await
    }
}

/// The Rust edition a project declares, for rustfmt's `--edition`.
///
/// rustfmt is not cargo: it reads no manifest, defaults to 2015, and a `--edition` on the command
/// line *overrides* whatever the project's own `rustfmt.toml` says. So a constant is worse than it
/// looks — on a 2024 crate it formats to the wrong style edition and refuses to parse syntax that
/// edition added, which reads as a broken file rather than a wrong flag. `rustfmt.toml` first,
/// because that is the project speaking about formatting specifically; then the manifest; then the
/// last edition Luxe can safely assume.
///
/// A line scan rather than a TOML parse, deliberately: one key, three well-known file names, and no
/// dependency worth adding for it. Anything it cannot read falls back rather than guessing.
fn rust_edition(root: &Path) -> String {
    ["rustfmt.toml", ".rustfmt.toml", "Cargo.toml"]
        .iter()
        .find_map(|name| {
            std::fs::read_to_string(root.join(name)).ok().as_deref().and_then(edition_in)
        })
        .unwrap_or_else(|| "2021".to_string())
}

/// The value of an `edition = "20xx"` line in a TOML document.
///
/// Section-blind on purpose: `[package]` and `[workspace.package]` are the only two places the key
/// appears and they mean the same thing here. `edition.workspace = true` carries no value, so it
/// falls through to the next file, which is the workspace root that answers it.
fn edition_in(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("edition")?.trim_start().strip_prefix('='))
        .map(|value| value.trim().trim_matches(['"', '\'']).to_string())
        .find(|value| value.len() == 4 && value.chars().all(|c| c.is_ascii_digit()))
}

/// How long any one formatter may take before it is reaped.
///
/// Format-on-save runs inside an edit, which runs inside a turn, so an unbounded formatter stalls
/// the session with nothing for the user to cancel. Generous enough that a large file through a slow
/// formatter still finishes.
const FORMAT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Run a formatter that writes to the file directly (rustfmt, biome, gofmt, etc.).
///
/// Every stream is redirected, never inherited. Format-on-save runs underneath a full-screen TUI
/// that owns the alternate screen: a child writing to the real stdout paints into cells the
/// renderer does not know changed, and a half-consumed escape sequence shifts the whole frame.
/// Inherited stdin is the same hazard in reverse — the child would eat the user's keystrokes.
async fn run_in_place(spec: &FormatterSpec, path: &Path) -> Result<(), FormatterError> {
    let mut cmd = TokioCommand::new(&spec.command);
    cmd.args(&spec.args).arg(path).stdin(Stdio::null()).kill_on_drop(true);
    crate::program::detach_from_terminal(&mut cmd);

    let child =
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(io(spec, "start"))?;
    let pid = child.id().unwrap_or(0);
    let output = match tokio::time::timeout(FORMAT_TIMEOUT, child.wait_with_output()).await {
        Ok(result) => result.map_err(io(spec, "finish"))?,
        Err(_) => {
            crate::program::kill_tree(pid);
            return Err(FormatterError::Failed {
                status: "timeout".into(),
                stderr: format!("formatter did not finish within {}s", FORMAT_TIMEOUT.as_secs()),
                command: spec.command.clone(),
            });
        }
    };

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(FormatterError::Failed {
            status: output.status.to_string(),
            stderr,
            command: spec.command.clone(),
        })
    }
}

/// An IO failure while `action` was being done for `spec`.
fn io<'a>(
    spec: &'a FormatterSpec,
    action: &'static str,
) -> impl Fn(std::io::Error) -> FormatterError + 'a {
    move |source| FormatterError::Io { command: spec.command.clone(), action, error: source }
}

/// Run a formatter that reads from stdin and writes to stdout (taplo, etc.).
///
/// Its answer replaces the file only when it is not empty for a file that was not, and only if the
/// file still holds what was handed over: an edit made meanwhile is newer than the answer.
async fn run_via_stdin(spec: &FormatterSpec, path: &Path) -> Result<(), FormatterError> {
    let input = tokio::fs::read(path).await.map_err(io(spec, "read the file"))?;
    let output = filter(spec, &input).await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(FormatterError::Failed {
            status: output.status.to_string(),
            stderr,
            command: spec.command.clone(),
        });
    }
    if output.stdout.is_empty() && !input.is_empty() {
        return Err(FormatterError::EmptyOutput { command: spec.command.clone() });
    }
    let (path, formatted) = (path.to_path_buf(), output.stdout);
    let written = tokio::task::spawn_blocking(move || {
        if std::fs::read(&path)? != input {
            return Ok(false);
        }
        crate::fs_atomic::replace_keeping_mode(&path, &formatted).map(|()| true)
    })
    .await
    .map_err(|error| io(spec, "write the file")(std::io::Error::other(error)))?
    .map_err(io(spec, "write the file"))?;
    match written {
        true => Ok(()),
        false => Err(FormatterError::Changed { command: spec.command.clone() }),
    }
}

/// `input` through the formatter's stdin, and what it answered, under one deadline over both.
async fn filter(
    spec: &FormatterSpec,
    input: &[u8],
) -> Result<std::process::Output, FormatterError> {
    let mut cmd = TokioCommand::new(&spec.command);
    cmd.args(&spec.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // Captured, not inherited: a formatter's warnings must not land on the TUI's screen.
        .stderr(Stdio::piped());
    cmd.kill_on_drop(true);
    crate::program::detach_from_terminal(&mut cmd);
    let mut child = cmd.spawn().map_err(io(spec, "start"))?;
    let pid = child.id().unwrap_or(0);

    // One deadline over both halves. A pipe holds ~64KB, so writing a larger file to a formatter
    // that is slow to drain it blocks here — and format-on-save runs inside an edit, which runs
    // inside a turn, so an unbounded wait here stalls the whole session.
    let deadline = tokio::time::Instant::now() + FORMAT_TIMEOUT;
    let timed_out = || {
        crate::program::kill_tree(pid);
        FormatterError::Failed {
            status: "timeout".into(),
            stderr: format!("formatter did not finish within {}s", FORMAT_TIMEOUT.as_secs()),
            command: spec.command.clone(),
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        match tokio::time::timeout_at(deadline, stdin.write_all(input)).await {
            // it stopped reading and answered anyway; what it answered is judged below
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::BrokenPipe => {}
            Ok(result) => result.map_err(io(spec, "write to it"))?,
            Err(_) => return Err(timed_out()),
        }
        // dropping stdin is the EOF
    }
    match tokio::time::timeout_at(deadline, child.wait_with_output()).await {
        Ok(result) => result.map_err(io(spec, "finish")),
        Err(_) => Err(timed_out()),
    }
}

#[cfg(test)]
mod tests;
