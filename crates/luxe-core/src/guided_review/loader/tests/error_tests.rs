//! Tests — typed error surfaces and bounded stderr.

use super::super::*;

#[tokio::test]
async fn local_git_maps_a_failed_command_to_a_bounded_typed_error() {
    let project = tempfile::tempdir().unwrap();
    let error = LocalGit::new(project.path())
        .run("verify head", &["rev-parse", "--verify", "HEAD"])
        .await
        .unwrap_err();

    match error {
        GuidedReviewError::GitFailed { operation, stderr, .. } => {
            assert_eq!(operation, "verify head");
            assert!(!stderr.is_empty());
            assert!(stderr.len() <= MAX_GIT_STDERR_CHARS);
        }
        other => panic!("expected a Git failure, got {other:?}"),
    }
}

#[test]
fn stderr_is_control_safe_and_bounded() {
    let stderr = format!("\u{1b}[31m{}", "x".repeat(MAX_GIT_STDERR_CHARS + 1));
    let bounded = bounded_git_stderr(stderr.as_bytes());

    assert_eq!(bounded.len(), MAX_GIT_STDERR_CHARS);
    assert!(!bounded.contains('\u{1b}'));
}

#[test]
fn unavailable_errors_name_the_actual_source_constraint() {
    let error = GuidedReviewError::from(ReviewSourceUnavailable::ExcludedUntracked {
        path: PathBuf::from("large.generated"),
        constraint: "exceeds the configured source size limit".into(),
    });

    assert!(error.to_string().contains("large.generated"));
    assert!(error.to_string().contains("size limit"));
}
