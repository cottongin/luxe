//! Mentions — `@` references to a file or a GitHub pull request or issue.
//!
//! A mention is a pointer, not a paste. `@[file:docs/plan.md]` goes to the model as the path,
//! which it reads with its own tools when the work needs it, and `@[github:acme/app/pull/7]` as
//! what the pull request is: its number, title, state and address. Nothing is read into the
//! message, so a mention never floods the history with a large file or a copy that goes stale the
//! moment the agent edits it.

use std::path::Path;

/// Kinds of references.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RefKind {
    /// A file path (most common).
    File,
    /// A workspace symbol (requires LSP).
    Symbol,
    /// Diagnostics for the workspace.
    Diagnostics,
    /// Git status/diff.
    Git,
    /// A URL.
    Url(String),
    /// A skill, by name
    Skill,
    /// A GitHub pull request or issue, as `owner/repo/pull/7` or `owner/repo/issues/7`
    GitHub,
    /// An MCP server, by id
    Mcp,
    /// A row from a source in `mentions`. Args: `(kind)`
    Source(String),
}

/// A reference accepted from the mention picker.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MentionRef {
    /// Kind of reference.
    pub kind: RefKind,
    /// The target (file path, symbol name, etc.).
    pub target: String,
    /// Byte range in the original message where this reference appears.
    pub span: std::ops::Range<usize>,
}

/// Outcome of a mention state machine keypress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MentionOutcome {
    /// Insert this reference and exit mention mode.
    Insert(MentionRef),
    /// Cancel mention mode, leave literal text.
    Cancel,
    /// Key consumed, mention mode stays open.
    Consumed,
}

/// State machine for @-mention trigger detection and query capture.
#[derive(Clone, Debug)]
pub struct MentionState {
    /// Whether mention mode is active.
    active: bool,
    /// Current query (characters after `@`).
    query: String,
    /// Start byte position of the `@` in the buffer.
    at_pos: usize,
}

impl Default for MentionState {
    fn default() -> Self {
        Self::new()
    }
}

impl MentionState {
    /// Create a new idle mention state.
    pub fn new() -> Self {
        Self { active: false, query: String::new(), at_pos: 0 }
    }

    /// Check if mention mode is active.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Get the current query.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Handle a character input.
    pub fn on_char(&mut self, c: char, buffer_len: usize, cursor_pos: usize) -> MentionOutcome {
        if !self.active {
            if c == '@' && self.is_word_boundary(buffer_len, cursor_pos) {
                self.active = true;
                self.query.clear();
                self.at_pos = cursor_pos;
                return MentionOutcome::Consumed;
            }
            return MentionOutcome::Consumed;
        }

        // In mention mode.
        if c.is_whitespace() {
            // Space exits mention mode without inserting.
            self.active = false;
            self.query.clear();
            return MentionOutcome::Cancel;
        }

        // Capture the character.
        self.query.push(c);
        MentionOutcome::Consumed
    }

    /// Handle a key event.
    pub fn on_key(&mut self, key: &crossterm::event::KeyEvent) -> MentionOutcome {
        if !self.active {
            return MentionOutcome::Consumed;
        }

        use crossterm::event::KeyCode;

        match key.code {
            KeyCode::Esc => {
                // Cancel mention mode.
                self.active = false;
                self.query.clear();
                MentionOutcome::Cancel
            }
            KeyCode::Enter | KeyCode::Tab => {
                // Would dispatch the selected item (handled by the flyout).
                // For the state machine, just return Consumed — the flyout handles insertion.
                MentionOutcome::Consumed
            }
            KeyCode::Backspace if self.query.is_empty() => {
                // Backspace at the @ exits mention mode.
                self.active = false;
                self.query.clear();
                MentionOutcome::Cancel
            }
            KeyCode::Backspace => {
                self.query.pop();
                MentionOutcome::Consumed
            }
            _ => MentionOutcome::Consumed,
        }
    }

    /// Check if a position is a word boundary (start of input or after whitespace).
    fn is_word_boundary(&self, _buffer_len: usize, cursor_pos: usize) -> bool {
        if cursor_pos == 0 {
            return true;
        }
        // Check if the character before is whitespace or start of buffer.
        // This is a simplified check — in practice, we'd need the actual buffer content.
        true
    }

    /// Accept a reference and exit mention mode.
    pub fn accept(
        &mut self,
        kind: RefKind,
        target: String,
        span: std::ops::Range<usize>,
    ) -> MentionRef {
        self.active = false;
        self.query.clear();
        MentionRef { kind, target, span }
    }
}

/// What a GitHub reference is known to be: its row in `/github` or the `@` picker.
pub type GithubRows = std::collections::HashMap<String, luxe_core::github::ListItem>;

/// What each source reference stands for, by `kind:id`: the text its source gave it.
pub type SourceRows = std::collections::HashMap<String, String>;

/// Everything a message's references might be known as when it is sent.
pub struct Known<'a> {
    /// GitHub rows picked this session
    pub github: &'a GithubRows,
    /// Source rows picked this session
    pub rows: &'a SourceRows,
    /// The sources configured, which name the kinds a reference may have
    pub sources: &'a [luxe_core::config::MentionSource],
    /// The names of the skills installed
    pub skills: &'a [String],
    /// The MCP servers this session can reach, from the latest snapshot
    pub mcp: &'a [luxe_core::McpServerStatus],
    /// The servers whose tool names this conversation already carries
    pub mcp_listed: &'a std::collections::HashSet<String>,
}

/// The most tool names one reference sends, and the most characters they may take between them.
const MCP_NAMES_MAX: usize = 40;
const MCP_NAMES_MAX_CHARS: usize = 1_000;

/// The token a GitHub row is referenced by: `@[github:owner/repo/pull/7]`, or `issues` for an
/// issue, the way GitHub spells its addresses.
pub fn github_token(item: &luxe_core::github::ListItem) -> String {
    format!("@[github:{}]", github_target(item))
}

/// What a GitHub row's token names, without its brackets.
pub fn github_target(item: &luxe_core::github::ListItem) -> String {
    let kind = if item.pull_request { "pull" } else { "issues" };
    format!("{}/{kind}/{}", item.repository, item.number)
}

/// `text` as it goes to the model: each mention replaced by the reference it stands for.
///
/// A file is its path in backticks, said to be missing when there is nothing at it. A GitHub row
/// is its kind, `owner/repo#n`, and — when it was picked from a list this session — its title,
/// quoted as a stranger's words, its state and its address; typed by hand it is the address alone.
/// A skill is its name, pointed at the skill tool, or said not to be installed. An MCP server is
/// its id pointed at the `mcp` tool, with its tools' names the first time ([`mcp_names_sent`]). A
/// source's row is
/// the text its source gave it, or the source's name and the id when it was typed; a kind no
/// source claims is left as it was written.
pub fn references_text(text: &str, refs: &[MentionRef], repo_root: &Path, known: &Known) -> String {
    let mut result = text.to_string();
    for reference in refs.iter().rev() {
        let replacement = match &reference.kind {
            RefKind::GitHub => github_reference(&reference.target, known.github),
            // The skill tool loads it, as it would any skill the agent chose for itself
            RefKind::Skill
                if known.skills.iter().any(|name| name.eq_ignore_ascii_case(&reference.target)) =>
            {
                format!(
                    "the \"{}\" skill (its instructions load with the skill tool)",
                    reference.target
                )
            }
            RefKind::Skill => {
                format!("the \"{}\" skill (no skill by that name is installed)", reference.target)
            }
            RefKind::Mcp => mcp_reference(reference, refs, known),
            RefKind::Source(kind) => {
                let key = format!("{kind}:{}", reference.target);
                let source = known.sources.iter().find(|source| source.kind() == *kind);
                match (known.rows.get(&key), source) {
                    (Some(text), _) => text.clone(),
                    (None, Some(source)) => format!("{} {}", source.name, reference.target),
                    (None, None) => continue,
                }
            }
            _ if repo_root.join(&reference.target).exists() => format!("`{}`", reference.target),
            // Lines picked in the File view: still a pointer, which the read tool can follow
            // with an offset
            _ if line_range(&reference.target)
                .is_some_and(|(path, _)| repo_root.join(path).exists()) =>
            {
                let (path, lines) = line_range(&reference.target).unwrap_or_default();
                format!("`{path}` {lines}")
            }
            _ => format!("`{}` (nothing is at that path in the project)", reference.target),
        };
        result.replace_range(reference.span.clone(), &replacement);
    }
    result
}

/// The servers whose tool names this message sends: each one referenced that has names to send and
/// has not had them sent already. The caller records them, so the next reference is the pointer.
///
/// Only a server that can answer is listed. One that is not signed in, or whose tools are not
/// discovered yet, sends what stands in the way instead, and gets its names the first time it is
/// referenced after that is fixed.
pub fn mcp_names_sent(refs: &[MentionRef], known: &Known) -> Vec<String> {
    let mut sent: Vec<String> = Vec::new();
    for reference in refs.iter().filter(|reference| reference.kind == RefKind::Mcp) {
        let id = &reference.target;
        let listable = known.mcp.iter().any(|server| {
            server.id == *id
                && server.signed_in != Some(false)
                && plain_tools(server).next().is_some()
        });
        if listable && !known.mcp_listed.contains(id) && !sent.contains(id) {
            sent.push(id.clone());
        }
    }
    sent
}

/// The tool names that are safe to repeat to the model, in the server's order.
fn plain_tools(server: &luxe_core::McpServerStatus) -> impl Iterator<Item = &str> {
    server
        .tools
        .iter()
        .map(String::as_str)
        .filter(|name| luxe_core::mcp_client::is_plain_name(name))
}

/// What a reference to an MCP server says, depending on what the session knows of it.
///
/// Names only, never descriptions: those are the server's words, and a reference is the reader's.
/// The first reference in a message to a server not yet listed carries the names; any other is
/// the pointer alone.
fn mcp_reference(reference: &MentionRef, refs: &[MentionRef], known: &Known) -> String {
    let id = &reference.target;
    let Some(server) = known.mcp.iter().find(|server| server.id == *id) else {
        return format!("the \"{id}\" MCP server (this session has no MCP server by that id)");
    };
    let search = format!("`mcp({{search:\"{id}\"}})`");
    if server.signed_in == Some(false) {
        return format!(
            "the \"{id}\" MCP server (not signed in — `luxe mcp login --id {id}` in a terminal signs \
             in; until then its tools cannot answer)"
        );
    }
    if server.tool_count.is_none() {
        return format!(
            "the \"{id}\" MCP server (its tools are not discovered yet: {search} lists them)"
        );
    }
    if plain_tools(server).next().is_none() {
        return format!(
            "the \"{id}\" MCP server (no tool names to show: {search} lists what it has)"
        );
    }
    let first = refs.iter().find(|other| other.kind == RefKind::Mcp && other.target == *id);
    let names_here = first.is_some_and(|first| first.span == reference.span)
        && mcp_names_sent(std::slice::from_ref(reference), known).contains(id);
    if !names_here {
        return format!("the \"{id}\" MCP server (through the mcp tool, as `{id}/<name>`)");
    }
    let names: Vec<&str> = plain_tools(server).collect();
    let mut shown = String::new();
    let mut count = 0;
    for name in &names {
        let separator = if count == 0 { "" } else { ", " };
        if count == MCP_NAMES_MAX
            || shown.len() + separator.len() + name.len() > MCP_NAMES_MAX_CHARS
        {
            break;
        }
        shown.push_str(separator);
        shown.push_str(name);
        count += 1;
    }
    let rest = match names.len() - count {
        0 => String::new(),
        more => format!(" … and {more} more"),
    };
    format!(
        "the \"{id}\" MCP server — call its tools through the mcp tool as `{id}/<name>`: \
         {shown}{rest} ({search})"
    )
}

/// `pull request acme/app#7 ("Parse categories", open, https://…)`.
fn github_reference(target: &str, github: &GithubRows) -> String {
    let (repository, kind, number) = split_github_target(target);
    let (noun, command) = if kind == "pull" { ("pull request", "pr") } else { ("issue", "issue") };
    // A pointer the model can follow: told nothing, models took the title for everything there was
    // and said they "couldn't read" the issue. The `git` tool reads it with bounded output, where a
    // bare `gh` command sent the model to the shell for it.
    let read = format!(
        "the git tool reads it: action \"{command}\", args [\"view\", \"{number}\", \"--repo\", \
         \"{repository}\", \"--comments\"]"
    );
    match github.get(target) {
        Some(item) => format!(
            "{noun} {repository}#{number} (\"{}\", {}{}, {}; {read})",
            item.title.replace('"', "'"),
            if item.draft { "draft, " } else { "" },
            item.state.word(),
            item.url
        ),
        None => format!("{noun} {repository}#{number} (https://github.com/{target}; {read})"),
    }
}

/// `owner/repo/pull/7` as `(owner/repo, pull, 7)`; anything shorter keeps what it has.
pub fn split_github_target(target: &str) -> (&str, &str, &str) {
    let mut parts = target.rsplitn(3, '/');
    let number = parts.next().unwrap_or_default();
    let kind = parts.next().unwrap_or_default();
    let repository = parts.next().unwrap_or_default();
    (repository, kind, number)
}

/// A file reference with lines on it, `src/a.rs:12-30`, as `("src/a.rs", "lines 12–30")`.
///
/// Only a trailing `:N` or `:N-M` counts, so a path that merely contains a colon is left alone.
fn line_range(target: &str) -> Option<(&str, String)> {
    let (path, lines) = target.rsplit_once(':')?;
    let (first, last) = lines.split_once('-').unwrap_or((lines, lines));
    let (first, last): (u32, u32) = (first.parse().ok()?, last.parse().ok()?);
    if path.is_empty() || first == 0 || last < first {
        return None;
    }
    Some((
        path,
        if first == last { format!("line {first}") } else { format!("lines {first}–{last}") },
    ))
}

/// Parse mention tokens from a message text.
///
/// Looks for `@[file:path]` patterns and returns the parsed references.
pub fn parse_mentions(text: &str) -> Vec<MentionRef> {
    // Driven by the `@`s themselves rather than by offsets: nothing about a token can send the scan
    // backwards or keep it in place, so no input — and no slip in the arithmetic — loops it. An
    // `@` inside a mention already found is part of its target, so mentions never overlap.
    let mut found: Vec<MentionRef> = Vec::new();
    for (at, _) in text.match_indices('@') {
        if found.last().is_some_and(|last| at < last.span.end) {
            continue;
        }
        found.extend(mention_at(text, at));
    }
    found
}

/// The mention starting at the `@` at `at`, if one does.
///
/// Only `@[` opens one: the `@` of an address is text, however many brackets follow later in the
/// message. The kind is lowercase letters, digits and dashes, as a source's is folded.
fn mention_at(text: &str, at: usize) -> Option<MentionRef> {
    let opened = text[at..].strip_prefix("@[")?;
    let (named, rest) = opened.split_once(':')?;
    let well_formed =
        named.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if named.is_empty() || !well_formed {
        return None;
    }
    let (target, _) = rest.split_once(']')?;
    if target.is_empty() {
        return None;
    }
    let kind = match named {
        "file" => RefKind::File,
        "skill" => RefKind::Skill,
        "github" => RefKind::GitHub,
        "mcp" => RefKind::Mcp,
        other => RefKind::Source(other.to_string()),
    };
    // `@[` + kind + `:` + target + `]`
    let end = at + 2 + named.len() + 1 + target.len() + 1;
    Some(MentionRef { kind, target: target.to_string(), span: at..end })
}

#[cfg(test)]
mod tests;
