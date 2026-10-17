//! Bash display — a `bash` command as a card shows it, without the `cd` that does nothing.

use std::path::Path;

/// Drop a leading `cd <dir> &&` from a shell command when `dir` is where it would have started
/// anyway.
///
/// `bash` spawns a fresh shell rooted at the working directory on every call, so this prefix does
/// nothing at all — and it is the most common thing a model writes, most of the time naming the
/// directory the shell was already in. The habit is a model-family prior rather than anything the
/// harness asks for. The cost lands on the reader, whose card leads with fifty characters of
/// no-op before the command they came to see.
///
/// Only the *display* drops it. What ran is unchanged, and the session transcript keeps the model's
/// arguments verbatim.
///
/// Conservative by construction, because a `cd` that is not a no-op is load-bearing: it fires only
/// for a literal path naming `root`, never for one the shell would have to expand, never for an
/// operator other than `&&` or `;` (which are equivalent once the `cd` cannot fail), and never when
/// nothing follows it.
///
/// `root` comes from the caller, so a label is a pure function of the call and where it ran.
pub(super) fn strip_redundant_cd<'a>(command: &'a str, root: &Path) -> &'a str {
    redundant_cd_tail(command, root).unwrap_or(command)
}

/// What is left of `command` once a redundant `cd` prefix is removed, or `None` if there isn't one.
fn redundant_cd_tail<'a>(command: &'a str, root: &Path) -> Option<&'a str> {
    let rest = command.trim_start().strip_prefix("cd")?;
    // `cdate` starts with the same two letters and is not this.
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let rest = rest.trim_start();
    let (target, after) = match rest.chars().next()? {
        quote @ ('"' | '\'') => {
            let body = &rest[quote.len_utf8()..];
            let end = body.find(quote)?;
            (&body[..end], &body[end + quote.len_utf8()..])
        }
        _ => {
            // An unquoted path ends at whitespace or at the operator that follows it, so
            // `cd /project;ls` and `cd /project && ls` yield the same target.
            let end = rest
                .find(|c: char| c.is_whitespace() || matches!(c, ';' | '&' | '|' | '>' | '<'))
                .unwrap_or(rest.len());
            rest.split_at(end)
        }
    };
    // A path only a shell could resolve is one this must not second-guess.
    if target.is_empty() || target.contains(['$', '`', '*', '?', '~']) {
        return None;
    }
    if !names_directory(Path::new(target), root) {
        return None;
    }
    let after = after.trim_start();
    let tail = after.strip_prefix("&&").or_else(|| after.strip_prefix(';'))?.trim_start();
    // A bare `cd` with nothing after it is the whole command; there would be no card left.
    (!tail.is_empty()).then_some(tail)
}

/// Whether `target` names `root` — or names it the long way round, as `.` does.
///
/// [`crate::paths::contains`] both ways is equality the way the platform compares filenames, which
/// also makes a trailing separator and an interior `.` the no-ops they are.
fn names_directory(target: &Path, root: &Path) -> bool {
    if target.components().all(|part| part == std::path::Component::CurDir) {
        return true;
    }
    crate::paths::contains(target, root) && crate::paths::contains(root, target)
}

#[cfg(test)]
mod tests;
