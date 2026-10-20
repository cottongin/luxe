//! Default permission ruleset — permissive, with a few high-confidence guardrails.
//!
//! Out of the box luxe allows everything (parity with Pi's frictionless feel), then layers a
//! small set of guards a more specific rule can always override:
//!
//! - **`.env` reads** are denied (`.env.example` allowed) so a stray read can't leak secrets.
//! - **`external_directory`** defaults to `ask`: work outside the project root prompts first
//!   (auto-approved in headless/loop).
//! - **`doom_loop`** is denied: the same call a third time in a row is refused, with a message that
//!   it already ran ([`DOOM_LOOP`]).
//! - A curated **catastrophic bash deny-list**: irreversible, never-part-of-a-repo-task commands
//!   (root/home/system-dir wipes, `mkfs`, `dd` to a device). These are a guardrail, not a security
//!   boundary — a determined model can obfuscate around bash matching; real isolation is a
//!   container (docs/guide/security.md).
//! - **Running as root** (`sudo`, `doas`, `su` and the like) is denied: the person runs those.
//! - Force-push defaults to `ask` (dangerous but sometimes intended).
//! - **Committing** defaults to `ask`, and for a different reason than the rest of this list.

use super::request::FORKBOMB;
use super::{rule, PermissionAction, Ruleset};

/// A named permission preset selected by config or a CLI flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PermissionMode {
    /// Permissive base + guardrails (`.env` deny, external ask, doom-loop and catastrophic deny).
    #[default]
    Default,
    /// "Always ask" before any mutating tool (guardrails still apply).
    Strict,
    /// Deny every mutating tool; reads still allowed.
    ReadOnly,
    /// Allow everything, guardrails OFF (explicit opt-out).
    Yolo,
}

impl PermissionMode {
    /// Parse a mode name (`default`/`strict`/`read-only`/`yolo`), case- and separator-insensitive.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().replace(['_', ' '], "-").as_str() {
            "default" => Some(Self::Default),
            "strict" => Some(Self::Strict),
            "read-only" | "readonly" | "ro" => Some(Self::ReadOnly),
            "yolo" => Some(Self::Yolo),
            _ => None,
        }
    }

    /// The stable name for display/serialization.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Strict => "strict",
            Self::ReadOnly => "read-only",
            Self::Yolo => "yolo",
        }
    }
}

/// Guard key: fires when a tool touches a path outside the project working directory.
pub const EXTERNAL_DIRECTORY: &str = "external_directory";

/// Guard key: fires when the same tool call repeats identically (a runaway loop).
///
/// Denied rather than asked, because a repeat is not a question of permission — the call already
/// ran and produced that result. An `ask` would also be a no-op where a runaway is most expensive:
/// headless and loop runs auto-allow it. A deny reaches the model as feedback it can act on. Retune
/// it in `permissions.json` if you would rather be prompted.
pub const DOOM_LOOP: &str = "doom_loop";

/// Guard key: fires when a `BeforeToolUse` command hook answers `ask`.
///
/// A hook escalation is a *check*, not a second prompt. Routing it here is what gives it the
/// resolution rules everything else already has — `AskResolution` collapse in unattended runs,
/// the front-end prompt, strictest-wins against the tool's own rules — instead of a parallel path
/// that would have to reimplement them and would drift.
///
/// It also leaves the reader in charge of a hook they installed: `{"hook_ask": "allow"}` in
/// `permissions.json` silences every escalation, and `"deny"` promotes them all to hard blocks.
pub const HOOK_ASK: &str = "hook_ask";

/// System directories and their descendants aren't valid repository cleanup targets.
const PROTECTED_ROOTS: &[&str] = &[
    "/", "/etc", "/usr", "/bin", "/sbin", "/boot", "/dev", "/lib", "/lib64", "/sys", "/var",
    "/root",
];

pub(crate) const CATASTROPHIC_RM: &str = "catastrophic_rm";

pub(crate) fn is_catastrophic_rm(command: &str) -> bool {
    super::wildcard::command_segments(command)
        .iter()
        .any(|segment| catastrophic_rm_segment(segment))
}

fn catastrophic_rm_segment(segment: &[String]) -> bool {
    if segment.first().map(String::as_str) != Some("rm") {
        return false;
    }
    let (mut options, mut recursive, mut dangerous) = (true, false, false);
    for argument in &segment[1..] {
        if options && argument == "--" {
            options = false;
            continue;
        }
        if options && argument.starts_with('-') {
            recursive |= recursive_rm_flag(argument);
            dangerous |= argument.starts_with("--no-preserve-root");
            continue;
        }
        dangerous |= catastrophic_rm_target(argument);
    }
    recursive && dangerous
}

fn recursive_rm_flag(flag: &str) -> bool {
    flag == "--recursive"
        || (flag.starts_with('-') && !flag.starts_with("--") && flag.contains(['r', 'R']))
}

fn catastrophic_rm_target(target: &str) -> bool {
    let target = target.trim_matches(['\'', '"']);
    if matches!(target, "/" | "/*") {
        return true;
    }
    let target = target.trim_end_matches('/');
    matches!(target, "~" | "~/*" | "$HOME" | "${HOME}" | "$HOME/*" | "${HOME}/*")
        || PROTECTED_ROOTS.iter().any(|root| {
            *root != "/" && (target == *root || target.starts_with(&format!("{root}/")))
        })
}

/// Bash commands denied by default — irreversible and never part of a normal repo workflow.
fn catastrophic_bash() -> Vec<String> {
    vec![
        "mkfs*".to_string(),
        "mkfs.* *".to_string(),
        "dd of=/dev/*".to_string(),
        "dd * of=/dev/*".to_string(),
    ]
}

/// Dangerous-but-sometimes-intended bash commands that prompt first.
const DANGEROUS_ASK_BASH: &[&str] = &[
    // Star before and after the flag so it matches `--force` anywhere in the push args.
    "git push * --force *",
    "git push * -f *",
];

/// Commands that run something as another user, root in practice. Denied: the person runs them.
///
/// A command that needs root needs the reader's attention and consent, and typing it into their own
/// terminal is how they give both. Asking first did not help either way: approved, `sudo` wrote its
/// password prompt over the TUI and stopped until the timeout killed it, and with no terminal at all
/// (headless, the loop) a failed PAM conversation can count toward an account lockout. So it never
/// starts, and the refusal tells the agent to hand the command over instead (`runs_as_root`).
///
/// A person can still decide otherwise ahead of time, like any guardrail: an `allow` rule in the
/// global permissions file, which only works with passwordless sudo, or `--yolo`.
const RUN_AS_ROOT: &[&str] =
    &["sudo *", "doas *", "su *", "pkexec *", "run0 *", "gsudo *", "runas *"];

/// Whether a request runs a command as root, for the refusal that says what to do instead.
pub(crate) fn runs_as_root(request: &super::PermissionRequest) -> bool {
    request.checks.iter().any(|check| {
        check.permission == "bash"
            && RUN_AS_ROOT.iter().any(|pattern| super::input_matches("bash", &check.input, pattern))
    })
}

/// Capture modes that photograph the reader's screen rather than a page luxe rendered.
///
/// The most out-of-project thing any tool does — whatever is on the display, which is not the
/// project and may not even be the reader's to share: a password manager, a private message, a
/// colleague on a shared screen. It then travels inline to a model provider. Merely *reading* a
/// file outside the project root is already an `ask`, so this could not stay an outright allow.
///
/// `mode=url` is deliberately absent. It captures a page luxe was asked to load, the URL is a
/// thing a rule can be written about, and it is the mode most visual work actually wants.
const SCREEN_CAPTURE_ASK: &[&str] = &["fullscreen", "region", "window"];

/// Commit patterns that prompt first, one per route to the same act.
///
/// Unlike everything above, a commit is not dangerous — it is *the reader's move*. luxe's whole
/// review flow is built on the working tree: the Context Dock lists what changed, and a guided
/// review turns its chapters into the commits. An agent that commits at the end of its turn takes
/// that away silently — `HEAD` moves, the diff against it empties, and the CHANGES section the
/// reviewer was about to open goes blank with no sign anything was there.
///
/// Both routes are covered because a model refused one takes the other: a commit the `git` tool
/// refuses comes back through `bash` in the next call. A guard on one door only teaches which door
/// to use.
///
/// The prompt is cheap to retire and the answer is real: "always" writes the rule to the global
/// permissions file, and `--yolo` or a `"git": {"commit*": "allow"}` in config turns it off for
/// good. Unattended runs auto-allow an `ask`, so headless and loop work is untouched.
const COMMIT_ASK: &[(&str, &str)] = &[
    // The `git` tool's input is `"<action> <args>"`, so this covers `commit` and `commit --amend`.
    ("git", "commit*"),
    // Bash matching is segment-aware, so this also catches the commit buried in
    // `cd x && git add -A && git commit -m …` — which is the exact line that ran.
    ("bash", "git commit *"),
];

/// GitHub writes that prompt first, on both routes to them.
///
/// A comment, a review, a merge or a closed issue is public the moment it lands, other people are
/// notified, and some of it cannot be taken back. The `git` tool's input is `"<action> <args>"`, so
/// its `pr` writes read `pr merge 3`; `bash` matching is segment-aware, so a `gh pr merge` at the
/// end of a `&&` chain is still caught. Reads (`pr list`, `pr view`, `pr diff`, `pr checks`,
/// `issue view`) stay free. Re-running CI is here too: on a private repository it spends Actions
/// minutes, which cost money past the plan's allowance.
const GITHUB_WRITE_ASK: &[(&str, &str, Option<&str>)] = &[
    ("git", "pr create*", None),
    ("git", "pr merge*", Some("Merging a pull request")),
    ("git", "pr close*", Some("Closing a pull request")),
    ("git", "pr reopen*", None),
    ("git", "pr comment*", None),
    ("git", "pr review*", Some("Submitting a pull request review")),
    ("git", "pr edit*", None),
    ("git", "pr ready*", None),
    ("bash", "gh pr create *", None),
    ("bash", "gh pr merge *", Some("Merging a pull request")),
    ("bash", "gh pr close *", Some("Closing a pull request")),
    ("bash", "gh pr reopen *", None),
    ("bash", "gh pr comment *", None),
    ("bash", "gh pr review *", Some("Submitting a pull request review")),
    ("bash", "gh pr edit *", None),
    ("bash", "gh pr ready *", None),
    ("bash", "gh issue create *", None),
    ("bash", "gh issue comment *", None),
    ("bash", "gh issue close *", Some("Closing an issue")),
    ("bash", "gh issue reopen *", None),
    ("bash", "gh issue edit *", None),
    ("bash", "gh run rerun *", None),
    ("bash", "gh workflow run *", None),
];

/// The act a request performs that needs a person to approve it, if any.
///
/// An `ask` resolves through `agent.on_ask` where nobody can answer it, and that defaults to allow,
/// which is right for the loop opening a pull request and wrong for it merging one. These are the
/// acts that should not happen with nobody watching: the engine refuses them on the one path where
/// an `ask` would otherwise auto-allow.
///
/// Checked by the engine only where an `ask` would auto-allow, so it never overrides a decision
/// somebody made: an `allow` rule makes the request `Allow` before this is consulted.
pub(crate) fn needs_a_person(request: &super::PermissionRequest) -> Option<&'static str> {
    request.checks.iter().find_map(|check| {
        GITHUB_WRITE_ASK.iter().find_map(|(key, pattern, act)| {
            let act = (*act)?;
            (check.permission == *key && super::input_matches(key, &check.input, pattern))
                .then_some(act)
        })
    })
}

/// The default ruleset: a permissive base plus the guardrails above.
pub fn default_ruleset() -> Ruleset {
    use PermissionAction::{Allow, Ask, Deny};
    let mut rules = vec![
        // Permissive base: anything not covered by a more specific guard is allowed.
        rule("*", "*", Allow),
        // Secret hygiene.
        rule("read", "*.env", Deny),
        rule("read", "*.env.*", Deny),
        rule("read", "*.env.example", Allow),
        // Guards.
        rule(EXTERNAL_DIRECTORY, "*", Ask),
        rule(DOOM_LOOP, "*", Deny),
        rule(HOOK_ASK, "*", Ask),
        rule(FORKBOMB, "*", Deny),
        rule(CATASTROPHIC_RM, "*", Deny),
    ];
    // Catastrophic bash — deny; dangerous-but-sometimes-intended bash — ask.
    rules.extend(catastrophic_bash().iter().map(|p| rule("bash", p, Deny)));
    rules.extend(DANGEROUS_ASK_BASH.iter().map(|p| rule("bash", p, Ask)));
    rules.extend(RUN_AS_ROOT.iter().map(|p| rule("bash", p, Deny)));
    rules.extend(COMMIT_ASK.iter().map(|(key, pattern)| rule(key, pattern, Ask)));
    rules.extend(GITHUB_WRITE_ASK.iter().map(|(key, pattern, _)| rule(key, pattern, Ask)));
    rules.extend(SCREEN_CAPTURE_ASK.iter().map(|mode| rule("screenshot", mode, Ask)));
    rules
}

const MUTATING_TOOLS: [&str; 4] = ["edit", "bash", "git", "debug"];

/// A `read-only` overlay: deny the mutating tools (reads still allowed by the base).
pub(super) fn readonly_overlay() -> Ruleset {
    MUTATING_TOOLS.into_iter().map(|tool| rule(tool, "*", PermissionAction::Deny)).collect()
}

/// A `strict` ("always ask") overlay: prompt before every mutating tool.
pub(super) fn strict_overlay() -> Ruleset {
    MUTATING_TOOLS.into_iter().map(|tool| rule(tool, "*", PermissionAction::Ask)).collect()
}

#[cfg(test)]
mod tests;
