//! One project file, as the app's Files tab shows it.
//!
//! Each line numbered and highlighted the way a diff's lines are, so a file reads like the context
//! lines of its diff, and like those it keeps its indentation and scrolls sideways.

use super::diff::note;
use super::{escape, Renderer};
use luxe_core::project_files::{ProjectFile, ProjectFileFailure};
use std::path::Path;

/// The file's lines, or a line saying why there are none, with a note when the text stops short.
pub(super) fn file(renderer: &dyn Renderer, file: &ProjectFile) -> String {
    if file.binary {
        return note("Binary file");
    }
    if file.text.is_empty() {
        return note("Empty file");
    }
    // core labels a diff's language by its extension, so a file's is the same
    let language = Path::new(&file.path).extension().and_then(|extension| extension.to_str());
    let lines: String = file
        .text
        .lines()
        .enumerate()
        .map(|(index, line)| line_html(renderer, language, index + 1, line))
        .collect();
    let path = escape(&file.path);
    format!("<div class=\"file\" data-path=\"{path}\">{lines}</div>{}", cut_short(file))
}

/// Why the file couldn't be read, in core's words.
pub(super) fn failed(error: &ProjectFileFailure) -> String {
    note(&escape(&error.to_string()))
}

fn line_html(renderer: &dyn Renderer, language: Option<&str>, number: usize, text: &str) -> String {
    format!(
        "<p class=\"fl\" data-line=\"{number}\"><span class=\"ln\">{number}</span><code>{}</code></p>",
        renderer.code_line(text, language)
    )
}

fn cut_short(file: &ProjectFile) -> String {
    if !file.truncated {
        return String::new();
    }
    note(&format!("First {} of {}", size(file.text.len() as u64), size(file.bytes)))
}

/// Bytes the way a person reads them: `900 B`, `12 KB`, `3.4 MB`.
fn size(bytes: u64) -> String {
    const KB: u64 = 1024;
    match bytes {
        0..KB => format!("{bytes} B"),
        KB..1_048_576 => format!("{} KB", bytes / KB),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

#[cfg(test)]
mod tests;
