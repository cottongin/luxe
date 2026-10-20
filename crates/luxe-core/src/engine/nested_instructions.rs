//! Nested instructions, delivered — the engine side of [`crate::instructions::nested`].
//!
//! Runs on a finished batch, just before it commits, so a file rides on the result of the first
//! call that reached its folder and is durable history from then on. Every later request carries
//! it as part of the transcript it already appends to; the system prompt never moves.

use super::*;
use crate::instructions::nested::{self, NestedFile};
use crate::message::ToolOutput;
use std::collections::HashSet;
use std::path::PathBuf;

impl Engine {
    /// Appends each nested instruction file the batch reached, and the model doesn't have yet, to
    /// the result of the first call that reached it.
    ///
    /// Args: `(leaf, calls, results, event_tx)`. `leaf` is the reply the results answer, so the
    /// conversation read is the one these results join.
    pub(super) async fn attach_nested_instructions(
        &self,
        leaf: &EntryId,
        calls: &[ToolCallInfo],
        results: &mut [ContentBlock],
        event_tx: &mpsc::Sender<Event>,
    ) {
        if !nested::enabled() {
            return;
        }
        let wanted = self.nested_wanted(calls, results);
        if wanted.is_empty() {
            return;
        }
        let held = self.nested_on_path(leaf).await;
        for (call_id, file) in wanted.into_iter().filter(|(_, file)| !held.contains(&file.shown)) {
            attach_one(&call_id, &file, results, event_tx).await;
        }
    }

    /// The nested files this batch's successful calls reached, each once, with the call that
    /// reached it first.
    fn nested_wanted(
        &self,
        calls: &[ToolCallInfo],
        results: &[ContentBlock],
    ) -> Vec<(String, NestedFile)> {
        let launch = self.instruction_dir();
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut wanted = Vec::new();
        for call in calls.iter().filter(|call| succeeded(results, &call.id)) {
            for file in reached(&launch, call) {
                if seen.insert(file.path.clone()) {
                    wanted.push((call.id.clone(), file));
                }
            }
        }
        wanted
    }

    /// The nested files the conversation up to `leaf` already carries, by the notes that open
    /// them. Read rather than remembered, so one a rewind or a compaction took away goes again.
    async fn nested_on_path(&self, leaf: &EntryId) -> HashSet<String> {
        let sid = self.current_session_id().await;
        let read = crate::session::with_active_path(&**self.sessions(), &sid, leaf, |entries| {
            crate::compaction::context_messages(entries)
        });
        match read {
            Ok(messages) => notes_in(&messages),
            Err(error) => {
                // Sending a file twice costs tokens; not sending it costs the rules
                warn!(%error, "could not read which nested instructions the model has");
                HashSet::new()
            }
        }
    }
}

/// The nested files `call` reached, from luxe launched at `launch`.
///
/// Relative paths resolve the way the tool resolved them, against where tools run.
fn reached(launch: &std::path::Path, call: &ToolCallInfo) -> Vec<NestedFile> {
    let args = parse_tool_arguments(&call.arguments);
    let base = ToolContext::working_dir();
    let targets = nested::touched_paths(&args);
    targets
        .into_iter()
        .flat_map(|target| nested::nested_files(launch, &base.join(target)))
        .collect()
}

/// The output of the result answering `call_id`.
fn output_of<'a>(
    results: &'a mut [ContentBlock],
    call_id: &str,
) -> Option<&'a mut Vec<ToolOutput>> {
    results.iter_mut().find_map(|block| match block {
        ContentBlock::ToolResult { call_id: id, content, .. } if id == call_id => Some(content),
        _ => None,
    })
}

/// Whether the result answering `call_id` ran and succeeded.
fn succeeded(results: &[ContentBlock], call_id: &str) -> bool {
    results.iter().any(|block| {
        matches!(block, ContentBlock::ToolResult { call_id: id, is_error: false, .. } if id == call_id)
    })
}

/// Every nested file named by a note in a tool result of `messages`.
fn notes_in(messages: &[Message]) -> HashSet<String> {
    let outputs =
        messages.iter().flat_map(|message| &message.content).filter_map(|block| match block {
            ContentBlock::ToolResult { content, .. } => Some(content),
            _ => None,
        });
    outputs
        .flatten()
        .filter_map(|output| match output {
            ToolOutput::Text(text) => nested::noted(text).map(str::to_string),
            _ => None,
        })
        .collect()
}

/// Reads `file` and appends it to the result answering `call_id`, telling the person it did.
async fn attach_one(
    call_id: &str,
    file: &NestedFile,
    results: &mut [ContentBlock],
    event_tx: &mpsc::Sender<Event>,
) {
    let Some(injected) = super::prompt::read_context_file(&file.path) else {
        return;
    };
    let Some(content) = output_of(results, call_id) else {
        return;
    };
    let block = crate::redact::process_secrets().redact(&nested::note(&file.shown, &injected.text));
    content.push(ToolOutput::Text(block));
    debug!(file = %file.shown, "nested instructions joined the conversation");
    let _ = event_tx.send(Event::Notice(format!("Following {}", file.shown))).await;
    if let Some(notice) = injected.notice {
        let _ = event_tx.send(Event::Notice(notice)).await;
    }
}
