//! Glob-style matching for permission patterns.
//!
//! Two flavors, both borrowed from OpenCode's model and hardened against its known matching gaps
//! (issue #6676 `kill *` vs `kill -9`; compound-command bypasses):
//!
//! - [`matches`] — plain glob (`*` any run, `?` one char, else literal) for tool/path/url keys,
//!   with the trailing-` *` convenience so `git *` matches both `git` and `git status`.
//! - [`matches_command`] — structured bash matching: the command line is split into segments on
//!   the shell operators (`&&`, `||`, `;`, `|`, newline) and each segment tokenized, so a pattern
//!   matches even when the risky command is buried (`ls && rm -rf /`) or carries flags
//!   (`rm *` matches `rm -rf /`). It is a best-effort matcher, not a shell parser: a determined
//!   model can obfuscate around it, which is why bash rules are a guardrail, not a boundary.

use regex::Regex;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

type CachedGlob = Option<Arc<Regex>>;
static COMPILED_GLOBS: LazyLock<Mutex<HashMap<String, CachedGlob>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Match `text` against a glob `pattern`.
///
/// `*` matches any run (including empty), `?` matches exactly one character, everything else is
/// literal. A trailing ` *` (space + star) also matches the bare prefix, so `git *` matches `git`.
pub(crate) fn matches(text: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    compiled_glob(pattern).map(|regex| regex.is_match(text)).unwrap_or(false)
}

fn compiled_glob(pattern: &str) -> CachedGlob {
    let mut cache = COMPILED_GLOBS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    cache
        .entry(pattern.to_string())
        .or_insert_with(|| match Regex::new(&glob_to_regex(pattern)) {
            Ok(regex) => Some(Arc::new(regex)),
            Err(error) => {
                tracing::error!(%error, pattern_bytes = pattern.len(), "failed to compile permission glob");
                None
            }
        })
        .clone()
}

/// Translate a glob into an anchored regex string.
fn glob_to_regex(pattern: &str) -> String {
    let mut body = String::with_capacity(pattern.len() * 2);
    for c in pattern.chars() {
        match c {
            '*' => body.push_str(".*"),
            '?' => body.push('.'),
            '.' | '+' | '^' | '$' | '{' | '}' | '(' | ')' | '|' | '[' | ']' | '\\' => {
                body.push('\\');
                body.push(c);
            }
            _ => body.push(c),
        }
    }
    // A trailing " *" should also match the bare prefix ("git *" matches "git").
    if let Some(prefix) = body.strip_suffix(" .*") {
        body = format!("{prefix}( .*)?");
    }
    format!("^{body}$")
}

/// Whether `pattern` matches any segment of a shell `command`.
///
/// The pattern's first token must match the segment's command name; the remaining pattern tokens
/// must match the segment's arguments as a token-level glob (`*` skips any run of tokens). Matching
/// any segment means a risky command hidden behind `&&`/`;`/`|` is still caught.
pub(crate) fn matches_command(command: &str, pattern: &str) -> bool {
    let pat: Vec<&str> = pattern.split_whitespace().collect();
    if pat.is_empty() {
        return true;
    }
    command_segments(command).iter().any(|seg| segment_matches(seg, &pat))
}

/// Split a command line into segments on top-level shell operators, tokenizing each on whitespace.
///
/// Best-effort: it does not track quotes, subshells, or here-docs. Good enough to catch a risky
/// command in a common `a && b`, `a; b`, `a | b`, or `a & b` chain.
pub(crate) fn command_segments(command: &str) -> Vec<Vec<String>> {
    let chars: Vec<char> = command.chars().collect();
    let mut segments = Vec::new();
    let mut current = String::new();
    let push = |current: &mut String, segments: &mut Vec<Vec<String>>| {
        let tokens: Vec<String> = current.split_whitespace().map(str::to_string).collect();
        if !tokens.is_empty() {
            segments.push(tokens);
        }
        current.clear();
    };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        let previous = i.checked_sub(1).and_then(|back| chars.get(back)).copied();
        match c {
            ';' | '\n' => {
                push(&mut current, &mut segments);
                i += 1;
            }
            '&' if next == Some('&') => {
                push(&mut current, &mut segments);
                i += 2;
            }
            // A lone `&` backgrounds what precedes it and runs what follows, exactly like `;` — so
            // it has to split, or `ls & rm -rf /` hides the `rm` from every bash rule while
            // `ls && rm -rf /` is caught. Redirects borrow the same character and must not split:
            // `2>&1` and `<&3` carry it after `>`/`<`, and `&>file` puts it before `>`.
            '&' if !matches!(previous, Some('>') | Some('<')) && next != Some('>') => {
                push(&mut current, &mut segments);
                i += 1;
            }
            '|' if next == Some('|') => {
                push(&mut current, &mut segments);
                i += 2;
            }
            '|' => {
                push(&mut current, &mut segments);
                i += 1;
            }
            _ => {
                current.push(c);
                i += 1;
            }
        }
    }
    push(&mut current, &mut segments);
    segments
}

/// A single command segment matches a pattern when the head (command name) matches and the
/// argument tokens match the rest of the pattern as a token-level glob.
fn segment_matches(seg: &[String], pat: &[&str]) -> bool {
    match (seg.first(), pat.first()) {
        (Some(head), Some(phead)) => matches(head, phead) && tokens_match(&seg[1..], &pat[1..]),
        _ => false,
    }
}

/// Classic two-pointer wildcard over a token slice: a `*` pattern token matches any run of
/// argument tokens; every other pattern token must match one argument token (via [`matches`], so
/// tokens may themselves carry `*`/`?`), consecutively except across a `*`.
fn tokens_match(items: &[String], pats: &[&str]) -> bool {
    let (mut ii, mut pi) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None; // (pattern index, item index at the star)
    while ii < items.len() {
        if pi < pats.len() && pats[pi] != "*" && matches(&items[ii], pats[pi]) {
            ii += 1;
            pi += 1;
        } else if pi < pats.len() && pats[pi] == "*" {
            star = Some((pi, ii));
            pi += 1;
        } else if let Some((sp, si)) = star {
            pi = sp + 1;
            star = Some((sp, si + 1));
            ii = si + 1;
        } else {
            return false;
        }
    }
    while pi < pats.len() && pats[pi] == "*" {
        pi += 1;
    }
    pi == pats.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recompiling on a repeated public match must not replace the cached regex allocation.
    #[test]
    fn repeated_matches_reuse_the_compiled_glob() {
        let pattern = "permission-cache-proof-?";
        let cached = || {
            COMPILED_GLOBS
                .lock()
                .unwrap()
                .get(pattern)
                .cloned()
                .flatten()
                .expect("matches populates the cache")
        };
        assert!(matches("permission-cache-proof-a", pattern));
        let first = cached();
        assert!(!matches("permission-cache-proof-ab", pattern));
        let second = cached();
        assert!(Arc::ptr_eq(&first, &second), "the same pattern was compiled again");
    }

    #[test]
    fn glob_basics() {
        assert!(matches("anything", "*"));
        assert!(matches("test.rs", "*.rs"));
        assert!(!matches("test.txt", "*.rs"));
        assert!(matches("file1.txt", "file?.txt"));
        assert!(!matches("file12.txt", "file?.txt"));
    }

    #[test]
    fn trailing_star_matches_bare_prefix() {
        assert!(matches("git", "git *"));
        assert!(matches("git status", "git *"));
        assert!(!matches("gitk", "git *"));
    }

    #[test]
    fn dots_are_literal_not_regex() {
        assert!(matches("a.env", "*.env"));
        assert!(!matches("aXenv", "*.env"));
    }

    #[test]
    fn command_flags_do_not_defeat_matching() {
        // OpenCode issue #6676: "kill *" must match "kill -9 1234".
        assert!(matches_command("kill -9 1234", "kill *"));
        assert!(matches_command("rm -rf /", "rm *"));
        assert!(matches_command("rm -rf /", "rm -rf *"));
    }

    #[test]
    fn buried_command_in_a_chain_is_caught() {
        assert!(matches_command("ls && rm -rf /", "rm -rf *"));
        assert!(matches_command("cd /tmp; rm -rf .", "rm *"));
        assert!(matches_command("cat x | tee y", "tee *"));
        assert!(!matches_command("echo rm -rf /", "rm *")); // 'rm' is an arg to echo, not a head
    }

    /// Every separator gets its own case, because one input that happens to contain several of
    /// them proves only that the function works on that input — not that each arm is load-bearing.
    #[test]
    fn each_shell_separator_splits_on_its_own() {
        for chain in [
            "ls && rm -rf /",
            "ls || rm -rf /",
            "ls ; rm -rf /",
            "ls | rm -rf /",
            "ls\nrm -rf /",
            // A lone `&` backgrounds `ls` and runs `rm` — it hid the `rm` from every bash rule
            // until it was made a separator like the rest.
            "ls & rm -rf /",
        ] {
            assert!(matches_command(chain, "rm *"), "the rm is hidden in: {chain:?}");
        }
    }

    /// Skipping two characters for a lone pipe hides the next command when there is no space.
    #[test]
    fn an_unspaced_pipe_keeps_the_following_command_head() {
        assert_eq!(command_segments("|a"), vec![vec!["a"]]);
        assert_eq!(
            command_segments("git status|curl example.invalid"),
            vec![vec!["git", "status"], vec!["curl", "example.invalid"]]
        );
        assert!(matches_command("echo ok|rm -rf /", "rm *"));
        assert!(matches_command("rm -rf /|echo ok", "rm *"));
    }

    /// Redirects spell themselves with the same character a background `&` uses. Splitting on
    /// those would cut a command in half and hand the matcher a head that was never a command.
    #[test]
    fn a_redirect_is_not_a_separator() {
        assert_eq!(command_segments("a&&b"), vec![vec!["a"], vec!["b"]]);
        assert_eq!(command_segments("abc&&def"), vec![vec!["abc"], vec!["def"]]);
        assert_eq!(command_segments("a||b"), vec![vec!["a"], vec!["b"]]);
        assert_eq!(command_segments("abc||def"), vec![vec!["abc"], vec!["def"]]);
        assert_eq!(command_segments("cmd 2>&1"), vec![vec!["cmd", "2>&1"]]);
        assert_eq!(command_segments("cmd &>out"), vec![vec!["cmd", "&>out"]]);
        assert_eq!(command_segments("cmd <&3"), vec![vec!["cmd", "<&3"]]);
        // Double operators take priority over the single-ampersand redirect exception
        assert_eq!(command_segments("cmd >&&next"), vec![vec!["cmd", ">"], vec!["next"]]);
        assert_eq!(command_segments("cmd <&&next"), vec![vec!["cmd", "<"], vec!["next"]]);
        // And a redirect still does not smuggle a second command past the matcher.
        assert!(matches_command("rm -rf / 2>&1", "rm *"));
        assert!(!matches_command("cmd 2>&1", "1"));
    }

    /// Empty segments before or after an ampersand must not become commands.
    #[test]
    fn a_trailing_ampersand_yields_one_segment() {
        assert_eq!(command_segments("&a"), vec![vec!["a"]]);
        assert_eq!(command_segments("sleep 10 &"), vec![vec!["sleep", "10"]]);
        assert!(matches_command("rm -rf / &", "rm *"));
    }

    /// The documented limits, pinned so a future change to the splitter has to face them. A
    /// determined model can obfuscate around this matcher; bash rules are a guardrail, not a
    /// boundary, and pretending otherwise is how a guardrail gets trusted as one.
    #[test]
    fn substitution_and_subshells_are_known_gaps() {
        assert!(!matches_command("echo $(rm -rf /)", "rm *"));
        assert!(!matches_command("echo `rm -rf /`", "rm *"));
        assert!(!matches_command("(rm -rf /)", "rm *"));
    }

    #[test]
    fn head_must_match_command_name() {
        assert!(matches_command("git push --force", "git push *"));
        assert!(!matches_command("git log", "git push *"));
        assert!(matches_command("git commit -m x", "git commit *"));
    }

    #[test]
    fn bare_pattern_requires_no_args() {
        assert!(matches_command("rm", "rm"));
        assert!(!matches_command("rm -rf /", "rm"));
    }

    #[test]
    fn star_skips_runs_of_tokens() {
        assert!(matches_command("git push origin main --force", "git push * --force"));
        assert!(matches_command("git push --force", "git push * --force"));
    }
}
