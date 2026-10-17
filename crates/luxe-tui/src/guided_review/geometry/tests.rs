use super::*;
use crate::fixture::{fixture_guided_review_scaled, fixture_large_guided_review};

/// Every stop a chapter offers has to survive being resolved through the index rather than by
/// scanning — the whole change is a lookup, and a lookup that misses drops a hunk out of the
/// walkthrough silently.
#[test]
fn an_indexed_lookup_finds_exactly_what_a_scan_did() {
    let review = fixture_large_guided_review();
    let index = StopIndex::of(&review);
    for chapter in &review.chapters {
        let indexed = chapter_stops(&review, chapter, &index);
        let scanned = chapter_stops_by_scanning(&review, chapter);
        assert_eq!(indexed, scanned, "chapter {:?}", chapter.title);
        assert!(!indexed.is_empty(), "chapter {:?} offers nothing to step to", chapter.title);
    }
}

/// A chapter that names a file *and* one of its hunks lists that hunk once.
#[test]
fn a_repeated_item_earns_one_stop() {
    let mut review = fixture_large_guided_review();
    let file = review.files.iter().find(|file| !file.hunks.is_empty()).expect("a text file");
    let (file_id, hunk_id) = (file.id.clone(), file.hunks[0].id.clone());
    let chapter = &mut review.chapters[0];
    chapter.items = vec![
        ReviewItemRef::File(file_id),
        ReviewItemRef::Hunk(hunk_id),
        ReviewItemRef::Hunk(review.files[0].hunks[0].id.clone()),
    ];

    let review = review.clone();
    let stops = chapter_stops(&review, &review.chapters[0], &StopIndex::of(&review));
    let mut seen = stops.clone();
    seen.sort_by_key(|stop| (stop.file, stop.hunk));
    seen.dedup();
    assert_eq!(seen.len(), stops.len(), "a stop was listed twice: {stops:?}");
}

/// An id the review no longer holds is skipped, not resolved to whatever sorts nearest.
#[test]
fn an_unknown_id_contributes_no_stop() {
    let mut review = fixture_large_guided_review();
    let ghost = FileId::from_path_lineage(None, Some(std::path::Path::new("gone.rs")));
    review.chapters[0].items = vec![ReviewItemRef::File(ghost)];
    let review = review.clone();

    assert!(chapter_stops(&review, &review.chapters[0], &StopIndex::of(&review)).is_empty());
}

/// Copies of the fixture have to be distinct items, or a scaling measurement taken against
/// them measures a lookup collapsing duplicates rather than a review getting bigger.
#[test]
fn a_scaled_review_holds_distinct_items() {
    let single = fixture_large_guided_review();
    let quadruple = fixture_guided_review_scaled(4);
    assert_eq!(quadruple.files.len(), single.files.len() * 4);
    assert_eq!(quadruple.chapters.len(), single.chapters.len() * 4);

    let index = StopIndex::of(&quadruple);
    assert_eq!(index.files.len(), quadruple.files.len(), "file ids collided");
    let hunks: usize = quadruple.files.iter().map(|file| file.hunks.len()).sum();
    assert_eq!(index.hunks.len(), hunks, "hunk ids collided");
    for chapter in &quadruple.chapters {
        assert!(
            !chapter_stops(&quadruple, chapter, &index).is_empty(),
            "a copy's chapter resolves to nothing, so its items point at another copy"
        );
    }
}

/// Each copy's chapter notes name that copy's items, and none of the original's.
///
/// The break this catches: scaling renamed the items but not the notes keyed to them, so every
/// copy's notes pointed at the first copy's hunks.
#[test]
fn scaled_chapter_notes_name_only_that_copys_items() {
    let scaled = fixture_guided_review_scaled(3);
    for chapter in &scaled.chapters {
        let items: Vec<String> = chapter
            .items
            .iter()
            .map(|item| match item {
                ReviewItemRef::Hunk(id) => id.as_str().to_owned(),
                ReviewItemRef::File(id) => id.as_str().to_owned(),
            })
            .collect();
        for note in &chapter.notes {
            assert!(items.contains(&note.item), "a note names another copy's item");
        }
    }
}

/// A scaled review's totals are its own files', not one copy's.
#[test]
fn scaled_stats_equal_stats_derived_from_files() {
    let scaled = fixture_guided_review_scaled(3);
    let derived = luxe_core::guided_review::ReviewStats::from_files(&scaled.files);

    let mut excluding = fixture_large_guided_review();
    excluding.stats.excluded_files = 2;
    let scaled_excluding = crate::fixture::scaled::scale(excluding, 3);
    assert_eq!(scaled_excluding.stats.excluded_files, 6, "each copy excludes what one does");
    assert_eq!((scaled.stats.files, scaled.stats.hunks), (derived.files, derived.hunks));
    assert_eq!(
        (scaled.stats.additions, scaled.stats.deletions),
        (derived.additions, derived.deletions)
    );
}

/// A demo hunk's header counts every line on its side, context included.
#[test]
fn demo_hunk_ranges_count_context_on_each_side() {
    let review = crate::fixture::fixture_demo_guided_review();
    for hunk in review.files.iter().flat_map(|file| &file.hunks) {
        let old = hunk.lines.iter().filter(|line| line.old_line.is_some()).count() as u32;
        let new = hunk.lines.iter().filter(|line| line.new_line.is_some()).count() as u32;
        assert_eq!((hunk.old_range.count, hunk.new_range.count), (old, new));
    }
}

/// What `chapter_stops` used to do: resolve every item by scanning the review.
///
/// Kept as the oracle the indexed version is checked against. It is also the shape the fix
/// removed — quadratic in the review, and run once per chapter on every frame.
fn chapter_stops_by_scanning(review: &GuidedReview, chapter: &ReviewChapter) -> Vec<NavStop> {
    let mut stops: Vec<NavStop> = Vec::new();
    let push = |stops: &mut Vec<NavStop>, stop: NavStop| {
        if !stops.contains(&stop) {
            stops.push(stop);
        }
    };
    for item in &chapter.items {
        match item {
            ReviewItemRef::File(file_id) => {
                let Some((index, file)) =
                    review.files.iter().enumerate().find(|(_, file)| &file.id == file_id)
                else {
                    continue;
                };
                if file.non_text.is_some() {
                    push(&mut stops, NavStop { file: index, hunk: None });
                } else {
                    for hunk in 0..file.hunks.len() {
                        push(&mut stops, NavStop { file: index, hunk: Some(hunk) });
                    }
                }
            }
            ReviewItemRef::Hunk(hunk_id) => {
                for (index, file) in review.files.iter().enumerate() {
                    if let Some(hunk) = file.hunks.iter().position(|hunk| &hunk.id == hunk_id) {
                        push(&mut stops, NavStop { file: index, hunk: Some(hunk) });
                        break;
                    }
                }
            }
        }
    }
    stops
}
