//! instructions — filtering what a repository's instruction files may put in the prompt.
//!
//! `AGENTS.md` and a skill's `SKILL.md` are read out of the working tree and land in the system
//! prompt, which makes them the one place a repository writes text the model reads as trusted
//! instruction rather than as data. Cloning a repository is not consent to that, so the text is
//! filtered on the way in.
//!
//! What comes out is narrow and shares one property: **the file lies to the reader about its own
//! contents**.
//!
//! - **Comments.** Markdown renders an HTML comment, or a link reference definition used as one
//!   (`[//]: # (…)`), to nothing, so on every surface a human reviews the file on — a diff view, a
//!   rendered README, a code host — the text is simply absent, while the raw bytes still reach the
//!   model.
//! - **Invisible characters.** Tag characters, variation selectors and bidirectional overrides
//!   carry text no editor displays, which is the same trick with no markdown required, written
//!   raw or as a character reference (`&#x202E;`) that a renderer decodes.
//!
//! This is not a defense against a hostile repository, and nothing here pretends to be one:
//! instructions written in plain sight are still instructions, and a checker that guessed at intent
//! would be one nobody trusts. It removes the classes where review cannot work by construction.
//!
//! # Why strip rather than warn
//!
//! rustc (for CVE-2021-42574), GitHub, VS Code, gcc and clang all *warn* about these characters,
//! which is right when a human is the one about to be fooled. Here the consumer is a model, which
//! makes a warning useless twice over: the model cannot act on it, and by the time a person reads
//! one the text has already been in the prompt. So the same judgment produces a different remedy —
//! drop them, which also leaves nothing to false-positive about.
//!
//! A file whose link definitions cannot be checked (one short label repeated past what the parser
//! can be asked to tell apart) is withheld whole, with a notice, rather than guessed at.
//!
//! Also not covered: text in a raw HTML tag's attributes, footnote and link definitions with a real
//! destination that nothing references, and character references inside raw HTML. Each renders as
//! nothing too; stripping them means reimplementing more of a browser for classes no real
//! instruction file uses, and the line has to be drawn somewhere a reviewer can see.
//!
//! Two attacks that are *not* covered, deliberately. **Confusable glyphs** (Cyrillic `а` for Latin
//! `a`) spoof identifiers in code, not instructions in prose, and detecting them misfires on every
//! file that is not English. **ANSI escapes** hide text from a terminal rather than from a model,
//! and instruction files are rendered through markdown here rather than echoed raw.

mod hidden;
mod link_comments;
pub(crate) mod nested;

/// Instruction text with the unreviewable parts removed, and what came out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sanitized {
    /// The text as the model will see it.
    pub text: String,
    /// HTML comments and comment-only link definitions removed
    pub comments: usize,
    /// Invisible characters removed.
    pub invisible: usize,
    /// Whether the whole file was held back because its link definitions could not be checked
    pub withheld: bool,
}

impl Sanitized {
    /// Whether anything was removed — the cue to tell somebody.
    pub fn is_clean(&self) -> bool {
        self.comments == 0 && self.invisible == 0 && !self.withheld
    }

    /// What was removed, phrased for a person, or `None` when the file was clean.
    pub fn notice(&self, name: &str) -> Option<String> {
        if self.is_clean() {
            return None;
        }
        if self.withheld {
            return Some(format!(
                "{name}: left out of the prompt — it repeats link definitions more often than Luxe \
                 can check for hidden text"
            ));
        }
        let mut parts = Vec::new();
        if self.comments > 0 {
            parts.push(format!(
                "{} hidden comment{}",
                self.comments,
                if self.comments == 1 { "" } else { "s" }
            ));
        }
        if self.invisible > 0 {
            parts.push(format!(
                "{} invisible character{}",
                self.invisible,
                if self.invisible == 1 { "" } else { "s" }
            ));
        }
        Some(format!(
            "{name}: dropped {} before adding it to the prompt — text a reader of the file cannot \
             see does not get to instruct the model",
            parts.join(" and ")
        ))
    }
}

/// Filters instruction text for injection.
pub fn sanitize(text: &str) -> Sanitized {
    // Invisible characters, raw or as references, become same-length placeholders while the
    // structure is read: deleting one first could turn its line blank and change what the renderer
    // makes of the lines around it.
    let raw = text.chars().filter(|c| is_invisible(*c)).count();
    let mut masked = String::with_capacity(text.len());
    for c in text.chars() {
        match is_invisible(c) {
            true => (0..c.len_utf8()).for_each(|_| masked.push(PLACEHOLDER)),
            false => masked.push(c),
        }
    }
    let references = hidden::invisible_references(&masked);
    for range in &references {
        // a reference is ASCII, so a byte-for-byte placeholder keeps every offset
        masked.replace_range(range.clone(), &PLACEHOLDER.to_string().repeat(range.len()));
    }
    let invisible = raw + references.len();
    match hidden::strip(&masked) {
        Some((mut text, comments)) => {
            text.retain(|c| c != PLACEHOLDER);
            Sanitized { text, comments, invisible, withheld: false }
        }
        None => Sanitized { text: WITHHELD.into(), comments: 0, invisible, withheld: true },
    }
}

/// What stands in for an invisible character while the structure is read: one byte, not space,
/// and nothing anybody types.
const PLACEHOLDER: char = '\u{7f}';

/// The whole text of a withheld file, so every caller passes the reason on rather than an empty
/// string.
const WITHHELD: &str = "[withheld: this file repeats link definitions more often than Luxe can \
                        check for hidden text]";

/// Characters that put text on the page without putting it in front of anyone.
///
/// The bidi set is exactly rustc's `text_direction_codepoint_in_literal` lint — `U+202A`–`U+202E`
/// and `U+2066`–`U+2069`, the Trojan Source vector — which is a useful check on the judgment: the
/// same nine codepoints, picked twice for the same reason.
///
/// Deliberately not every format control. `U+200C`/`U+200D` are load-bearing in Persian and Indic
/// scripts and in emoji sequences, so removing them corrupts real content to close a channel that
/// ordinary whitespace leaves open anyway — VS Code walked back its own highlighting for the same
/// reason, suppressing it where the surrounding script is non-ASCII. What is left has no job in a
/// prose instruction file: the tag block is deprecated and its only current use is smuggling. The
/// supplementary variation selectors and the other default-ignorable marks below carry payloads the
/// same way; their real uses, rare ideographic variants and Mongolian shaping, are rare enough in
/// an instruction file that losing them, with a notice saying so, is the better trade.
pub(crate) fn is_invisible(c: char) -> bool {
    matches!(c,
        '\u{00AD}'                  // soft hyphen
        | '\u{034F}'                // combining grapheme joiner
        | '\u{180E}'                // Mongolian vowel separator
        | '\u{200B}'                // zero-width space
        | '\u{2060}'                // word joiner
        | '\u{FEFF}'                // zero-width no-break space (BOM)
        | '\u{202A}'..='\u{202E}'   // bidi embeddings and overrides
        | '\u{2066}'..='\u{2069}'   // bidi isolates
        | '\u{E0000}'..='\u{E007F}' // tag characters
        | '\u{E0100}'..='\u{E01EF}' // variation selectors supplement
    )
}

/// Instruction file names, in the order a directory is checked. The first hit at each level wins,
/// so a repo that still carries `CLAUDE.md` keeps working without renaming anything.
pub const INSTRUCTION_FILES: &[&str] = &["AGENTS.md", "CLAUDE.md"];

/// Whose rules a context file carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextScope {
    /// Every project on the machine
    Global,
    /// The top of the checkout, or the project directory when there is none
    Project,
    /// A folder below that, on the way down to where Luxe was launched
    Folder,
}

/// One instruction file that applies, and whether the prompt carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextFile {
    /// The file
    pub path: std::path::PathBuf,
    /// Whose rules it carries
    pub scope: ContextScope,
    /// The earlier file this one repeats, when it does.
    ///
    /// Either the same file reached a second time — a symlink, or `AGENTS.md` answering for
    /// `agents.md` on a case-insensitive volume — or a different file with the same text. The
    /// prompt carries it once, under the first.
    pub duplicate_of: Option<std::path::PathBuf>,
}

impl ContextFile {
    /// Whether the prompt carries this file.
    pub fn is_loaded(&self) -> bool {
        self.duplicate_of.is_none()
    }
}

/// Every instruction file that applies to `cwd`, in the order the prompt carries them.
///
/// The global files first, then the checkout's top down to `cwd`, so the most specific
/// instructions are the ones the model reads most recently. Nothing above the checkout is read,
/// and with no checkout at all only `cwd` is: a parent directory's `AGENTS.md` belongs to whoever
/// put it there, not to this project.
///
/// Only files that exist come back, which makes this the honest answer to "what does a turn read"
/// for the prompt, the dashboard and the context list alike. A file that repeats an earlier one is
/// listed with [`ContextFile::duplicate_of`] set rather than left out, so a surface can still show
/// it exists — leaving it out would offer to create a file that is already there.
pub fn context_files(cwd: &std::path::Path) -> Vec<ContextFile> {
    let global = global_context_files().into_iter().map(|path| (path, ContextScope::Global));
    let local =
        crate::project::chain_from_top(cwd).into_iter().enumerate().filter_map(|(depth, dir)| {
            let path =
                INSTRUCTION_FILES.iter().map(|name| dir.join(name)).find(|path| path.is_file())?;
            let scope = if depth == 0 { ContextScope::Project } else { ContextScope::Folder };
            Some((path, scope))
        });
    dedupe(global.filter(|(path, _)| path.is_file()).chain(local))
}

/// Marks each file that repeats an earlier one, by identity first and then by text.
///
/// Byte-identical text is the only overlap Luxe settles on its own. Two files that say *mostly*
/// the same thing are two files someone wrote, and choosing between their sentences is the
/// author's job.
fn dedupe(found: impl Iterator<Item = (std::path::PathBuf, ContextScope)>) -> Vec<ContextFile> {
    let mut files: Vec<ContextFile> = Vec::new();
    let mut kept: Vec<(std::path::PathBuf, Option<same_file::Handle>, Option<String>)> = Vec::new();
    for (path, scope) in found {
        let handle = same_file::Handle::from_path(&path).ok();
        let text = std::fs::read_to_string(&path).ok();
        let duplicate_of = kept
            .iter()
            .find(|(_, other, other_text)| {
                let same_file = handle.is_some() && other.is_some() && handle == *other;
                let same_text = text.as_deref().is_some_and(|text| {
                    !text.trim().is_empty() && other_text.as_deref() == Some(text)
                });
                same_file || same_text
            })
            .map(|(earlier, _, _)| earlier.clone());
        if duplicate_of.is_none() {
            kept.push((path.clone(), handle, text));
        }
        files.push(ContextFile { path, scope, duplicate_of });
    }
    files
}

/// Where a project's own instruction file goes when there is not one yet: `AGENTS.md` at the top
/// of the checkout, or in `cwd` when there is no checkout.
///
/// The top rather than `cwd`, because instructions written from a subdirectory would apply to
/// that subtree alone — which is a real feature and never what someone means by "add one".
pub fn project_context_file(cwd: &std::path::Path) -> std::path::PathBuf {
    let top = crate::project::chain_from_top(cwd).into_iter().next();
    top.unwrap_or_else(|| cwd.to_path_buf()).join(INSTRUCTION_FILES[0])
}

/// The global instruction file Luxe creates when asked: `~/.config/luxe/agents.md`.
pub fn global_context_file() -> std::path::PathBuf {
    crate::user_dirs::luxe_config_dir().join("agents.md")
}

/// Every global instruction file Luxe reads, broadest first, whether or not it exists.
///
/// `~/.agents/AGENTS.md` is the file a growing number of people keep for *every* agent they run,
/// and Luxe's own sits after it so the Luxe-specific word lands later. Somebody who symlinked one to
/// the other gets it once.
pub fn global_context_files() -> Vec<std::path::PathBuf> {
    let shared = crate::user_dirs::home_dir().map(|home| home.join(".agents").join("AGENTS.md"));
    shared.into_iter().chain(std::iter::once(global_context_file())).collect()
}

#[cfg(test)]
mod tests;
