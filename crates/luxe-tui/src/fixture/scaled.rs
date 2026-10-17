//! Scaled — the large review repeated, for measuring against review size.

use super::fixture_large_guided_review;
use luxe_core::guided_review::{GuidedReview, ReviewStats};

/// [`fixture_large_guided_review`] repeated `times` over, for measuring against review size.
///
/// Whole copies rather than a bigger generator, so the shape stays the one real reviews have — the
/// same mix of chapter kinds, folded churn and non-text entries at every scale. Ids are rewritten
/// per copy, which is what makes the copies distinct items rather than duplicates that a lookup
/// would collapse, and each copy's notes follow its own items. Paths repeat on purpose: a stress
/// case of the same tree many times over. `times` of 1 is the fixture itself.
pub fn fixture_guided_review_scaled(times: usize) -> GuidedReview {
    scale(fixture_large_guided_review(), times)
}

/// `base` repeated `times` over, the way [`fixture_guided_review_scaled`] describes.
pub(crate) fn scale(base: GuidedReview, times: usize) -> GuidedReview {
    use luxe_core::guided_review::{ChapterId, FileId, HunkId, ReviewItemRef};
    use std::collections::HashMap;

    let mut scaled = base.clone();
    for round in 1..times.max(1) {
        let mut files = base.files.clone();
        let mut chapters = base.chapters.clone();
        // A chapter references its items by id, so every rename has to travel into the chapters of
        // the same copy — otherwise the copy's sections point at the original's files.
        let mut renamed_files: HashMap<FileId, FileId> = HashMap::new();
        let mut renamed_hunks: HashMap<HunkId, HunkId> = HashMap::new();
        for file in &mut files {
            let previous = file.id.clone();
            file.id = FileId::from_path_lineage(
                None,
                Some(&std::path::PathBuf::from(format!("copy-{round}/{}", previous.as_str()))),
            );
            renamed_files.insert(previous, file.id.clone());
            for hunk in &mut file.hunks {
                let previous = hunk.id.clone();
                hunk.id =
                    HunkId::from_content(&file.id, &format!("copy-{round}"), previous.as_str());
                renamed_hunks.insert(previous, hunk.id.clone());
            }
        }
        // notes name items by their id's text, so they look up through one map of both kinds
        let renamed_notes: HashMap<String, String> = renamed_hunks
            .iter()
            .map(|(old, new)| (old.as_str(), new.as_str()))
            .chain(renamed_files.iter().map(|(old, new)| (old.as_str(), new.as_str())))
            .map(|(old, new)| (old.to_owned(), new.to_owned()))
            .collect();
        for chapter in &mut chapters {
            chapter.id =
                ChapterId::from_item_keys(&format!("copy-{round}"), &[chapter.id.as_str()]);
            for item in &mut chapter.items {
                *item = match item {
                    ReviewItemRef::File(id) => ReviewItemRef::File(
                        renamed_files.get(id).cloned().unwrap_or_else(|| id.clone()),
                    ),
                    ReviewItemRef::Hunk(id) => ReviewItemRef::Hunk(
                        renamed_hunks.get(id).cloned().unwrap_or_else(|| id.clone()),
                    ),
                };
            }
            for note in &mut chapter.notes {
                if let Some(new) = renamed_notes.get(&note.item) {
                    note.item.clone_from(new);
                }
            }
        }
        scaled.files.extend(files);
        scaled.chapters.extend(chapters);
    }
    // every copy excludes what the original did
    let excluded_files = base.stats.excluded_files * times.max(1) as u64;
    scaled.stats = ReviewStats { excluded_files, ..ReviewStats::from_files(&scaled.files) };
    scaled
}
