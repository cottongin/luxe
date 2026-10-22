use super::*;
use crate::guided_review::{parse_unified_diff, TurnProvenance};

fn review_with(diff: &str) -> GuidedReview {
    let files = parse_unified_diff(diff).expect("fixture diff parses").files;
    let revision_id = super::super::ReviewRevisionId::from_canonical_bytes(diff.as_bytes());
    GuidedReview {
        conversation: None,
        id: super::super::ReviewId::new(),
        revision_id: revision_id.clone(),
        target: super::super::ReviewTarget::WorkingTree,
        source: super::super::ReviewSource {
            base: super::super::ReviewBase::EmptyTree,
            content_digest: revision_id,
            includes_staged: false,
            includes_unstaged: true,
            included_untracked_files: 0,
            captured_at_ms: 0,
            uncovered_paths: Vec::new(),
        },
        context: super::super::ReviewContextDraft::default().build(),
        stats: super::super::ReviewStats::from_files(&files),
        files,
        chapters: Vec::new(),
        checks: super::super::CheckSummary::default(),
        generated_at_ms: 0,
        insight: None,
        provenance: None,
        seen: None,
        brief: None,
    }
}

/// Moving a test, touching CI, or dropping a lint flag is ordinary work, not a call-out: guessing
/// "weakened" from those shapes fired on every test moved into its own file.
#[test]
fn gate_shaped_changes_are_not_called_out() {
    for diff in [
        "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,1 +1,2 @@\n-#[test]\n+#[ignore]\n+#![allow(dead_code)]\n",
        "diff --git a/.github/workflows/ci.yml b/.github/workflows/ci.yml\n--- a/.github/workflows/ci.yml\n+++ b/.github/workflows/ci.yml\n@@ -1,1 +1,1 @@\n-  run: cargo clippy -- -D warnings\n+  run: cargo clippy\n",
        "diff --git a/deny.toml b/deny.toml\n--- a/deny.toml\n+++ b/deny.toml\n@@ -1,1 +1,1 @@\n-a = 1\n+a = 2\n",
    ] {
        let signals = review_attention(&review_with(diff));
        assert!(signals.is_empty(), "{signals:?}");
    }
}

#[test]
fn a_committed_credential_outranks_everything_else() {
    let review = review_with(
        "diff --git a/src/config.rs b/src/config.rs\n--- a/src/config.rs\n+++ b/src/config.rs\n@@ -1,1 +1,2 @@\n let a = 1;\n+let key = \"sk-7Kq2mZv9RtY4bNx8Wc3LpF6h\";\n", // luxe:allow-secret
    );

    let signals = review_attention(&review);

    assert_eq!(signals[0].kind, AttentionKind::SecretShaped);
    // The location is reported; the value itself never is.
    assert!(!signals[0].detail.contains("sk-7Kq2mZv9RtY4bNx8Wc3LpF6h")); // luxe:allow-secret
    assert!(signals[0].hunk_id.is_some(), "a credential is anchored to its hunk");
}

/// An unread edit is a call-out; a change the agent did not make is not. Who wrote a line says
/// nothing about whether it needs a decision, and guessing it off the transcript was wrong often
/// enough (formatters, pasted files, a path spelled two ways) to hide real work.
#[test]
fn an_unread_edit_is_called_out_and_authorship_is_not() {
    let mut review = review_with(
        "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,1 +1,1 @@\n-let a = 1;\n+let a = 2;\ndiff --git a/src/b.rs b/src/b.rs\n--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1,1 +1,1 @@\n-let b = 1;\n+let b = 2;\n",
    );
    review.provenance = Some(TurnProvenance {
        written_paths: ["src/a.rs".to_owned()].into_iter().collect(),
        ..TurnProvenance::default()
    });

    let signals = review_attention(&review);

    assert_eq!(signals.len(), 1, "{signals:?}");
    assert_eq!(signals[0].kind, AttentionKind::BlindEdit);
    assert_eq!(signals[0].path.as_deref(), Some("src/a.rs"));
}

/// The list is only useful at a length someone reads: one cheap signal must never crowd out the
/// rest. A real review put 236 graph rows in front of a possible credential.
#[test]
fn one_kind_cannot_crowd_out_the_others() {
    let foreign: std::collections::BTreeSet<String> =
        (0..40).map(|index| format!("src/other_{index}.rs")).collect();
    let mut review = review_with(
        "diff --git a/src/config.rs b/src/config.rs\n--- a/src/config.rs\n+++ b/src/config.rs\n@@ -1,1 +1,2 @@\n let a = 1;\n+let key = \"sk-7Kq2mZv9RtY4bNx8Wc3LpF6h\";\n", // luxe:allow-secret
    );
    review.provenance = Some(TurnProvenance {
        written_paths: foreign.clone(),
        read_paths: foreign,
        ..Default::default()
    });

    let signals = review_attention(&review);

    assert_eq!(signals[0].kind, AttentionKind::SecretShaped, "the credential still leads");
    assert!(signals.len() <= 8, "a list a reviewer reads in full: {}", signals.len());
}

/// A credential is a credential wherever it lands, including a README.
#[test]
fn a_credential_in_prose_is_still_reported() {
    let review = review_with(
        "diff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n@@ -1,1 +1,2 @@\n docs\n+export KEY=sk-7Kq2mZv9RtY4bNx8Wc3LpF6h\n", // luxe:allow-secret
    );

    let signals = review_attention(&review);

    assert_eq!(signals[0].kind, AttentionKind::SecretShaped);
}

/// The detector's own tests, a setup example and a fixture all contain the shape on purpose.
/// This file flagged *itself* in a real review, twice, which is how a warning list loses a
/// reader.
#[test]
fn a_manufactured_token_is_not_reported_as_a_credential() {
    for placeholder in [
        "sk-abcdefghijklmnopqrstuvwxyz123456",
        "sk-xxxxxxxxxxxxxxxxxxxxxxxx",
        "sk-live_example_key_do_not_use_1234",
        "ghp_exampleexampleexampleexample",
    ] {
        let review = review_with(&format!(
            "diff --git a/src/scan.rs b/src/scan.rs\n--- a/src/scan.rs\n+++ b/src/scan.rs\n@@ -1,1 +1,2 @@\n let a = 1;\n+let fixture = \"{placeholder}\";\n"
        ));

        let signals = review_attention(&review);

        assert!(
            !signals.iter().any(|signal| signal.kind == AttentionKind::SecretShaped),
            "{placeholder} is a placeholder, not a credential: {signals:?}"
        );
    }
}

/// The loose `name = value` rule is the one that fires on ordinary code, so it is held to a
/// higher bar: real code, real literal. Published key formats are not — a leak is a leak.
#[test]
fn the_loose_rule_needs_real_code_but_a_real_key_format_does_not() {
    let quiet = [
        "fn is_placeholder_token(token: &str) -> bool {", // a parameter, not an assignment
        "// `password = \"hunter2\"` is worth a look", // documentation about secrets // luxe:allow-secret
        "    let token = compute();",                  // a value, but not a literal
    ];
    for line in quiet {
        let review = review_with(&format!(
            "diff --git a/src/scan.rs b/src/scan.rs\n--- a/src/scan.rs\n+++ b/src/scan.rs\n@@ -1,1 +1,2 @@\n existing\n+{line}\n"
        ));
        assert!(
            !review_attention(&review)
                .iter()
                .any(|signal| signal.kind == AttentionKind::SecretShaped),
            "not a credential: {line}"
        );
    }

    let loud = [
        "    password = \"h7Kq2mZv9Rt\";", // a literal assignment in code // luxe:allow-secret
        "// contact support with ghp_9fKq2mZv8RtY4bNx7Wc3LpF6hJd5s", // a real format, anywhere // luxe:allow-secret
    ];
    for line in loud {
        let review = review_with(&format!(
            "diff --git a/src/scan.rs b/src/scan.rs\n--- a/src/scan.rs\n+++ b/src/scan.rs\n@@ -1,1 +1,2 @@\n existing\n+{line}\n"
        ));
        assert!(
            review_attention(&review)
                .iter()
                .any(|signal| signal.kind == AttentionKind::SecretShaped),
            "worth a warning: {line}"
        );
    }
}

/// An explicit marker is the only way to silence a plausible token, and it silences exactly the
/// line it sits on.
#[test]
fn an_allowlisted_line_is_not_reported_but_its_neighbor_still_is() {
    let review = review_with(
        "diff --git a/src/scan.rs b/src/scan.rs\n--- a/src/scan.rs\n+++ b/src/scan.rs\n@@ -1,1 +1,3 @@\n let a = 1;\n+let fixture = \"sk-7Kq2mZv9RtY4bNx8Wc3LpF6h\"; // luxe:allow-secret\n+let real = \"sk-3Jd8pQw5ZxV2nMy7Tb4KrH9c\";\n",
    );

    let signals = review_attention(&review);
    let secrets: Vec<_> =
        signals.iter().filter(|signal| signal.kind == AttentionKind::SecretShaped).collect();

    assert_eq!(secrets.len(), 1, "the marked line is exempt, the one below it is not: {secrets:?}");
}

/// The exemption judges the token's randomness, so anything that could plausibly be live still
/// gets called out. Without this the fix above would be a hole rather than a filter.
#[test]
fn a_random_looking_token_is_still_reported_wherever_it_lands() {
    for path in ["src/config.rs", "README.md", "config/settings.json"] {
        let review = review_with(&format!(
            "diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n@@ -1,1 +1,2 @@\n existing\n+key = \"sk-7Kq2mZv9RtY4bNx8Wc3LpF6h\"\n" // luxe:allow-secret
        ));

        let signals = review_attention(&review);

        assert!(
            signals.iter().any(|signal| signal.kind == AttentionKind::SecretShaped),
            "a plausible credential in {path} must be reported: {signals:?}"
        );
    }
}

/// Two hunks of one file tripping the same detector say the same thing, so they are one row.
#[test]
fn an_exact_repeat_is_listed_once() {
    let review = review_with(
        "diff --git a/src/config.rs b/src/config.rs\n--- a/src/config.rs\n+++ b/src/config.rs\n@@ -1,1 +1,2 @@\n let a = 1;\n+let key = \"sk-7Kq2mZv9RtY4bNx8Wc3LpF6h\";\n@@ -40,1 +41,2 @@\n let b = 2;\n+let other = \"sk-9Jr4nXw2QsZ7cMv5Hb8TkD3g\";\n", // luxe:allow-secret
    );

    let signals = review_attention(&review);

    assert_eq!(
        signals.iter().filter(|signal| signal.kind == AttentionKind::SecretShaped).count(),
        1,
        "{signals:?}"
    );
}

#[test]
fn nothing_notable_produces_no_signals() {
    let review = review_with(
        "diff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n@@ -1,1 +1,1 @@\n-old prose\n+new prose\n",
    );

    assert!(review_attention(&review).is_empty());
}
