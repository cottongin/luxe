//! Fixture corpus smoke tests for P30 Guided Review parsing.

use luxe_core::guided_review::{fallback_chapters, parse_unified_diff, validate_chapter_coverage};

const SEMANTIC: &str = include_str!("fixtures/guided_review/multi_file_semantic.diff");
const RENAME: &str = include_str!("fixtures/guided_review/rename.diff");
const COPY: &str = include_str!("fixtures/guided_review/copy.diff");
const RENAME_ONLY: &str = include_str!("fixtures/guided_review/rename_only.diff");
const COPY_ONLY: &str = include_str!("fixtures/guided_review/copy_only.diff");
const QUOTED_PATHS: &str = include_str!("fixtures/guided_review/quoted_paths.diff");
const UNTRACKED: &str = include_str!("fixtures/guided_review/untracked.diff");
const BINARY: &str = include_str!("fixtures/guided_review/binary.diff");
const GIT_BINARY_PATCH: &str = include_str!("fixtures/guided_review/git_binary_patch.diff");
const MODE_ONLY: &str = include_str!("fixtures/guided_review/mode_only.diff");
const SUBMODULE: &str = include_str!("fixtures/guided_review/submodule.diff");
const GENERATED: &str = include_str!("fixtures/guided_review/generated.diff");
const FORMATTING: &str = include_str!("fixtures/guided_review/formatting_only.diff");
const LARGE: &str = include_str!("fixtures/guided_review/large_hunk.diff");
const EMPTY: &str = include_str!("fixtures/guided_review/no_changes.diff");

#[test]
fn guided_review_fixture_corpus_covers_source_edge_cases() {
    assert_eq!(SEMANTIC.matches("diff --git").count(), 2);
    assert!(RENAME.contains("rename from") && RENAME.contains("rename to"));
    assert!(COPY.contains("copy from") && COPY.contains("copy to"));
    assert!(!RENAME_ONLY.contains("@@") && RENAME_ONLY.contains("rename from"));
    assert!(!COPY_ONLY.contains("@@") && COPY_ONLY.contains("copy from"));
    assert!(QUOTED_PATHS.contains("\\303\\251") && QUOTED_PATHS.contains("\\\"plan\\\""));
    assert!(UNTRACKED.contains("--- /dev/null") && UNTRACKED.contains("new file mode"));
    assert!(BINARY.contains("Binary files") && !BINARY.contains("@@"));
    assert!(GIT_BINARY_PATCH.contains("GIT binary patch"));
    assert!(MODE_ONLY.contains("old mode") && MODE_ONLY.contains("new mode"));
    assert_eq!(SUBMODULE.matches("Subproject commit").count(), 2);
    assert!(GENERATED.contains("@generated"));
    assert!(FORMATTING.contains("-pub fn render(){") && FORMATTING.contains("+pub fn render() {"));
    assert!(LARGE.matches('\n').count() >= 25);
    assert!(EMPTY.is_empty());
}

#[test]
fn every_fixture_is_accepted_as_a_complete_canonical_source() {
    for source in [
        SEMANTIC,
        RENAME,
        COPY,
        RENAME_ONLY,
        COPY_ONLY,
        QUOTED_PATHS,
        UNTRACKED,
        BINARY,
        GIT_BINARY_PATCH,
        MODE_ONLY,
        SUBMODULE,
        GENERATED,
        FORMATTING,
        LARGE,
        EMPTY,
    ] {
        parse_unified_diff(source).unwrap();
    }
}

#[test]
fn fallback_chapters_use_only_grounded_templates_for_fixture_corpus() {
    const GROUNDED_RATIONALES: &[&str] = &[
        "Groups source-backed implementation changes.",
        "Groups source-backed verification changes.",
        "Groups source-backed documentation, configuration, or other supporting changes.",
        "Groups source-backed generated, lockfile, or mechanical changes.",
    ];

    for (name, source) in fixture_sources() {
        let parsed = parse_unified_diff(source).unwrap();
        let chapters = fallback_chapters(&parsed.files).unwrap();
        for chapter in chapters {
            assert!(
                GROUNDED_RATIONALES.contains(&chapter.rationale.as_str()),
                "fallback invented an ungrounded rationale in the {name} fixture"
            );
            assert!(
                chapter.impact.is_none(),
                "fallback invented product impact in the {name} fixture"
            );
        }
    }
}

#[test]
fn fallback_chapters_cover_each_fixture_item_once() {
    for (name, source) in fixture_sources() {
        let parsed = parse_unified_diff(source).unwrap();
        let chapters = fallback_chapters(&parsed.files).unwrap();
        assert!(
            validate_chapter_coverage(&parsed.files, &chapters).is_ok(),
            "fallback lost, duplicated, or fabricated an item in the {name} fixture"
        );
    }
}

fn fixture_sources() -> [(&'static str, &'static str); 15] {
    [
        ("semantic", SEMANTIC),
        ("rename", RENAME),
        ("copy", COPY),
        ("rename-only", RENAME_ONLY),
        ("copy-only", COPY_ONLY),
        ("quoted-paths", QUOTED_PATHS),
        ("untracked", UNTRACKED),
        ("binary", BINARY),
        ("git-binary-patch", GIT_BINARY_PATCH),
        ("mode-only", MODE_ONLY),
        ("submodule", SUBMODULE),
        ("generated", GENERATED),
        ("formatting", FORMATTING),
        ("large", LARGE),
        ("empty", EMPTY),
    ]
}
