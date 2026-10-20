//! herdr — the agent's state, reported to herdr when luxe runs in one of its panes.
//!
//! herdr is a terminal workspace manager for coding agents. It shows each agent's state in its
//! sidebar, rolls it up to tabs and workspaces, and raises its own notification when an agent
//! finishes or needs a decision. That last part matters here: herdr emulates the terminal in each
//! pane and keeps an `OSC 9` for itself, so the desktop notification luxe writes through the
//! terminal never leaves the pane. Reporting the state is how a reader inside herdr hears from luxe.
//!
//! The state is derived from the app on every pass of the loop ([`crate::app::App::agent_state`])
//! rather than pushed from each place it changes, and only a change is sent: `working` while a turn
//! or a review's guide runs, `blocked` while a permission or question prompt waits on the reader,
//! `idle` otherwise. Reports go through `$HERDR_BIN_PATH`, herdr's documented portable interface,
//! off the render thread and under a deadline. Only the newest state is ever delivered, and a
//! failure costs a debug line, never a frame.

use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::watch;

/// What this integration calls itself to herdr. Stable, and not `herdr:`-prefixed, which herdr
/// keeps for the integrations it ships.
const SOURCE: &str = "luxe";

/// The name herdr shows for the agent.
const AGENT: &str = "luxe";

/// Longest one report may take before it is abandoned. herdr answers in milliseconds; a report
/// that has not landed by now describes a state that has probably moved on.
const DEADLINE: Duration = Duration::from_secs(3);

/// Longest a block's message runs. herdr shows it on one sidebar row and in a toast.
const MAX_MESSAGE_CHARS: usize = 80;

/// What the agent is doing, in herdr's vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentState {
    /// Ready for the next message
    Idle,
    /// A turn, or a review's guide, is running
    Working,
    /// Waiting on the reader, and why
    Blocked(String),
}

impl AgentState {
    /// The `--state` herdr expects.
    fn word(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked(_) => "blocked",
        }
    }
}

/// The pane luxe runs in, and the herdr that owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pane {
    /// `$HERDR_BIN_PATH`: the binary of the herdr that owns the pane, which may not be on `PATH`
    bin: PathBuf,
    /// `$HERDR_PANE_ID`
    id: String,
}

/// The pane herdr says luxe runs in, or `None` outside herdr.
///
/// All of it or nothing: herdr's own guidance is to report only when `HERDR_ENV=1` and the rest
/// are set, so a shell that merely inherited one variable reports nowhere.
fn pane(env: &dyn Fn(&str) -> Option<String>) -> Option<Pane> {
    let set = |name: &str| env(name).filter(|value| !value.trim().is_empty());
    if set("HERDR_ENV").as_deref() != Some("1") {
        return None;
    }
    let id = set("HERDR_PANE_ID")?;
    let bin = set("HERDR_BIN_PATH")?;
    Some(Pane { bin: PathBuf::from(bin), id })
}

/// One state report, numbered so herdr can drop one that arrives late.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Report {
    state: AgentState,
    /// The session on screen, for the command that resumes it after herdr restarts
    session: Option<String>,
    seq: u64,
}

/// Arguments for `herdr pane report-agent`.
///
/// The resume command rides along when a session is known — `luxe --resume <id>`, a plain command
/// name as herdr requires, so a restarted herdr reopens the same conversation in the same pane.
/// herdr before 0.10 ignores it, and the state still lands.
fn report_args(pane: &str, report: &Report) -> Vec<String> {
    let mut args = [
        "pane",
        "report-agent",
        pane,
        "--source",
        SOURCE,
        "--agent",
        AGENT,
        "--state",
        report.state.word(),
        "--seq",
    ]
    .map(str::to_string)
    .to_vec();
    args.push(report.seq.to_string());
    if let AgentState::Blocked(message) = &report.state {
        args.extend(["--message".to_string(), message_text(message)]);
    }
    // herdr refuses a resume command with an apostrophe or a control character in it, and the
    // whole report with it, so an id that could carry one is left off rather than risk the state
    if let Some(session) = report.session.as_deref().filter(|id| resumable(id)) {
        args.extend(["--agent-session-id".to_string(), session.to_string()]);
        args.extend(["--", "luxe", "--resume", session].map(str::to_string));
    }
    args
}

/// Arguments for `herdr pane release-agent`.
fn release_args(pane: &str, seq: u64) -> Vec<String> {
    let mut args = ["pane", "release-agent", pane, "--source", SOURCE, "--agent", AGENT, "--seq"]
        .map(str::to_string)
        .to_vec();
    args.push(seq.to_string());
    args
}

/// Whether a session id can travel in a resume command: what luxe writes, nothing a shell or herdr
/// would read as more than one plain word.
fn resumable(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// A block's reason as herdr shows it: one line, no control characters, short.
fn message_text(message: &str) -> String {
    let line = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let clean: String = line.chars().filter(|c| !c.is_control()).collect();
    match clean.chars().count() > MAX_MESSAGE_CHARS {
        true => {
            let mut cut: String = clean.chars().take(MAX_MESSAGE_CHARS - 1).collect();
            cut.push('…');
            cut
        }
        false => clean,
    }
}

/// Report numbers: rising across reports, and across restarts of luxe, which a clock gives for free.
#[derive(Debug, Default)]
struct Sequence {
    last: u64,
}

impl Sequence {
    fn next(&mut self) -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX));
        self.last = now.max(self.last.saturating_add(1));
        self.last
    }
}

/// Reports the agent's state to the herdr pane luxe runs in.
pub struct Reporter {
    pane: Pane,
    /// What was last handed to the worker, so an unchanged state is not sent again
    last: Option<(AgentState, Option<String>)>,
    sequence: Sequence,
    /// The newest report; the worker sends whatever it holds when it gets to it
    newest: watch::Sender<Option<Report>>,
    worker: tokio::task::JoinHandle<()>,
}

impl Reporter {
    /// A reporter for this pane, or `None` outside herdr or with reporting switched off.
    pub fn start(enabled: bool) -> Option<Self> {
        Self::start_in(enabled, &|name: &str| std::env::var(name).ok())
    }

    /// [`Self::start`], against an environment lookup.
    fn start_in(enabled: bool, env: &dyn Fn(&str) -> Option<String>) -> Option<Self> {
        // `LUXE_HERDR=0` and `"herdr": false` both silence the reports
        let opted_out = env("LUXE_HERDR").is_some_and(|value| {
            matches!(value.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off")
        });
        if !enabled || opted_out {
            return None;
        }
        let pane = pane(env)?;
        tracing::info!(pane = %pane.id, "running in a herdr pane; reporting the agent's state");
        let (newest, receiver) = watch::channel(None);
        let worker = tokio::spawn(deliver(pane.clone(), receiver));
        Some(Self { pane, last: None, sequence: Sequence::default(), newest, worker })
    }

    /// Hands `state` to the worker, unless it is what herdr was last told.
    ///
    /// Args: `(state, session id)`.
    pub fn report(&mut self, state: AgentState, session: Option<&str>) {
        let current = (state, session.map(str::to_string));
        if self.last.as_ref() == Some(&current) {
            return;
        }
        let report = Report {
            state: current.0.clone(),
            session: current.1.clone(),
            seq: self.sequence.next(),
        };
        self.last = Some(current);
        // `send_replace`, not `send`: the newest report has to be stored whether or not the worker
        // is waiting on it right now
        self.newest.send_replace(Some(report));
    }

    /// Tells herdr the agent has left the pane, which clears its name, state and resume command.
    ///
    /// Only for a real quit. herdr would notice on its own once the shell prompt is back, a second
    /// or two later; this is the prompt version of that.
    pub async fn release(self) {
        self.worker.abort();
        let mut sequence = self.sequence;
        run(&self.pane.bin, release_args(&self.pane.id, sequence.next())).await;
    }
}

/// Sends each newest report in turn until the reporter goes away.
async fn deliver(pane: Pane, mut newest: watch::Receiver<Option<Report>>) {
    while newest.changed().await.is_ok() {
        let Some(report) = newest.borrow_and_update().clone() else {
            continue;
        };
        run(&pane.bin, report_args(&pane.id, &report)).await;
    }
}

/// Runs the herdr CLI once, under the deadline, saying why in the log if it did not work.
async fn run(bin: &std::path::Path, args: Vec<String>) {
    let mut command = tokio::process::Command::new(bin);
    command
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    luxe_core::program::detach_from_terminal(&mut command);
    match tokio::time::timeout(DEADLINE, command.output()).await {
        Ok(Ok(output)) if output.status.success() => {}
        Ok(Ok(output)) => tracing::debug!(
            status = %output.status,
            stderr = %String::from_utf8_lossy(&output.stderr).trim(),
            "herdr refused a report"
        ),
        Ok(Err(error)) => tracing::debug!(%error, bin = %bin.display(), "could not run herdr"),
        Err(_) => tracing::debug!("a herdr report timed out"),
    }
}

#[cfg(test)]
mod tests;
