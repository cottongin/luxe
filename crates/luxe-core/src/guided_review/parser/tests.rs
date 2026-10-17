use super::*;

#[test]
fn parses_complete_text_hunks_with_stable_line_anchors() {
    let parsed = parse_unified_diff(include_str!(
        "../../../tests/fixtures/guided_review/multi_file_semantic.diff"
    ))
    .unwrap();

    assert_eq!(parsed.files.len(), 2);
    assert_eq!(parsed.files[0].hunks.len(), 1);
    assert_eq!(parsed.files[0].hunks[0].lines[0].old_line, Some(18));
    assert_eq!(parsed.files[0].hunks[0].lines[0].new_line, Some(18));
    assert_eq!(parsed.files[0].hunks[0].lines[1].old_line, None);
    assert_eq!(parsed.files[0].hunks[0].lines[1].new_line, Some(19));
}

#[test]
fn preserves_unaffected_hunk_ids_when_another_file_changes() {
    let baseline = parse_unified_diff(include_str!(
        "../../../tests/fixtures/guided_review/multi_file_semantic.diff"
    ))
    .unwrap();
    let expanded = format!(
        "{}diff --git a/extra.rs b/extra.rs\n--- a/extra.rs\n+++ b/extra.rs\n@@ -1 +1 @@\n-old\n+new\n",
        include_str!("../../../tests/fixtures/guided_review/multi_file_semantic.diff")
    );
    let refreshed = parse_unified_diff(&expanded).unwrap();

    assert_eq!(baseline.files.len(), 2);
    assert_eq!(refreshed.files.len(), 3);
    assert_eq!(baseline.files[0].hunks[0].id, refreshed.files[0].hunks[0].id);
    assert_eq!(baseline.files[1].hunks[0].id, refreshed.files[1].hunks[0].id);
}

#[test]
fn preserves_no_newline_markers_without_counting_them_as_source_lines() {
    let diff = concat!(
        "diff --git a/file.rs b/file.rs\n",
        "--- a/file.rs\n",
        "+++ b/file.rs\n",
        "@@ -1 +1 @@\n",
        "-old\n",
        "\\ No newline at end of file\n",
        "+new\n",
        "\\ No newline at end of file\n",
    );

    let parsed = parse_unified_diff(diff).unwrap();
    let lines = &parsed.files[0].hunks[0].lines;
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[0].old_line, Some(1));
    assert_eq!(lines[0].new_line, None);
    assert_eq!(lines[1].kind, ReviewLineKind::NoNewlineMarker);
    assert_eq!(lines[1].old_line, None);
    assert_eq!(lines[1].new_line, None);
    assert_eq!(lines[2].old_line, None);
    assert_eq!(lines[2].new_line, Some(1));
    assert_eq!(lines[3].kind, ReviewLineKind::NoNewlineMarker);
}

#[test]
fn rejects_hunks_with_incomplete_declared_source_coverage() {
    let diff =
        "diff --git a/file.rs b/file.rs\n--- a/file.rs\n+++ b/file.rs\n@@ -1,2 +1 @@\n-old\n+new\n";

    assert_eq!(
        parse_unified_diff(diff),
        Err(GuidedReviewParseError::HunkLineCountMismatch {
            header: "@@ -1,2 +1 @@".into(),
            expected_old: 2,
            actual_old: 1,
            expected_new: 1,
            actual_new: 1,
        })
    );
}

#[test]
fn rejects_noncanonical_source_text_outside_git_file_blocks() {
    assert_eq!(
        parse_unified_diff("not a unified diff\n"),
        Err(GuidedReviewParseError::UnrecognizedSource { line: "not a unified diff".into() })
    );
    assert!(matches!(
        parse_unified_diff("preamble\ndiff --git a/file.rs b/file.rs\n--- a/file.rs\n+++ b/file.rs\n@@ -1 +1 @@\n-old\n+new\n"),
        Err(GuidedReviewParseError::UnrecognizedSource { line }) if line == "preamble"
    ));
    assert_eq!(parse_unified_diff("\n\t\n").unwrap().files, Vec::new());
}

#[test]
fn rejects_unsafe_paths_in_file_headers() {
    let diff = "diff --git a/../secret.rs b/../secret.rs\n--- a/../secret.rs\n+++ b/../secret.rs\n@@ -1 +1 @@\n-old\n+new\n";

    assert_eq!(
        parse_unified_diff(diff),
        Err(GuidedReviewParseError::InvalidFileHeader { line: "--- a/../secret.rs".into() })
    );
}

#[test]
fn rejects_duplicate_file_blocks_in_a_canonical_source() {
    let block =
        "diff --git a/file.rs b/file.rs\n--- a/file.rs\n+++ b/file.rs\n@@ -1 +1 @@\n-old\n+new\n";

    assert_eq!(
        parse_unified_diff(&format!("{block}{block}")),
        Err(GuidedReviewParseError::DuplicateFileBlock {
            header: "diff --git a/file.rs b/file.rs".into(),
        })
    );
}

#[test]
fn rejects_unrecognized_git_file_blocks() {
    let diff = "diff --git a/file.rs b/file.rs\nindex 1234567..89abcde 100644\n";

    assert_eq!(
        parse_unified_diff(diff),
        Err(GuidedReviewParseError::UnrecognizedFileBlock {
            header: "diff --git a/file.rs b/file.rs".into(),
        })
    );
}

#[test]
fn rejects_text_file_headers_without_source_hunks() {
    let diff = "diff --git a/file.rs b/file.rs\n--- a/file.rs\n+++ b/file.rs\n";

    assert_eq!(
        parse_unified_diff(diff),
        Err(GuidedReviewParseError::MissingTextHunk { line: "--- a/file.rs".into() })
    );
}

#[test]
fn parses_dev_null_additions_without_fabricating_old_paths() {
    let parsed =
        parse_unified_diff(include_str!("../../../tests/fixtures/guided_review/untracked.diff"))
            .unwrap();

    let file = &parsed.files[0];
    assert!(file.old_path.is_none());
    assert_eq!(file.status, ReviewFileStatus::Added);
    assert_eq!(file.additions, Some(4));
}

#[test]
fn classifies_conventional_generated_markers_without_path_heuristics() {
    let parsed =
        parse_unified_diff(include_str!("../../../tests/fixtures/guided_review/generated.diff"))
            .unwrap();

    let file = &parsed.files[0];
    assert!(file.generated);
    assert!(file.hunks.iter().all(|hunk| hunk.noise == NoiseKind::Generated));
}

#[test]
fn classifies_known_lockfiles_as_lower_signal_without_hiding_hunks() {
    let diff = "diff --git a/Cargo.lock b/Cargo.lock\n--- a/Cargo.lock\n+++ b/Cargo.lock\n@@ -1 +1 @@\n-old\n+new\n";
    let parsed = parse_unified_diff(diff).unwrap();

    let file = &parsed.files[0];
    assert!(!file.generated);
    assert_eq!(file.hunks.len(), 1);
    assert_eq!(file.hunks[0].noise, NoiseKind::Lockfile);
    assert_eq!(file.hunks[0].lines[1].text, "new");
}

#[test]
fn classifies_proven_line_comment_hunks_without_hiding_them() {
    let diff = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-// old rationale\n+// revised rationale\n";
    let parsed = parse_unified_diff(diff).unwrap();

    let hunk = &parsed.files[0].hunks[0];
    assert_eq!(hunk.noise, NoiseKind::CommentOnly);
    assert_eq!(hunk.lines.len(), 2);
}

#[test]
fn leaves_mixed_comment_and_code_hunks_semantic() {
    let diff = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,2 @@\n-// old rationale\n-pub const VERSION: u32 = 1;\n+// revised rationale\n+pub const VERSION: u32 = 2;\n";
    let parsed = parse_unified_diff(diff).unwrap();

    assert_eq!(parsed.files[0].hunks[0].noise, NoiseKind::Semantic);
}

#[test]
fn classifies_safe_delimiter_whitespace_changes_as_formatting_only() {
    let parsed = parse_unified_diff(include_str!(
        "../../../tests/fixtures/guided_review/formatting_only.diff"
    ))
    .unwrap();

    assert_eq!(parsed.files[0].hunks[0].noise, NoiseKind::FormattingOnly);
}

#[test]
fn leaves_operator_whitespace_changes_semantic() {
    let diff = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-let value = left + right;\n+let value = left+right;\n";
    let parsed = parse_unified_diff(diff).unwrap();

    assert_eq!(parsed.files[0].hunks[0].noise, NoiseKind::Semantic);
}

#[test]
fn leaves_quoted_whitespace_changes_semantic() {
    let diff = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-let label = \"one space\";\n+let label = \"one  space\";\n";
    let parsed = parse_unified_diff(diff).unwrap();

    assert_eq!(parsed.files[0].hunks[0].noise, NoiseKind::Semantic);
}

#[test]
fn parses_binary_blocks_as_explicit_non_text_items() {
    let parsed =
        parse_unified_diff(include_str!("../../../tests/fixtures/guided_review/binary.diff"))
            .unwrap();

    let file = &parsed.files[0];
    assert!(file.binary);
    assert!(file.hunks.is_empty());
    assert_eq!(file.additions, None);
    assert_eq!(file.non_text.as_ref().map(|change| change.kind), Some(NonTextChangeKind::Binary));
}

#[test]
fn parses_git_binary_patches_as_explicit_non_text_items() {
    let parsed = parse_unified_diff(include_str!(
        "../../../tests/fixtures/guided_review/git_binary_patch.diff"
    ))
    .unwrap();

    let file = &parsed.files[0];
    assert!(file.binary);
    assert_eq!(file.additions, None);
    assert_eq!(file.deletions, None);
    assert_eq!(
        file.non_text.as_ref().map(|change| change.summary.as_str()),
        Some("binary content")
    );
}

#[test]
fn preserves_rename_and_copy_status_with_text_hunks() {
    let renamed =
        parse_unified_diff(include_str!("../../../tests/fixtures/guided_review/rename.diff"))
            .unwrap();
    let copied =
        parse_unified_diff(include_str!("../../../tests/fixtures/guided_review/copy.diff"))
            .unwrap();

    assert_eq!(renamed.files[0].status, ReviewFileStatus::Renamed);
    assert_eq!(copied.files[0].status, ReviewFileStatus::Copied);
    assert_eq!(renamed.files[0].hunks.len(), 1);
    assert_eq!(copied.files[0].hunks.len(), 1);
    assert_ne!(renamed.files[0].old_path, renamed.files[0].new_path);
}

#[test]
fn decodes_quoted_and_octal_escaped_paths() {
    let parsed =
        parse_unified_diff(include_str!("../../../tests/fixtures/guided_review/quoted_paths.diff"))
            .unwrap();

    let file = &parsed.files[0];
    assert_eq!(file.status, ReviewFileStatus::Renamed);
    assert_eq!(
        file.old_path.as_deref().and_then(|path| path.to_str()),
        Some("docs/café \"plan\".md")
    );
    assert_eq!(
        file.new_path.as_deref().and_then(|path| path.to_str()),
        Some("docs/café \"plan updated\".md")
    );
    assert_eq!(file.hunks.len(), 1);
}

#[test]
fn preserves_rename_and_copy_only_blocks_as_explicit_items() {
    let renamed =
        parse_unified_diff(include_str!("../../../tests/fixtures/guided_review/rename_only.diff"))
            .unwrap();
    let copied =
        parse_unified_diff(include_str!("../../../tests/fixtures/guided_review/copy_only.diff"))
            .unwrap();

    assert_eq!(renamed.files[0].status, ReviewFileStatus::Renamed);
    assert_eq!(copied.files[0].status, ReviewFileStatus::Copied);
    assert_eq!(
        renamed.files[0].non_text.as_ref().map(|change| change.kind),
        Some(NonTextChangeKind::Rename)
    );
    assert_eq!(
        copied.files[0].non_text.as_ref().map(|change| change.kind),
        Some(NonTextChangeKind::Copy)
    );
    assert!(renamed.files[0].hunks.is_empty());
    assert_eq!(renamed.files[0].additions, None);
}

#[test]
fn parses_mode_only_and_submodule_blocks_as_non_text_items() {
    let mode_only =
        parse_unified_diff(include_str!("../../../tests/fixtures/guided_review/mode_only.diff"))
            .unwrap();
    let submodule =
        parse_unified_diff(include_str!("../../../tests/fixtures/guided_review/submodule.diff"))
            .unwrap();

    assert_eq!(
        mode_only.files[0].non_text.as_ref().map(|change| change.kind),
        Some(NonTextChangeKind::ModeOnly)
    );
    assert_eq!(
        mode_only.files[0].non_text.as_ref().map(|change| change.summary.as_str()),
        Some("mode 100644 \u{2192} 100755")
    );
    assert!(!mode_only.files[0].binary);
    assert!(mode_only.files[0].hunks.is_empty());
    assert_eq!(mode_only.files[0].additions, None);
    assert_eq!(mode_only.files[0].deletions, None);
    assert_eq!(
        submodule.files[0].non_text.as_ref().map(|change| change.kind),
        Some(NonTextChangeKind::Submodule)
    );
    assert_eq!(
        submodule.files[0].non_text.as_ref().map(|change| change.summary.as_str()),
        Some("submodule commit changed")
    );
    assert!(!submodule.files[0].binary);
    assert!(submodule.files[0].hunks.is_empty());
    assert_eq!(submodule.files[0].additions, None);
    assert_eq!(submodule.files[0].deletions, None);
}

/// A removed line `-- text` arrives as `--- text`, and inside a hunk it is payload.
///
/// The break this catches: the hunk reader refused a `--- ` line as a stray file header, though
/// position had already told the headers apart, so one changed SQL comment failed the review.
#[test]
fn a_removed_line_that_reads_like_a_header_is_payload() {
    let diff = "diff --git a/q.sql b/q.sql\n--- a/q.sql\n+++ b/q.sql\n@@ -1,2 +1 @@\n--- note\n select 1;\n";
    let file = &parse_unified_diff(diff).unwrap().files[0];
    let line = &file.hunks[0].lines[0];
    assert_eq!(
        (line.kind, line.text.as_str(), line.old_line),
        (ReviewLineKind::Deletion, "-- note", Some(1))
    );
    assert_eq!(file.deletions, Some(1));
}

/// An added line `++ text` arrives as `+++ text`, and inside a hunk it is payload.
#[test]
fn an_added_line_that_reads_like_a_header_is_payload() {
    let diff = "diff --git a/q.c b/q.c\n--- a/q.c\n+++ b/q.c\n@@ -1 +1,2 @@\n x;\n+++ y;\n";
    let file = &parse_unified_diff(diff).unwrap().files[0];
    let line = &file.hunks[0].lines[1];
    assert_eq!(
        (line.kind, line.text.as_str(), line.new_line),
        (ReviewLineKind::Addition, "++ y;", Some(2))
    );
    assert_eq!(file.additions, Some(1));
}
