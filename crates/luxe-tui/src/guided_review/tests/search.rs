//! Search in the diff.

use super::super::*;
use super::{review_of, text_file};

#[test]
fn find_in_diff_jumps_across_files_and_wraps() {
    let file_a = text_file("src/a.rs");
    let file_b = text_file("src/b.rs");
    let files = vec![file_a.clone(), file_b.clone()];
    let review = review_of(&files);
    let mut workspace = GuidedReviewWorkspace::default();
    workspace.position.file_id = Some(file_a.id.clone());
    workspace.position.hunk_id = Some(file_a.hunks[0].id.clone());
    workspace.position.line_offset = 0;
    workspace.begin_search();
    for character in "value".chars() {
        workspace.search_input(character);
    }

    // Row 0 is the hunk header; row 1 is the matching "let value = 1;" line.
    assert!(workspace.jump_search(&review, true));
    assert_eq!(workspace.position.file_id.as_ref(), Some(&file_a.id));
    assert_eq!(workspace.position.line_offset, 1);

    // No further match in file A, so the next jump crosses into file B.
    assert!(workspace.jump_search(&review, true));
    assert_eq!(
        workspace.position.file_id.as_ref(),
        Some(&file_b.id),
        "the search jumps to the next file's match"
    );
    assert_eq!(workspace.position.line_offset, 1);

    // And once more wraps back to file A.
    assert!(workspace.jump_search(&review, true));
    assert_eq!(
        workspace.position.file_id.as_ref(),
        Some(&file_a.id),
        "the search wraps back to the first file"
    );

    // Backward from file A crosses to file B's last match.
    assert!(workspace.jump_search(&review, false));
    assert_eq!(workspace.position.file_id.as_ref(), Some(&file_b.id));
}
