//! Review — the open Guided Review as the app shows it: the guide, then one chapter at a time.
//!
//! The guide is the TUI's guide pane as a page: the brief, then each chapter's number, kind and
//! title, with either its rationale and files or the one line saying nothing in it needs the
//! reader. That call is the host's (`open_review_shown`'s reading), since it's decided from lines
//! the app doesn't send back. A chapter is its items in order, grouped under the file they're in,
//! each with the guide's sentence for it above its diff, as the TUI's narration band puts it.

use super::{diff, escape, Renderer, Voice};
use luxe_core::guided_review::{
    ChapterId, ChapterKind, ChapterReading, GuidedReview, ReviewChapter, ReviewFile,
    ReviewFileStatus, ReviewFinding, ReviewItemRef,
};
use std::collections::HashMap;

mod findings;

/// File rows a chapter lists in the guide before the rest are counted, as the TUI windows them.
const FILE_ROWS: usize = 8;

/// The whole guide: the brief, then every chapter.
pub(super) fn guide(
    renderer: &dyn Renderer,
    review: &GuidedReview,
    reading: &[ChapterReading],
    walked: &[ChapterId],
) -> String {
    let owners = Owners::of(&review.files);
    let total = review.chapters.len();
    let chapters: String = review
        .chapters
        .iter()
        .enumerate()
        .map(|(index, chapter)| {
            let place = Place { number: index + 1, total, walked: walked.contains(&chapter.id) };
            let reading = reading.iter().find(|it| it.chapter_id == chapter.id);
            guide_chapter(renderer, chapter, &place, reading, &owners.files_of(chapter))
        })
        .collect();
    format!("<div class=\"rv-guide\">{}{chapters}</div>", brief(renderer, review))
}

/// One chapter opened: its heading, then its items with their notes and diffs.
pub(super) fn chapter(
    renderer: &dyn Renderer,
    chapter: &ReviewChapter,
    files: &[ReviewFile],
    place: &Place,
    reading: Option<&ChapterReading>,
    findings: &[ReviewFinding],
) -> String {
    let mut out = format!("<header class=\"rv-head\">{}", heading(chapter, place, reading));
    out.push_str(&prose(renderer, chapter, reading));
    out.push_str("</header>");
    out.push_str(&findings::on_chapter(renderer, findings, &chapter.id));
    out.push_str(&items_html(renderer, chapter, files, findings));
    format!("<div class=\"rv-chapter-view\">{out}</div>")
}

/// A chapter's items in its order, with a file's path said once above the run of items in it and
/// the comments on a hunk under it.
fn items_html(
    renderer: &dyn Renderer,
    chapter: &ReviewChapter,
    files: &[ReviewFile],
    findings: &[ReviewFinding],
) -> String {
    let mut out = String::new();
    let mut current: Option<&str> = None;
    for item in &chapter.items {
        let Some((file, hunk)) = locate(files, item) else { continue };
        if current != Some(file.id.as_str()) {
            current = Some(file.id.as_str());
            out.push_str(&format!("<p class=\"rv-file\">{}</p>", escape(&path_of(file))));
        }
        out.push_str(&item_html(renderer, chapter, file, hunk));
        let comments = hunk.map(|hunk| findings::on_hunk(renderer, findings, &hunk.id));
        out.push_str(&comments.unwrap_or_default());
    }
    out
}

/// Where a chapter sits: `number` of `total`, and whether the reader has walked it.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub(crate) struct Place {
    pub(crate) number: usize,
    pub(crate) total: usize,
    #[serde(default)]
    pub(crate) walked: bool,
}

/// The guide's opening: the summary's intent and claims, or the agent's own words when no summary
/// was written.
fn brief(renderer: &dyn Renderer, review: &GuidedReview) -> String {
    let words = match review.brief.as_ref() {
        Some(brief) => {
            let claims: String =
                brief.account.iter().map(|claim| bullet(renderer, claim)).collect();
            let claims = if claims.is_empty() { claims } else { format!("<ul>{claims}</ul>") };
            renderer.markdown(&brief.intent, Voice::Answer) + &claims
        }
        None => agents_words(renderer, review),
    };
    if words.is_empty() {
        return String::new();
    }
    format!("<section class=\"rv-brief md\">{words}</section>")
}

fn bullet(renderer: &dyn Renderer, claim: &str) -> String {
    format!("<li>{}</li>", renderer.markdown(claim, Voice::Answer))
}

/// What the turn was asked and what it said it did, with the TUI's line saying so.
fn agents_words(renderer: &dyn Renderer, review: &GuidedReview) -> String {
    let Some(provenance) = review.provenance.as_ref() else {
        return String::new();
    };
    let mut out: String =
        provenance.requests.iter().map(|it| renderer.markdown(it, Voice::Answer)).collect();
    if let Some(account) = provenance.account.as_deref() {
        out.push_str(&renderer.markdown(account, Voice::Answer));
        out.push_str(
            "<p class=\"rv-dim\">the agent\u{2019}s own words \u{2014} no summary was written for \
             this review</p>",
        );
    }
    out
}

fn guide_chapter(
    renderer: &dyn Renderer,
    chapter: &ReviewChapter,
    place: &Place,
    reading: Option<&ChapterReading>,
    files: &[&ReviewFile],
) -> String {
    let quiet = if needs_nothing(reading) { " quiet" } else { "" };
    let rows = if needs_nothing(reading) { String::new() } else { file_rows(files) };
    format!(
        "<section class=\"rv-chapter{quiet}\" data-chapter=\"{}\">{}{}{rows}</section>",
        escape(chapter.id.as_str()),
        heading(chapter, place, reading),
        prose(renderer, chapter, reading),
    )
}

/// `01 / 05   CORE   reviewed`, then the title, which opens the chapter.
fn heading(chapter: &ReviewChapter, place: &Place, reading: Option<&ChapterReading>) -> String {
    let walked = if place.walked { "<span class=\"rv-walked\">reviewed</span>" } else { "" };
    let quiet = if needs_nothing(reading) { " quiet" } else { "" };
    format!(
        "<p class=\"rv-meta\"><span>{:02} / {:02}</span><span>{}</span>{walked}</p>\
         <button type=\"button\" class=\"rv-title{quiet}\" data-chapter=\"{}\">{}</button>",
        place.number,
        place.total,
        kind_label(chapter.kind),
        escape(chapter.id.as_str()),
        escape(&chapter.title),
    )
}

/// The rationale and impact, or the one line that says why there's nothing to read.
fn prose(
    renderer: &dyn Renderer,
    chapter: &ReviewChapter,
    reading: Option<&ChapterReading>,
) -> String {
    if needs_nothing(reading) {
        let reason = reading.and_then(|it| it.reason.as_deref());
        let words = reason.map_or_else(
            || "nothing here needs you".to_owned(),
            |reason| format!("{reason} \u{2014} nothing here needs you"),
        );
        return format!("<p class=\"rv-reason\">{}</p>", escape(&words));
    }
    let rationale = renderer.markdown(&chapter.rationale, Voice::Answer);
    let impact = chapter.impact.as_deref().map(|impact| {
        format!("<div class=\"rv-impact\">{}</div>", renderer.markdown(impact, Voice::Answer))
    });
    format!("<div class=\"rv-prose md\">{rationale}{}</div>", impact.unwrap_or_default())
}

/// A chapter the host found nothing in for the reader. No reading means the host didn't say, and
/// that's not the same as nothing.
fn needs_nothing(reading: Option<&ChapterReading>) -> bool {
    reading.is_some_and(|reading| reading.needs_review == 0)
}

fn kind_label(kind: ChapterKind) -> &'static str {
    match kind {
        ChapterKind::Core => "CORE",
        ChapterKind::Consequence => "CONSEQUENCE",
        ChapterKind::Test => "TESTS",
        ChapterKind::Supporting => "SUPPORTING",
        ChapterKind::Generated => "GENERATED",
    }
}

fn file_rows(files: &[&ReviewFile]) -> String {
    let rows: String = files.iter().take(FILE_ROWS).map(|file| file_row(file)).collect();
    let more = files.len().saturating_sub(FILE_ROWS);
    let more = if more > 0 {
        format!("<li class=\"rv-more\">\u{2026} {more} more</li>")
    } else {
        String::new()
    };
    format!("<ul class=\"rv-files\">{rows}{more}</ul>")
}

/// `M path +12 −3`, or what the change was for a file with no lines to count.
fn file_row(file: &ReviewFile) -> String {
    let (letter, tone) = status(file.status);
    let change = match (file.non_text.as_ref(), file.additions, file.deletions) {
        (Some(change), _, _) => format!("<span class=\"rv-dim\">{}</span>", escape(&change.summary)),
        (None, Some(added), Some(deleted)) => format!(
            "<span class=\"rv-added\">+{added}</span><span class=\"rv-deleted\">\u{2212}{deleted}</span>"
        ),
        _ => String::new(),
    };
    format!(
        "<li><span class=\"rv-status {tone}\">{letter}</span><span class=\"rv-path\">{}</span>\
         {change}</li>",
        escape(&path_of(file))
    )
}

fn status(status: ReviewFileStatus) -> (&'static str, &'static str) {
    match status {
        ReviewFileStatus::Added => ("A", "tone-added"),
        ReviewFileStatus::Untracked => ("?", "tone-added"),
        ReviewFileStatus::Modified => ("M", "tone-modified"),
        ReviewFileStatus::Deleted => ("D", "tone-deleted"),
        ReviewFileStatus::Renamed => ("R", "tone-moved"),
        ReviewFileStatus::Copied => ("C", "tone-moved"),
    }
}

/// One item: the hunk's symbol and the guide's sentence for it, then its lines.
fn item_html(
    renderer: &dyn Renderer,
    chapter: &ReviewChapter,
    file: &ReviewFile,
    hunk: Option<&luxe_core::guided_review::ReviewHunk>,
) -> String {
    let Some(hunk) = hunk else {
        let note = chapter.note_for(&ReviewItemRef::File(file.id.clone()));
        return note_html(note) + &diff::whole(renderer, file);
    };
    let symbol = hunk.symbol.as_ref().and_then(|symbol| symbol.name.as_deref());
    let symbol = symbol.map(|it| format!("<p class=\"rv-symbol\">{}</p>", escape(it)));
    let note = note_html(chapter.note_for_hunk(&hunk.id));
    let lines = diff::hunk_html(renderer, file, hunk);
    format!("{}{note}{}{lines}</div>", symbol.unwrap_or_default(), diff::opening(file))
}

fn note_html(note: Option<&str>) -> String {
    note.map(|note| format!("<p class=\"rv-note\">{}</p>", escape(note))).unwrap_or_default()
}

/// The file an item is in and, for a hunk, the hunk; `None` when the app didn't send it.
fn locate<'a>(
    files: &'a [ReviewFile],
    item: &ReviewItemRef,
) -> Option<(&'a ReviewFile, Option<&'a luxe_core::guided_review::ReviewHunk>)> {
    match item {
        ReviewItemRef::File(id) => files.iter().find(|file| &file.id == id).map(|it| (it, None)),
        ReviewItemRef::Hunk(id) => files.iter().find_map(|file| {
            file.hunks.iter().find(|hunk| &hunk.id == id).map(|hunk| (file, Some(hunk)))
        }),
    }
}

fn path_of(file: &ReviewFile) -> String {
    let path = file.new_path.as_deref().or(file.old_path.as_deref());
    path.map_or_else(|| "(unknown path)".to_owned(), |path| path.display().to_string())
}

/// Which file holds each id a chapter can name, built once per guide rather than scanned per item.
struct Owners<'a> {
    by_id: HashMap<&'a str, &'a ReviewFile>,
}

impl<'a> Owners<'a> {
    fn of(files: &'a [ReviewFile]) -> Self {
        let mut by_id = HashMap::new();
        for file in files {
            by_id.insert(file.id.as_str(), file);
            for hunk in &file.hunks {
                by_id.insert(hunk.id.as_str(), file);
            }
        }
        Self { by_id }
    }

    /// A chapter's files in the order it names them, each once.
    fn files_of(&self, chapter: &ReviewChapter) -> Vec<&'a ReviewFile> {
        let mut files: Vec<&ReviewFile> = Vec::new();
        for item in &chapter.items {
            let id = match item {
                ReviewItemRef::File(id) => id.as_str(),
                ReviewItemRef::Hunk(id) => id.as_str(),
            };
            let Some(file) = self.by_id.get(id) else { continue };
            if !files.iter().any(|seen| seen.id == file.id) {
                files.push(file);
            }
        }
        files
    }
}

#[cfg(test)]
mod tests;
