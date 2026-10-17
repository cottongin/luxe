//! Interactive question broker — bridges a blocked `question` tool call to the front-end's answer.
//!
//! Mirrors the permission broker (`crate::permission::broker`): when the `question` tool runs in an
//! interactive session it registers a pending request here (getting a one-shot receiver) and emits
//! `Event::QuestionAsked`. The engine's action loop, on the matching `Action::AnswerQuestion`, calls
//! [`QuestionBroker::resolve`] to hand the answer back through the one-shot, unblocking the tool.
//! Cancelling a turn drops every pending sender via [`QuestionBroker::cancel_all`]. Headless/loop
//! runs never attach a broker, so the tool resolves to its `default` instead of blocking.
//!
//! However it ends, the call reports itself through [`QuestionOutcome`], which owns both the text
//! the model reads and the parse a front-end rebuilds the decision from — the modal is gone by the
//! time anyone scrolls back, so its card is the only record of what was chosen.

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Deserializer, Serialize};
use tokio::sync::oneshot;
use uuid::Uuid;

/// One answer the user can pick, with the reasoning that makes it pickable.
///
/// A bare label forces the model to choose between an unexplained option and a label so long the
/// panel clips it, which is why the rationale is its own field: the front-end can render it dim
/// and wrapped beneath the label, and `recommended` can carry a badge instead of `(Recommended)`
/// smuggled into the text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuestionOption {
    /// Short label the user picks (a few words)
    pub label: String,
    /// One line on what the option means or what it costs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether the agent recommends this option
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub recommended: bool,
}

impl QuestionOption {
    /// An option with no rationale attached.
    pub fn new(label: impl Into<String>) -> Self {
        Self { label: label.into(), description: None, recommended: false }
    }

    /// Attach the one-line rationale.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        let description = description.into();
        self.description = (!description.trim().is_empty()).then_some(description);
        self
    }

    /// Mark this as the option the agent would choose.
    pub fn recommended(mut self) -> Self {
        self.recommended = true;
        self
    }
}

impl From<&str> for QuestionOption {
    fn from(label: &str) -> Self {
        Self::new(label)
    }
}

/// Accepts either the full object or a bare label string, so a recorded event from an older build
/// — or a model that ignores the schema — still deserializes.
impl<'de> Deserialize<'de> for QuestionOption {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Label(String),
            Full {
                label: String,
                #[serde(default)]
                description: Option<String>,
                #[serde(default)]
                recommended: bool,
            },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Label(label) => Self::new(label),
            Wire::Full { label, description, recommended } => Self {
                label,
                description: description.filter(|text| !text.trim().is_empty()),
                recommended,
            },
        })
    }
}

/// The index the cursor should open on: the recommended option, else the first.
///
/// Opening on the recommendation is the whole payoff of marking one — `Enter` then does the
/// sensible thing without the user having to weigh every row first.
pub fn default_selection(options: &[QuestionOption]) -> usize {
    options.iter().position(|option| option.recommended).unwrap_or(0)
}

/// Correlates an emitted `QuestionAsked` event with the `AnswerQuestion` action that answers it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QuestionRequestId(String);

impl QuestionRequestId {
    /// Mint a fresh, unique request id.
    pub fn new() -> Self {
        Self(format!("ask:{}", Uuid::new_v4()))
    }

    /// The stable serialized identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for QuestionRequestId {
    fn default() -> Self {
        Self::new()
    }
}

/// The user's answer to an interactive question.
///
/// Tagged `snake_case` like the rest of the RPC vocabulary it now travels in. Nothing persists a
/// reply — it lives only between the broker and the waiting tool call — so the wire form is free
/// to match its neighbors rather than serde's default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionReply {
    /// One or more chosen answers (option labels and/or a free-text entry). Single-select carries
    /// exactly one; multi-select may carry several; an empty list means "answered with nothing".
    Answered(Vec<String>),
    /// The user dismissed the prompt without answering (Esc); the tool reports this to the model.
    Cancelled,
}

/// How a finished `question` call reports itself: the text the model reads, and the decision a
/// front-end reads back out of that same text.
///
/// Both directions live together because the card in the transcript is the only record of the
/// answer once the modal closes, and a card that rebuilt it by pattern-matching prose written in
/// another crate would go quietly blank the first time a sentence was reworded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionOutcome {
    /// One or more chosen answers: option labels, the user's own text, or both.
    Answered(Vec<String>),
    /// Submitted without choosing anything.
    Empty,
    /// Dismissed without answering.
    Dismissed,
    /// Nobody was there to ask, so the call resolved to a default.
    Unattended {
        /// The question that was never put to anyone.
        prompt: String,
        /// What the call resolved to.
        answer: String,
    },
}

// The four endings, written once. A bare `ANSWERED` line introduces the list under it.
const ANSWERED: &str = "The user answered:";
const EMPTY: &str = "The user submitted the question with no selection.";
const DISMISSED: &str = "The user dismissed the question without answering.";
const UNATTENDED: &str = "[Auto-answered, no interactive session]";

impl QuestionOutcome {
    /// The answers this outcome carries, in the order they were given.
    ///
    /// Empty for a dismissal, so a card can render "what was chosen" without asking which ending
    /// it is looking at first.
    pub fn answers(&self) -> Vec<&str> {
        match self {
            Self::Answered(answers) => answers.iter().map(String::as_str).collect(),
            Self::Unattended { answer, .. } => vec![answer.as_str()],
            Self::Empty | Self::Dismissed => Vec::new(),
        }
    }

    /// The result text the model sees.
    ///
    /// Several answers go on their own lines rather than into one comma-joined sentence: a free
    /// text answer may contain a comma, and a list nobody can split back apart is a list the card
    /// has to guess at.
    pub fn result_text(&self) -> String {
        match self {
            Self::Answered(answers) => match answers.as_slice() {
                [] => EMPTY.to_string(),
                [only] => format!("{ANSWERED} {only}"),
                many => {
                    let listed =
                        many.iter().map(|answer| format!("- {answer}")).collect::<Vec<_>>();
                    format!("{ANSWERED}\n{}", listed.join("\n"))
                }
            },
            Self::Empty => EMPTY.to_string(),
            Self::Dismissed => DISMISSED.to_string(),
            Self::Unattended { prompt, answer } => {
                format!("{UNATTENDED} {prompt}\nAnswer: {answer}")
            }
        }
    }

    /// The outcome a result text carries, or `None` when the text is not one of ours.
    ///
    /// `None` rather than a guess: a card that renders nothing is a card that has lost a row, and a
    /// card that invents an answer nobody gave is worse than either.
    pub fn parse(result: &str) -> Option<Self> {
        let mut lines = result.lines();
        let first = lines.next()?.trim_end();
        if let Some(rest) = first.strip_prefix(ANSWERED) {
            let rest = rest.trim();
            if !rest.is_empty() {
                return Some(Self::Answered(vec![rest.to_string()]));
            }
            let listed: Vec<String> = lines
                .filter_map(|line| line.trim().strip_prefix("- ").map(str::to_string))
                .filter(|answer| !answer.is_empty())
                .collect();
            return Some(if listed.is_empty() { Self::Empty } else { Self::Answered(listed) });
        }
        if let Some(prompt) = first.strip_prefix(UNATTENDED) {
            let answer = lines.find_map(|line| line.trim().strip_prefix("Answer:"))?;
            return Some(Self::Unattended {
                prompt: prompt.trim().to_string(),
                answer: answer.trim().to_string(),
            });
        }
        match first {
            EMPTY => Some(Self::Empty),
            DISMISSED => Some(Self::Dismissed),
            _ => None,
        }
    }
}

/// A registry of in-flight question prompts awaiting a user reply.
#[derive(Debug, Default)]
pub struct QuestionBroker {
    pending: Mutex<HashMap<QuestionRequestId, oneshot::Sender<QuestionReply>>>,
}

impl QuestionBroker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new pending prompt, returning its id and the receiver the tool awaits.
    pub fn register(&self) -> (QuestionRequestId, oneshot::Receiver<QuestionReply>) {
        let id = QuestionRequestId::new();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("question broker lock").insert(id.clone(), tx);
        (id, rx)
    }

    /// Deliver a reply to the matching pending prompt (no-op if it's gone/stale).
    pub fn resolve(&self, id: &QuestionRequestId, reply: QuestionReply) {
        if let Some(tx) = self.pending.lock().expect("question broker lock").remove(id) {
            let _ = tx.send(reply);
        }
    }

    /// Forget a pending prompt without answering (the awaiting tool resolves via its own fallback).
    pub fn forget(&self, id: &QuestionRequestId) {
        self.pending.lock().expect("question broker lock").remove(id);
    }

    /// Drop every pending sender (turn cancelled/closed) so awaiting tools unblock.
    pub fn cancel_all(&self) {
        self.pending.lock().expect("question broker lock").clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolve_delivers_the_reply() {
        let broker = QuestionBroker::new();
        let (id, rx) = broker.register();
        broker.resolve(&id, QuestionReply::Answered(vec!["Yes".into()]));
        assert_eq!(rx.await.unwrap(), QuestionReply::Answered(vec!["Yes".into()]));
    }

    #[tokio::test]
    async fn cancel_all_drops_pending_and_unblocks_receiver() {
        let broker = QuestionBroker::new();
        let (_id, rx) = broker.register();
        broker.cancel_all();
        assert!(rx.await.is_err(), "a dropped sender surfaces as a receive error");
    }

    #[tokio::test]
    async fn resolving_a_stale_id_is_a_noop() {
        let broker = QuestionBroker::new();
        broker.resolve(&QuestionRequestId::new(), QuestionReply::Cancelled);
    }

    #[test]
    fn an_option_deserializes_from_an_object_or_a_bare_label() {
        let full: QuestionOption = serde_json::from_str(
            r#"{"label":"tokio","description":"already a dependency","recommended":true}"#,
        )
        .expect("object form");
        assert_eq!(full.label, "tokio");
        assert_eq!(full.description.as_deref(), Some("already a dependency"));
        assert!(full.recommended);

        let bare: QuestionOption = serde_json::from_str(r#""tokio""#).expect("bare label");
        assert_eq!(bare, QuestionOption::new("tokio"));
        assert!(bare.description.is_none() && !bare.recommended);

        // A blank rationale is the same as none, so the front-end never renders an empty row.
        let blank: QuestionOption =
            serde_json::from_str(r#"{"label":"a","description":"   "}"#).expect("blank");
        assert!(blank.description.is_none());
    }

    #[test]
    fn a_bare_option_round_trips_without_inventing_fields() {
        let json = serde_json::to_string(&QuestionOption::new("tokio")).expect("serialize");
        assert_eq!(json, r#"{"label":"tokio"}"#);
    }

    /// The card in the transcript is the only record of a decision once the modal closes, so the
    /// text the model reads has to be the text a front-end can read back.
    #[test]
    fn every_ending_survives_the_round_trip() {
        let endings = [
            QuestionOutcome::Answered(vec!["tokio".into()]),
            QuestionOutcome::Answered(vec!["tokio".into(), "rustls, but only for now".into()]),
            QuestionOutcome::Empty,
            QuestionOutcome::Dismissed,
            QuestionOutcome::Unattended { prompt: "Which runtime?".into(), answer: "tokio".into() },
        ];
        for ending in endings {
            let text = ending.result_text();
            assert_eq!(QuestionOutcome::parse(&text), Some(ending.clone()), "{text:?}");
        }
    }

    /// Several answers go on their own lines, because a free-text answer may contain the comma a
    /// joined list would be split on.
    #[test]
    fn a_multi_answer_result_stays_splittable() {
        let answered = QuestionOutcome::Answered(vec!["Dark mode".into(), "also, vim keys".into()]);
        assert_eq!(answered.result_text(), "The user answered:\n- Dark mode\n- also, vim keys");
        assert_eq!(answered.answers(), vec!["Dark mode", "also, vim keys"]);
        assert!(QuestionOutcome::Dismissed.answers().is_empty());
    }

    /// Anything else is left alone rather than guessed at: an invented answer is worse than a card
    /// that stays quiet.
    #[test]
    fn a_result_that_is_not_ours_parses_to_nothing() {
        for text in ["", "Error: 'prompt' argument is required", "The user answered"] {
            assert_eq!(QuestionOutcome::parse(text), None, "{text:?}");
        }
        // An answered lead with nothing under it is an empty submission, not a lost answer.
        assert_eq!(QuestionOutcome::parse("The user answered:"), Some(QuestionOutcome::Empty));
        // And an empty list of answers reports itself the same way.
        assert_eq!(QuestionOutcome::Answered(Vec::new()).result_text(), EMPTY);
    }

    #[test]
    fn the_cursor_opens_on_the_recommendation() {
        let options = vec![
            QuestionOption::new("async-std"),
            QuestionOption::new("tokio").recommended(),
            QuestionOption::new("smol"),
        ];
        assert_eq!(default_selection(&options), 1);
        assert_eq!(default_selection(&[QuestionOption::new("only")]), 0, "no marker: first row");
        assert_eq!(default_selection(&[]), 0, "free-text-only question");
    }
}
