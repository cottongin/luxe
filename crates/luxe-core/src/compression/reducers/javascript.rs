//! JavaScript — jest, vitest, and Node's own runner in its spec and TAP reporters.

use super::{count_before, push_unique, TestFailure, TestSummary};

pub(super) fn parse_jest(text: &str) -> Option<TestSummary> {
    let mut summary = TestSummary { framework: "jest".to_string(), ..Default::default() };
    let mut found = false;
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("Tests:") {
            if rest.contains("total") {
                found = true;
                summary.passed = count_before(rest, "passed").unwrap_or(0);
                summary.failed = count_before(rest, "failed").unwrap_or(0);
                summary.ignored = count_before(rest, "skipped")
                    .or_else(|| count_before(rest, "todo"))
                    .unwrap_or(0);
            }
        } else if let Some(name) = t.strip_prefix("✕ ").or_else(|| t.strip_prefix("× ")) {
            let name = name.rsplit_once(" (").map(|(n, _)| n).unwrap_or(name);
            push_unique(&mut summary.failures, name.trim());
        } else if let Some(name) = t.strip_prefix("● ").filter(|name| !is_jest_notice(name)) {
            // Where a default run actually names its failures: `  ● math › multiplies`, repeated in
            // the "summary of all failing tests" block, which `push_unique` folds back together.
            // The `✕` list above is verbose mode, so on its own it found nothing at all.
            push_unique(&mut summary.failures, name.trim());
        }
    }
    if found {
        Some(summary)
    } else {
        None
    }
}

/// Whether a jest `●` header introduces something other than a failing test.
///
/// The bullet is jest's section marker, not a failure marker — console output and deprecation
/// notices wear it too, and a run whose only "failures" were warnings would name them as tests.
fn is_jest_notice(header: &str) -> bool {
    ["Console", "Deprecation", "Validation", "Warning"]
        .iter()
        .any(|notice| header.starts_with(notice))
}

/// vitest, which reports the same facts as jest in a different shape.
///
/// Its counts line carries no colon and separates with `|`, and its failure names arrive as
/// `FAIL  file > test` — so jest's parser matched none of it, and every vitest run fell through to a
/// raw dump of the runner's own output.
pub(super) fn parse_vitest(text: &str) -> Option<TestSummary> {
    let mut summary = TestSummary { framework: "vitest".to_string(), ..Default::default() };
    let mut found = false;
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) =
            t.strip_prefix("Tests").filter(|rest| rest.starts_with(char::is_whitespace))
        {
            found = true;
            summary.passed = count_before(rest, "passed").unwrap_or(0);
            summary.failed = count_before(rest, "failed").unwrap_or(0);
            summary.ignored =
                count_before(rest, "skipped").or_else(|| count_before(rest, "todo")).unwrap_or(0);
        } else if let Some(name) = t.strip_prefix("FAIL ").filter(|name| name.contains(" > ")) {
            // The file-level `FAIL x.test.js` line says nothing a count does not; the one carrying
            // `>` names the test, and reads back as a filter.
            push_unique(&mut summary.failures, name.trim());
        }
    }
    found.then_some(summary)
}

/// Node's built-in runner (`node --test`), in either of its reporters.
///
/// The spec reporter is what a terminal gets: `✔`/`✖` per test, an `ℹ tests 5` block of counts, and
/// a `✖ failing tests:` section repeating each failure under `test at <file:line:col>` with its
/// assertion below. TAP is what older Nodes print to a pipe: `# pass 3` counts and `not ok 2 - name`
/// with a YAML block carrying `location` and `error`. Either becomes the same summary, which is the
/// point — left unparsed, one failing assertion came back as forty lines of runner chatter.
pub(super) fn parse_node(text: &str) -> Option<TestSummary> {
    let mut summary = TestSummary { framework: "node".to_string(), ..Default::default() };
    let count = |line: &str, keys: &[&str]| {
        keys.iter().find_map(|key| {
            line.strip_prefix(key)?.split_whitespace().next()?.parse::<usize>().ok()
        })
    };
    let (mut tests, mut spec, mut tap) = (None, false, false);
    for line in text.lines() {
        let t = line.trim();
        if let Some(n) = count(t, &["ℹ tests ", "# tests "]) {
            tests = Some(n);
        }
        if let Some(n) = count(t, &["ℹ pass ", "# pass "]) {
            summary.passed = n;
            spec |= t.starts_with('ℹ');
            tap |= t.starts_with('#');
        }
        if let Some(n) = count(t, &["ℹ fail ", "# fail "]) {
            summary.failed = n;
        }
        if let Some(n) = count(t, &["ℹ skipped ", "# skipped "]) {
            summary.ignored += n;
        }
        if let Some(n) = count(t, &["ℹ todo ", "# todo "]) {
            summary.ignored += n;
        }
    }
    tests?;
    if spec {
        node_spec_failures(text, &mut summary);
    } else if tap {
        node_tap_failures(text, &mut summary);
    }
    explain_file_failures(text, &mut summary);
    Some(summary)
}

/// A test file that died before its tests ran, as Node prints it above the results.
struct FileCrash {
    /// The path from the `file://` header, forward slashes
    path: String,
    line: String,
    /// The error's headline, then the source line and caret that point at it
    message: String,
}

/// Every file crash in the output: a `file:///…:N` header, the source line and caret, then the
/// error itself, possibly after a blank line. TAP prints the same block as `#` comments.
fn node_file_crashes(text: &str) -> Vec<FileCrash> {
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            let line = line.strip_prefix("# ").or_else(|| line.strip_prefix('#')).unwrap_or(line);
            line.replace("\\#", "#")
        })
        .collect();
    let mut crashes = Vec::new();
    for (at, header) in lines.iter().enumerate() {
        let Some(place) = header.trim().strip_prefix("file://") else { continue };
        let Some((path, line)) = place.rsplit_once(':') else { continue };
        if line.parse::<u32>().is_err() {
            continue;
        }
        let window = &lines[at + 1..lines.len().min(at + 6)];
        let Some(error) = window.iter().position(|line| is_error_headline(line)) else { continue };
        let pointer: Vec<&str> = window[..error]
            .iter()
            .map(|line| line.trim_end())
            .filter(|line| !line.trim().is_empty())
            .collect();
        let mut message = window[error].trim_end().to_string();
        for line in pointer {
            message.push('\n');
            message.push_str(line);
        }
        crashes.push(FileCrash { path: path.replace('\\', "/"), line: line.to_string(), message });
    }
    crashes
}

/// `SyntaxError: …`, `ReferenceError: …`, `Error [ERR_MODULE_NOT_FOUND]: …` — an error's first
/// line, flush left the way Node prints an uncaught one.
fn is_error_headline(line: &str) -> bool {
    let Some((kind, _)) = line.split_once(": ") else { return false };
    let kind = kind.split_once(" [").map_or(kind, |(kind, _)| kind);
    !kind.is_empty()
        && kind.ends_with("Error")
        && kind.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Node reports a file that crashed while loading as one failure named for the file, whose only
/// reason is `test failed`. The real reason was printed above the results, so it replaces that,
/// and the location moves to the line that crashed. Without it a missing export read as a failing
/// test, and the model was left to guess.
fn explain_file_failures(text: &str, summary: &mut TestSummary) {
    let placeholder = |message: &Option<String>| {
        message.as_deref().is_none_or(|message| message.trim().trim_matches('\'') == "test failed")
    };
    if !summary.failures.iter().any(|failure| placeholder(&failure.message)) {
        return;
    }
    let crashes = node_file_crashes(text);
    for failure in summary.failures.iter_mut().filter(|failure| placeholder(&failure.message)) {
        let Some(location) = failure.location.as_deref() else { continue };
        let file = location.rsplit_once(':').map_or(location, |(file, _)| file).replace('\\', "/");
        let Some(crash) = crashes.iter().find(|crash| {
            crash.path == file
                || crash.path.ends_with(&format!("/{}", file.trim_start_matches("./")))
        }) else {
            continue;
        };
        failure.location = Some(format!(
            "{}:{}",
            location.rsplit_once(':').map_or(location, |(file, _)| file),
            crash.line
        ));
        failure.message = Some(crash.message.clone());
    }
}

/// A test's name as the spec reporter prints it, without the `(0.53ms)` it ends in.
fn without_duration(name: &str) -> &str {
    match name.rfind(" (") {
        Some(at) if name.ends_with("ms)") => &name[..at],
        _ => name,
    }
}

/// `test/filter.test.js:19:1` → `test/filter.test.js:19`, which is where an editor wants to go.
fn without_column(location: &str) -> String {
    let location = location.trim().trim_matches('\'');
    match location.rsplit_once(':') {
        Some((head, column))
            if column.chars().all(|c| c.is_ascii_digit()) && head.contains(':') =>
        {
            head.to_string()
        }
        _ => location.to_string(),
    }
}

/// Most lines of an assertion kept: the headline and the actual-versus-expected diff under it,
/// which is what tells the model what to change. The stack after them does not.
const NODE_MESSAGE_LINES: usize = 10;

/// An assertion's lines, ending before the stack trace and without the blank lines around them.
fn assertion(lines: Vec<&str>) -> Option<String> {
    let kept: Vec<&str> = lines
        .into_iter()
        .take_while(|line| !line.trim_start().starts_with("at ") && line.trim() != "{")
        .take(NODE_MESSAGE_LINES)
        .collect();
    let message = kept.join("\n");
    let message = message.trim_matches('\n').trim_end();
    (!message.trim().is_empty()).then(|| message.to_string())
}

/// The failures from the spec reporter's `✖ failing tests:` section, each with where and why.
fn node_spec_failures(text: &str, summary: &mut TestSummary) {
    let Some((_, section)) = text.split_once("✖ failing tests:") else { return };
    let lines: Vec<&str> = section.lines().collect();
    let mut location = None;
    let mut at = 0;
    while at < lines.len() {
        let line = lines[at].trim();
        at += 1;
        if let Some(place) = line.strip_prefix("test at ") {
            location = Some(without_column(place));
        } else if let Some(name) = line.strip_prefix("✖ ") {
            // The assertion is indented under its test; the next `test at` starts another
            let body: Vec<&str> = lines[at..]
                .iter()
                .take_while(|next| !next.trim_start().starts_with("test at "))
                .map(|next| next.strip_prefix("  ").unwrap_or(next))
                .collect();
            at += body.len();
            let name = without_duration(name).to_string();
            if !summary.failures.iter().any(|failure| failure.name == name) {
                let message = assertion(body);
                summary.failures.push(TestFailure { name, location: location.take(), message });
            }
        }
    }
}

/// The failures from TAP's `not ok` lines, with the `location` and `error` of the YAML below each.
fn node_tap_failures(text: &str, summary: &mut TestSummary) {
    let lines: Vec<&str> = text.lines().collect();
    let mut current: Option<usize> = None;
    let mut at = 0;
    while at < lines.len() {
        let line = lines[at];
        at += 1;
        let t = line.trim();
        // Top level only: a failing subtest is reported again by the test that holds it
        if let Some(rest) = line.strip_prefix("not ok ") {
            let name = rest.split_once(" - ").map_or(rest, |(_, name)| name).trim();
            summary.failures.push(TestFailure {
                name: name.replace("\\#", "#"),
                location: None,
                message: None,
            });
            current = Some(summary.failures.len() - 1);
        } else if line.starts_with("ok ") {
            current = None;
        } else if let Some(failure) = current.and_then(|at| summary.failures.get_mut(at)) {
            if let Some(location) = t.strip_prefix("location: ") {
                failure.location = Some(without_column(location));
            } else if let Some(error) = t.strip_prefix("error: ") {
                // `|-` opens a block: the message is every line indented past the key's own
                let inline = error.trim().trim_matches('\'');
                if !inline.starts_with('|') {
                    failure.message = (!inline.is_empty()).then(|| inline.to_string());
                    continue;
                }
                let key = line.len() - line.trim_start().len();
                let block: Vec<&str> = lines[at..]
                    .iter()
                    .take_while(|next| {
                        next.trim().is_empty() || next.len() - next.trim_start().len() > key
                    })
                    .map(|next| next.get(key + 2..).unwrap_or("").trim_end())
                    .collect();
                at += block.len();
                failure.message = assertion(block);
            }
        }
    }
}

#[cfg(test)]
pub(super) mod tests;
