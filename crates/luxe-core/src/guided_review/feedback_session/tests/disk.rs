//! The session file as it is on disk, pinned against records copied from real `.luxe/reviews`.
//!
//! A round-trip through the current types reads its own output back and would sail straight past
//! a renamed field. These compare what the types write with what a real record holds (as JSON
//! values, so key order is free), read records that predate optional fields, and spell every
//! variant out, so a rename, a dropped default or a changed spelling fails here before it strands
//! a reviewer's comments.

use super::*;

/// A pull request review with a question sent to the agent and answered, a hunk comment and a
/// review-level comment posted to GitHub. Copied from a real record; only the prose is shortened.
const ANSWERED_QUESTION: &str = r#"{
  "schema_version": 1,
  "revision": 8,
  "id": "review:2d2e0e28-fbf2-4a37-bef5-1f7dbc8320b8",
  "target": {
    "kind": "pull_request",
    "number": 7
  },
  "source": {
    "base": {
      "kind": "pull_request",
      "number": 7,
      "repository": "AtonHealth/study-assignment-plugin",
      "base_ref": "dev",
      "head_ref": "least-privilege-deploy-role",
      "merge_base": "5f13b1e33671aad051427616f14ace27d0d957ce",
      "head_oid": "9b587534550ed6882f820d9a8b51bdf201c4ab23"
    },
    "content_digest": "revision:3d4a9a3fc1543db2a16b596a2bc801ccf75716dc0ab1b6a3049228281bacfa7b",
    "includes_staged": false,
    "includes_unstaged": false,
    "included_untracked_files": 0,
    "captured_at_ms": 1791395615967
  },
  "progress": {
    "walked_chapter_ids": [
      "chapter:72862f248dbea7a2b79d99da7b0be02e0c693f3f61b594dcbbac19bd1a1662cc",
      "chapter:8b0a81a7400f9e6d473fb124b813492376f8dbfff40be7364018fc92df4c093f",
      "chapter:cf4790b38a09fabeae69a35e150b87ff69bfcf8df59ff5ca7f96fda37527d463"
    ],
    "seen_item_ids": [],
    "seen_revision": null
  },
  "findings": [
    {
      "id": "finding:2edb0305-609d-425e-ac34-6c823e794ba5",
      "label": "R1",
      "anchor": {
        "kind": "hunk",
        "file_id": "file:f4051d2dca15fde8219e71e843c784a8a1da3810e0646eac54bf786738585d76",
        "hunk_id": "hunk:e99c293ee21b76ee5b3a658aee94263f7b36bdc371ad44b4c022a67048f0159e"
      },
      "severity": "question",
      "title": "Is this list still the source of truth for Pass 2?",
      "body": "Is this list still the source of truth for Pass 2?",
      "proposed_fix": null,
      "state": "open",
      "thread": [
        {
          "author": "agent",
          "body": "Keep the list. The comment is accurate but does two jobs at once.",
          "created_at_ms": 1791396057885
        }
      ]
    },
    {
      "id": "finding:2b45fb8a-e2ed-4d2a-90d5-27a5bf44b33c",
      "label": "R2",
      "anchor": {
        "kind": "review"
      },
      "severity": "issue",
      "title": "Looks good. Approving. Thanks!",
      "body": "Looks good. Approving. Thanks!",
      "proposed_fix": null,
      "state": "draft",
      "thread": [],
      "posted": {
        "review_id": 5446333435,
        "url": "https://github.com/AtonHealth/study-assignment-plugin/pull/7#pullrequestreview-5446333435"
      }
    }
  ],
  "submissions": [
    {
      "id": "submission:15945e29-ab55-451c-b00b-98fda16a89a8",
      "revision_id": "revision:3d4a9a3fc1543db2a16b596a2bc801ccf75716dc0ab1b6a3049228281bacfa7b",
      "mode": "question",
      "findings": [
        {
          "finding_id": "finding:2edb0305-609d-425e-ac34-6c823e794ba5",
          "label": "R1",
          "anchor": {
            "kind": "hunk",
            "file_id": "file:f4051d2dca15fde8219e71e843c784a8a1da3810e0646eac54bf786738585d76",
            "hunk_id": "hunk:e99c293ee21b76ee5b3a658aee94263f7b36bdc371ad44b4c022a67048f0159e"
          },
          "evidence": {
            "path": "scripts/verify-sandbox-guids.ps1",
            "side": null,
            "line": null,
            "symbol": "$Url = $Url.TrimEnd(\"/\")",
            "source": null
          },
          "severity": "question",
          "title": "Is this list still the source of truth for Pass 2?",
          "body": "Is this list still the source of truth for Pass 2?",
          "proposed_fix": null,
          "thread": []
        }
      ],
      "dispatch": "completed",
      "created_at_ms": 1791396046933
    }
  ],
  "outcome": "accepted",
  "created_at_ms": 1791395615973,
  "updated_at_ms": 1791396188461
}"#;

/// A line comment left as a draft, with no `posted` field at all. Copied from a real record, owner
/// renamed.
const DRAFT_LINE_COMMENT: &str = r#"{
  "schema_version": 1,
  "revision": 0,
  "id": "review:f68c9857-65d8-44d8-91e1-5cd0f9332c6a",
  "target": {
    "kind": "pull_request",
    "number": 155
  },
  "source": {
    "base": {
      "kind": "pull_request",
      "number": 155,
      "repository": "nora-quinn/luxe-dev",
      "base_ref": "master",
      "head_ref": "scratch/fix-in-a-worktree",
      "merge_base": "f2b9dfb9d33537b74ed5f358668703a838bf8739",
      "head_oid": "ad8a78736c1359ea1c77ca608ce9392b13254074"
    },
    "content_digest": "revision:32c34cb36a8d0a74db986d6f7829e0d084e3d86a274871ba6449bfcb7557a495",
    "includes_staged": false,
    "includes_unstaged": false,
    "included_untracked_files": 0,
    "captured_at_ms": 1790341983291
  },
  "progress": {
    "walked_chapter_ids": [],
    "seen_item_ids": [],
    "seen_revision": null
  },
  "findings": [
    {
      "id": "finding:f200cee3-8623-48d6-b48c-6cc3b6e2c3f9",
      "label": "R1",
      "anchor": {
        "kind": "line",
        "file_id": "file:99c3810e1cc1c385b9c27b76402fe5b9f0af2f81dace0ca79cdd8c7339c482fc",
        "hunk_id": "hunk:386036e0b2f4222c8265ba6221f53fae597b9ac3224969f9550e3331e77e5663",
        "side": "new",
        "line": 3,
        "context_hash": "sha256:c864ac4ece9d95dde6ef517aa27b42b197ad8f4029ef266b77d5f439ebbad6a6"
      },
      "severity": "issue",
      "title": "Typo: recieve should be receive.",
      "body": "Typo: recieve should be receive.",
      "proposed_fix": null,
      "state": "draft",
      "thread": []
    }
  ],
  "submissions": [],
  "outcome": null,
  "created_at_ms": 1790341983467,
  "updated_at_ms": 1790342041269
}"#;

/// Reads `literal` as a session and writes it back, which must give the record it came from.
fn rewritten(literal: &str) -> (ReviewSession, serde_json::Value) {
    let session: ReviewSession = serde_json::from_str(literal).expect("a real record reads");
    let written = serde_json::to_value(&session).expect("it writes");
    assert_eq!(written, serde_json::from_str::<serde_json::Value>(literal).unwrap());
    (session, written)
}

#[test]
fn a_real_answered_question_reads_and_writes_back_as_written() {
    let (session, _) = rewritten(ANSWERED_QUESTION);
    assert_eq!(session.findings[0].severity, FindingSeverity::Question);
    assert!(matches!(session.findings[0].anchor, FindingAnchor::Hunk { .. }));
    assert_eq!(session.findings[0].thread.len(), 1, "the agent's answer");
    assert!(session.findings[1].posted.is_some(), "the review-level comment went to GitHub");
    assert_eq!(session.submissions[0].dispatch, SubmissionDispatch::Completed);
    assert_eq!(session.submissions[0].findings[0].label, "R1");
}

#[test]
fn a_real_draft_line_comment_reads_and_writes_back_as_written() {
    let (session, written) = rewritten(DRAFT_LINE_COMMENT);
    assert!(matches!(session.findings[0].anchor, FindingAnchor::Line { .. }));
    assert!(session.findings[0].posted.is_none());
    assert!(written["findings"][0].get("posted").is_none(), "an unposted comment writes no field");
}

/// Sessions written before the severities narrowed still carry the old three, and each reads and
/// writes back under its own spelling.
#[test]
fn every_legacy_severity_reads_and_writes_back_as_written() {
    for (spelling, severity) in [
        ("note", FindingSeverity::Note),
        ("suggestion", FindingSeverity::Suggestion),
        ("blocking", FindingSeverity::Blocking),
    ] {
        let literal = DRAFT_LINE_COMMENT
            .replace("\"severity\": \"issue\"", &format!("\"severity\": \"{spelling}\""));
        assert_ne!(literal, DRAFT_LINE_COMMENT, "the literal carries a severity to replace");
        let (session, _) = rewritten(&literal);
        assert_eq!(session.findings[0].severity, severity, "{spelling}");
    }
}

/// Your own pull request's thread, adopted as a comment and sent with its author. No real record
/// holds one yet, so this is written from the fields `adopt_github_threads` and a submission fill.
const GITHUB_THREAD: &str = r#"{
  "id": "finding:00000000-0000-4000-8000-000000000001",
  "label": "R3",
  "anchor": { "kind": "chapter", "chapter_id": "chapter:abc" },
  "severity": "issue",
  "title": "Keep the old path",
  "body": "Keep the old path.",
  "proposed_fix": null,
  "state": "needs_verification",
  "thread": [
    { "author": "reviewer", "body": "Still wrong.", "created_at_ms": 2 },
    { "author": "system", "body": "Moved.", "created_at_ms": 3 }
  ],
  "github_thread": {
    "id": "PRRT_kwDOKbIFZc6k0MXq",
    "url": "https://github.com/acme/app/pull/146#discussion_r1",
    "author": "mara",
    "can_reply": true,
    "can_resolve": false,
    "reply_url": "https://github.com/acme/app/pull/146#discussion_r2",
    "resolved": true,
    "conversation": true
  }
}"#;

#[test]
fn a_github_thread_reads_and_writes_back_as_written() {
    let finding: ReviewFinding = serde_json::from_str(GITHUB_THREAD).expect("it reads");
    let thread = finding.github_thread.as_ref().expect("the thread");
    assert!(thread.conversation && thread.resolved && thread.reply_url.is_some());
    assert_eq!(
        serde_json::to_value(&finding).unwrap(),
        serde_json::from_str::<serde_json::Value>(GITHUB_THREAD).unwrap()
    );
    let mut submitted = serde_json::from_str::<serde_json::Value>(ANSWERED_QUESTION).unwrap()
        ["submissions"][0]["findings"][0]
        .clone();
    submitted["github_author"] = "mara".into();
    let read: SubmittedFinding = serde_json::from_value(submitted.clone()).expect("it reads");
    assert_eq!(read.github_author.as_deref(), Some("mara"));
    assert_eq!(serde_json::to_value(&read).unwrap(), submitted);
}

/// Records written before labels, evidence and submitted threads existed still read, each field
/// taking its default.
#[test]
fn a_record_from_before_the_optional_fields_still_reads() {
    let mut record = serde_json::from_str::<serde_json::Value>(ANSWERED_QUESTION).unwrap();
    record["findings"][1].as_object_mut().unwrap().remove("label");
    let submitted = record["submissions"][0]["findings"][0].as_object_mut().unwrap();
    for field in ["label", "evidence", "thread"] {
        submitted.remove(field);
    }
    let mut line = serde_json::from_str::<serde_json::Value>(DRAFT_LINE_COMMENT).unwrap();
    line["findings"][0]["anchor"].as_object_mut().unwrap().remove("context_hash");

    let session: ReviewSession = serde_json::from_value(record).expect("an older record reads");
    assert_eq!(session.findings[1].label, "");
    let submitted = &session.submissions[0].findings[0];
    assert_eq!((submitted.label.as_str(), submitted.thread.len()), ("", 0));
    assert_eq!(submitted.evidence, FindingEvidence::default());
    let line: ReviewSession =
        serde_json::from_value(line).expect("a line without its digest reads");
    assert!(
        matches!(&line.findings[0].anchor, FindingAnchor::Line { context_hash, .. } if context_hash.is_empty())
    );
}

/// Writes `value` and reads `spelling` back, both of which must match.
fn spelled<T>(value: T, spelling: &str)
where
    T: Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    assert_eq!(serde_json::to_value(&value).unwrap(), serde_json::json!(spelling), "{value:?}");
    assert_eq!(serde_json::from_value::<T>(serde_json::json!(spelling)).unwrap(), value);
}

/// Every variant a session file can hold, spelled the way the files spell it.
#[test]
fn every_persisted_variant_keeps_its_spelling() {
    use FindingState as State;
    for (state, spelling) in [
        (State::Draft, "draft"),
        (State::Open, "open"),
        (State::NeedsVerification, "needs_verification"),
        (State::Resolved, "resolved"),
        (State::Stale, "stale"),
    ] {
        spelled(state, spelling);
    }
    for (dispatch, spelling) in [
        (SubmissionDispatch::Queued, "queued"),
        (SubmissionDispatch::Dispatched, "dispatched"),
        (SubmissionDispatch::FailedRetryable, "failed_retryable"),
        (SubmissionDispatch::Completed, "completed"),
    ] {
        spelled(dispatch, spelling);
    }
    for (mode, spelling) in [
        (ReviewFeedbackMode::RequestFixes, "request_fixes"),
        (ReviewFeedbackMode::CommentOnly, "comment_only"),
        (ReviewFeedbackMode::Question, "question"),
    ] {
        spelled(mode, spelling);
    }
    for (outcome, spelling) in [
        (ReviewOutcome::Accepted, "accepted"),
        (ReviewOutcome::ChangesRequested, "changes_requested"),
        (ReviewOutcome::Commented, "commented"),
    ] {
        spelled(outcome, spelling);
    }
    for (author, spelling) in [
        (FindingMessageAuthor::Reviewer, "reviewer"),
        (FindingMessageAuthor::Agent, "agent"),
        (FindingMessageAuthor::System, "system"),
    ] {
        spelled(author, spelling);
    }
    spelled(DiffSide::Old, "old");
    spelled(DiffSide::New, "new");
}
