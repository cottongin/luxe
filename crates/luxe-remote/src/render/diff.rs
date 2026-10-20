//! One file's diff, as the app shows it when a changed file is tapped.
//!
//! Unified, since a phone has no room for two columns, with each line's numbers, its sign and its
//! code highlighted the way the TUI's diff views highlight it. The row colors are the theme's diff
//! tints, applied by the app's stylesheet.

use super::{escape, Renderer};
use luxe_core::file_diff::{FileDiff, FileDiffFailure};
use luxe_core::guided_review::{ReviewFile, ReviewHunk, ReviewLine, ReviewLineKind};

/// The whole diff, or a line saying why there are no lines to show.
pub(super) fn file(renderer: &dyn Renderer, diff: &FileDiff) -> String {
    let Some(file) = diff.file.as_deref() else {
        return note("No change: the file matches the last commit.");
    };
    whole(renderer, file)
}

/// Every hunk of `file`, or a line saying why there are no lines to show.
pub(super) fn whole(renderer: &dyn Renderer, file: &ReviewFile) -> String {
    if !file.is_text() {
        return note("This change isn't text, so there are no lines to show.");
    }
    let hunks: String = file.hunks.iter().map(|hunk| hunk_html(renderer, file, hunk)).collect();
    format!("{}{hunks}</div>", opening(file))
}

/// A diff block's opening tag, naming the file its lines can be picked in: the new side's path,
/// since a pick points at the file as it is now. A deleted file has nothing to point at.
pub(super) fn opening(file: &ReviewFile) -> String {
    match file.new_path.as_deref() {
        Some(path) => {
            format!("<div class=\"diff\" data-path=\"{}\">", escape(&path.display().to_string()))
        }
        None => "<div class=\"diff\">".to_owned(),
    }
}

/// Why the diff couldn't be captured, in core's words.
pub(super) fn failed(error: &FileDiffFailure) -> String {
    note(&escape(&error.to_string()))
}

/// One hunk under its `@@` line.
pub(super) fn hunk_html(renderer: &dyn Renderer, file: &ReviewFile, hunk: &ReviewHunk) -> String {
    let (old, new) = (hunk.old_range, hunk.new_range);
    let heading = hunk.heading.as_deref().map(|heading| format!(" {}", escape(heading)));
    let mut out = format!(
        "<div class=\"hunk\"><p class=\"hunk-head\">@@ -{},{} +{},{} @@{}</p>",
        old.start,
        old.count,
        new.start,
        new.count,
        heading.unwrap_or_default()
    );
    for line in &hunk.lines {
        out.push_str(&line_html(renderer, file.language.as_deref(), line));
    }
    out + "</div>"
}

fn line_html(renderer: &dyn Renderer, language: Option<&str>, line: &ReviewLine) -> String {
    let (kind, sign) = match line.kind {
        ReviewLineKind::Context => ("context", " "),
        ReviewLineKind::Deletion => ("deletion", "-"),
        ReviewLineKind::Addition => ("addition", "+"),
        // a note about the file, not a line of it, so it isn't code either
        ReviewLineKind::NoNewlineMarker => {
            return format!("<p class=\"dl dl-note\">{}</p>", escape(&line.text));
        }
    };
    // only a line the file still has can be picked; a deleted one has no number to point at
    let picks = line.new_line.map(|line| format!(" data-line=\"{line}\"")).unwrap_or_default();
    format!(
        "<p class=\"dl dl-{kind}\"{picks}><span class=\"ln\">{}</span><span class=\"ln\">{}</span>\
         <span class=\"sign\">{sign}</span><code>{}</code></p>",
        number(line.old_line),
        number(line.new_line),
        renderer.code_line(&line.text, language)
    )
}

fn number(line: Option<u32>) -> String {
    line.map(|line| line.to_string()).unwrap_or_default()
}

/// A line about the file rather than of it; `words` must already be escaped.
pub(super) fn note(words: &str) -> String {
    format!("<p class=\"diff-note\">{words}</p>")
}

#[cfg(test)]
mod tests;
