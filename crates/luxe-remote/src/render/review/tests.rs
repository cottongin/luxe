use super::{chapter, guide, Place};
use crate::render::{escape, Renderer, StatusFacts, StatusSegment, Voice};
use luxe_core::guided_review::{
    ChapterReading, GuidedReview, ItemNote, ReviewBrief, ReviewFile, TurnProvenance,
};

/// Prose marked so a test can tell it went through the markdown hook, and code by language.
struct Marked;

impl Renderer for Marked {
    fn markdown(&self, text: &str, _voice: Voice) -> String {
        format!("<p>md:{}</p>", escape(text))
    }
    fn tool_glyph(&self, _tool: &str) -> String {
        String::new()
    }
    fn status(&self, _facts: &StatusFacts) -> Vec<StatusSegment> {
        Vec::new()
    }
    fn code_line(&self, text: &str, language: Option<&str>) -> String {
        format!("[{}:{}]", language.unwrap_or("-"), escape(text))
    }
}

/// A real `open_review_shown` (PR #230's review on this repository), cut to three items a
/// chapter with hunks of four lines, and with the planning notes and PR body blanked.
fn captured() -> (GuidedReview, Vec<ChapterReading>) {
    #[derive(serde::Deserialize)]
    struct Shown {
        review: GuidedReview,
        reading: Vec<ChapterReading>,
    }
    let shown: Shown =
        serde_json::from_str(include_str!("pr-230.json")).expect("the captured review parses");
    (shown.review, shown.reading)
}

fn guide_of(review: &GuidedReview, reading: &[ChapterReading]) -> String {
    guide(&Marked, review, reading, &[])
}

/// The section `guide` drew for chapter `index`.
fn section(html: &str, review: &GuidedReview, index: usize) -> String {
    let id = review.chapters[index].id.as_str();
    let start = html.find(&format!("data-chapter=\"{id}\"")).expect("the chapter is drawn");
    let end = html[start..].find("</section>").expect("its section closes") + start;
    html[start..end].to_owned()
}

/// Every chapter, in order, numbered against the whole and labeled by kind.
///
/// The break this catches: numbering from zero or against the wrong total, or a kind mislabeled,
/// which is how a reader tells the core of a change from its tests.
#[test]
fn chapters_are_numbered_and_labeled_in_order() {
    let (review, reading) = captured();
    let html = guide_of(&review, &reading);
    let places = ["01 / 03</span><span>CORE", "02 / 03</span><span>TESTS", "03 / 03</span>"];
    let at: Vec<usize> = places.iter().map(|it| html.find(it).expect(it)).collect();
    assert!(at.windows(2).all(|pair| pair[0] < pair[1]), "in the review's order");
    assert!(html.contains("<span>SUPPORTING</span>"));
}

/// A walked chapter says so; the others don't.
///
/// The break this catches: every chapter or none reading as reviewed, so the list can't say where
/// the reader got to.
#[test]
fn only_walked_chapters_say_reviewed() {
    let (review, reading) = captured();
    let html = guide(&Marked, &review, &reading, &[review.chapters[1].id.clone()]);
    assert!(!section(&html, &review, 0).contains("reviewed"));
    assert!(section(&html, &review, 1).contains("<span class=\"rv-walked\">reviewed</span>"));
}

/// A chapter to read shows its rationale through the markdown hook and its files, each once, in
/// the order it names them.
///
/// The break this catches: a file listed per hunk (a chapter touching `cli.rs` twice lists it
/// twice), or the rationale skipping the TUI's markdown.
#[test]
fn a_chapter_to_read_shows_its_rationale_and_files_once() {
    let (review, reading) = captured();
    let core = section(&guide_of(&review, &reading), &review, 0);
    assert!(core.contains(&format!("md:{}", escape(&review.chapters[0].rationale))));
    assert_eq!(core.matches("crates/luxe-cli/src/cli.rs").count(), 1, "{core}");
    let (first, second) = (core.find("cli.rs").expect("cli.rs"), core.find("args.rs"));
    assert!(second.is_some_and(|second| first < second), "the chapter's order");
    assert!(core.contains("<span class=\"rv-status tone-modified\">M</span>"));
}

/// An impact sits under the rationale, marked as one.
///
/// The break this catches: the impact dropped, which is the line saying what the change does to
/// everything else.
#[test]
fn an_impact_follows_the_rationale() {
    let (mut review, reading) = captured();
    review.chapters[0].impact = Some("Clients see a new frame".into());
    let core = section(&guide_of(&review, &reading), &review, 0);
    assert!(core.contains("<div class=\"rv-impact\"><p>md:Clients see a new frame</p></div>"));
}

/// The reading for one chapter, overriding what the capture said.
fn reading_with(
    review: &GuidedReview,
    needs_review: usize,
    reason: Option<&str>,
) -> ChapterReading {
    let chapter_id = review.chapters[2].id.clone();
    ChapterReading { chapter_id, needs_review, reason: reason.map(str::to_owned) }
}

/// A chapter nothing in needs the reader is one quiet line saying why, without its files.
///
/// The break this catches: a lockfile chapter spending the page like the core change, or saying
/// nothing about why it's safe to skip.
#[test]
fn a_chapter_needing_nothing_says_why_in_one_line() {
    let (review, _) = captured();
    let reading = [reading_with(&review, 0, Some("formatting only"))];
    let quiet = section(&guide_of(&review, &reading), &review, 2);
    assert!(quiet.contains("formatting only \u{2014} nothing here needs you"), "{quiet}");
    assert!(quiet.contains("rv-title quiet"));
    assert!(!quiet.contains("rv-files") && !quiet.contains("md:"));
}

/// With no one reason, it still says there's nothing to read.
///
/// The break this catches: an empty line where the reason would go.
#[test]
fn a_chapter_needing_nothing_for_mixed_reasons_says_only_that() {
    let (review, _) = captured();
    let reading = [reading_with(&review, 0, None)];
    let quiet = section(&guide_of(&review, &reading), &review, 2);
    assert!(quiet.contains("<p class=\"rv-reason\">nothing here needs you</p>"), "{quiet}");
}

/// A chapter the host said nothing about is one to read.
///
/// The break this catches: a missing reading folding the chapter away, so a stale or partial
/// frame hides work.
#[test]
fn a_chapter_without_a_reading_is_one_to_read() {
    let (review, _) = captured();
    let open = section(&guide_of(&review, &[]), &review, 2);
    assert!(open.contains("rv-files") && !open.contains("rv-reason"));
}

/// The summary leads: its intent, then its claims as a list.
///
/// The break this catches: the brief left out, or its claims run together as one paragraph.
#[test]
fn the_summary_leads_the_guide() {
    let (mut review, reading) = captured();
    let account = vec!["Adds a frame".into(), "Pins it".into()];
    review.brief = Some(ReviewBrief { intent: "Let a phone join".into(), account });
    let html = guide_of(&review, &reading);
    let brief = "<section class=\"rv-brief md\"><p>md:Let a phone join</p><ul><li><p>md:Adds a \
                 frame</p></li><li><p>md:Pins it</p></li></ul></section>";
    assert!(html.starts_with(&format!("<div class=\"rv-guide\">{brief}")), "{html}");
}

/// Without a summary, the agent's own words lead, said to be just that.
///
/// The break this catches: the turn's account passed off as a reviewed summary.
#[test]
fn without_a_summary_the_agents_words_lead_and_say_so() {
    let (mut review, reading) = captured();
    let provenance = TurnProvenance {
        requests: vec!["make it join".into()],
        account: Some("I added show_open_review".into()),
        ..TurnProvenance::default()
    };
    review.provenance = Some(provenance);
    let html = guide_of(&review, &reading);
    let words = html.find("md:make it join").zip(html.find("md:I added show_open_review"));
    assert!(words.is_some_and(|(asked, said)| asked < said), "{html}");
    assert!(html.contains("no summary was written for this review"));
}

/// With neither, the guide opens on the chapters.
///
/// The break this catches: an empty brief box above the first chapter.
#[test]
fn with_nothing_to_say_there_is_no_brief() {
    let (review, reading) = captured();
    assert!(!guide_of(&review, &reading).contains("rv-brief"));
}

/// Past eight files, the rest are counted.
///
/// The break this catches: a forty-file chapter listing every path, or the count off by one.
#[test]
fn files_past_eight_are_counted() {
    let (mut review, _) = captured();
    let template = review.files[0].clone();
    review.chapters[2].items = (0..11).map(|n| copy_of(&mut review, &template, n)).collect();
    let html = guide_of(&review, &[]);
    assert_eq!(section(&html, &review, 2).matches("<li><span class=\"rv-status").count(), 8);
    assert!(html.contains("<li class=\"rv-more\">\u{2026} 3 more</li>"));
}

/// A file numbered `n`, added to the review, named by the item returned.
fn copy_of(
    review: &mut GuidedReview,
    template: &ReviewFile,
    n: usize,
) -> luxe_core::guided_review::ReviewItemRef {
    let mut file = template.clone();
    file.id = serde_json::from_value(serde_json::json!(format!("file:{n}"))).expect("an id");
    file.new_path = Some(format!("docs/{n}.md").into());
    review.files.push(file.clone());
    luxe_core::guided_review::ReviewItemRef::File(file.id)
}

/// Titles and paths are text, never markup.
///
/// The break this catches: a branch's chapter title running script in the page.
#[test]
fn titles_are_escaped() {
    let (mut review, reading) = captured();
    review.chapters[0].title = "<img src=x onerror=alert(1)>".into();
    let html = guide_of(&review, &reading);
    assert!(html.contains("&lt;img src=x onerror=alert(1)&gt;") && !html.contains("<img"));
}

fn item_id(item: &luxe_core::guided_review::ReviewItemRef) -> &str {
    match item {
        luxe_core::guided_review::ReviewItemRef::File(id) => id.as_str(),
        luxe_core::guided_review::ReviewItemRef::Hunk(id) => id.as_str(),
    }
}

/// The opened chapter's files with only its hunks, as the app sends them.
fn opened(review: &GuidedReview, index: usize) -> Vec<ReviewFile> {
    let items = &review.chapters[index].items;
    let named = |id: &str| items.iter().any(|item| item_id(item) == id);
    let mut files: Vec<ReviewFile> = review.files.clone();
    files.retain_mut(|file| {
        file.hunks.retain(|hunk| named(hunk.id.as_str()));
        named(file.id.as_str()) || !file.hunks.is_empty()
    });
    files
}

fn open(review: &GuidedReview, index: usize) -> String {
    let place = Place { number: index + 1, total: review.chapters.len(), walked: false };
    chapter(&Marked, &review.chapters[index], &opened(review, index), &place, None, &[])
}

/// An opened chapter names each file once above its run of hunks, in the chapter's order.
///
/// The break this catches: the path repeated above every hunk, or a hunk drawn under the wrong
/// file.
#[test]
fn an_opened_chapter_names_each_file_once_above_its_hunks() {
    let (review, _) = captured();
    let html = open(&review, 0);
    assert_eq!(html.matches("<p class=\"rv-file\">crates/luxe-cli/src/cli.rs</p>").count(), 1);
    let path = html.find("rv-file\">crates/luxe-cli/src/cli.rs").expect("the path");
    let hunks: Vec<usize> = html.match_indices("<div class=\"hunk\">").map(|(at, _)| at).collect();
    assert_eq!(hunks.len(), 3, "the chapter's three hunks");
    assert!(path < hunks[1], "cli.rs heads its hunks: {html}");
}

/// The guide's sentence for a hunk sits above that hunk's lines.
///
/// The break this catches: notes dropped, or put under the wrong hunk, which narrates code the
/// reader isn't looking at.
#[test]
fn a_note_sits_above_its_hunk() {
    let (mut review, _) = captured();
    let second = item_id(&review.chapters[0].items[1]).to_owned();
    review.chapters[0].notes = vec![ItemNote { item: second, note: "Reads the flag".into() }];
    let html = open(&review, 0);
    let note = html.find("<p class=\"rv-note\">Reads the flag</p>").expect("the note");
    let hunks: Vec<usize> = html.match_indices("<div class=\"hunk\">").map(|(at, _)| at).collect();
    assert!(hunks[0] < note && note < hunks[1], "{html}");
}

/// The symbol a hunk sits in heads it, as the TUI's narration names it.
///
/// The break this catches: the function name dropped, leaving only line numbers to say where the
/// change is.
#[test]
fn a_hunks_symbol_heads_it() {
    let (mut review, _) = captured();
    let symbol = luxe_core::guided_review::SymbolContext {
        kind: "function".into(),
        name: Some("run_rpc".into()),
    };
    let first = item_id(&review.chapters[0].items[0]).to_owned();
    let hunk = review.files.iter_mut().flat_map(|file| file.hunks.iter_mut());
    hunk.filter(|hunk| hunk.id.as_str() == first)
        .for_each(|hunk| hunk.symbol = Some(symbol.clone()));
    let html = open(&review, 0);
    let named = html.find("<p class=\"rv-symbol\">run_rpc</p>").expect("the symbol");
    assert!(named < html.find("<div class=\"hunk\">").expect("a hunk"), "{html}");
}

/// The capture with chapter 2 naming one binary file as a whole, noted by the guide.
fn with_a_binary_file() -> GuidedReview {
    let (mut review, _) = captured();
    let template = review.files[0].clone();
    let item = copy_of(&mut review, &template, 1);
    let file = review.files.last_mut().expect("the copy");
    file.hunks.clear();
    file.non_text = Some(luxe_core::guided_review::NonTextChange {
        kind: luxe_core::guided_review::NonTextChangeKind::Binary,
        summary: "binary, 4 KiB".into(),
    });
    let note = ItemNote { item: item_id(&item).to_owned(), note: "The new icon".into() };
    review.chapters[2].items = vec![item];
    review.chapters[2].notes = vec![note];
    review
}

/// A file named as a whole shows its note and says why there are no lines.
///
/// The break this catches: a binary or mode-only change drawn as nothing at all, so the reader
/// can't tell it was part of the chapter.
#[test]
fn a_whole_file_item_shows_its_note_and_why_there_are_no_lines() {
    let review = with_a_binary_file();
    let html = open(&review, 2);
    let note = html.find("<p class=\"rv-note\">The new icon</p>").expect("the note");
    let why = html.find("isn&#39;t text").or_else(|| html.find("isn't text"));
    assert!(why.is_some_and(|why| note < why), "{html}");
}

/// A file with no lines to count says what changed in the guide's row instead.
///
/// The break this catches: a binary file's row with no figures and no words.
#[test]
fn a_file_without_lines_says_what_changed() {
    let review = with_a_binary_file();
    let row = section(&guide_of(&review, &[]), &review, 2);
    assert!(row.contains("<span class=\"rv-dim\">binary, 4 KiB</span>"), "{row}");
}

/// An item whose file the app didn't send is left out rather than failing the chapter.
///
/// The break this catches: one missing file blanking the whole chapter.
#[test]
fn an_item_without_its_file_is_left_out() {
    let (review, _) = captured();
    let place = Place { number: 1, total: 3, walked: false };
    let html = chapter(&Marked, &review.chapters[0], &[], &place, None, &[]);
    assert!(html.contains(&escape(&review.chapters[0].title)) && !html.contains("hunk"));
}
