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
//!
//! Each line is led by guides for the phase and iteration it belongs to ([`paint`]), and a tool
//! call's line takes the color of its result once it has one ([`screen`]).

mod paint;
mod screen;

use paint::{CallState, Depth, Edge, Paint};
use screen::{Printed, Screen};
use std::collections::HashMap;
use std::time::Instant;

use luxe_core::event::Event;
use luxe_core::hooks::self_continuation::parse_iteration_notice;
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
    /// The phase the run is on, once known
    phase: Option<u32>,
    /// Whether an iteration's rail is open
    in_iteration: bool,
    /// The phase the run started on, until its first turn opens it: no iteration notice does
    start_phase: Option<u32>,
    /// Calls that have started and not ended, with their line and what it said
    running: HashMap<String, RunningCall>,
    paint: Paint,
    screen: Screen,
}

/// A started call's line, kept to redraw it with the result.
struct RunningCall {
    printed: Printed,
    /// When in the run it was printed, so the redraw keeps its time
    at: std::time::Duration,
    name: String,
    label: String,
    depth: Depth,
}

impl LoopView {
    /// A view for a run starting on `phase`, in color on a terminal.
    pub(crate) fn new(quiet: bool, phase: u32) -> Self {
        Self::with(quiet, Some(phase), Paint::detect(), Screen::detect())
    }

    fn with(quiet: bool, start_phase: Option<u32>, paint: Paint, screen: Screen) -> Self {
        Self {
            quiet,
            started: Instant::now(),
            labels: HashMap::new(),
            tools_this_turn: 0,
            turn_started: None,
            phase: None,
            in_iteration: false,
            start_phase,
            running: HashMap::new(),
            paint,
            screen,
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
            Event::Notice(message) => match parse_iteration_notice(message) {
                Some((iteration, phase)) => {
                    self.open_iteration(phase, &format!("iteration {iteration}"))
                }
                None => self.note(message),
            },
            Event::ProviderRetrying { attempt, attempts, reason } => {
                self.note(&format!("retrying ({attempt}/{attempts}): {reason}"));
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
                            let said = format!("· {}", bounded(first_line(text), MAX_SAID));
                            self.within(&said);
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
            Event::ToolStarted { call_id, name } => self.tool_started(call_id, name),
            Event::ToolEnded { call_id, is_error } => self.tool_ended(call_id, *is_error),
            Event::CompactionDone { tokens_before, tokens_after } => self.note(&format!(
                "compacted {} → {}",
                thousands(*tokens_before),
                thousands(*tokens_after)
            )),
            Event::Error(message) => self.note(&format!("error: {}", first_line(message))),
            Event::TurnEnded { reason } => self.turn_ended(reason),
            _ => {}
        }
    }

    /// Note that a turn has begun, so the line closing it can say how long it took. The run's
    /// first turn opens its phase here, since no iteration notice comes before it.
    pub(crate) fn turn_started(&mut self) {
        self.turn_started = Some(Instant::now());
        if self.quiet {
            return;
        }
        if let Some(phase) = self.start_phase.take() {
            self.open_iteration(phase, "start");
        }
    }

    fn tool_started(&mut self, call_id: &str, name: &str) {
        if luxe_core::activity::is_bookkeeping(name) {
            return;
        }
        self.tools_this_turn += 1;
        let label = self.labels.get(call_id).cloned().unwrap_or_default();
        let depth = self.body_depth();
        let at = self.started.elapsed();
        let printed = self.print(depth, &self.paint.tool(name, &label, CallState::Running));
        let call = RunningCall { printed, at, name: name.to_string(), label, depth };
        self.running.insert(call_id.to_string(), call);
    }

    /// Redraws the call's line in its result's color. Where it can't be redrawn, a failure gets a
    /// line of its own and a success needs none.
    fn tool_ended(&mut self, call_id: &str, is_error: bool) {
        let Some(call) = self.running.remove(call_id) else { return };
        let state = if is_error { CallState::Failed } else { CallState::Succeeded };
        let line = self.paint.tool(&call.name, &call.label, state);
        let stamped = self.stamped(call.at, call.depth, &line);
        let redrawn = self.paint.in_color() && self.screen.rewrite(call.printed, &stamped);
        if !redrawn && is_error {
            self.print(self.body_depth(), &line);
        }
    }

    fn turn_ended(&mut self, reason: &luxe_core::StopReason) {
        let took = self.turn_started.take().map(|at| at.elapsed()).unwrap_or_default();
        let tools = std::mem::take(&mut self.tools_this_turn);
        self.labels.clear();
        self.running.clear();
        let ended = match reason {
            // Every ordinary ending says the same thing; the interesting ones name
            // themselves — a runaway guard, a cancel, a turn that failed.
            luxe_core::StopReason::EndTurn => "turn ended".to_string(),
            other => format!("turn ended: {other:?}"),
        };
        let summary = self.paint.dim(&format!("{ended} · {tools} tools · {}", duration(took)));
        let depth = match (self.phase, self.in_iteration) {
            (Some(phase), true) => Depth::Iteration(phase, Edge::Close),
            _ => self.body_depth(),
        };
        self.in_iteration = false;
        self.print(depth, &summary);
    }

    /// Opens an iteration, under a heading first when it starts a new phase.
    fn open_iteration(&mut self, phase: u32, label: &str) {
        if self.phase != Some(phase) {
            self.print(Depth::Phase(phase), &self.paint.phase_heading(phase));
            self.phase = Some(phase);
        }
        self.in_iteration = true;
        self.print(Depth::Iteration(phase, Edge::Open), label);
    }

    /// A line inside whatever is open.
    fn within(&mut self, text: &str) {
        self.print(self.body_depth(), text);
    }

    /// A hook's or the engine's notice: inside the iteration when one is open.
    fn note(&mut self, text: &str) {
        self.within(text);
    }

    /// The depth a body line sits at now.
    fn body_depth(&self) -> Depth {
        match (self.phase, self.in_iteration) {
            (Some(phase), true) => Depth::Iteration(phase, Edge::Within),
            (Some(phase), false) => Depth::Phase(phase),
            (None, _) => Depth::Run,
        }
    }

    /// One line: how long the run has been going, the guides, then what happened.
    fn print(&mut self, depth: Depth, text: &str) -> Printed {
        let line = self.stamped(self.started.elapsed(), depth, text);
        self.screen.print(&line)
    }

    fn stamped(&self, elapsed: std::time::Duration, depth: Depth, text: &str) -> String {
        format!("{:>9}  {}{text}", duration(elapsed), self.paint.guides(depth))
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
        let mut view = recording(true);
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
        let mut view = recording(false);
        view.turn_started();
        view.observe(&Event::ToolStarted { call_id: "a".into(), name: "read".into() });
        view.observe(&Event::ToolStarted { call_id: "b".into(), name: "edit".into() });
        assert_eq!(view.tools_this_turn, 2);

        view.observe(&Event::TurnEnded { reason: luxe_core::StopReason::EndTurn });
        assert_eq!(view.tools_this_turn, 0, "the next turn starts from nothing");
        assert!(view.labels.is_empty(), "and does not inherit the last turn's labels");
    }

    /// luxe's own bookkeeping is not work anyone asked to watch.
    #[test]
    fn activating_a_tool_is_not_a_step_worth_a_line() {
        let mut view = recording(false);
        view.observe(&Event::ToolStarted {
            call_id: "c".into(),
            name: luxe_core::tool::ACTIVATE_TOOLS_NAME.into(),
        });
        assert_eq!(view.tools_this_turn, 0);
    }

    /// A plain view of a run starting on phase 0, its lines kept.
    fn recording(quiet: bool) -> LoopView {
        LoopView::with(quiet, Some(0), Paint::plain(), Screen::recording())
    }

    /// What the view printed, past the time column.
    fn printed(view: &LoopView) -> Vec<String> {
        view.screen.lines().iter().map(|line| line[11..].to_string()).collect()
    }

    fn notice(text: &str) -> Event {
        Event::Notice(text.into())
    }

    fn ended(view: &mut LoopView, call_id: &str, is_error: bool) {
        view.observe(&Event::ToolEnded { call_id: call_id.into(), is_error });
    }

    /// The first turn opens its phase and iteration itself; the turn's end closes the iteration.
    #[test]
    fn the_first_turn_is_bracketed_under_its_phase() {
        let mut view = recording(false);
        view.turn_started();
        view.observe(&Event::ToolStarted { call_id: "a".into(), name: "read".into() });
        view.observe(&Event::TurnEnded { reason: luxe_core::StopReason::EndTurn });
        let lines = printed(&view);
        assert_eq!(lines[..3], ["┃ phase 0", "┃ ┌ start", "┃ │ ○ read"], "{lines:?}");
        assert!(lines[3].starts_with("┃ └ turn ended · 1 tools"), "{lines:?}");
    }

    /// An iteration in the same phase opens without a heading; a new phase gets one first.
    #[test]
    fn a_new_phase_gets_a_heading_and_the_same_phase_does_not() {
        let mut view = recording(false);
        view.turn_started();
        view.observe(&notice("iteration 2 · phase 0"));
        view.observe(&notice("iteration 3 · phase 1"));
        let lines = printed(&view);
        assert_eq!(lines[2..], ["┃ ┌ iteration 2", "┃ phase 1", "┃ ┌ iteration 3"], "{lines:?}");
    }

    /// Other notices sit inside the open iteration, and between iterations on the phase rail.
    #[test]
    fn a_notice_sits_at_the_depth_it_arrives() {
        let mut view = recording(false);
        view.turn_started();
        view.observe(&notice("phase 0 claimed complete — running the gate"));
        view.observe(&Event::TurnEnded { reason: luxe_core::StopReason::EndTurn });
        view.observe(&notice("phase 0 accepted — now on phase 1"));
        let lines = printed(&view);
        assert_eq!(lines[2], "┃ │ phase 0 claimed complete — running the gate", "{lines:?}");
        assert_eq!(lines[4], "┃ phase 0 accepted — now on phase 1", "{lines:?}");
    }

    /// Where the line can't be redrawn, a failure still gets a line saying so, and a success
    /// adds nothing to the call's own line.
    #[test]
    fn without_a_redraw_only_a_failure_adds_a_line() {
        let mut view = recording(false);
        view.turn_started();
        view.observe(&Event::ToolStarted { call_id: "a".into(), name: "read".into() });
        view.observe(&Event::ToolStarted { call_id: "b".into(), name: "bash".into() });
        ended(&mut view, "a", false);
        ended(&mut view, "b", true);
        let lines = printed(&view);
        assert_eq!(lines.len(), 5, "{lines:?}");
        assert_eq!(lines[4], "┃ │ ✗ bash · failed", "{lines:?}");
    }
}
