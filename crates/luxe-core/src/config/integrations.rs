//! Integration settings — launchers, `@` sources, worktrees, GitHub, updates, language servers
//! and Guided Review.

use serde::{Deserialize, Serialize};

/// A program a key hands the terminal to: gh-dash, lazygit, anything that takes over a terminal
/// and gives it back.
///
/// Read from the global config only, like command hooks: a key that runs a program a repository
/// named is the same arbitrary execution a project's hooks would be.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Launcher {
    /// The chord that starts it, like `alt+l`
    pub key: String,
    /// The program, looked up on `PATH`
    pub command: String,
    /// Its arguments
    #[serde(default)]
    pub args: Vec<String>,
}

/// A program the `@` picker runs to list references of a kind of its own — Jira issues, Asana
/// tasks, anything a script can reach — each row inserted as `@[kind:id]`.
///
/// Read from the global config only, like launchers and command hooks: a source runs a program,
/// and a program a repository named is arbitrary execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MentionSource {
    /// The tab's name, like `Jira`
    pub name: String,
    /// The reference's kind, as in `@[jira:PROJ-12]`; the name in lowercase when unset
    #[serde(default)]
    pub kind: Option<String>,
    /// The program, looked up on `PATH`
    pub command: String,
    /// Its arguments
    #[serde(default)]
    pub args: Vec<String>,
    /// A glyph drawn beside the tab's name, like a Nerd Font's `md-jira`; a tag when unset
    #[serde(default)]
    pub icon: Option<String>,
}

impl MentionSource {
    /// The glyph it asked for, when that is one: a character or two — a mark and its variation
    /// selector — with nothing that moves the cursor. Anything else is ignored for the default,
    /// since a tab row is not the place to find out a config value was a sentence.
    pub fn icon(&self) -> Option<&str> {
        let icon = self.icon.as_deref()?.trim();
        let count = icon.chars().count();
        let clean = icon.chars().all(|c| !c.is_whitespace() && !c.is_control());
        ((1..=2).contains(&count) && clean).then_some(icon)
    }

    /// The kind its references carry: the one set, or the name folded to lowercase letters, digits
    /// and dashes.
    pub fn kind(&self) -> String {
        let named = self.kind.as_deref().unwrap_or(&self.name);
        let folded: String = named
            .trim()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
            .collect();
        folded.trim_matches('-').to_string()
    }

    /// Whether it can stand beside files, skills, GitHub and MCP: it has a kind, and not one of
    /// theirs.
    pub fn usable(&self) -> bool {
        let kind = self.kind();
        !kind.is_empty() && !self.reserved() && !self.command.trim().is_empty()
    }

    /// Whether its kind is one Luxe's own tabs use, which leaves it without a tab of its own.
    ///
    /// Worth telling the reader rather than only skipping: `mcp` joined the list after sources
    /// could be named anything, so a source that worked yesterday can be the one it displaced.
    pub fn reserved(&self) -> bool {
        RESERVED_MENTION_KINDS.contains(&self.kind().as_str())
    }
}

/// The reference kinds Luxe's own `@` tabs use: `@[file:…]`, `@[skill:…]`, `@[github:…]`,
/// `@[mcp:…]`.
pub const RESERVED_MENTION_KINDS: [&str; 4] = ["file", "skill", "github", "mcp"];

/// Session worktree preferences.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct WorktreeConfig {
    /// Give every new session a worktree of its own, as `--worktree` does. `None` — the default —
    /// means no.
    pub always: Option<bool>,
}

impl WorktreeConfig {
    /// Whether a new session gets a worktree without being asked.
    pub fn always(&self) -> bool {
        self.always.unwrap_or(false)
    }
}

/// GitHub preferences.
///
/// Read from the global config only, like [`UpdateConfig`]: how often Luxe calls out on a timer is
/// the reader's decision, not one a repository they cloned gets to make.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct GithubConfig {
    /// Seconds between background checks of the branch's pull request once nothing is in flight.
    /// `None` — the default — means 60; `0` turns background checks off.
    pub refresh_secs: Option<u64>,
    /// The sections `/github` lists, each a GitHub search; `None` means review requested, mine,
    /// assigned issues, and every open pull request and issue, in this repository.
    pub sections: Option<Vec<GithubSection>>,
}

/// One section of `/github`: a name, and the GitHub search that fills it. `{repo}` in the query
/// stands for the checkout's repository, `owner/name`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GithubSection {
    /// What the tab says
    pub name: String,
    /// A GitHub search, as the site's search box takes it
    pub query: String,
}

impl GithubConfig {
    /// The sections `/github` lists: the configured ones, or the defaults — what is addressed to
    /// you first, then everything open, most recently active first. All in this repository, since
    /// that is where each row's action works.
    pub fn sections(&self) -> Vec<GithubSection> {
        self.sections.clone().unwrap_or_else(|| {
            [
                ("Review requested", "is:open is:pr review-requested:@me repo:{repo}"),
                ("Mine", "is:open is:pr author:@me repo:{repo}"),
                ("Assigned issues", "is:open is:issue assignee:@me repo:{repo}"),
                ("Pull requests", "is:open is:pr sort:updated-desc repo:{repo}"),
                ("Issues", "is:open is:issue sort:updated-desc repo:{repo}"),
            ]
            .into_iter()
            .map(|(name, query)| GithubSection { name: name.into(), query: query.into() })
            .collect()
        })
    }

    /// The settled interval, in seconds; `0` means no background checks.
    pub fn refresh_secs(&self) -> u64 {
        self.refresh_secs.unwrap_or(crate::github::refresh::DEFAULT_REFRESH_SECS)
    }
}

/// Update-notice preferences.
///
/// Luxe never updates itself; the only question here is whether it may *notice* that it is behind.
/// The check fetches a public manifest and sends nothing about you — see `luxe_core::update` — but
/// it is still an outbound request, so it gets a switch like every other one.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct UpdateConfig {
    /// Look for a newer release. `None` — the default — means yes.
    pub check: Option<bool>,
}

impl UpdateConfig {
    /// Whether the check may run.
    pub fn enabled(&self) -> bool {
        self.check.unwrap_or(true)
    }
}

/// Language-server preferences.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct LspConfig {
    /// Whether a server the *project* installed may be launched. `None` = [`ProjectServers::Auto`].
    ///
    /// **Read from the global config only.** A project that could set this would be granting
    /// itself permission to run its own binaries, which is the whole thing being decided — the same
    /// reason `.luxe/hooks.json` is ignored without `--trust-project`.
    ///
    /// Format-on-save reads the same answer, because it is the same question: a `.venv/bin/ruff`
    /// came from the repository exactly as a `node_modules/.bin` server did. It lives under `lsp`
    /// because language servers asked first — a user who said "do not run this project's binaries"
    /// did not mean only those.
    pub project_servers: Option<ProjectServers>,
}

/// Whether Luxe may launch a language server out of the project's `node_modules/.bin`.
///
/// npm puts a dev dependency there rather than on `PATH`, so for the JavaScript servers it is
/// routinely the only copy that matches the project — `svelteserver` has to agree with the Svelte
/// major, a TypeScript server with the `typescript` the build uses. Every editor prefers it.
///
/// It is also a binary the repository supplied, so the default asks who is watching rather than
/// answering the same way everywhere. See [`ProjectServers::Auto`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProjectServers {
    /// Use them with a human at the keyboard; refuse when nobody is there (default).
    ///
    /// An interactive session is someone who opened this repository in their coding agent on
    /// purpose, and a populated `node_modules/.bin` means `npm install` already ran this project's
    /// install scripts — arbitrary code, with more privilege than one language server. Refusing
    /// there protects nothing and costs the correct server.
    ///
    /// Unattended is the opposite: `--headless`, the loop and the review agent can be pointed at a
    /// tree nobody has read, and there is no one to notice. Those refuse unless `--trust-project`
    /// says otherwise, which is the same flag that lets a repository's hooks run.
    #[default]
    Auto,
    /// Always, including unattended. For a loop over repositories you own.
    Always,
    /// Never; only servers found on `PATH` and in the usual install directories.
    Never,
}

/// Guided Review preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ReviewConfig {
    /// Extra project-relative planning files a review reads for context, on top of the built-in
    /// `PLAN.md`/`AGENTS.md`. Each must resolve inside the opened project.
    #[serde(default)]
    pub context_files: Vec<String>,
    /// Project-relative globs a review leaves out, on top of Luxe's own `.luxe/` directory.
    ///
    /// For bookkeeping a repo carries but nobody reviews — a loop's `PROGRESS.md`, a planning
    /// directory, another agent's memory. Excluded files are *counted and named*, never silently
    /// dropped: the Brief says how many and the review is honest that it is not the whole diff.
    #[serde(default)]
    pub exclude: Vec<String>,
}
