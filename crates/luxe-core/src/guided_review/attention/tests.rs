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

#[test]
fn a_disabled_or_deleted_test_is_called_out() {
    let review = review_with(
        "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,1 +1,1 @@\n-#[test]\n+#[ignore]\n",
    );

    let signals = review_attention(&review);

    assert!(signals.iter().all(|signal| signal.kind == AttentionKind::GateWeakened));
    let details: Vec<&str> = signals.iter().map(|signal| signal.detail.as_str()).collect();
    assert!(details.iter().any(|detail| detail.contains("a test was removed")));
    assert!(details.iter().any(|detail| detail.contains("a test was disabled")));
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

#[test]
fn ci_and_supply_chain_paths_are_gate_changes_by_definition() {
    let review = review_with(
        "diff --git a/.github/workflows/ci.yml b/.github/workflows/ci.yml\n--- a/.github/workflows/ci.yml\n+++ b/.github/workflows/ci.yml\n@@ -1,1 +1,1 @@\n-  run: cargo test\n+  run: cargo test || true\n",
    );

    let signals = review_attention(&review);

    assert_eq!(signals[0].kind, AttentionKind::GateWeakened);
    assert!(signals[0].detail.contains("CI or supply-chain gating"));
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

/// Prose that mentions a gate has not weakened one.
#[test]
fn a_document_quoting_a_gate_flag_is_not_a_weakened_gate() {
    let review = review_with(
        "diff --git a/PLAN.md b/PLAN.md\n--- a/PLAN.md\n+++ b/PLAN.md\n@@ -1,1 +1,2 @@\n-run clippy with -D warnings\n+run clippy with the warning gate on\n+#[ignore]\n",
    );

    assert!(
        review_attention(&review).is_empty(),
        "documentation is not a gate: {:?}",
        review_attention(&review)
    );
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

/// A serialized record quotes the very command that enforces the gate. Reporting that as a
/// weakened gate is the same category error as reading a changelog as code.
#[test]
fn a_data_file_quoting_the_lint_gate_is_not_a_weakened_gate() {
    let review = review_with(
        "diff --git a/.agent/state.json b/.agent/state.json\n--- a/.agent/state.json\n+++ b/.agent/state.json\n@@ -1,2 +1,2 @@\n {\n-  \"gate\": \"cargo clippy --workspace -- -D warnings\"\n+  \"gate\": \"cargo clippy --workspace --all-targets\"\n",
    );

    let signals = review_attention(&review);

    assert!(
        !signals.iter().any(|signal| signal.kind == AttentionKind::GateWeakened),
        "a state file records a command, it does not gate anything: {signals:?}"
    );
}

/// ...while the same edit to a real gate still counts.
#[test]
fn dropping_the_lint_gate_from_a_script_is_still_reported() {
    let review = review_with(
        "diff --git a/scripts/gate.sh b/scripts/gate.sh\n--- a/scripts/gate.sh\n+++ b/scripts/gate.sh\n@@ -1,2 +1,2 @@\n set -euo pipefail\n-cargo clippy --workspace -- -D warnings\n+cargo clippy --workspace\n",
    );

    let signals = review_attention(&review);

    assert!(
        signals.iter().any(|signal| signal.kind == AttentionKind::GateWeakened),
        "the gate really was relaxed here: {signals:?}"
    );
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
