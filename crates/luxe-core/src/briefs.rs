//! Briefs — the messages Luxe writes to the agent itself, told apart from the reader's own.
//!
//! A draft, a question from a review and a batch of review comments each reach the agent as a
//! user message Luxe composed: the instructions, the evidence, the IDs a report has to echo. The
//! transcript is the reader's conversation, and a page of plumbing in their own colors reads as
//! something they said. So a front-end shows one line saying what happened instead — or, for a
//! question from a review, just the question, since those words *are* the reader's — and the
//! messages themselves stay in the session exactly as sent. Each opener here is what its builder
//! writes first, and the builders' tests pin that [`label`] knows it.

/// How a reply draft's brief opens.
const DRAFT_REPLY: &str = "Draft a reply to ";

/// How a review summary draft's brief opens.
const DRAFT_REVIEW_SUMMARY: &str = "Draft the summary of my review";

/// How review feedback opens, and the line that says it only asks.
const REVIEW_FEEDBACK: &str = "Guided review feedback\n";
const ASKS_ONLY: &str = "\nMode: Question\n";

/// How the one message asking for a missing remediation report opens.
pub(crate) const REPORT_REQUEST: &str = "Guided review report\n";

/// How context a hook added opens, on the user message it rides with.
pub(crate) const HOOK_CONTEXT: &str = "[hook] ";

/// How the note that the checkout moved to another branch opens.
pub(crate) const BRANCH_NOTE: &str = "[branch: ";

/// How the one-time briefing a resumed session's first message carries opens.
pub(crate) const RESUME_BRIEFING: &str = "## Resuming\n";

/// A user message's text blocks the reader wrote, by block index: not the mode notes, hook context,
/// branch notes and resume briefing that ride in it for the model, which a transcript would
/// otherwise show as their words.
///
/// Those are only ever added after what the reader sent, so the first block is theirs whatever it
/// starts with: a pasted hook log is still a message.
pub fn readers_words(content: &[crate::message::ContentBlock]) -> Vec<(usize, String)> {
    let for_the_model = |text: &str| {
        let ours = [HOOK_CONTEXT, BRANCH_NOTE, RESUME_BRIEFING];
        crate::mode::note(text).is_some() || ours.iter().any(|opener| text.starts_with(opener))
    };
    let text = content.iter().enumerate().filter_map(|(index, block)| match block {
        crate::message::ContentBlock::Text(text) => Some((index, text)),
        _ => None,
    });
    text.filter(|(index, text)| *index == 0 || !for_the_model(text))
        .map(|(index, text)| (index, text.clone()))
        .collect()
}

/// What a brief stands for, as one line of the transcript; `None` for anything the reader wrote.
pub fn label(text: &str) -> Option<&'static str> {
    if text.starts_with(DRAFT_REPLY) {
        Some("Drafted a reply")
    } else if text.starts_with(DRAFT_REVIEW_SUMMARY) {
        Some("Drafted the review summary")
    } else if text.starts_with(REPORT_REQUEST) {
        Some("Asked the agent for its report")
    } else if text.starts_with(REVIEW_FEEDBACK) {
        Some(match text.contains(ASKS_ONLY) {
            true => "Asked the agent about the review",
            false => "Sent your review comments to the agent",
        })
    } else {
        None
    }
}

/// A question asked from a review: where, and in the reader's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewQuestion {
    /// Where it was asked, `path:line (side) · symbol` or `entire review`
    pub location: String,
    /// What the reader asked; on a follow-up, the newest of it
    pub question: String,
}

/// How a submitted comment's entry opens, on the line after its `- Severity · title` header.
const FINDING_ID: &str = "  Finding ID: ";
const EVIDENCE: &str = "  Evidence: ";
const REVIEWER: &str = "  Reviewer: ";
/// The lines that follow a comment's body; none of them is the reader's question.
const AFTER_BODY: [&str; 4] = ["  Source: ", "  Proposed fix: ", "  Agent: ", "  Luxe: "];
/// How review feedback closes.
const CLOSING: &str = "Do not rewrite unrelated code.";

/// The questions a review's question brief asked, in order; empty for any other text.
///
/// Read back out of the brief rather than carried beside it, so a resumed session shows them
/// exactly as the live one did. A comment's entry is found by its header and the `Finding ID` line
/// under it, and its words run until the next field, so a question that spans lines or holds a
/// list of its own comes back whole.
pub fn review_questions(text: &str) -> Vec<ReviewQuestion> {
    if !text.starts_with(REVIEW_FEEDBACK) || !text.contains(ASKS_ONLY) {
        return Vec::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    let opens_entry = |at: usize| {
        lines[at].starts_with("- ")
            && lines.get(at + 1).is_some_and(|next| next.starts_with(FINDING_ID))
    };
    enum Reading {
        Body,
        Reviewer,
        Other,
    }
    let mut asked = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        if !opens_entry(at) {
            at += 1;
            continue;
        }
        at += 2;
        let location = match lines.get(at).and_then(|line| line.strip_prefix(EVIDENCE)) {
            Some(evidence) => {
                at += 1;
                evidence.to_owned()
            }
            None => "the review".to_owned(),
        };
        let (mut body, mut follow_up) = (Vec::new(), None::<Vec<&str>>);
        let mut reading = Reading::Body;
        while at < lines.len() && !opens_entry(at) && lines[at] != CLOSING {
            let line = lines[at];
            if let Some(said) = line.strip_prefix(REVIEWER) {
                follow_up = Some(vec![said]);
                reading = Reading::Reviewer;
            } else if AFTER_BODY.iter().any(|field| line.starts_with(field)) {
                reading = Reading::Other;
            } else {
                match (&reading, follow_up.as_mut()) {
                    (Reading::Body, _) => body.push(line.strip_prefix("  ").unwrap_or(line)),
                    (Reading::Reviewer, Some(said)) => said.push(line),
                    _ => {}
                }
            }
            at += 1;
        }
        let question = follow_up.unwrap_or(body).join("\n").trim().to_owned();
        if !question.is_empty() {
            asked.push(ReviewQuestion { location, question });
        }
    }
    asked
}

/// How a draft's fence opens, its newline included.
const DRAFT_FENCE: &str = "```luxe-draft\n";

/// An answer as the reader sees it: without what the agent wrote for Luxe to read.
///
/// A fix's remediation report goes, since the review shows it comment by comment, and the prose
/// around it stays. An answer carrying a draft goes whole: the draft opens in the card or composer
/// that asked for it, and anything the agent said beside it travels there as its note
/// ([`draft_note`]), where it is read with the draft rather than in the conversation behind it.
pub fn for_the_reader(assistant_text: &str) -> std::borrow::Cow<'_, str> {
    let text = crate::guided_review::without_remediation_report(assistant_text);
    match crate::guided_review::remediation_without_fence(&text, DRAFT_FENCE) {
        std::borrow::Cow::Borrowed(_) => text,
        std::borrow::Cow::Owned(_) => std::borrow::Cow::Borrowed(""),
    }
}

/// Longest note a draft carries, in characters.
const MAX_NOTE_CHARS: usize = 1_000;

/// What a drafting answer said outside its draft — a caveat, usually — or `None` when it said
/// nothing else.
pub fn draft_note(answer: &str) -> Option<String> {
    let rest = crate::guided_review::remediation_without_fence(answer, DRAFT_FENCE);
    let note = rest.trim();
    (!note.is_empty() && rest.len() < answer.len())
        .then(|| note.chars().take(MAX_NOTE_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use super::{draft_note, for_the_reader, label, review_questions, ReviewQuestion};

    /// A report goes and the prose around it stays; an answer with a draft in it goes whole,
    /// whole or still arriving, and what it said beside the draft becomes the draft's note.
    #[test]
    fn an_answer_loses_what_it_wrote_for_luxe() {
        let draft = "Here is the reply.\n\n```luxe-draft\nFixed in 4c1e2a0.\n```";
        assert_eq!(for_the_reader(draft), "");
        assert_eq!(draft_note(draft).as_deref(), Some("Here is the reply."));
        assert_eq!(for_the_reader("```luxe-draft\nFixed in"), "");
        let both = "Done.\n```luxe-remediation\n{}\n```\n```luxe-draft\nx\n```";
        assert_eq!(for_the_reader(both), "");
        let report = "Done.\n```luxe-remediation\n{}\n```";
        assert_eq!(for_the_reader(report), "Done.");
        assert_eq!(for_the_reader("Plain words."), "Plain words.");
    }

    /// A draft with nothing beside it has no note, and an answer with no draft has none either.
    #[test]
    fn a_note_is_only_what_was_said_beside_a_draft() {
        let caveat = "```luxe-draft\nApproving.\n```\n\nI haven't read the diff myself.";
        assert_eq!(draft_note(caveat).as_deref(), Some("I haven't read the diff myself."));
        assert_eq!(draft_note("```luxe-draft\nApproving.\n```\n"), None);
        assert_eq!(draft_note("No draft here."), None);
    }

    /// Each brief has its line, a question is told from fixes, and the reader's own words —
    /// even ones that mention a draft — have none.
    #[test]
    fn briefs_have_a_line_and_the_readers_words_do_not() {
        assert_eq!(label("Draft a reply to a comment by @jon"), Some("Drafted a reply"));
        assert_eq!(
            label("Draft the summary of my review of pull request #7"),
            Some("Drafted the review summary")
        );
        let feedback = "Guided review feedback\nSubmission: s\nReview: r\nSource revision: v\n";
        assert_eq!(
            label(&format!("{feedback}Mode: RequestFixes\n")),
            Some("Sent your review comments to the agent")
        );
        assert_eq!(
            label(&format!("{feedback}Mode: Question\n")),
            Some("Asked the agent about the review")
        );
        assert_eq!(label("Can you draft a reply to jon?"), None);
        assert_eq!(label("Fix the parser."), None);
    }

    /// A question brief gives back each question where it was asked and in the reader's words —
    /// a follow-up's newest words, a question over several lines whole — and nothing else does.
    /// The brief is a literal copied from a real session, so a change to how feedback is written
    /// that this cannot read fails here rather than in the transcript.
    #[test]
    fn a_question_brief_gives_back_what_was_asked() {
        let brief = "Guided review feedback\nSubmission: submission:1\nReview: review:2\n\
            Source revision: revision:3\nMode: Question\n\n\
            Answer each question directly and concisely, citing the code.\n\n\
            Severity meanings:\n- Question: answer it; make no edit for it\n\n\
            - Question · The comment here indicates that the list of tables here isn't the source of tru…\n\
            \x20 Finding ID: finding:2edb0305-609d-425e-ac34-6c823e794ba5\n\
            \x20 Evidence: scripts/verify-sandbox-guids.ps1 · $Url = $Url.TrimEnd(\"/\")\n\
            \x20 The comment here indicates that the list of tables here isn't the source of \
            truth. Is this potentially unnecessary?\n\
            - Question · Why two passes?\n\
            \x20 Finding ID: finding:2\n\
            \x20 Evidence: src/a.rs:3 (new)\n\
            \x20 Why two passes?\n\
            - Couldn't one do?\n\
            \x20 Agent: Pass 2 needs the lookup.\n\
            - Rename it\n\
            \x20 Reviewer: Then is the name wrong?\n\
            It reads like the plural.\n\
            Do not rewrite unrelated code.";
        assert_eq!(
            review_questions(brief),
            vec![
                ReviewQuestion {
                    location: "scripts/verify-sandbox-guids.ps1 · $Url = $Url.TrimEnd(\"/\")"
                        .into(),
                    question: "The comment here indicates that the list of tables here isn't the \
                        source of truth. Is this potentially unnecessary?"
                        .into(),
                },
                ReviewQuestion {
                    location: "src/a.rs:3 (new)".into(),
                    question: "Then is the name wrong?\nIt reads like the plural.".into(),
                },
            ]
        );
        let fixes = brief.replace("Mode: Question", "Mode: RequestFixes");
        assert!(review_questions(&fixes).is_empty());
        assert!(review_questions("Why two passes?").is_empty());
    }

    /// What rides along for the model is hidden, and what the reader sent first never is.
    ///
    /// The break this catches: a message that itself began with `[hook] ` — a pasted hook log —
    /// vanished from a resumed transcript, mistaken for context a hook had added.
    #[test]
    fn only_what_rides_after_the_reader_s_words_is_hidden() {
        use crate::message::ContentBlock::Text;
        let content = [
            Text("[hook] failed: exit 2".into()),
            Text("[hook] branch main".into()),
            Text("[mode: plan] Still in plan.".into()),
            Text("[branch: fix] The checkout is now on branch `fix`.".into()),
            Text("## Resuming\n\nYou are picking up an existing session.".into()),
        ];
        assert_eq!(super::readers_words(&content), [(0, "[hook] failed: exit 2".to_string())]);
    }
}
