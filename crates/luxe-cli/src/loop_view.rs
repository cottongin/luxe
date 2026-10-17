//! What a running loop looks like from outside.
//!
//! A run nobody can watch is a run nobody can judge: you cannot tell a model thinking hard from a
//! provider that stopped answering, and the first question about an unattended loop is always "is
//! it still going?".
//!
//! This is that view, and deliberately no more: one bounded line per thing that happened, printed
//! as it happens. It reads the same `Event` stream every front-end reads, so it cannot disagree
//! with the transcript, and it is not a second one — the conversation is in
//! `.luxe/sessions/`, the durable account is in `.luxe/logs/luxe.log`, and the run's numbers are in
//! `luxe loop status`. What belongs here is only what a person watching needs: where the loop is,
//! what it is doing now, and what the gate said.
//!
//! Timed from the run's own start rather than the wall clock. "Twenty minutes in and still on
//! phase 0" is the shape of the question being asked, and the absolute time is in the log for
//! anything forensic.

use crate::output::outln;
use std::collections::HashMap;
use std::time::Instant;

use luxe_core::event::Event;
use luxe_core::message::{ContentBlock, Role};

/// Longest assistant line echoed into the view.
///
/// A line, not a summary: enough to see what the model just decided, short enough that a terminal
/// scrolling by stays scannable.
const MAX_SAID: usize = 120;

/// Longest tool label shown beside a call.
const MAX_LABEL: usize = 80;

/// Prints a running loop's activity, one line at a time.
pub(crate) struct LoopView {
    /// Set by `--quiet`: the run still writes its log and its journal, and says nothing here.
    quiet: bool,
    /// When the run started, which every line is timed from.
    started: Instant,
    /// Call id to the label its arguments described, learned when the message is committed.
    labels: HashMap<String, String>,
    /// Tool calls in the turn now running, for the line that closes it.
    tools_this_turn: usize,
    /// When the turn now running began.
    turn_started: Option<Instant>,
}

impl LoopView {
    pub(crate) fn new(quiet: bool) -> Self {
        Self {
            quiet,
            started: Instant::now(),
            labels: HashMap::new(),
            tools_this_turn: 0,
            turn_started: None,
        }
    }

    /// Print one line about `event`, when it is one of the few worth a line.
    pub(crate) fn observe(&mut self, event: &Event) {
        if self.quiet {
            return;
        }
        match event {
            // A hook talking: the phase moved, the gate refused, the run is blocked. These are the
            // lines the whole view exists for.
            Event::Notice(message) => self.line("", message),
            Event::ProviderRetrying { attempt, attempts, reason } => {
                self.line("", &format!("retrying ({attempt}/{attempts}): {reason}"));
            }
            // Only the assistant's. The steer that opens each iteration and the gate's refusals
            // arrive as user messages the engine wrote to itself, and the notices above already
            // said what happened — in one line rather than the several hundred words the model is
            // handed.
            Event::MessageCommitted { message, .. } if message.role == Role::Assistant => {
                // Tool arguments arrive here, before the call starts — which is where a label like
                // `logstats.py · 2 edits` comes from.
                for block in &message.content {
                    match block {
                        ContentBlock::Text(text) if !text.trim().is_empty() => {
                            self.line("· ", &bounded(first_line(text), MAX_SAID));
                        }
                        ContentBlock::ToolCall { id, name, arguments } => {
                            let label = luxe_core::describe_call(
                                name,
                                arguments,
                                luxe_core::ToolContext::working_dir(),
                            );
                            self.labels.insert(id.clone(), bounded(label, MAX_LABEL));
                        }
                        _ => {}
                    }
                }
            }
            Event::ToolStarted { call_id, name } => {
                if luxe_core::activity::is_bookkeeping(name) {
                    return;
                }
                self.tools_this_turn += 1;
                match self.labels.get(call_id) {
                    Some(label) if !label.is_empty() => self.line("  ", &format!("{name} {label}")),
                    _ => self.line("  ", name),
                }
            }
            Event::ToolEnded { call_id, is_error } if *is_error => {
                let label = self.labels.get(call_id).cloned().unwrap_or_default();
                self.line("  ", format!("failed: {label}").trim_end());
            }
            Event::CompactionDone { tokens_before, tokens_after } => self.line(
                "",
                &format!("compacted {} → {}", thousands(*tokens_before), thousands(*tokens_after)),
            ),
            Event::Error(message) => self.line("", &format!("error: {}", first_line(message))),
            Event::TurnEnded { reason } => {
                let took = self.turn_started.take().map(|at| at.elapsed()).unwrap_or_default();
                let tools = std::mem::take(&mut self.tools_this_turn);
                self.labels.clear();
                let ended = match reason {
                    // Every ordinary ending says the same thing; the interesting ones name
                    // themselves — a runaway guard, a cancel, a turn that failed.
                    luxe_core::StopReason::EndTurn => "turn ended".to_string(),
                    other => format!("turn ended: {other:?}"),
                };
                self.line("", &format!("{ended} · {tools} tools · {}", duration(took)));
            }
            _ => {}
        }
    }

    /// Note that a turn has begun, so the line closing it can say how long it took.
    pub(crate) fn turn_started(&mut self) {
        self.turn_started = Some(Instant::now());
    }

    /// One line: how long the run has been going, then what happened.
    fn line(&self, indent: &str, message: &str) {
        outln!("{:>9}  {indent}{message}", duration(self.started.elapsed()));
    }
}

/// `4m12s`, `1h04m`, `12s` — a duration at the precision the reader can use.
fn duration(elapsed: std::time::Duration) -> String {
    let seconds = elapsed.as_secs();
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m{:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h{:02}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

/// `41.2k` past a thousand, the plain number below it.
fn thousands(tokens: u64) -> String {
    match tokens {
        0..=999 => tokens.to_string(),
        _ => format!("{:.1}k", tokens as f64 / 1000.0),
    }
}

/// The first non-empty line of `text`, whitespace collapsed to single spaces.
fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

/// `text`, cut to `max` characters on a character boundary, with an ellipsis when it was cut.
fn bounded(text: String, max: usize) -> String {
    if text.chars().count() <= max {
        return text;
    }
    let kept: String = text.chars().take(max).collect();
    format!("{}…", kept.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The elapsed column is readable at every scale a loop run reaches.
    #[test]
    fn a_duration_reads_at_the_scale_it_is() {
        use std::time::Duration;
        assert_eq!(duration(Duration::from_secs(9)), "9s");
        assert_eq!(duration(Duration::from_secs(59)), "59s");
        assert_eq!(duration(Duration::from_secs(60)), "1m00s");
        assert_eq!(duration(Duration::from_secs(252)), "4m12s");
        assert_eq!(duration(Duration::from_secs(3_599)), "59m59s");
        assert_eq!(duration(Duration::from_secs(3_600)), "1h00m");
        assert_eq!(duration(Duration::from_secs(8_130)), "2h15m");
    }

    #[test]
    fn token_counts_are_readable_rather_than_exact() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(41_213), "41.2k");
    }

    /// A model's paragraph becomes one line, and a long one is cut rather than wrapped.
    #[test]
    fn what_the_model_said_is_one_bounded_line() {
        assert_eq!(first_line("\n\n  Added rank_words.\nThen the tests.\n"), "Added rank_words.");
        assert_eq!(first_line("a   b\tc"), "a b c", "whitespace is collapsed");
        assert_eq!(first_line("   \n  "), "");

        let long = "x".repeat(MAX_SAID + 40);
        let cut = bounded(long, MAX_SAID);
        assert_eq!(cut.chars().count(), MAX_SAID + 1, "the ellipsis is the only thing added");
        assert!(cut.ends_with('…'));
        assert_eq!(bounded("short".into(), MAX_SAID), "short", "and a short line is left alone");
    }

    /// `--quiet` means quiet: the view holds no state and prints nothing.
    ///
    /// Worth pinning because the observer runs on every event of every turn — a quiet view that
    /// still built labels would be a memory cost for output nobody asked for.
    #[test]
    fn a_quiet_view_does_not_even_look() {
        let mut view = LoopView::new(true);
        view.observe(&Event::MessageCommitted {
            session_id: "s".into(),
            entry_id: luxe_core::session::EntryId::new(),
            message: luxe_core::message::Message::assistant("something"),
        });
        view.observe(&Event::ToolStarted { call_id: "c".into(), name: "read".into() });
        assert!(view.labels.is_empty());
        assert_eq!(view.tools_this_turn, 0);
    }

    /// A turn's tool count belongs to that turn, and starts again with the next one.
    #[test]
    fn each_turn_counts_its_own_tools() {
        let mut view = LoopView::new(false);
        view.turn_started();
        view.observe(&Event::ToolStarted { call_id: "a".into(), name: "read".into() });
        view.observe(&Event::ToolStarted { call_id: "b".into(), name: "edit".into() });
        assert_eq!(view.tools_this_turn, 2);

        view.observe(&Event::TurnEnded { reason: luxe_core::StopReason::EndTurn });
        assert_eq!(view.tools_this_turn, 0, "the next turn starts from nothing");
        assert!(view.labels.is_empty(), "and does not inherit the last turn's labels");
    }

    /// Luxe's own bookkeeping is not work anyone asked to watch.
    #[test]
    fn activating_a_tool_is_not_a_step_worth_a_line() {
        let mut view = LoopView::new(false);
        view.observe(&Event::ToolStarted {
            call_id: "c".into(),
            name: luxe_core::tool::ACTIVATE_TOOLS_NAME.into(),
        });
        assert_eq!(view.tools_this_turn, 0);
    }
}
