//! `activate_tools` — load deferred tool schemas on demand (lazy-tools mode).
//!
//! When the registry is tiered ([`luxe_core::tool::ToolRegistry::set_tiers`]), only a curated
//! core set ships in every request; situational tools are listed in the prompt as available and
//! their full schemas are withheld until the model activates them here. This shrinks the request
//! floor (tool schemas dominate it) and keeps tool-selection accuracy high (which degrades past
//! ~30–50 tools).
//!
//! The tool holds no state. A successful call *is* the activation: the engine reads it back off
//! the transcript ([`luxe_core::tool_activation`]) to decide what the next request loads, and an
//! adapter that loads tools in place finds it there too.

use std::collections::BTreeSet;

use async_trait::async_trait;
use luxe_core::provider::ToolSchema;
use luxe_core::tool::{Tool, ToolContext, ToolResultData, ACTIVATE_TOOLS_NAME};
use serde_json::json;

/// The `activate_tools` meta-tool, which checks requested names against the deferred set.
pub struct ActivateToolsTool {
    deferred: BTreeSet<String>,
}

impl ActivateToolsTool {
    /// `deferred` is the set of names it may activate.
    pub fn new(deferred: impl IntoIterator<Item = String>) -> Self {
        Self { deferred: deferred.into_iter().collect() }
    }

    fn available(&self) -> String {
        self.deferred.iter().map(String::as_str).collect::<Vec<_>>().join(", ")
    }
}

#[async_trait]
impl Tool for ActivateToolsTool {
    fn name(&self) -> &str {
        ACTIVATE_TOOLS_NAME
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: ACTIVATE_TOOLS_NAME.to_string(),
            // The names live in the system prompt's catalog, which is always there when this tool
            // is; listing them here too paid for them twice on every request
            description: "Load tools that aren't sent up front. Call this with names from the \
                          \"More tools\" list in your instructions, and their full schemas become \
                          available to call on your next step."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "names": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Tool names to activate, chosen from the available list."
                    }
                },
                "required": ["names"]
            }),
        }
    }

    async fn run(&self, args: serde_json::Value, _cx: &ToolContext) -> ToolResultData {
        if args.get("names").and_then(|v| v.as_array()).is_none() {
            return ToolResultData::error(
                "Error: 'names' must be an array of tool names to activate.".to_string(),
            );
        }
        // The same reading the engine applies to the transcript, so the two can't disagree
        let requested = luxe_core::tool_activation::requested_names(&args);
        if requested.is_empty() {
            return ToolResultData::error(format!(
                "Error: 'names' was empty. Available to activate: {}.",
                self.available()
            ));
        }

        let (mut activated, mut unknown): (Vec<String>, Vec<String>) =
            requested.into_iter().partition(|name| self.deferred.contains(name));
        activated.sort();
        unknown.sort();

        let mut message = String::new();
        if !activated.is_empty() {
            message.push_str(&format!(
                "Activated: {}. They are now available to call directly on your next step.",
                activated.join(", ")
            ));
        }
        if !unknown.is_empty() {
            if !message.is_empty() {
                message.push('\n');
            }
            message.push_str(&format!(
                "Not activatable (already available or unknown): {}. Available to activate: {}.",
                unknown.join(", "),
                self.available()
            ));
        }
        // A request that activated nothing valid is an error so the model corrects course, and so
        // the transcript records that nothing was loaded
        if activated.is_empty() {
            ToolResultData::error(message)
        } else {
            ToolResultData::text(message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> ActivateToolsTool {
        ActivateToolsTool::new(["debug".to_string(), "ast_grep".to_string()])
    }

    async fn run(args: serde_json::Value) -> (String, bool) {
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(1);
        let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let cx = ToolContext {
            cwd: std::env::temp_dir(),
            session_id: "s".into(),
            cancelled: std::sync::Arc::new(cancel_rx),
            event_tx,
            hook_ctx: None,
            question_broker: None,
        };
        let result = tool().run(args, &cx).await;
        let text = result
            .content
            .iter()
            .filter_map(|block| match block {
                luxe_core::ContentBlock::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        (text, result.is_error)
    }

    /// The catalog in the system prompt names every deferred tool; the schema shouldn't repeat it.
    #[test]
    fn the_schema_does_not_repeat_the_catalog() {
        let description = tool().schema().description;
        assert!(
            !description.contains("ast_grep") && !description.contains("debug"),
            "{description}"
        );
        assert!(description.contains("More tools"), "it says where the names are: {description}");
    }

    #[tokio::test]
    async fn a_deferred_tool_activates() {
        let (text, is_error) = run(json!({ "names": ["debug"] })).await;
        assert!(!is_error);
        assert!(text.starts_with("Activated: debug."), "{text}");
    }

    /// A partly wrong request still loads what it can and names what it couldn't, and it's not an
    /// error: the engine reads a successful call as an activation of the names it asked for.
    #[tokio::test]
    async fn a_mixed_request_loads_what_it_can() {
        let (text, is_error) = run(json!({ "names": ["read", "debug", "ast_grep"] })).await;
        assert!(!is_error);
        assert!(text.contains("Activated: ast_grep, debug."), "{text}");
        assert!(text.contains("Not activatable (already available or unknown): read."), "{text}");
    }

    /// Nothing valid is an error, which is also what keeps the transcript from recording a load.
    #[tokio::test]
    async fn nothing_valid_is_an_error() {
        let (text, is_error) = run(json!({ "names": ["read"] })).await;
        assert!(is_error, "{text}");
        let (text, is_error) = run(json!({ "names": [] })).await;
        assert!(is_error && text.contains("was empty"), "{text}");
        let (text, is_error) = run(json!({ "names": "debug" })).await;
        assert!(is_error && text.contains("must be an array"), "{text}");
    }
}
