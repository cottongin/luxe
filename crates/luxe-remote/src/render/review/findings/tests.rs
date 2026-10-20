use super::super::{chapter, Place};
use super::{acts, state_words};
use crate::render::{escape, Renderer, StatusFacts, StatusSegment, Voice};
use luxe_core::guided_review::{
    FindingMessage, FindingMessageAuthor, FindingSeverity, FindingState, GuidedReview,
    ReviewFinding,
};

/// Prose marked so a test can tell it went through the markdown hook.
struct Marked;

impl Renderer for Marked {
    fn markdown(&self, text: &str, _voice: Voice) -> String {
        format!("<p>md:{}</p>", escape(text))
    }
    fn tool_glyph(&self, _tool: &str) -> String {
        String::new()
    }
    fn status(&self, _facts: &StatusFacts) -> Vec<StatusSegment> {
        Vec::new()
    }
    fn code_line(&self, text: &str, _language: Option<&str>) -> String {
        escape(text)
    }
}

/// A real review of a scratch project and the two comments made on it over RPC: a question on its
/// first added line (R1) and an issue on its chapter (R2), both unsent.
fn captured() -> (GuidedReview, Vec<ReviewFinding>) {
    #[derive(serde::Deserialize)]
    struct Scratch {
        review: GuidedReview,
        findings: Vec<ReviewFinding>,
    }
    let scratch: Scratch =
        serde_json::from_str(include_str!("scratch.json")).expect("the captured comments parse");
    (scratch.review, scratch.findings)
}

fn open(review: &GuidedReview, findings: &[ReviewFinding]) -> String {
    let place = Place { number: 1, total: 1, walked: false };
    chapter(&Marked, &review.chapters[0], &review.files, &place, None, findings)
}

/// A line comment sits under its hunk, and a chapter comment above the chapter's first hunk.
///
/// The break this catches: comments dropped from the chapter, or drawn away from what they're
/// about, so the reader can't tell which line was asked about.
#[test]
fn comments_sit_under_what_they_are_about() {
    let (review, findings) = captured();
    let html = open(&review, &findings);
    let hunk = html.find("<div class=\"hunk\">").expect("the hunk");
    let r1 = html.find("data-finding=\"finding:06d4fc78").expect("the line comment");
    let r2 = html.find(">R2<").expect("the chapter comment");
    assert!(r2 < hunk && hunk < r1, "{html}");
}

/// A card says what it is, where it is and where it stands, then the comment through the
/// markdown hook, with the edits an unsent one takes.
///
/// The break this catches: a card missing its label, kind, line or state, the comment shown raw,
/// or an unsent comment that can't be sent or deleted from the phone.
#[test]
fn an_unsent_card_says_what_and_where_and_offers_send() {
    let (review, findings) = captured();
    let html = open(&review, &findings[..1]);
    let head = "<span class=\"rv-label\">R1</span><span>question</span><span class=\"rv-state\">\
                unsent</span><span>line 1</span>";
    assert!(html.contains(head), "{html}");
    assert!(html.contains("<div class=\"rv-finding-body md\"><p>md:Why 4519?</p></div>"));
    assert!(html.contains("data-act=\"send\">Send</button>"));
    assert!(html.contains("data-act=\"delete\">Delete</button>"));
    assert!(html.contains("rv-finding rv-tone-warning"));
}

/// Comments on another hunk or chapter stay off this one.
///
/// The break this catches: every comment in the review piled onto whichever chapter is open.
#[test]
fn comments_elsewhere_stay_off_the_chapter() {
    let (review, mut findings) = captured();
    for finding in &mut findings {
        let mut anchor = serde_json::to_value(&finding.anchor).expect("an anchor");
        let field = if anchor.get("hunk_id").is_some() { "hunk_id" } else { "chapter_id" };
        anchor[field] = serde_json::json!(format!("{field}:elsewhere"));
        finding.anchor = serde_json::from_value(anchor).expect("an anchor elsewhere");
    }
    assert!(!open(&review, &findings).contains("rv-finding"));
}

/// A finding in `state`, its thread ending with a word from `last`.
fn finding(
    state: FindingState,
    severity: FindingSeverity,
    last: Option<FindingMessageAuthor>,
) -> ReviewFinding {
    let (_, mut findings) = captured();
    let mut finding = findings.remove(0);
    finding.state = state;
    finding.severity = severity;
    finding.thread = last.map(|author| said(author, "reply")).into_iter().collect();
    finding
}

fn said(author: FindingMessageAuthor, body: &str) -> FindingMessage {
    FindingMessage { author, body: body.into(), created_at_ms: 1 }
}

/// Where a sent comment stands, in the TUI's words.
///
/// The break this catches: the phone saying "waiting" when the agent has answered, or calling an
/// answer to a comment an answer to a question, which is what tells the reader to follow up.
#[test]
fn a_sent_comment_says_whether_the_agent_has_answered() {
    use FindingMessageAuthor::{Agent, System};
    use FindingSeverity::{Issue, Question};
    let words = |state, severity, last| state_words(&finding(state, severity, last)).0;
    assert_eq!(words(FindingState::Open, Question, Some(Agent)), "answered");
    assert_eq!(words(FindingState::Open, Issue, Some(Agent)), "the agent replied");
    assert_eq!(words(FindingState::Open, Issue, Some(System)), "no word from the agent");
    assert_eq!(words(FindingState::Open, Issue, None), "sent \u{00b7} waiting on the agent");
    assert_eq!(words(FindingState::NeedsVerification, Issue, None), "to verify");
    assert_eq!(words(FindingState::Resolved, Issue, None), "verified");
    assert_eq!(words(FindingState::Stale, Issue, None), "the code moved");
}

/// Each state offers only the edits the review desk takes in it.
///
/// The break this catches: Delete offered on a sent comment (refused), Reply on an unsent one, or
/// a button on a moved comment, whose re-anchoring is the TUI's.
#[test]
fn each_state_offers_the_edits_it_takes() {
    let names = |state| -> Vec<&str> {
        acts(&finding(state, FindingSeverity::Issue, None)).iter().map(|(act, _)| *act).collect()
    };
    assert_eq!(names(FindingState::Draft), ["send", "delete"]);
    assert_eq!(names(FindingState::Open), ["reply", "resolve"]);
    assert_eq!(names(FindingState::NeedsVerification), ["reply", "resolve"]);
    assert_eq!(names(FindingState::Resolved), ["reopen"]);
    assert!(names(FindingState::Stale).is_empty());
}

/// A comment posted to GitHub is GitHub's now: it says so and offers nothing.
///
/// The break this catches: Send offered on a comment already posted, which would post it twice.
#[test]
fn a_posted_comment_is_on_github() {
    let mut posted = finding(FindingState::Draft, FindingSeverity::Issue, None);
    posted.posted = serde_json::from_value(serde_json::json!({"review_id": 7, "url": "https://x"}))
        .expect("a posted comment");
    assert_eq!(state_words(&posted).0, "on GitHub");
    assert!(acts(&posted).is_empty());
}

/// The thread follows the comment, each word by who said it, the reader's opening not repeated.
///
/// The break this catches: the agent's answer missing from the card (the phone has no thread
/// window), told as the reader's, or the comment said twice.
#[test]
fn the_thread_follows_the_comment_by_who_said_it() {
    let (review, mut findings) = captured();
    let body = findings[0].body.clone();
    findings[0].state = FindingState::Open;
    findings[0].thread = vec![
        said(FindingMessageAuthor::Reviewer, &body),
        said(FindingMessageAuthor::Agent, "It is the bridge port"),
        said(FindingMessageAuthor::Reviewer, "Say so in a comment"),
    ];
    let html = open(&review, &findings[..1]);
    assert_eq!(html.matches("md:Why 4519?").count(), 1, "{html}");
    let word = |who: &str, words: &str| {
        html.find(&format!("<p class=\"rv-who\">{who}</p><div class=\"md\"><p>md:{words}"))
    };
    let (agent, you) = (word("agent", "It is the bridge port"), word("you", "Say so in a comment"));
    assert!(agent.zip(you).is_some_and(|(agent, you)| agent < you), "{html}");
}

/// A comment on a removed line says so: its number is the old file's.
///
/// The break this catches: a removed line's comment saying "line 12" of a file whose line 12 is
/// something else now.
#[test]
fn a_comment_on_a_removed_line_says_removed() {
    let (review, mut findings) = captured();
    let mut anchor = serde_json::to_value(&findings[0].anchor).expect("an anchor");
    anchor["side"] = serde_json::json!("old");
    findings[0].anchor = serde_json::from_value(anchor).expect("an old-side anchor");
    assert!(open(&review, &findings[..1]).contains("<span>removed line 1</span>"));
}
