//! Command-line surface — the clap argument parser, subcommands, and arg→settings mappers.

use clap::{Parser, ValueEnum};

mod commands;
mod permission_mode;

pub(crate) use commands::*;
pub(crate) use permission_mode::PermissionModeArg;

/// Luxe's command line. `--help` shows the crate description from `Cargo.toml`, not this doc.
#[derive(Parser, Debug)]
#[command(name = "luxe", version, about)]
pub(crate) struct Args {
    /// Run headless (non-interactive single prompt).
    #[arg(long)]
    pub(crate) headless: bool,

    /// Speak the JSONL front-end protocol on stdio: wire actions in, events out. For editor
    /// plugins, alternative UIs, and anything driving Luxe as a session rather than a one-shot.
    /// See `docs/guide/rpc.md`.
    #[arg(long, conflicts_with = "headless")]
    pub(crate) rpc: bool,

    /// Let an `--rpc` client publish to GitHub: post comments and reviews, answer threads, and open,
    /// change, merge and restack pull requests. Off by default, so a client can read GitHub but not
    /// write to it unless you say so.
    /// A read-only session refuses these writes even with this flag. Governs what the client can do
    /// directly, not the agent, whose GitHub writes still ask first.
    #[arg(long)]
    pub(crate) github_writes: bool,

    /// The prompt to run in headless mode.
    #[arg(short = 'p', long)]
    pub(crate) prompt: Option<String>,

    /// Headless output format: `text` (streamed assistant text) or `json` (a JSONL event
    /// stream with per-turn usage). Ignored outside `--headless`.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub(crate) format: OutputFormat,

    /// Provider to use, by id: `anthropic`, `openai`, `lmstudio`, and so on. Overrides env + config.
    #[arg(long, global = true)]
    pub(crate) provider: Option<String>,

    /// Model to use: an id (`gpt-4o`) or `provider/id` (`anthropic/claude-opus-5-5`), which picks the provider too. Overrides env + config.
    #[arg(long, global = true)]
    pub(crate) model: Option<String>,

    /// API key over env + config; `$ENV`/`!command` work too, and a literal shows in others' `ps`.
    #[arg(long, global = true)]
    pub(crate) api_key: Option<String>,

    /// Base URL override (must include the API version segment, e.g. `.../v1`).
    #[arg(long, global = true)]
    pub(crate) base_url: Option<String>,

    /// Initial reasoning level for models that support it: `off`, `minimal`, `low`, `medium`,
    /// `high`, `xhigh`, or `ultra`. Defaults to `off`. In the TUI, `/thinking` changes it live.
    #[arg(long, global = true)]
    pub(crate) thinking: Option<String>,

    /// Start in plan mode: read, ask, and write only the plan (`plan.files`, `PLAN.md` by default).
    /// In the TUI, shift+tab or `/mode` switches back to build.
    #[arg(long, global = true)]
    pub(crate) plan: bool,

    /// Permission mode: `default`, `strict` (ask before mutating tools), `read-only` (deny
    /// mutating tools), or `yolo` (allow everything, guardrails off). Overrides the config file.
    #[arg(long, value_enum, ignore_case = true, value_parser = permission_mode::PermissionModeParser, global = true)]
    pub(crate) permission_mode: Option<PermissionModeArg>,

    /// Shortcut for `--permission-mode read-only` (look, don't touch).
    #[arg(long, global = true)]
    pub(crate) read_only: bool,

    /// Allow everything, disabling the built-in guardrails. An explicit opt-out; use with care.
    #[arg(long, global = true)]
    pub(crate) yolo: bool,

    /// What an `ask` resolves to in a run with nobody to ask: `allow` (the default) or `deny`.
    /// Applies to `--headless`, `luxe loop` and the review agent, which cannot prompt. `deny`
    /// refuses the call and tells the model the run is unattended, so it takes another route.
    /// Sets `agent.on_ask` for one run; `--rpc` and the TUI reject it, since there you answer.
    #[arg(long, value_enum, global = true, conflicts_with = "auto_approve")]
    pub(crate) on_ask: Option<OnAskArg>,

    /// Start with auto-approve on, for a TUI or RPC session you intend to walk away from: an
    /// `ask` is allowed without a card, every `deny` is still enforced, and the status bar says
    /// so. The same state `/permissions on` reaches. Headless and `luxe loop` allow by default
    /// — see `--on-ask`.
    #[arg(long, global = true)]
    pub(crate) auto_approve: bool,

    /// Trust this project: honor its `.luxe/hooks.json` and `gate.commands`, let its
    /// `.luxe/permissions.json` loosen policy rather than only tighten it, and start the language
    /// servers, formatters and MCP servers it supplies even in a headless, loop or review run.
    #[arg(long, global = true)]
    pub(crate) trust_project: bool,

    /// Leave a tool unregistered for this run, on top of whatever config already disables.
    /// Repeatable: `--no-tool web_search --no-tool web_fetch`. `read`, `write`, `edit` and `bash`
    /// are refused — the agent cannot work without them. See `/tools`.
    #[arg(long = "no-tool", value_name = "TOOL", global = true)]
    pub(crate) no_tool: Vec<String>,

    /// Register a tool your config disables, for this run only. The counterpart to `--no-tool`,
    /// so a standing preference does not have to be edited to make an exception to it.
    #[arg(long = "with-tool", value_name = "TOOL", global = true)]
    pub(crate) with_tool: Vec<String>,

    /// Resume the most recent session for this project.
    #[arg(short = 'c', long = "continue")]
    pub(crate) continue_session: bool,

    /// Resume a specific session by id (see `luxe sessions list`).
    #[arg(long)]
    pub(crate) resume: Option<String>,

    /// Take up issue NUMBER: a new session in its own worktree, on a branch named for the issue,
    /// whose first message carries the issue's words.
    #[arg(long, value_name = "NUMBER", conflicts_with_all = ["resume", "continue_session", "from_pr"])]
    pub(crate) issue: Option<u64>,

    /// Fix pull request NUMBER: a new session with it checked out in its own worktree, where a
    /// plain `git push` lands on it. `--from-pr` resumes one that already worked on it.
    #[arg(
        long,
        value_name = "NUMBER",
        conflicts_with_all = ["resume", "continue_session", "from_pr", "issue", "worktree"]
    )]
    pub(crate) pr: Option<u64>,

    /// Work in a git worktree of this session's own, on BRANCH (a generated `luxe/…` name when
    /// none is given), so the agent's edits never land in your checkout.
    #[arg(long, value_name = "BRANCH", num_args = 0..=1, default_missing_value = "")]
    pub(crate) worktree: Option<String>,

    /// Resume the newest session that worked on this pull request: opened it, pushed to it, or
    /// answered its threads.
    #[arg(long = "from-pr", value_name = "NUMBER", conflicts_with_all = ["resume", "continue_session"])]
    pub(crate) from_pr: Option<u64>,

    /// Loop subcommand for continuous development.
    #[command(subcommand)]
    pub(crate) command: Option<SubCommand>,
}

/// A flag combination `luxe` refuses, worded for the reader: `main` prints it after `luxe:` and
/// exits 1.
#[derive(Debug)]
pub(crate) struct UsageError(String);

impl std::fmt::Display for UsageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

impl Args {
    /// Whether this run owns project state — and so may create `.luxe/` in the project it found.
    ///
    /// The question exists because creating that directory is not free: [`luxe_core::project`] ranks
    /// a directory holding one above a nearer checkout, on purpose, so a `.luxe/` left in `/tmp` by
    /// a command that only wanted somewhere to log re-roots every repository underneath it.
    ///
    /// A session owns project state; so does `luxe loop`, which keeps its run there, and
    /// `luxe mcp add`/`remove`, which write `.luxe/mcp.json`. Everything else reads and reports.
    /// The matches are exhaustive so a new subcommand has to answer this before it compiles.
    pub(crate) fn owns_project_state(&self) -> bool {
        match &self.command {
            // No subcommand is a session: the TUI, `--headless`, or `--rpc`.
            None => true,
            Some(SubCommand::Loop(_)) => true,
            Some(SubCommand::Mcp(mcp)) => match mcp.command {
                McpCommand::Add { .. } | McpCommand::Remove { .. } => true,
                McpCommand::List | McpCommand::Tools | McpCommand::Login { .. } => false,
            },
            Some(
                SubCommand::Auth(_)
                | SubCommand::Docs(_)
                | SubCommand::Doctor
                | SubCommand::Init(_)
                | SubCommand::Join(_)
                | SubCommand::Models(_)
                | SubCommand::Sessions(_)
                | SubCommand::Update(_)
                | SubCommand::Wire(_),
            ) => false,
            #[cfg(feature = "remote")]
            Some(SubCommand::Remote(_)) => false,
        }
    }

    /// Whether a human answers this run's permission prompts — the TUI and `--rpc`.
    ///
    /// What `--on-ask` is *not* for: it settles what an `ask` becomes where there is nobody to ask,
    /// so on these two surfaces it would quietly do nothing while reading like a policy.
    pub(crate) fn prompts_a_human(&self) -> bool {
        self.command.is_none() && !self.headless
    }

    /// Whether `--github-writes` was given anywhere but an `--rpc` session.
    ///
    /// Checked by hand because clap's `requires` cannot see it: a `bool` flag carries an implicit
    /// default, so `rpc` always counts as present. Elsewhere the flag would read as "the agent may
    /// now publish", which it neither says nor does.
    pub(crate) fn github_writes_misplaced(&self) -> bool {
        self.github_writes && !(self.rpc && self.command.is_none())
    }

    /// Refuses a combination of flags that would read as one thing and do another.
    ///
    /// Clap checks what it can see; these it cannot (see [`Self::github_writes_misplaced`]). Asked
    /// first, so a refusal happens before anything has moved, and answered as an error so the run
    /// unwinds through its destructors instead of exiting from the middle of it.
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        if self.on_ask.is_some() && self.prompts_a_human() {
            // Not a warning: the flag reads as a policy and would do nothing here, which is the
            // shape of mistake that is only discovered by an agent doing something unwatched.
            return Err(UsageError(
                "--on-ask settles what an `ask` becomes when nobody can answer it, so it is for \
                 --headless and `luxe loop`. In the TUI and --rpc you answer the prompts \
                 yourself; --auto-approve is the walk-away switch there."
                    .to_string(),
            )
            .into());
        }
        if self.github_writes_misplaced() {
            return Err(UsageError(
                "--github-writes lets an --rpc client publish to GitHub, so it only works with \
                 --rpc. The agent's own GitHub writes are governed by permissions, not this flag."
                    .to_string(),
            )
            .into());
        }
        if self.command.is_none() && self.headless && self.prompt.is_none() {
            return Err(UsageError("--headless requires --prompt <text>".to_string()).into());
        }
        Ok(())
    }

    /// Whether an engine runs, and so whether a startup notice reaches the reader as an event.
    ///
    /// The TUI owns the terminal, so a notice printed to stderr there lands on the drawing surface
    /// and is gone; it goes through [`luxe_core::Event::Notice`] instead, the same path headless and
    /// RPC use. A subcommand has no engine, so its notices are printed.
    pub(crate) fn starts_an_engine(&self) -> bool {
        match &self.command {
            None => true,
            Some(SubCommand::Loop(r#loop)) => matches!(r#loop.command, LoopCommand::Start { .. }),
            Some(
                SubCommand::Auth(_)
                | SubCommand::Docs(_)
                | SubCommand::Doctor
                | SubCommand::Init(_)
                | SubCommand::Join(_)
                | SubCommand::Mcp(_)
                | SubCommand::Models(_)
                | SubCommand::Sessions(_)
                | SubCommand::Update(_)
                | SubCommand::Wire(_),
            ) => false,
            #[cfg(feature = "remote")]
            Some(SubCommand::Remote(_)) => false,
        }
    }

    /// Collect the provider/model/auth overrides supplied on the command line.
    pub(crate) fn overrides(&self) -> crate::engine_setup::CliOverrides {
        crate::engine_setup::CliOverrides {
            provider: self.provider.clone(),
            model: self.model.clone(),
            api_key: self.api_key.clone(),
            base_url: self.base_url.clone(),
        }
    }
}

/// Parse a `--thinking` level name into a `ThinkingLevel`, accepting the same spellings Luxe
/// displays (`off`..`ultra`, plus `max` as an alias for `ultra`). Returns `None` for unknown input.
pub(crate) fn parse_thinking_level(raw: &str) -> Option<luxe_core::provider::ThinkingLevel> {
    let name = raw.trim().to_ascii_lowercase();
    if name == "max" {
        return Some(luxe_core::provider::ThinkingLevel::Ultra);
    }
    luxe_core::provider::ThinkingLevel::ALL.into_iter().find(|level| level.as_str() == name)
}

/// `--on-ask`, which is [`luxe_core::config::OnAsk`] with clap's spellings.
///
/// A `ValueEnum` rather than a parsed string, deliberately unlike `--thinking`: that warns and
/// carries on, which is right for a preference and wrong for a policy. `--on-ask dney` has
/// to stop the run, because ignoring it leaves an unattended agent approving its own escalations
/// while its operator believes otherwise.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OnAskArg {
    /// Let an unattended `ask` through, the default.
    Allow,
    /// Refuse it, and tell the model why.
    Deny,
}

impl From<OnAskArg> for luxe_core::config::OnAsk {
    fn from(arg: OnAskArg) -> Self {
        match arg {
            OnAskArg::Allow => Self::Allow,
            OnAskArg::Deny => Self::Deny,
        }
    }
}

/// Headless output format.
#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) enum OutputFormat {
    /// Stream assistant text to stdout; tool activity to stderr (human-friendly).
    #[default]
    Text,
    /// Emit a JSONL event stream (one object per line) with per-turn usage — for scripts,
    /// harness adapters, and benchmarks.
    Json,
}

/// Resolve the permission-mode override from the flags, highest precedence first:
/// `--yolo` > `--read-only` > `--permission-mode <name>`. Returns `None` to defer to the config
/// file / built-in default. Clap rejects unrecognized mode names before startup.
pub(crate) fn permission_mode_from_args(
    args: &Args,
) -> Option<luxe_core::permission::PermissionMode> {
    use luxe_core::permission::PermissionMode;
    if args.yolo {
        return Some(PermissionMode::Yolo);
    }
    if args.read_only {
        return Some(PermissionMode::ReadOnly);
    }
    args.permission_mode.map(Into::into)
}

/// Map the resume flags (`--resume <id>` / `-c/--continue`) to a `ResumeMode`.
pub(crate) fn resume_from_args(args: &Args) -> crate::engine_setup::ResumeMode {
    use crate::engine_setup::ResumeMode;
    if let Some(id) = args.resume.clone() {
        ResumeMode::Session(id)
    } else if let Some(number) = args.from_pr {
        ResumeMode::PullRequest(number)
    } else if args.continue_session {
        ResumeMode::Latest
    } else {
        ResumeMode::New
    }
}

#[cfg(test)]
mod tests;
