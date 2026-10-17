//! Git — the repository operations the schema lists, and `gh`-backed pull requests, issues and CI
//! checks.
//!
//! One `action` picks the operation and `args` carries git's own flags for it. A commit message is
//! its own argument rather than another flag in that list, because the one thing a commit cannot be
//! spelled without should not be the thing a caller has to guess at. [`args`] does the accepting;
//! this module plans each action and tells its answer, and `run` does the running.

mod args;
mod checks;
mod failure;
mod run;

use args::{commit_args, gh_args, is_pr_create, stash_args};
use async_trait::async_trait;
use failure::explain_gh_failure;
use luxe_core::provider::ToolSchema;
use luxe_core::tool::{Tool, ToolContext, ToolResultData};
use std::path::Path;

/// Max lines of `gh pr` output returned before the result is capped.
///
/// `pr list` in a busy repository is unbounded, and a `pr view` carries a whole PR body written by
/// somebody who likes templates. Neither is worth the context it would otherwise take.
const GH_OUTPUT_MAX_LINES: usize = 120;

/// Max `git diff` lines returned before the result is capped (the guided review is the full view).
const GIT_DIFF_MAX_LINES: usize = 400;
/// Max `git log` lines returned before the result is capped.
const GIT_LOG_MAX_LINES: usize = 200;

/// Commits a bare `log` shows, most recent first.
const LOG_DEFAULT_COUNT: usize = 10;

/// Git: the actions the schema lists, with `gh` behind the pull request, issue and CI ones.
///
/// Stateless: whether `gh` can run is a question about the machine, asked at the call, not a flag
/// held on the tool.
#[derive(Default)]
pub(crate) struct GitTool;

impl GitTool {
    pub(crate) fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GitTool {
    fn name(&self) -> &str {
        "git"
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "git".to_string(),
            description: "Git repository operations with bounded output: status, diff, log, \
                          show (a commit, or a file at a revision: [\"<rev>:<path>\"]), add, \
                          commit, branch, checkout, stash, `gh`-backed pull requests and issues \
                          (reading one with its comments is `issue` with [\"view\", \"12\", \
                          \"--comments\"]), and \
                          `checks`: why CI fails on the branch's pull request, with each failing \
                          step's log tail. A commit's message goes in `message`, not `args`. Use \
                          `bash` for git subcommands this does not cover."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["status", "diff", "log", "show", "add", "commit", "branch", "checkout", "stash", "pr", "issue", "checks"],
                        "description": "Git action to perform"
                    },
                    "message": {
                        "type": "string",
                        "description": "Commit message, subject line first. Required for `commit` unless `args` already say where the message comes from (--amend --no-edit, -F, -C, --fixup)."
                    },
                    "args": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "What the action operates on: paths for `add`, a branch for `checkout`, a subcommand for `stash`, a `gh pr` command line for `pr` (e.g. [\"create\", \"--title\", \"...\", \"--body\", \"...\"]; the branch is pushed for you if it has no upstream), a `gh issue` command line for `issue` (e.g. [\"view\", \"12\", \"--comments\"]), a PR number for `checks` (default: this branch's), otherwise git's own flags. Never the commit message."
                    }
                },
                "required": ["action"]
            }),
        }
    }

    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        let action = match args.get("action") {
            Some(v) => v.as_str().unwrap_or(""),
            None => {
                return ToolResultData::error("Error: 'action' argument is required");
            }
        };
        let Some(action) = GitAction::parse(action) else {
            return ToolResultData::error(format!(
                "Unknown action: {action}. Valid actions: {}",
                GitAction::ALL.map(GitAction::name).join(", ")
            ));
        };
        let request: Vec<String> = args
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        match action {
            GitAction::Checks => {
                let github = luxe_core::github::GhCli::new();
                match checks::checks(&github, &cx.cwd, &request).await {
                    Ok(report) => ToolResultData::text(report),
                    Err(reason) => ToolResultData::error(format!("Error: {reason}")),
                }
            }
            GitAction::Pr | GitAction::Issue => self.run_github(action, &request, &cx.cwd).await,
            _ => {
                let plan = match plan(action, &args, &request) {
                    Ok(plan) => plan,
                    Err(refusal) => return refusal,
                };
                let argv: Vec<&str> = plan.argv.iter().map(String::as_str).collect();
                finish(action, self.run_git(&cx.cwd, &argv).await, plan.shape)
            }
        }
    }
}

impl GitTool {
    /// `pr` and `issue`: the `gh` command line, with the branch published first when a pull request
    /// is being opened.
    async fn run_github(
        &self,
        action: GitAction,
        request: &[String],
        cwd: &Path,
    ) -> ToolResultData {
        // Asked of the machine, not of a config flag: `gh` is an external program the user
        // installs, the same as a language server or a debug adapter, and the useful answer
        // names it rather than an internal switch.
        if !luxe_core::program::is_available("gh") {
            return ToolResultData::error(
                "Error: the GitHub CLI (`gh`) is not on PATH. Install it and run \
                 `gh auth login`, or drive it through `bash` yourself.",
            );
        }

        // Opening a pull request needs a published branch; reading about one does not.
        let mut preamble = None;
        if action == GitAction::Pr && is_pr_create(request) {
            match self.ready_branch_for_pr(cwd).await {
                Ok(note) => preamble = note,
                Err(e) => {
                    return ToolResultData::error(format!("Cannot open a pull request: {e}"));
                }
            }
        }

        let args = gh_args(action.name(), request);
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        match self.run_gh(cwd, &refs).await {
            Ok(output) => {
                let body = crate::output_limit::cap_lines(
                    output.trim(),
                    GH_OUTPUT_MAX_LINES,
                    "lines of gh output",
                    "narrow it with gh's own flags (--limit, --json)",
                );
                let text = match preamble {
                    Some(note) => format!("{note}\n{body}"),
                    None => body,
                };
                ToolResultData::text(or_else_message(&text, "gh reported nothing"))
            }
            Err(e) => {
                let reported = e.to_string();
                let name = action.name();
                ToolResultData::error(match explain_gh_failure(&reported) {
                    Some(explanation) => format!("gh {name} failed: {explanation}"),
                    None => format!("gh {name} failed: {reported}"),
                })
            }
        }
    }
}

/// What the tool can be asked to do: the `action` enum of the schema, and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitAction {
    Status,
    Diff,
    Log,
    Show,
    Add,
    Commit,
    Branch,
    Checkout,
    Stash,
    Pr,
    Issue,
    Checks,
}

impl GitAction {
    /// Every action, in the order the schema lists them.
    const ALL: [Self; 12] = [
        Self::Status,
        Self::Diff,
        Self::Log,
        Self::Show,
        Self::Add,
        Self::Commit,
        Self::Branch,
        Self::Checkout,
        Self::Stash,
        Self::Pr,
        Self::Issue,
        Self::Checks,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Diff => "diff",
            Self::Log => "log",
            Self::Show => "show",
            Self::Add => "add",
            Self::Commit => "commit",
            Self::Branch => "branch",
            Self::Checkout => "checkout",
            Self::Stash => "stash",
            Self::Pr => "pr",
            Self::Issue => "issue",
            Self::Checks => "checks",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.name() == raw)
    }
}

/// One git invocation and how its answer is told.
struct Plan {
    argv: Vec<String>,
    shape: Shape,
}

/// How a git command's stdout becomes the result.
enum Shape {
    /// As git printed it
    Raw,
    /// Trimmed
    Trimmed,
    /// As printed, or this sentence when there was nothing
    OrMessage(&'static str),
    /// A fixed sentence: git said nothing worth reading
    Said(&'static str),
    /// Capped at `max` lines, naming what was counted and what to do about it
    Capped { max: usize, noun: &'static str, hint: &'static str, empty: Option<&'static str> },
    /// The default `log`, which asked for one more than it shows so it can say whether history
    /// goes on
    LogDefault,
}

/// The git command line for `action`, or the refusal a caller should see instead.
///
/// `request` is what the caller put in `args`. Every action here is a git subcommand; `pr`, `issue`
/// and `checks` go to `gh` and have their own paths.
fn plan(
    action: GitAction,
    args: &serde_json::Value,
    request: &[String],
) -> Result<Plan, ToolResultData> {
    let with = |subcommand: &str, default: &[&str]| {
        let mut argv = vec![subcommand.to_string()];
        if request.is_empty() {
            argv.extend(default.iter().map(|arg| arg.to_string()));
        } else {
            argv.extend(request.iter().cloned());
        }
        argv
    };
    let needs = |what: &str| ToolResultData::error(format!("Error: 'args' must include {what}"));
    let (argv, shape) = match action {
        GitAction::Status => (
            vec!["status".to_string(), "--short".to_string()],
            Shape::OrMessage("clean — nothing to commit, working tree clean"),
        ),
        GitAction::Diff => (
            with("diff", &["--unified=3"]),
            Shape::Capped {
                max: GIT_DIFF_MAX_LINES,
                noun: "diff lines",
                hint:
                    "Pass a path to scope the diff, or `--stat` first to see which files changed.",
                empty: Some("no changes — the diff is empty"),
            },
        ),
        GitAction::Show => (
            with("show", &[]),
            Shape::Capped {
                max: GIT_DIFF_MAX_LINES,
                noun: "lines",
                hint: "Pass `--stat` to see which files a commit changed, then show one \
                       path at a time (`<rev> -- <path>`, or `<rev>:<path>` for the whole \
                       file).",
                empty: None,
            },
        ),
        GitAction::Log if request.is_empty() => {
            (vec!["log".to_string(), "--oneline".to_string(), "-11".to_string()], Shape::LogDefault)
        }
        GitAction::Log => (
            with("log", &[]),
            Shape::Capped {
                max: GIT_LOG_MAX_LINES,
                noun: "log lines",
                hint: "Pass a narrower range or -n <count> to see fewer commits.",
                empty: Some("no commits matched"),
            },
        ),
        GitAction::Add if request.is_empty() => return Err(needs("file paths to add")),
        GitAction::Add => (with("add", &[]), Shape::Said("Files staged.")),
        GitAction::Commit => match commit_args(args, request) {
            Ok(argv) => (argv, Shape::Trimmed),
            Err(error) => return Err(ToolResultData::error(error.message())),
        },
        GitAction::Branch => (with("branch", &["-a"]), Shape::Raw),
        GitAction::Checkout if request.is_empty() => return Err(needs("branch name")),
        GitAction::Checkout => (with("checkout", &[]), Shape::Trimmed),
        GitAction::Stash => (stash_args(request), Shape::Trimmed),
        GitAction::Pr | GitAction::Issue | GitAction::Checks => {
            return Err(ToolResultData::error(format!(
                "Error: `{}` goes to `gh`, not to git",
                action.name()
            )))
        }
    };
    Ok(Plan { argv, shape })
}

/// The result of running a planned command: its output told as its shape says, or its failure.
fn finish(action: GitAction, ran: anyhow::Result<String>, shape: Shape) -> ToolResultData {
    match ran {
        Ok(output) => ToolResultData::text(shape.tell(&output)),
        Err(error) => ToolResultData::error(format!("Git {} failed: {error}", action.name())),
    }
}

impl Shape {
    fn tell(&self, output: &str) -> String {
        match self {
            Self::Raw => output.to_string(),
            Self::Trimmed => output.trim().to_string(),
            Self::OrMessage(fallback) => or_else_message(output, fallback),
            Self::Said(text) => (*text).to_string(),
            Self::Capped { max, noun, hint, empty } => match empty {
                Some(message) if output.trim().is_empty() => (*message).to_string(),
                _ => crate::output_limit::cap_lines(output, *max, noun, hint),
            },
            Self::LogDefault if output.trim().is_empty() => "no commits matched".to_string(),
            Self::LogDefault if output.lines().count() > LOG_DEFAULT_COUNT => {
                let shown: Vec<&str> = output.lines().take(LOG_DEFAULT_COUNT).collect();
                format!(
                    "{}\n… older commits not shown. Pass `-n <count>`, a range such as \
                     `main..HEAD`, or a path for more.",
                    shown.join("\n")
                )
            }
            Self::LogDefault => crate::output_limit::cap_lines(
                output,
                GIT_LOG_MAX_LINES,
                "log lines",
                "Pass a narrower range or -n <count> to see fewer commits.",
            ),
        }
    }
}

/// Use `fallback` when a git command produced nothing: an empty tool result reads as "did this
/// even run?" in a transcript.
fn or_else_message(output: &str, fallback: &str) -> String {
    if output.trim().is_empty() {
        fallback.to_string()
    } else {
        output.to_string()
    }
}

#[cfg(test)]
mod tests;
