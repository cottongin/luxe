//! Chapter coverage — validates that Guides preserve immutable source items.
//!
//! Guides may only organize source-backed item references. This module enforces the bijection
//! before a model or fallback manifest reaches a UI.

use super::{ChapterId, ChapterKind, NoiseKind, ReviewChapter, ReviewFile, ReviewItemRef};
use std::collections::HashSet;
use thiserror::Error;

/// Rejects Guide chapter layouts that do not cover the exact immutable source item set.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChapterCoverageError {
    /// A chapter referenced an item absent from the captured review source.
    #[error("guide referenced an unknown source item")]
    UnknownItem {
        /// Unrecognized immutable reference.
        item: ReviewItemRef,
    },
    /// A source item appeared in more than one chapter or more than once in one chapter.
    #[error("guide referenced one source item more than once")]
    DuplicateItem {
        /// Repeated immutable reference.
        item: ReviewItemRef,
    },
    /// One or more source items did not appear in any chapter.
    #[error("guide omitted immutable source items")]
    MissingItems {
        /// Source items absent from all chapters, in canonical source order.
        items: Vec<ReviewItemRef>,
    },
}

/// Returns the exact immutable item sequence represented by canonical review files.
pub fn source_items(files: &[ReviewFile]) -> Vec<ReviewItemRef> {
    files
        .iter()
        .flat_map(|file| {
            if file.non_text.is_some() {
                vec![ReviewItemRef::File(file.id.clone())]
            } else {
                file.hunks.iter().cloned().map(|hunk| ReviewItemRef::Hunk(hunk.id)).collect()
            }
        })
        .collect()
}

/// Creates a deterministic, grounded Guide when model output is unavailable or invalid.
pub fn fallback_chapters(files: &[ReviewFile]) -> Result<Vec<ReviewChapter>, ChapterCoverageError> {
    let mut core = Vec::new();
    let mut tests = Vec::new();
    let mut supporting = Vec::new();
    let mut generated = Vec::new();

    for file in files {
        let items = if file.non_text.is_some() {
            vec![ReviewItemRef::File(file.id.clone())]
        } else {
            file.hunks.iter().cloned().map(|hunk| ReviewItemRef::Hunk(hunk.id)).collect()
        };
        let target = if file.generated
            || (!items.is_empty()
                && file.hunks.iter().all(|hunk| {
                    matches!(
                        hunk.noise,
                        NoiseKind::Generated | NoiseKind::Lockfile | NoiseKind::Mechanical
                    )
                })) {
            &mut generated
        } else {
            match fallback_file_kind(file) {
                ChapterKind::Core => &mut core,
                ChapterKind::Test => &mut tests,
                ChapterKind::Supporting => &mut supporting,
                _ => unreachable!("fallback file classification is bounded"),
            }
        };
        target.extend(items);
    }

    let mut chapters = Vec::new();
    for (kind, title, rationale, items) in [
        (
            ChapterKind::Core,
            "Core implementation",
            "Groups source-backed implementation changes.",
            core,
        ),
        (
            ChapterKind::Test,
            "Tests and fixtures",
            "Groups source-backed verification changes.",
            tests,
        ),
        (
            ChapterKind::Supporting,
            "Supporting changes",
            "Groups source-backed documentation, configuration, or other supporting changes.",
            supporting,
        ),
        (
            ChapterKind::Generated,
            "Generated or mechanical changes",
            "Groups source-backed generated, lockfile, or mechanical changes.",
            generated,
        ),
    ] {
        if let Some(chapter) = fallback_chapter(kind, title, rationale, items) {
            chapters.push(chapter);
        }
    }
    validate_chapter_coverage(files, &chapters)?;
    Ok(chapters)
}

/// Deterministic grouping for one *batch* of items, used when a model pass fails.
///
/// Scoped to `items` so a failed batch degrades alone; the rest of the review keeps its
/// model-authored sections.
pub fn fallback_groups(files: &[ReviewFile], items: &[ReviewItemRef]) -> Vec<super::GuideGroup> {
    use std::collections::BTreeMap;
    let mut buckets: BTreeMap<ChapterKind, Vec<ReviewItemRef>> = BTreeMap::new();
    for item in items {
        let Some(file) = owning_file(files, item) else {
            continue;
        };
        let kind = if file.generated
            || matches!(item_noise(file, item), Some(noise) if
            matches!(noise, NoiseKind::Generated | NoiseKind::Lockfile | NoiseKind::Mechanical))
        {
            ChapterKind::Generated
        } else {
            fallback_file_kind(file)
        };
        buckets.entry(kind).or_default().push(item.clone());
    }
    buckets
        .into_iter()
        .map(|(kind, items)| super::GuideGroup {
            title: fallback_title(kind).into(),
            rationale: fallback_rationale(kind).into(),
            impact: None,
            kind,
            items,
            notes: Vec::new(),
        })
        .collect()
}

fn owning_file<'a>(files: &'a [ReviewFile], item: &ReviewItemRef) -> Option<&'a ReviewFile> {
    match item {
        ReviewItemRef::File(file_id) => files.iter().find(|file| &file.id == file_id),
        ReviewItemRef::Hunk(hunk_id) => {
            files.iter().find(|file| file.hunks.iter().any(|hunk| &hunk.id == hunk_id))
        }
    }
}

fn item_noise(file: &ReviewFile, item: &ReviewItemRef) -> Option<NoiseKind> {
    match item {
        ReviewItemRef::Hunk(hunk_id) => {
            file.hunks.iter().find(|hunk| &hunk.id == hunk_id).map(|hunk| hunk.noise)
        }
        ReviewItemRef::File(_) => None,
    }
}

fn fallback_title(kind: ChapterKind) -> &'static str {
    match kind {
        ChapterKind::Core => "Core implementation",
        ChapterKind::Consequence => "Downstream integration",
        ChapterKind::Test => "Tests and fixtures",
        ChapterKind::Supporting => "Supporting changes",
        ChapterKind::Generated => "Generated or mechanical changes",
    }
}

fn fallback_rationale(kind: ChapterKind) -> &'static str {
    match kind {
        ChapterKind::Core => "Groups source-backed implementation changes.",
        ChapterKind::Consequence => "Groups source-backed downstream integration changes.",
        ChapterKind::Test => "Groups source-backed verification changes.",
        ChapterKind::Supporting => {
            "Groups source-backed documentation, configuration, or other supporting changes."
        }
        ChapterKind::Generated => {
            "Groups source-backed generated, lockfile, or mechanical changes."
        }
    }
}

fn fallback_file_kind(file: &ReviewFile) -> ChapterKind {
    let path = file
        .new_path
        .as_ref()
        .or(file.old_path.as_ref())
        .map(|path| path.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if path.split('/').any(|component| matches!(component, "test" | "tests" | "fixtures")) {
        ChapterKind::Test
    } else if path.ends_with(".md")
        || path.ends_with(".rst")
        || path.ends_with(".toml")
        || path.ends_with(".yaml")
        || path.ends_with(".yml")
        || path.ends_with(".json")
    {
        ChapterKind::Supporting
    } else {
        ChapterKind::Core
    }
}

fn fallback_chapter(
    kind: ChapterKind,
    title: &str,
    rationale: &str,
    items: Vec<ReviewItemRef>,
) -> Option<ReviewChapter> {
    (!items.is_empty()).then(|| ReviewChapter {
        id: ChapterId::from_item_keys(
            match kind {
                ChapterKind::Core => "core",
                ChapterKind::Test => "test",
                ChapterKind::Supporting => "supporting",
                ChapterKind::Generated => "generated",
                ChapterKind::Consequence => unreachable!("fallback does not infer consequences"),
            },
            &items.iter().map(item_key).collect::<Vec<_>>(),
        ),
        title: title.into(),
        rationale: rationale.into(),
        impact: None,
        kind,
        items,
        notes: Vec::new(),
    })
}

fn item_key(item: &ReviewItemRef) -> &str {
    match item {
        ReviewItemRef::Hunk(id) => id.as_str(),
        ReviewItemRef::File(id) => id.as_str(),
    }
}

/// Ensures every immutable review item occurs in exactly one Guide chapter.
pub fn validate_chapter_coverage(
    files: &[ReviewFile],
    chapters: &[ReviewChapter],
) -> Result<(), ChapterCoverageError> {
    let expected = source_items(files);
    let expected_set = expected.iter().cloned().collect::<HashSet<_>>();
    let mut seen = HashSet::new();

    for item in chapters.iter().flat_map(|chapter| &chapter.items) {
        if !expected_set.contains(item) {
            return Err(ChapterCoverageError::UnknownItem { item: item.clone() });
        }
        if !seen.insert(item.clone()) {
            return Err(ChapterCoverageError::DuplicateItem { item: item.clone() });
        }
    }

    let missing: Vec<_> = expected.into_iter().filter(|item| !seen.contains(item)).collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(ChapterCoverageError::MissingItems { items: missing })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guided_review::{
        ChapterId, ChapterKind, FileId, LineRange, NoiseKind, ReviewFileStatus, ReviewHunk,
    };
    use std::path::Path;

    fn source_file(path: &str) -> ReviewFile {
        let id = FileId::from_path_lineage(None, Some(Path::new(path)));
        let hunk_id = crate::guided_review::HunkId::from_content(&id, "", "+fn change() {}");
        ReviewFile {
            id,
            old_path: None,
            new_path: Some(path.into()),
            status: ReviewFileStatus::Added,
            language: Some("rust".into()),
            additions: Some(1),
            deletions: Some(0),
            hunks: vec![ReviewHunk {
                id: hunk_id,
                old_range: LineRange::new(0, 0),
                new_range: LineRange::new(1, 1),
                heading: None,
                symbol: None,
                lines: Vec::new(),
                noise: NoiseKind::Semantic,
            }],
            binary: false,
            generated: false,
            non_text: None,
        }
    }

    #[test]
    fn fallback_is_deterministic_and_lossless() {
        let file = source_file("src/lib.rs");
        let first = fallback_chapters(std::slice::from_ref(&file)).unwrap();
        let second = fallback_chapters(std::slice::from_ref(&file)).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].kind, ChapterKind::Core);
        assert_eq!(validate_chapter_coverage(&[file], &first), Ok(()));
    }

    #[test]
    fn fallback_orders_path_evidenced_sections_before_noise() {
        let mut generated = source_file("generated/schema.rs");
        generated.generated = true;
        let files = [
            source_file("src/model.rs"),
            source_file("tests/model.rs"),
            source_file("docs/model.md"),
            generated,
        ];

        let chapters = fallback_chapters(&files).unwrap();

        assert_eq!(
            chapters.iter().map(|chapter| chapter.kind).collect::<Vec<_>>(),
            [ChapterKind::Core, ChapterKind::Test, ChapterKind::Supporting, ChapterKind::Generated,]
        );
        assert_eq!(validate_chapter_coverage(&files, &chapters), Ok(()));
    }

    #[test]
    fn coverage_requires_each_source_item_once() {
        let file = source_file("src/lib.rs");
        let item = source_items(std::slice::from_ref(&file)).pop().unwrap();
        let hunk = match &item {
            ReviewItemRef::Hunk(id) => id.clone(),
            ReviewItemRef::File(_) => unreachable!("text file has a hunk item"),
        };
        let chapter = ReviewChapter {
            id: ChapterId::from_items("core", &[hunk]),
            title: "Core".into(),
            rationale: "Changes the source-backed item.".into(),
            impact: None,
            kind: ChapterKind::Core,
            items: vec![item.clone()],
            notes: Vec::new(),
        };

        assert_eq!(
            validate_chapter_coverage(std::slice::from_ref(&file), std::slice::from_ref(&chapter)),
            Ok(())
        );
        assert!(matches!(
            validate_chapter_coverage(std::slice::from_ref(&file), &[]),
            Err(ChapterCoverageError::MissingItems { items }) if items == vec![item.clone()]
        ));
        assert!(matches!(
            validate_chapter_coverage(&[file], &[chapter.clone(), chapter]),
            Err(ChapterCoverageError::DuplicateItem { .. })
        ));
    }
}
