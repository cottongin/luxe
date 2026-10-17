//! seen — the files a shell command printed for the model, so a `cat` counts as a read.
//!
//! The read-before-edit guard asks "have you looked at this file?", and a model that looked with
//! `cat` has. Counting only `read` would turn the commonest opening move — `cat` the files, then
//! edit them — into a refused edit per file and a second round of reads, which is all friction and
//! no protection: a one-line `read` already satisfies the guard, and what actually stops a blind
//! edit is `edit`'s exact-match rule (see [`luxe_core::read_tracker`]).
//!
//! So a command that printed files counts, read narrowly. A segment counts only when its output
//! reaches the model as printed: not redirected, not backgrounded, and piped only into `head` or
//! `tail` — `cat f | head -60` shows the model what `head -60 f` does, which is a slice of the file
//! like a `read` with a `limit`. Only the plain printers count — `cat`, `head`, `tail`, `nl`, and
//! `sed -n` (never `sed -i`) — with their file arguments resolved against the project and simple
//! globs expanded. Anything this cannot read with confidence counts for nothing, which leaves the
//! guard as strict as `read` alone.
//!
//! A search counts too, for the files whose lines it printed: `grep -n -A8 'def x' f.py` shows the
//! model a slice of `f.py` as surely as `sed -n` does, and grepping for the spot and then editing it
//! is how models work. Refusing that edit costs a round trip and a `read` of text the model had
//! just seen. A search that prints no lines — `-l`, `-c`, `-q`, `-o` — counts for nothing, and one
//! over many files counts the files its output names, since that output is what the model read.

use std::path::{Path, PathBuf};

/// The programs whose output is the file itself, or a slice of it.
const PRINTERS: &[&str] = &["cat", "head", "tail", "nl", "sed"];

/// Flags of `head`/`tail` that take a value as the next word.
const VALUE_FLAGS: &[&str] = &["-n", "-c", "--lines", "--bytes"];

/// The programs whose output is lines of the files they search.
const SEARCHERS: &[&str] = &["grep", "egrep", "fgrep", "rg"];

/// Short flags of a searcher that take a value, as the next word or attached (`-A8`).
const SEARCH_VALUE_SHORT: &str = "ABCefmdDgtTMj";

/// Long flags of a searcher that take a value as the next word when not given one with `=`.
const SEARCH_VALUE_LONG: &[&str] = &[
    "--regexp",
    "--file",
    "--after-context",
    "--before-context",
    "--context",
    "--max-count",
    "--include",
    "--exclude",
    "--exclude-dir",
    "--glob",
    "--type",
    "--type-not",
    "--max-columns",
    "--threads",
];

/// Searcher flags under which nothing printed is a line of a file.
const SEARCH_NO_LINES_SHORT: &str = "lLcqo";
const SEARCH_NO_LINES_LONG: &[&str] = &[
    "--files-with-matches",
    "--files-without-match",
    "--count",
    "--count-matches",
    "--quiet",
    "--silent",
    "--only-matching",
    "--files",
];

/// The existing files `command` printed, resolved against `cwd`.
///
/// `stdout` is what the command printed, which names the files a search over many of them showed.
pub(crate) fn files_shown(command: &str, cwd: &Path, stdout: &str) -> Vec<PathBuf> {
    let mut shown = Vec::new();
    let add = |path: PathBuf, shown: &mut Vec<PathBuf>| {
        if path.is_file() && !shown.contains(&path) {
            shown.push(path);
        }
    };
    let mut searched_many = false;
    for stages in segments(command) {
        let Some((first, slices)) = stages.split_first() else { continue };
        let Some(words) = shlex::split(first) else { continue };
        let sliced =
            slices.iter().all(|stage| shlex::split(stage).is_some_and(|words| slice(&words)));
        if !sliced {
            continue;
        }
        if let Some(paths) = search_paths(&words) {
            let paths: Vec<PathBuf> = paths.iter().flat_map(|word| expand(word, cwd)).collect();
            match paths.as_slice() {
                // One file prints its lines bare, with no name in front to find
                [file] if file.is_file() && !stdout.trim().is_empty() => {
                    add(file.clone(), &mut shown);
                }
                _ => searched_many = true,
            }
            continue;
        }
        for word in arguments(&words) {
            for path in expand(&word, cwd) {
                add(path, &mut shown);
            }
        }
    }
    if searched_many {
        for path in files_in_rows(stdout, cwd) {
            add(path, &mut shown);
        }
    }
    shown
}

/// The files named at the head of search-result rows, `path:12:text` or `path-12-text`.
///
/// Only names that are files under `cwd` count, so a compiler's `src/a.rs:3:1: error` in the same
/// output is the only kind of row that could be mistaken for one, and a search was asked for.
pub(crate) fn files_in_rows(output: &str, cwd: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut misses: Vec<&str> = Vec::new();
    for row in output.lines().take(5_000) {
        let bytes = row.as_bytes();
        for (at, byte) in bytes.iter().enumerate() {
            if !matches!(byte, b':' | b'-') || at == 0 {
                continue;
            }
            let digits = bytes[at + 1..].iter().take_while(|b| b.is_ascii_digit()).count();
            let closes = bytes.get(at + 1 + digits).is_some_and(|b| *b == b':' || *b == b'-');
            if digits == 0 || !closes {
                continue;
            }
            let name = &row[..at];
            if misses.contains(&name) {
                continue;
            }
            let path = cwd.join(name);
            if path.is_file() {
                if !found.contains(&path) {
                    found.push(path);
                }
                break;
            }
            misses.push(name);
        }
    }
    found
}

/// A searcher's path arguments, or `None` when `words` is not a search whose output is lines.
///
/// A search given no path reads standard input or the working directory, which comes back as
/// `Some` of nothing, so its rows are what name the files.
fn search_paths(words: &[String]) -> Option<Vec<String>> {
    let program = words.first()?;
    let program = program.rsplit('/').next().unwrap_or(program);
    if !SEARCHERS.contains(&program) {
        return None;
    }
    let mut positional = Vec::new();
    let mut pattern_given = false;
    let mut words = words[1..].iter();
    let mut flags_done = false;
    while let Some(word) = words.next() {
        if flags_done || !word.starts_with('-') || word == "-" {
            positional.push(word.clone());
        } else if word == "--" {
            flags_done = true;
        } else if let Some(long) = word.strip_prefix("--") {
            let name = format!("--{}", long.split('=').next().unwrap_or(long));
            if SEARCH_NO_LINES_LONG.contains(&name.as_str()) {
                return None;
            }
            if name == "--regexp" || name == "--file" {
                pattern_given = true;
            }
            if SEARCH_VALUE_LONG.contains(&name.as_str()) && !long.contains('=') {
                words.next();
            }
        } else {
            for (at, flag) in word.char_indices().skip(1) {
                if SEARCH_NO_LINES_SHORT.contains(flag) {
                    return None;
                }
                if SEARCH_VALUE_SHORT.contains(flag) {
                    if flag == 'e' || flag == 'f' {
                        pattern_given = true;
                    }
                    if at + 1 == word.len() {
                        words.next();
                    }
                    break;
                }
            }
        }
    }
    if !pattern_given && !positional.is_empty() {
        positional.remove(0);
    }
    Some(positional.into_iter().filter(|word| word != "-").collect())
}

/// The segments of a command list whose output reaches the model, each as its pipeline's stages.
///
/// Split on `;`, newlines, `&&` and `||` outside quotes, and each segment on `|`. A segment with a
/// redirection, a lone `&`, a substitution or a heredoc is dropped whole rather than parsed further.
fn segments(command: &str) -> Vec<Vec<&str>> {
    let mut out = Vec::new();
    let mut stages = Vec::new();
    let (mut start, mut clean) = (0, true);
    let (mut single, mut double) = (false, false);
    let bytes = command.as_bytes();
    let mut index = 0;
    fn close<'a>(
        out: &mut Vec<Vec<&'a str>>,
        mut stages: Vec<&'a str>,
        last: &'a str,
        clean: bool,
    ) {
        stages.push(last.trim());
        if clean && stages.iter().all(|stage| !stage.is_empty()) {
            out.push(stages);
        }
    }
    while index < bytes.len() {
        let byte = bytes[index];
        match byte {
            b'\'' if !double => single = !single,
            b'"' if !single => double = !double,
            b'\\' if !single => index += 1,
            b'`' | b'$' if !single => {
                // `$(…)` and backticks run something else; `$VAR` and `${VAR}` are just words
                if byte == b'`' || bytes.get(index + 1) == Some(&b'(') {
                    clean = false;
                }
            }
            _ if single || double => {}
            b';' | b'\n' => {
                close(&mut out, std::mem::take(&mut stages), &command[start..index], clean);
                (start, clean) = (index + 1, true);
            }
            b'&' | b'|' if bytes.get(index + 1) == Some(&byte) => {
                close(&mut out, std::mem::take(&mut stages), &command[start..index], clean);
                index += 1;
                (start, clean) = (index + 1, true);
            }
            b'|' => {
                stages.push(command[start..index].trim());
                start = index + 1;
            }
            // `2>` sends only stderr away, so what the model sees is untouched; `2>&1` folds stderr
            // into it, which hides nothing either. Any other redirection may hide the output.
            b'>' if stderr_redirect(bytes, index) => {
                if bytes.get(index + 1) == Some(&b'&') {
                    index += 1;
                }
            }
            b'&' | b'>' | b'<' => clean = false,
            _ => {}
        }
        index += 1;
    }
    close(&mut out, stages, &command[start..], clean && !single && !double);
    out
}

/// Whether the `>` at `at` is a `2>` standing on its own, not the tail of a longer number.
fn stderr_redirect(bytes: &[u8], at: usize) -> bool {
    at >= 1 && bytes[at - 1] == b'2' && (at == 1 || bytes[at - 2].is_ascii_whitespace())
}

/// Whether a pipeline stage only cuts its input down: `head` or `tail` with flags and no files.
fn slice(words: &[String]) -> bool {
    let Some(program) = words.first() else { return false };
    if !matches!(program.as_str(), "head" | "tail") {
        return false;
    }
    let follows = words.iter().any(|word| word == "-f" || word == "-F" || word == "--follow");
    !follows && arguments(words).is_empty()
}

/// The file arguments of one printer invocation, or none for anything else.
fn arguments(words: &[String]) -> Vec<String> {
    let Some(program) = words.first() else { return Vec::new() };
    let program = program.rsplit('/').next().unwrap_or(program);
    if !PRINTERS.contains(&program) {
        return Vec::new();
    }
    let rest = &words[1..];
    if program == "sed" {
        return sed_files(rest);
    }
    // `tail -f` never finishes, and one that did would print what arrived, not the file
    if program == "tail"
        && rest.iter().any(|word| word == "-f" || word == "-F" || word == "--follow")
    {
        return Vec::new();
    }
    let mut files = Vec::new();
    let mut words = rest.iter();
    let mut flags_done = false;
    while let Some(word) = words.next() {
        if word == "-" {
            // standard input, not a file called `-`, even past `--`
        } else if !flags_done && word == "--" {
            flags_done = true;
        } else if !flags_done && VALUE_FLAGS.contains(&word.as_str()) {
            words.next();
        } else if !flags_done && word.starts_with('-') {
            // a bundled flag such as `-40` or `-n40` carries its own value
        } else {
            files.push(word.clone());
        }
    }
    files
}

/// `sed -n 'script' file…`: the files after the script, and only for a `-n` that edits nothing.
fn sed_files(words: &[String]) -> Vec<String> {
    let flags: Vec<&String> = words.iter().take_while(|word| word.starts_with('-')).collect();
    let quiet = flags.iter().any(|flag| *flag == "-n" || *flag == "--quiet" || *flag == "--silent");
    let in_place =
        flags.iter().any(|flag| flag.starts_with("-i") || flag.starts_with("--in-place"));
    let scripted = flags.iter().any(|flag| *flag == "-e" || *flag == "-f");
    if !quiet || in_place || scripted {
        return Vec::new();
    }
    words.iter().skip(flags.len() + 1).cloned().collect()
}

/// `word` as paths under `cwd`: itself, or what a glob in its last component matches.
fn expand(word: &str, cwd: &Path) -> Vec<PathBuf> {
    let path = cwd.join(word);
    if !word.contains(['*', '?', '[']) {
        return vec![path];
    }
    let (Some(parent), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str()))
    else {
        return Vec::new();
    };
    // A glob in a folder name is more than this needs to understand
    if parent.to_string_lossy().contains(['*', '?', '[']) {
        return Vec::new();
    }
    let Ok(matcher) = globset::Glob::new(name).map(|glob| glob.compile_matcher()) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(parent) else { return Vec::new() };
    let mut matched: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.file_name().is_some_and(|name| matcher.is_match(name)))
        .collect();
    matched.sort();
    matched
}

#[cfg(test)]
mod tests;
