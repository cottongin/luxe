//! Compression — deterministic tool-output compression.
//!
//! Noisy command output (build logs, test runners, linters, git/docker) wastes the biggest, cheapest
//! slice of the context window. This module reduces it *deterministically* — no LLM, offline,
//! golden-file testable — before it reaches the model, while preserving the signal (errors,
//! warnings, failing tests, file:line, exit codes, auth prompts) via [`classify::is_important`].
//!
//! It is distinct from and complementary to conversation compaction ([`crate::compaction`]): this
//! shrinks what *enters* context at capture-time; compaction summarizes old turns when context fills.
//!
//! # Design
//!
//! `Action → capture output → compress → compact model-facing text (+ full output tee'd for
//! recovery by the caller)`. A [`ReducerRegistry`] resolves a command to a command-specific
//! [`OutputReducer`] (e.g. a future structured test reducer); everything else takes the generic
//! stage pipeline. The caller (the `bash` tool) owns artifact-teeing and final formatting.

mod classify;
mod reducers;
mod stages;

pub use classify::is_important;
pub use reducers::{is_test_command, parse_test_output, TestFailure, TestReducer, TestSummary};

/// A command whose output is being compressed.
#[derive(Debug, Clone)]
pub struct CommandInvocation {
    /// The program (`argv[0]`), e.g. `cargo`.
    pub executable: String,
    /// Arguments after the program, e.g. `["build"]`.
    pub args: Vec<String>,
    /// The original command line as written.
    pub original: String,
}

impl CommandInvocation {
    /// Parses a shell command line into `(executable, args)` with a whitespace/quote-aware split.
    /// Good enough to key reducers off the program + subcommand; not a full shell parser.
    ///
    /// The program is the one doing the work, not the setup in front of it: `cd tests &&`,
    /// `export X=1;`, a `FOO=bar` assignment, and launchers like `env`, `time` and `uv run` are
    /// stepped over, since `cd tests && python runtests.py` is how a model runs a suite.
    pub fn parse(command_line: &str) -> Self {
        let mut tokens = shell_split(command_line);
        loop {
            let setup = match tokens.first().map(String::as_str) {
                Some("cd" | "pushd" | "export" | "source" | ".") => {
                    tokens.iter().position(|token| {
                        matches!(token.as_str(), "&&" | "||") || token.ends_with(';')
                    })
                }
                Some(token) if token.ends_with(';') && tokens.len() > 1 => Some(0),
                Some("env" | "time" | "nice" | "command" | "exec") => Some(0),
                Some("uv" | "poetry" | "pipenv" | "hatch" | "pdm")
                    if tokens.get(1).map(String::as_str) == Some("run") =>
                {
                    Some(1)
                }
                Some(token) if is_assignment(token) => Some(0),
                _ => None,
            };
            match setup {
                Some(last) if last + 1 < tokens.len() => {
                    tokens.drain(..=last);
                }
                _ => break,
            }
        }
        let executable = tokens.first().cloned().unwrap_or_default();
        // Strip a leading path so `/usr/bin/cargo` still matches the `cargo` reducer.
        let executable = executable.rsplit(['/', '\\']).next().unwrap_or(&executable).to_string();
        let args = tokens.into_iter().skip(1).collect();
        Self { executable, args, original: command_line.to_string() }
    }
}

/// Captured process output.
#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

/// Tunables for the generic pipeline.
#[derive(Debug, Clone)]
pub struct CompressionConfig {
    /// Master switch (the `bash` tool wires an env opt-out to this).
    pub enabled: bool,
    /// Keep this many leading lines verbatim.
    pub head_lines: usize,
    /// Keep this many trailing lines verbatim.
    pub tail_lines: usize,
    /// Truncate a stream once it exceeds this many lines (head + important + tail survive).
    pub max_lines: usize,
    /// Only claim a "reduced" footer when at least this fraction of tokens was saved.
    pub min_savings_ratio: f64,
    /// …and at least this many tokens (avoids a noisy footer on tiny outputs).
    pub min_savings_tokens: usize,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            head_lines: 40,
            tail_lines: 40,
            max_lines: 200,
            min_savings_ratio: 0.15,
            min_savings_tokens: 50,
        }
    }
}

/// What the pipeline did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompressionDecision {
    /// Meaningfully smaller — worth a savings footer and an artifact tee.
    Reduced {
        /// Which reducer ran (`generic` or a command-specific id).
        reducer: String,
    },
    /// Ran the pipeline but savings were below threshold; output is the lightly-cleaned text.
    Minimal,
    /// Compression disabled; output is the raw (labeled) text.
    Disabled,
}

/// The outcome of compressing one command's output.
#[derive(Debug, Clone)]
pub struct CompressionResult {
    /// Model-facing body (labeled STDOUT/STDERR sections). Caller appends the exit code + footer.
    pub output: String,
    /// Estimated tokens before compression.
    pub original_tokens: usize,
    /// Estimated tokens after compression.
    pub compressed_tokens: usize,
    /// What happened.
    pub decision: CompressionDecision,
}

impl CompressionResult {
    /// Percentage of tokens saved (0.0 when the output grew or was unchanged).
    pub fn savings_pct(&self) -> f64 {
        if self.original_tokens == 0 || self.compressed_tokens >= self.original_tokens {
            return 0.0;
        }
        let saved = self.original_tokens - self.compressed_tokens;
        (saved as f64 / self.original_tokens as f64) * 100.0
    }

    /// Whether the result dropped enough content to warrant teeing the full output for recovery.
    pub fn was_reduced(&self) -> bool {
        matches!(self.decision, CompressionDecision::Reduced { .. })
    }

    /// A compact one-line savings footer, when the reduction was meaningful.
    pub fn footer(&self) -> Option<String> {
        match &self.decision {
            CompressionDecision::Reduced { reducer } => Some(format!(
                "[compressed {}→{} tok, -{:.0}%, reducer={reducer}]",
                self.original_tokens,
                self.compressed_tokens,
                self.savings_pct()
            )),
            _ => None,
        }
    }
}

/// A command-specific output reducer. Implementors turn a known command's output into a compact,
/// high-signal rendering (the structured test reducer is the first real one).
pub trait OutputReducer: Send + Sync {
    /// Stable id shown in the footer.
    fn id(&self) -> &str;
    /// Whether this reducer handles the given command.
    fn can_handle(&self, command: &CommandInvocation) -> bool;
    /// Produce the reduced, model-facing body.
    fn reduce(&self, command: &CommandInvocation, output: &CommandOutput) -> String;
}

/// Ordered set of command-specific reducers; the first match wins, else the generic pipeline runs.
#[derive(Default)]
pub struct ReducerRegistry {
    reducers: Vec<Box<dyn OutputReducer>>,
}

impl ReducerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a reducer (later registrations run after earlier ones on ties).
    pub fn register(&mut self, reducer: Box<dyn OutputReducer>) {
        self.reducers.push(reducer);
    }

    fn find(&self, command: &CommandInvocation) -> Option<&dyn OutputReducer> {
        self.reducers.iter().find(|r| r.can_handle(command)).map(|r| r.as_ref())
    }
}

/// Rough token estimate (~4 chars/token) — a dependency-free heuristic sufficient for savings math.
pub fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Compress a command's output: pick a reducer (or the generic pipeline), account for tokens, and
/// decide whether the reduction is worth flagging.
pub fn compress(
    command: &CommandInvocation,
    output: &CommandOutput,
    config: &CompressionConfig,
    registry: &ReducerRegistry,
) -> CompressionResult {
    let original_tokens = estimate_tokens(&output.stdout) + estimate_tokens(&output.stderr);

    if !config.enabled {
        return CompressionResult {
            output: label_streams(&output.stdout, &output.stderr),
            original_tokens,
            compressed_tokens: original_tokens,
            decision: CompressionDecision::Disabled,
        };
    }

    let (body, reducer) = match registry.find(command) {
        Some(reducer) => (reducer.reduce(command, output), reducer.id().to_string()),
        None => (generic_reduce(output, config), "generic".to_string()),
    };
    let compressed_tokens = estimate_tokens(&body);

    let saved = original_tokens.saturating_sub(compressed_tokens);
    let meaningful = saved >= config.min_savings_tokens
        && (saved as f64) >= (original_tokens as f64) * config.min_savings_ratio;
    let decision = if meaningful {
        CompressionDecision::Reduced { reducer }
    } else {
        CompressionDecision::Minimal
    };

    CompressionResult { output: body, original_tokens, compressed_tokens, decision }
}

/// The generic pipeline over both streams, producing a labeled body.
pub(crate) fn generic_reduce(output: &CommandOutput, config: &CompressionConfig) -> String {
    let stdout = reduce_stream(&output.stdout, config);
    let stderr = reduce_stream(&output.stderr, config);
    let mut body = String::new();
    if !stdout.trim().is_empty() {
        body.push_str("STDOUT:\n");
        body.push_str(&stdout);
    }
    if !stderr.trim().is_empty() {
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str("STDERR:\n");
        body.push_str(&stderr);
    }
    body
}

/// Apply the deterministic stages to one stream.
fn reduce_stream(text: &str, config: &CompressionConfig) -> String {
    if text.is_empty() {
        return String::new();
    }
    let stripped = stages::strip_ansi(text);
    let lines: Vec<String> =
        stripped.lines().map(|line| stages::collapse_carriage_returns(line).to_string()).collect();
    let lines = stages::filter_progress(lines, classify::is_important);
    let lines = stages::dedupe_consecutive(lines);
    let lines = stages::collapse_blank_lines(lines);
    let lines = stages::truncate_preserving(
        lines,
        classify::is_important,
        config.head_lines,
        config.tail_lines,
        config.max_lines,
    );
    lines.join("\n")
}

/// Label raw streams without reducing (disabled path).
fn label_streams(stdout: &str, stderr: &str) -> String {
    let mut body = String::new();
    if !stdout.is_empty() {
        body.push_str("STDOUT:\n");
        body.push_str(stdout);
    }
    if !stderr.is_empty() {
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str("STDERR:\n");
        body.push_str(stderr);
    }
    body
}

/// Whether `token` is a `NAME=value` assignment rather than a program.
fn is_assignment(token: &str) -> bool {
    token.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Minimal whitespace/quote-aware split (no expansion) for keying reducers.
fn shell_split(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut has_token = false;
    for c in input.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => current.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                has_token = true;
            }
            None if c.is_whitespace() => {
                if has_token {
                    tokens.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            None => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        tokens.push(current);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(stdout: &str, stderr: &str) -> CommandOutput {
        CommandOutput { stdout: stdout.to_string(), stderr: stderr.to_string(), exit_code: Some(0) }
    }

    #[test]
    fn parses_command_and_strips_path() {
        let cmd = CommandInvocation::parse("/usr/bin/cargo build --release");
        assert_eq!(cmd.executable, "cargo");
        assert_eq!(cmd.args, vec!["build", "--release"]);
    }

    /// The setup in front of a command is not the command.
    #[test]
    fn the_program_is_the_one_after_the_setup() {
        let program = |line: &str| {
            let parsed = CommandInvocation::parse(line);
            (parsed.executable, parsed.args.first().cloned().unwrap_or_default())
        };
        assert_eq!(
            program("cd tests && ../.venv/bin/python runtests.py x"),
            ("python".into(), "runtests.py".into())
        );
        assert_eq!(program("cd a; cd b && cargo test"), ("cargo".into(), "test".into()));
        assert_eq!(program("RUST_LOG=debug cargo test -p x"), ("cargo".into(), "test".into()));
        assert_eq!(program("export CI=1; env time pytest -q"), ("pytest".into(), "-q".into()));
        assert_eq!(program("uv run pytest"), ("pytest".into(), String::new()));
        assert_eq!(program("cd only"), ("cd".into(), "only".into()), "nothing after the setup");
        assert_eq!(program("echo a=b"), ("echo".into(), "a=b".into()));
    }

    #[test]
    fn shell_split_respects_quotes() {
        assert_eq!(shell_split("echo 'a b' c"), vec!["echo", "a b", "c"]);
    }

    #[test]
    fn compresses_noisy_log_but_keeps_error() {
        // A long, repetitive build log with one real error buried in the middle.
        let mut lines: Vec<String> =
            (0..500).map(|i| format!("   Compiling crate_{i} v1.0")).collect();
        lines[250] = "error[E0308]: mismatched types at src/main.rs:12:9".into();
        let log = lines.join("\n");
        let result = compress(
            &CommandInvocation::parse("cargo build"),
            &output(&log, ""),
            &CompressionConfig::default(),
            &ReducerRegistry::new(),
        );
        assert!(result.was_reduced(), "a 500-line log should reduce");
        assert!(result.output.contains("error[E0308]"), "the buried error must survive");
        assert!(result.output.contains("src/main.rs:12:9"), "file:line must survive");
        assert!(result.compressed_tokens < result.original_tokens);
        assert!(result.footer().unwrap().contains("reducer=generic"));
    }

    #[test]
    fn short_output_is_minimal_not_reduced() {
        let result = compress(
            &CommandInvocation::parse("echo hi"),
            &output("hi\n", ""),
            &CompressionConfig::default(),
            &ReducerRegistry::new(),
        );
        assert_eq!(result.decision, CompressionDecision::Minimal);
        assert!(result.footer().is_none());
        assert!(result.output.contains("hi"));
    }

    #[test]
    fn disabled_returns_raw_labeled() {
        let cfg = CompressionConfig { enabled: false, ..Default::default() };
        let result = compress(
            &CommandInvocation::parse("x"),
            &output("\u{1b}[31mraw\u{1b}[0m", ""),
            &cfg,
            &ReducerRegistry::new(),
        );
        assert_eq!(result.decision, CompressionDecision::Disabled);
        assert!(result.output.contains("\u{1b}["), "disabled keeps raw ANSI");
    }

    #[test]
    fn registry_prefers_command_reducer() {
        struct FakeReducer;
        impl OutputReducer for FakeReducer {
            fn id(&self) -> &str {
                "fake"
            }
            fn can_handle(&self, command: &CommandInvocation) -> bool {
                command.executable == "cargo"
            }
            fn reduce(&self, _: &CommandInvocation, _: &CommandOutput) -> String {
                "reduced-by-fake".to_string()
            }
        }
        let mut registry = ReducerRegistry::new();
        registry.register(Box::new(FakeReducer));
        let long = "noise\n".repeat(500);
        let result = compress(
            &CommandInvocation::parse("cargo test"),
            &output(&long, ""),
            &CompressionConfig::default(),
            &registry,
        );
        assert_eq!(result.output, "reduced-by-fake");
        assert!(
            matches!(result.decision, CompressionDecision::Reduced { reducer } if reducer == "fake")
        );
    }
}
