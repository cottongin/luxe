use super::*;

// The IDs and wrappers below are copied from real fix turns (`scripts/remediation-eval.py`), so
// what is pinned is what models write, not what a test author imagined they might.
const REVIEW: &str = "review:0becfd87-0fe5-45db-9aef-f30091e98bd4";
const REVISION: &str = "revision:5b086946b7aad397e13d5ecb097e375bbb2bc1c9f56b287ef915d05b83d7fadb";
const FIRST: &str = "finding:810ed8b2-afd5-49d6-8fc4-569c3d41d0bb";
const SECOND: &str = "finding:6852a6a0-d6d9-4b2b-8b07-7a0f4d4a73ec";

fn ids() -> (ReviewId, ReviewRevisionId, Vec<FindingId>) {
    let id = |text: &str| serde_json::Value::from(text);
    (
        serde_json::from_value(id(REVIEW)).unwrap(),
        serde_json::from_value(id(REVISION)).unwrap(),
        vec![
            serde_json::from_value(id(FIRST)).unwrap(),
            serde_json::from_value(id(SECOND)).unwrap(),
        ],
    )
}

fn extract(text: &str) -> Result<Option<RemediationReport>, RemediationReportError> {
    let (review, revision, known) = ids();
    extract_remediation_report(text, &review, &revision, known)
}

fn statuses(text: &str) -> Vec<(String, RemediationStatus)> {
    extract(text)
        .expect("a report Luxe can use")
        .expect("a report")
        .findings
        .into_iter()
        .map(|result| (result.finding_id.as_str().to_string(), result.status))
        .collect()
}

/// Claude Haiku 4.5, and gpt-4o every time: right JSON, a `json` fence.
const JSON_FENCE: &str = r#"2. **stats.py:8-9** — Added an empty list check that returns 0.0 before dividing.

```json
{
  "review_id": "review:0becfd87-0fe5-45db-9aef-f30091e98bd4",
  "revision_id": "revision:5b086946b7aad397e13d5ecb097e375bbb2bc1c9f56b287ef915d05b83d7fadb",
  "findings": [
    {
      "finding_id": "finding:810ed8b2-afd5-49d6-8fc4-569c3d41d0bb",
      "status": "fixed",
      "note": "Changed range(1, len(numbers)) to range(len(numbers)) to include the first element in the total"
    },
    {
      "finding_id": "finding:6852a6a0-d6d9-4b2b-8b07-7a0f4d4a73ec",
      "status": "fixed",
      "note": "Added guard clause returning 0.0 when numbers list is empty, preventing division by zero"
    }
  ]
}
```"#;

/// gpt-5-mini every time: the fence's name as a line of its own, then bare JSON.
const LABEL_LINE: &str = r#"If you'd like, I can add simple unit tests for total() and average() — say yes and I'll add them.

luxe-remediation
{"review_id":"review:0becfd87-0fe5-45db-9aef-f30091e98bd4","revision_id":"revision:5b086946b7aad397e13d5ecb097e375bbb2bc1c9f56b287ef915d05b83d7fadb","findings":[{"finding_id":"finding:810ed8b2-afd5-49d6-8fc4-569c3d41d0bb","status":"fixed","note":"Loop now iterates over all elements."},{"finding_id":"finding:6852a6a0-d6d9-4b2b-8b07-7a0f4d4a73ec","status":"fixed","note":"average() now returns 0.0 for empty input."}]}"#;

/// Most models: the named fence, as asked.
const NAMED_FENCE: &str = r#"Fixed both. Nothing else touched.

```luxe-remediation
{"review_id":"review:0becfd87-0fe5-45db-9aef-f30091e98bd4","revision_id":"revision:5b086946b7aad397e13d5ecb097e375bbb2bc1c9f56b287ef915d05b83d7fadb","findings":[{"finding_id":"finding:810ed8b2-afd5-49d6-8fc4-569c3d41d0bb","status":"fixed"},{"finding_id":"finding:6852a6a0-d6d9-4b2b-8b07-7a0f4d4a73ec","status":"not_fixed","note":"Out of scope."}]}
```"#;

/// The wrapper never decides whether there is a report: every one models were seen writing, and
/// the others a Markdown writer reaches for, read the same.
#[test]
fn a_report_reads_the_same_whatever_wraps_it() {
    use RemediationStatus::{Fixed, NotFixed};
    let both_fixed = vec![(FIRST.to_string(), Fixed), (SECOND.to_string(), Fixed)];
    assert_eq!(statuses(JSON_FENCE), both_fixed);
    assert_eq!(statuses(LABEL_LINE), both_fixed);
    assert_eq!(
        statuses(NAMED_FENCE),
        vec![(FIRST.to_string(), Fixed), (SECOND.to_string(), NotFixed)]
    );
    assert_eq!(statuses(&JSON_FENCE.replace("```json", "~~~").replace("```", "~~~")), both_fixed);
    assert_eq!(statuses(&JSON_FENCE.replace('\n', "\r\n")), both_fixed);
    let indented = JSON_FENCE.lines().map(|line| format!("   {line}")).collect::<Vec<_>>();
    assert_eq!(statuses(&format!("- Done:\n{}", indented.join("\n"))), both_fixed);
    let inline = LABEL_LINE.replace("luxe-remediation\n", "Report: ");
    assert_eq!(statuses(&inline), both_fixed);
}

/// Prose, code and JSON that is not a report are not a report.
#[test]
fn a_reply_without_a_report_has_none() {
    assert_eq!(extract("Implemented the requested fix."), Ok(None));
    assert_eq!(extract("```rust\nfn main() { let x = {1}; }\n```"), Ok(None));
    assert_eq!(extract("```json\n{\"name\": \"stats\", \"version\": 2}\n```"), Ok(None));
}

/// What a report is about is checked strictly: another review or revision refuses it whole.
#[test]
fn a_report_about_something_else_is_refused_whole() {
    let other_review = NAMED_FENCE.replace(REVIEW, "review:00000000-0000-0000-0000-000000000000");
    assert_eq!(extract(&other_review), Err(RemediationReportError::WrongReview));
    let other_revision = NAMED_FENCE.replace(REVISION, "revision:feed");
    assert_eq!(extract(&other_revision), Err(RemediationReportError::WrongRevision));
    let unprefixed = NAMED_FENCE
        .replace(REVIEW, REVIEW.trim_start_matches("review:"))
        .replace(REVISION, REVISION.trim_start_matches("revision:"))
        .replace(FIRST, FIRST.trim_start_matches("finding:"));
    assert_eq!(statuses(&unprefixed).len(), 2, "an ID without its prefix is still that ID");
}

/// A report that leaves the review and revision out is the turn's: the turn is pinned to them,
/// and a model is not made to copy a 64-digit hash to say so.
#[test]
fn a_report_without_its_review_and_revision_is_the_turns() {
    let bare = format!("{{\"findings\":[{{\"finding_id\":\"{FIRST}\",\"status\":\"fixed\"}}]}}");
    assert_eq!(statuses(&bare), vec![(FIRST.to_string(), RemediationStatus::Fixed)]);
    let report = extract(&bare).unwrap().unwrap();
    let (review, revision, _) = ids();
    assert_eq!((report.review_id, report.revision_id), (review, revision));
    // and hidden from the reader like any other
    assert_eq!(without_remediation_report(&format!("Done.\n```json\n{bare}\n```")), "Done.");
}

/// Inside a report, each entry stands alone: a bad one is dropped and the rest are kept.
#[test]
fn each_entry_stands_on_its_own() {
    use RemediationStatus::{Fixed, NeedsClarification, NotFixed};
    let report = |findings: serde_json::Value| {
        serde_json::json!({ "review_id": REVIEW, "revision_id": REVISION, "findings": findings })
            .to_string()
    };
    let spelled = report(serde_json::json!([
        { "finding_id": FIRST, "status": "Fixed", "extra": "ignored" },
        { "finding_id": SECOND, "status": "Needs-Clarification" },
    ]));
    assert_eq!(
        statuses(&spelled),
        vec![(FIRST.into(), Fixed), (SECOND.into(), NeedsClarification)]
    );
    assert_eq!(statuses(&spelled.replace("Needs-Clarification", "not fixed"))[1].1, NotFixed);

    let mixed = report(serde_json::json!([
        { "finding_id": "finding:not-sent", "status": "fixed" },
        { "finding_id": FIRST, "status": "partially_fixed" },
        { "finding_id": SECOND, "status": "fixed" },
        { "finding_id": SECOND, "status": "not_fixed" },
    ]));
    assert_eq!(
        statuses(&mixed),
        vec![(SECOND.into(), Fixed)],
        "the first word on a comment stands"
    );

    let nothing =
        report(serde_json::json!([{ "finding_id": "finding:not-sent", "status": "fixed" }]));
    assert_eq!(extract(&nothing), Err(RemediationReportError::NothingUsable));
    assert_eq!(extract(&report(serde_json::json!([]))), Err(RemediationReportError::NothingUsable));
}

/// A note too long is cut, on a character boundary and marked, rather than costing the report.
#[test]
fn a_long_note_is_cut_not_fatal() {
    let note = "é".repeat(MAX_REMEDIATION_NOTE_BYTES);
    let text = serde_json::json!({ "review_id": REVIEW, "revision_id": REVISION, "findings": [
        { "finding_id": FIRST, "status": "fixed", "note": note },
    ]})
    .to_string();
    let kept = extract(&text).unwrap().unwrap().findings[0].note.clone().unwrap();
    assert!(kept.ends_with('…'), "marked where it was cut");
    assert!(kept.len() <= MAX_REMEDIATION_NOTE_BYTES, "{} bytes", kept.len());
    assert!(
        kept.len() > MAX_REMEDIATION_NOTE_BYTES - 8,
        "and uses the room it has: {}",
        kept.len()
    );
}

/// The last report that reads wins; one that does not read never undoes one that did.
#[test]
fn the_last_report_that_reads_wins() {
    let corrected =
        format!("{}\n\nCorrection:\n{}", NAMED_FENCE.replace("not_fixed", "fixed"), NAMED_FENCE);
    assert_eq!(statuses(&corrected)[1].1, RemediationStatus::NotFixed);
    let then_garbage = format!("{NAMED_FENCE}\n{{\"review_id\": \"oops\", }}");
    assert_eq!(statuses(&then_garbage).len(), 2);
    let only_garbage = "{\"review_id\": \"review:x\", \"findings\": [";
    assert!(matches!(extract(only_garbage), Err(RemediationReportError::InvalidJson(_))));
}

/// A report may take its whole budget and not a byte more.
#[test]
fn a_report_has_a_size_budget() {
    let compact = serde_json::json!({ "review_id": REVIEW, "revision_id": REVISION, "findings": [
        { "finding_id": FIRST, "status": "fixed" },
    ]})
    .to_string();
    // whitespace inside the object counts toward its size and changes nothing else
    let sized =
        |bytes: usize| format!("{{ {}{}", " ".repeat(bytes - compact.len() - 1), &compact[1..]);
    assert_eq!(sized(MAX_REMEDIATION_REPORT_BYTES).len(), MAX_REMEDIATION_REPORT_BYTES);
    assert!(extract(&sized(MAX_REMEDIATION_REPORT_BYTES)).unwrap().is_some());
    assert_eq!(
        extract(&sized(MAX_REMEDIATION_REPORT_BYTES + 1)),
        Err(RemediationReportError::TooLarge)
    );
}

/// The template carries the real IDs, and reads as no report until a status is chosen, so a model
/// that echoes it untouched is asked again rather than believed.
#[test]
fn the_template_reads_once_its_statuses_are_filled_in() {
    let (_, _, known) = ids();
    let template = remediation_report_template(&known);
    assert!(template.starts_with("```luxe-remediation\n") && template.ends_with("\n```"));
    assert!(template.contains(FIRST) && template.contains(SECOND));
    assert!(!template.contains("review") && !template.contains("revision"), "nothing to miscopy");
    assert_eq!(extract(&template), Err(RemediationReportError::NothingUsable));
    let filled = template.replacen(STATUS_PLACEHOLDER, "fixed", 1);
    let report = extract(&filled).unwrap().unwrap();
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].note, None, "the placeholder note is no note");
}

/// The report goes, whole or half-arrived, and the prose around it stays as it was.
#[test]
fn a_person_sees_the_prose_without_the_report() {
    let report = "```luxe-remediation\n{\"findings\":[]}\n```";
    assert_eq!(without_remediation_report(&format!("Fixed it.\n{report}")), "Fixed it.");
    assert_eq!(
        without_remediation_report(&format!("Fixed it.\n\n{report}\n\nAnything else?")),
        "Fixed it.\n\nAnything else?"
    );
    assert_eq!(without_remediation_report(report), "");
    let padded = "Fixed.\n```luxe-remediation\n{}\n```   \nAnything else?";
    assert_eq!(without_remediation_report(padded), "Fixed.\n\nAnything else?");
    assert_eq!(without_remediation_report("Fixed.\n```luxe-remediation\n{\"find"), "Fixed.");
    assert_eq!(without_remediation_report("Fixed.\n```luxe-rem"), "Fixed.");
    let fence = "Here:\n```rust\nfn main() {}\n```";
    assert!(matches!(without_remediation_report(fence), std::borrow::Cow::Borrowed(_)));
    assert_eq!(without_remediation_report("Open one:\n```"), "Open one:\n```");
}

/// However the report was wrapped, the reader sees the prose and not the JSON, and not the label
/// line a model put in front of it either.
#[test]
fn a_person_never_sees_a_report_however_it_was_wrapped() {
    let prose = "2. **stats.py:8-9** — Added an empty list check that returns 0.0 before dividing.";
    assert_eq!(without_remediation_report(JSON_FENCE), prose);
    let asked = "If you'd like, I can add simple unit tests for total() and average() — say yes \
                 and I'll add them.";
    assert_eq!(without_remediation_report(LABEL_LINE), asked);
    let after = format!("{LABEL_LINE}\n\nAnything else?");
    assert_eq!(without_remediation_report(&after), format!("{asked}\n\nAnything else?"));
    // still arriving, fenced and bare
    let half = &JSON_FENCE[..JSON_FENCE.find("\"findings\"").unwrap()];
    assert_eq!(without_remediation_report(half), prose);
    let half = &LABEL_LINE[..LABEL_LINE.find("\"findings\"").unwrap()];
    assert_eq!(without_remediation_report(half), asked);

    let report = &LABEL_LINE[LABEL_LINE.find('{').unwrap()..];
    // a code fence before the report is passed over whole, and a bare fence is a fence
    let code = "Changed:\n```python\nreturn 0.0\n```";
    let fenced = format!("{code}\n\n```\n{report}\n```\n\nDone.");
    assert_eq!(without_remediation_report(&fenced), format!("{code}\n\nDone."));
    // a closing fence closes: it never opens a block that would swallow a bare report after it
    let between = format!("{code}\n{report}\n\nThen:\n```sh\nls\n```");
    assert_eq!(without_remediation_report(&between), format!("{code}\n\nThen:\n```sh\nls\n```"));
    // indented, trailed by spaces, under a dressed-up label
    for label in ["`luxe-remediation`:", "**luxe-remediation**", "Luxe-Remediation"] {
        let bare = format!("Done.\n{label}\n    {report}   \nAnything else?");
        assert_eq!(without_remediation_report(&bare), "Done.\n\nAnything else?", "{label}");
    }
}

/// JSON a reader asked for in conversation is theirs: only a review's report is hidden.
#[test]
fn json_that_is_not_a_report_stays_in_view() {
    for text in [
        "Here is the config:\n```json\n{\"findings\": [], \"name\": \"x\"}\n```",
        "Shape:\n```json\n{\"review_id\": \"my-review\", \"findings\": []}\n```",
        "Inline: {\"review_id\": 7, \"findings\": []} done",
        "{\"findings\": [1, 2]}",
        // shaped like a report but broken, and complete: the agent's words, shown as written
        "Report:\n{\"review_id\": \"review:x\", oops}\nMore.",
    ] {
        assert_eq!(without_remediation_report(text), text, "{text}");
    }
}
