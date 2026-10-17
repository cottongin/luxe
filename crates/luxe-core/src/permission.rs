//! Permission engine — allow/ask/deny for tool calls.
//!
//! Luxe is permissive by default; this engine is the seatbelt. It borrows OpenCode's shape
//! (per-tool `allow`/`ask`/`deny` rules keyed by a concrete input, plus the `external_directory`
//! and `doom_loop` guards) and improves on it in two ways:
//!
//! - **Order-independent matching.** Rather than "last rule in config order wins" (fragile — JSON
//!   object order isn't preserved through `serde_json`), the **most specific** matching rule wins,
//!   with the stricter action breaking ties. A literal beats a glob; an exact tool key beats `*`.
//! - **Tighten-only project config.** A project-local ruleset (`.luxe/permissions.json`, shipped
//!   by a possibly-untrusted repo) can only make a decision *stricter* than the trusted baseline
//!   (`deny > ask > allow`); it can never loosen it, unless the user explicitly trusts the project.
//!
//! Evaluation flows `Check → decide_one → decide` (fold to the strictest across a call's checks).
//! Per-tool extraction lives in [`build_request`]; the runtime ask round-trip lives in
//! the engine.

mod broker;
mod config;
pub(crate) mod defaults;
mod request;
pub(crate) mod wildcard;

pub use broker::{PermissionBroker, PermissionRequestId};
pub use config::{persist_rules, PermissionsFile};
pub use defaults::{default_ruleset, PermissionMode, DOOM_LOOP, EXTERNAL_DIRECTORY, HOOK_ASK};
pub use request::{
    build_request, external_target, is_mutating_path_tool, permission_key, primary_input,
    screenshot_mode, Check, PermissionRequest,
};

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What to do with a tool call: run it, prompt first, or block it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionAction {
    Allow,
    Ask,
    Deny,
}

impl PermissionAction {
    /// Ordering used for "stricter wins": `deny (2) > ask (1) > allow (0)`.
    fn strictness(self) -> u8 {
        match self {
            PermissionAction::Allow => 0,
            PermissionAction::Ask => 1,
            PermissionAction::Deny => 2,
        }
    }

    /// The stricter of two decisions. This is how project config *tightens* the baseline and how
    /// a call's multiple checks fold together (any `deny` denies; any `ask` asks).
    pub fn strictest(self, other: Self) -> Self {
        if other.strictness() > self.strictness() {
            other
        } else {
            self
        }
    }
}

/// The user's answer to an `ask` prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionReply {
    /// Allow just this call.
    Once,
    /// Allow this and future matching calls for the rest of the session.
    Always,
    /// Allow it for this session *and* write the rule to the global permissions file, so the
    /// approval survives a restart.
    AlwaysPersist,
    /// Block this call.
    Reject,
}

/// How the engine resolves an `ask` decision at runtime.
///
/// A front-end that can prompt uses [`AskResolution::Prompt`]; headless/loop runs can't block on a
/// human, so they collapse `ask` to [`AskResolution::AutoAllow`] while still enforcing every
/// `deny`. `OnAsk::Deny` resolves to [`AskResolution::AutoDeny`], so a config can choose it too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskResolution {
    /// Emit `PermissionRequested` and wait for the user's reply.
    Prompt,
    /// Auto-approve (never blocks; `deny` still enforced).
    AutoAllow,
    /// Auto-deny (strict unattended).
    AutoDeny,
}

/// The engine's initial permission configuration: the policy plus how to resolve an `ask`.
///
/// Carried on `EngineConfig`. Defaults to permissive-with-guardrails + [`AskResolution::AutoAllow`]
/// so embedders and tests never block; the CLI sets [`AskResolution::Prompt`] for the TUI.
#[derive(Debug, Clone)]
pub struct PermissionSettings {
    pub engine: PermissionEngine,
    pub ask: AskResolution,
}

impl Default for PermissionSettings {
    fn default() -> Self {
        Self { engine: PermissionEngine::permissive(), ask: AskResolution::AutoAllow }
    }
}

/// One rule: for tool key `permission`, inputs matching `pattern` resolve to `action`.
#[derive(Debug, Clone)]
pub struct Rule {
    pub permission: String,
    pub pattern: String,
    pub action: PermissionAction,
}

/// A flat, evaluated-together set of rules.
pub type Ruleset = Vec<Rule>;

/// Build a rule (terse helper for default/preset rulesets).
pub(crate) fn rule(permission: &str, pattern: &str, action: PermissionAction) -> Rule {
    Rule { permission: permission.to_string(), pattern: pattern.to_string(), action }
}

/// How specific a glob is — literals beat wildcards, longer beats shorter. Used to pick the
/// winning rule deterministically regardless of config order.
fn specificity(pattern: &str) -> usize {
    if pattern == "*" {
        return 0;
    }
    pattern.chars().filter(|&c| c != '*' && c != '?').count() + 1
}

/// Does a rule's `permission` key apply to `permission`? Exact match or `*`.
fn permission_applies(permission: &str, rule_permission: &str) -> bool {
    rule_permission == "*" || rule_permission == permission
}

/// Does `input` match `rule_pattern` for this permission key? `bash` uses command-aware matching
/// (segments + head/args); everything else is a plain glob over the input string.
///
/// Shared with command-hook matchers so a `hooks.json` pattern means exactly what the same
/// pattern means in `permissions.json` — one pattern language to learn, and the segment-aware
/// bash handling that a second implementation would have had to rediscover.
pub(crate) fn input_matches(permission: &str, input: &str, rule_pattern: &str) -> bool {
    if rule_pattern == "*" {
        return true; // catch-all matches any input, including multi-token bash commands
    }
    if permission == "bash" {
        wildcard::matches_command(input, rule_pattern)
    } else {
        wildcard::matches(input, rule_pattern)
    }
}

/// Evaluate one `(permission, input)` against a ruleset, returning the most specific match, or
/// `None` when nothing matches. Ties in specificity are broken by order — the later rule wins
/// (defaults first, then global config, then a trusted project overlay), so an explicit user rule
/// overrides a default. The tighten-only guarantee for *untrusted* project config is enforced
/// separately by the `strictest()` combine in `PermissionEngine::decide_one`, not here.
pub fn evaluate(permission: &str, input: &str, rules: &[Rule]) -> Option<PermissionAction> {
    rules
        .iter()
        .filter(|r| {
            permission_applies(permission, &r.permission)
                && input_matches(permission, input, &r.pattern)
        })
        // `max_by_key` returns the LAST element among equal keys, giving later rules the tie-break.
        .max_by_key(|r| ((r.permission != "*") as usize, specificity(&r.pattern)))
        .map(|r| r.action)
}

/// Config shape for the `permission` key (mirrors OpenCode): either a single action applied to
/// everything, or a map of tool key → (scalar action | pattern→action map).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PermissionConfig {
    /// `"permission": "allow"` — one action for every tool.
    All(PermissionAction),
    /// `"permission": { "bash": { "rm *": "deny" }, "edit": "allow", ... }`.
    Keyed(BTreeMap<String, PermValue>),
}

/// A tool's config value: a single action or a per-pattern map.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PermValue {
    Scalar(PermissionAction),
    Map(BTreeMap<String, PermissionAction>),
}

impl PermissionConfig {
    /// Flatten config into rules, expanding `~`/`$HOME` at the start of a pattern.
    pub fn to_ruleset(&self) -> Ruleset {
        match self {
            PermissionConfig::All(action) => vec![rule("*", "*", *action)],
            PermissionConfig::Keyed(map) => {
                let mut out = Vec::new();
                for (key, value) in map {
                    match value {
                        PermValue::Scalar(action) => out.push(rule(key, "*", *action)),
                        PermValue::Map(patterns) => {
                            for (pattern, action) in patterns {
                                out.push(rule(key, &expand_home(pattern), *action));
                            }
                        }
                    }
                }
                out
            }
        }
    }
}

/// Expand a leading `~`/`$HOME` in a pattern to the user's home directory.
///
/// Through [`crate::user_dirs::home_dir`] rather than `$HOME`, which Windows does not set: a `~/...`
/// rule would stay a literal tilde and match nothing. The expansion is rendered with forward
/// slashes to match the rest of the pattern it is spliced into, and the inputs it is compared against.
fn expand_home(pattern: &str) -> String {
    let home = || crate::user_dirs::home_dir().map(|path| crate::paths::display(&path));
    if pattern == "~" {
        return home().unwrap_or_else(|| pattern.to_string());
    }
    if let Some(rest) = pattern.strip_prefix("~/") {
        if let Some(h) = home() {
            return format!("{}/{}", h.trim_end_matches('/'), rest);
        }
    }
    if let Some(rest) = pattern.strip_prefix("$HOME/") {
        if let Some(h) = home() {
            return format!("{}/{}", h.trim_end_matches('/'), rest);
        }
    }
    if pattern == "$HOME" {
        return home().unwrap_or_else(|| pattern.to_string());
    }
    pattern.to_string()
}

/// The live permission policy: a trusted baseline (defaults ⊕ global config, ⊕ project when the
/// user trusts it), an untrusted project overlay that can only tighten, and a session-scoped set
/// of "always"-approved rules that can relax an `ask` to `allow` (but never override a `deny`).
#[derive(Debug, Clone, Default)]
pub struct PermissionEngine {
    trusted: Ruleset,
    project: Ruleset,
    approved: Ruleset,
}

impl PermissionEngine {
    /// Assemble the policy. `project` rules tighten-only unless `trust_project`, in which case they
    /// merge into the trusted baseline and win by specificity like any other rule.
    pub fn new(global: Ruleset, project: Ruleset, trust_project: bool) -> Self {
        Self::assemble(PermissionMode::Default, global, project, trust_project)
    }

    /// The default policy: permissive baseline + guardrails, no user config.
    pub fn permissive() -> Self {
        Self::new(Vec::new(), Vec::new(), false)
    }

    /// Assemble a policy from a mode preset, a trusted `global` ruleset, and an untrusted
    /// `project` ruleset. The project overlay can only *tighten* the result unless `trust_project`
    /// (then it merges into the trusted baseline and wins by specificity like any other rule).
    ///
    /// `Yolo` drops the guardrails entirely (an explicit opt-out); every other mode keeps the
    /// default guardrails and layers its overlay (strict = ask mutators, read-only = deny them).
    pub fn assemble(
        mode: PermissionMode,
        global: Ruleset,
        project: Ruleset,
        trust_project: bool,
    ) -> Self {
        let mut trusted = match mode {
            PermissionMode::Yolo => vec![rule("*", "*", PermissionAction::Allow)],
            _ => {
                let mut base = default_ruleset();
                match mode {
                    PermissionMode::Strict => base.extend(defaults::strict_overlay()),
                    PermissionMode::ReadOnly => base.extend(defaults::readonly_overlay()),
                    _ => {}
                }
                base
            }
        };
        trusted.extend(global);
        if trust_project {
            trusted.extend(project);
            Self { trusted, project: Vec::new(), approved: Vec::new() }
        } else {
            Self { trusted, project, approved: Vec::new() }
        }
    }

    /// Record session-scoped `always` approvals (relax future matching `ask`s to `allow`).
    pub fn approve(&mut self, rules: impl IntoIterator<Item = Rule>) {
        self.approved.extend(rules);
    }

    /// Decide a whole request: the strictest decision across all its checks.
    pub fn decide(&self, request: &PermissionRequest) -> PermissionAction {
        request
            .checks
            .iter()
            .map(|c| self.decide_one(c))
            .fold(PermissionAction::Allow, PermissionAction::strictest)
    }

    /// Decide one check: baseline (tightened by project), then let a session approval relax an
    /// `ask` — never a `deny`.
    fn decide_one(&self, check: &Check) -> PermissionAction {
        let trusted = evaluate(&check.permission, &check.input, &self.trusted)
            .unwrap_or(PermissionAction::Ask);
        let project = evaluate(&check.permission, &check.input, &self.project)
            .unwrap_or(PermissionAction::Allow);
        match trusted.strictest(project) {
            PermissionAction::Deny => PermissionAction::Deny,
            PermissionAction::Allow => PermissionAction::Allow,
            PermissionAction::Ask => {
                if evaluate(&check.permission, &check.input, &self.approved)
                    == Some(PermissionAction::Allow)
                {
                    PermissionAction::Allow
                } else {
                    PermissionAction::Ask
                }
            }
        }
    }
}

impl crate::tool::FileReadHook for PermissionEngine {
    fn decision(&self, path: &std::path::Path) -> PermissionAction {
        self.decide_one(&Check { permission: "read".into(), input: crate::paths::display(path) })
    }
}

/// A file found mid-call cannot be prompted for, so `ask` resolves the way this run resolves it
/// and only a prompting run is left holding an `Ask`, which the tool withholds.
impl crate::tool::FileReadHook for PermissionSettings {
    fn decision(&self, path: &std::path::Path) -> PermissionAction {
        match (self.engine.decision(path), self.ask) {
            (PermissionAction::Ask, AskResolution::AutoAllow) => PermissionAction::Allow,
            (PermissionAction::Ask, AskResolution::AutoDeny) => PermissionAction::Deny,
            (decision, _) => decision,
        }
    }
}

#[cfg(test)]
mod tests;
