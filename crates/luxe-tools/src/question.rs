// Interactive question tool.
//
// question — ONE question, two or three options with a marked recommendation, and free-text
// input. Resolves to the recommended option in unattended mode instead of blocking.

use async_trait::async_trait;
use luxe_core::event::Event;
use luxe_core::provider::ToolSchema;
use luxe_core::question::{QuestionOption, QuestionOutcome, QuestionReply};
use luxe_core::tool::{Tool, ToolContext, ToolResultData};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Hard ceiling on options. Decision time climbs steeply past a handful of choices, and a model
/// that lists seven has not made up its mind — the error says so and asks it to merge.
const MAX_OPTIONS: usize = 4;

/// Returns when the shared cancel flag flips to `true` (turn aborted), so a pending question
/// unblocks promptly instead of hanging the turn.
async fn wait_until_cancelled(cancel: &mut tokio::sync::watch::Receiver<bool>) {
    if *cancel.borrow() {
        return;
    }
    while cancel.changed().await.is_ok() {
        if *cancel.borrow() {
            return;
        }
    }
}

/// Parse the `options` argument, accepting object form or a bare label string per entry.
///
/// Unparseable entries are dropped rather than failing the call: a malformed option is a worse
/// reason to burn a round-trip than a slightly shorter list.
fn parse_options(args: &serde_json::Value) -> Vec<QuestionOption> {
    args.get("options")
        .and_then(|value| value.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| serde_json::from_value::<QuestionOption>(entry.clone()).ok())
                .filter(|option| !option.label.trim().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct QuestionConfig {
    /// Default resolution mode: "interactive" or "unattended".
    pub(crate) default_mode: String,
    /// Default answer for unattended mode.
    pub(crate) unattended_default: Option<String>,
}

impl Default for QuestionConfig {
    fn default() -> Self {
        Self { default_mode: "interactive".to_string(), unattended_default: None }
    }
}

pub(crate) struct QuestionTool {
    config: Arc<RwLock<QuestionConfig>>,
}

impl QuestionTool {
    pub(crate) fn new(config: QuestionConfig) -> Self {
        Self { config: Arc::new(RwLock::new(config)) }
    }
}

#[async_trait]
impl Tool for QuestionTool {
    fn name(&self) -> &str {
        "question"
    }

    /// No deadline: this tool waits on a person. A prompt that dismissed itself while the user was
    /// reading it would be a worse failure than any hang, and the wait is already bounded by the two
    /// things that should bound it — the user answering, or the user cancelling the turn.
    fn argument_aliases(&self) -> &'static [&'static str] {
        &["question"]
    }

    fn timeout(&self) -> Option<std::time::Duration> {
        None
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "question".to_string(),
            description: "Ask the user ONE question, with two or three distinct options and an \
                          always-available free-text answer. In the interactive TUI this opens a \
                          modal and blocks until the user answers; headless/loop runs resolve to \
                          `default` (or the recommended option) without blocking. Prefer this over \
                          asking in prose when you need a concrete decision.\n\n\
                          One decision per call — wait for the answer before asking the next \
                          thing; never stack several questions into one prompt. Give each option a \
                          short `label` plus a one-line `description` saying what it means or what \
                          it costs, and mark the one you would pick with `\"recommended\": true` \
                          (the modal opens on it). Two or three options is the target and four is \
                          the limit. Don't add an \"Other\" or \"Something else\" option — free \
                          text is always offered."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "prompt": {"type": "string", "description": "The single question to ask"},
                    "options": {
                        "type": "array",
                        "maxItems": MAX_OPTIONS,
                        "description": "Two or three distinct options (four maximum). A bare string is accepted but loses the rationale.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "label": {"type": "string", "description": "Short label the user picks (a few words)"},
                                "description": {"type": "string", "description": "One line on what this option means or costs"},
                                "recommended": {"type": "boolean", "description": "True on the one option you would pick; say why in its description"}
                            },
                            "required": ["label"]
                        }
                    },
                    "multi": {"type": "boolean", "description": "Allow multiple selections (only when several answers can be true at once)"},
                    "default": {"type": "string", "description": "Answer to use when nobody is there to ask"}
                },
                "required": ["prompt"]
            }),
        }
    }

    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        // `question` is what other harnesses call it, and a model trained on them reaches for it
        let prompt = match luxe_core::tool::arg(&args, &["prompt", "question"]) {
            Some(v) => v.as_str().unwrap_or(""),
            None => {
                return ToolResultData::error("Error: 'prompt' argument is required");
            }
        };

        let options = parse_options(&args);
        if options.len() > MAX_OPTIONS {
            return ToolResultData::error(format!(
                "Error: {} options is too many to choose between. Merge them into the two or \
                     three decisions that actually differ (at most {MAX_OPTIONS}), mark the one \
                     you recommend, and ask again — the free-text row covers the rest.",
                options.len()
            ));
        }

        let multi = args.get("multi").and_then(|v| v.as_bool()).unwrap_or(false);
        let default_ans = args.get("default").and_then(|v| v.as_str()).map(String::from);

        // Interactive path: a front-end can answer. Register with the broker, emit the prompt, and
        // block until the user answers or the turn is cancelled.
        if let Some(broker) = &cx.question_broker {
            let (id, rx) = broker.register();
            let _ = cx
                .event_tx
                .send(Event::QuestionAsked {
                    id: id.clone(),
                    session_id: cx.session_id.clone(),
                    prompt: prompt.to_string(),
                    options: options.clone(),
                    multi,
                })
                .await;

            let mut cancel = cx.cancelled.as_ref().clone();
            let reply = tokio::select! {
                reply = rx => reply.unwrap_or(QuestionReply::Cancelled),
                _ = wait_until_cancelled(&mut cancel) => {
                    broker.forget(&id);
                    QuestionReply::Cancelled
                }
            };

            let outcome = match reply {
                QuestionReply::Answered(answers) => QuestionOutcome::Answered(answers),
                QuestionReply::Cancelled => QuestionOutcome::Dismissed,
            };
            return ToolResultData::text(outcome.result_text());
        }

        // Unattended (headless/loop): resolve to a default without blocking. Prefer the call's
        // `default`, then the configured fallback, then the option the agent recommended (which is
        // a better unattended answer than whichever it happened to list first).
        let config = self.config.read().await;
        let answer = default_ans
            .or_else(|| config.unattended_default.clone())
            .or_else(|| {
                options
                    .get(luxe_core::question::default_selection(&options))
                    .map(|option| option.label.clone())
            })
            .unwrap_or_else(|| "No answer provided".to_string());
        let outcome = QuestionOutcome::Unattended { prompt: prompt.to_string(), answer };
        ToolResultData::text(outcome.result_text())
    }
}

#[cfg(test)]
mod tests;
