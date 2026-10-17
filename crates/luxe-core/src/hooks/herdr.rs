//! Herdr integration — report agent lifecycle state to the enclosing Herdr pane.
//!
//! [Herdr](https://herdr.dev) is a terminal multiplexer for coding agents: a sidebar rolls up
//! each agent's state (`working`/`blocked`/`idle`/`done`) across every pane so a human can see,
//! at a glance, which agent is running, which needs a decision, and which is ready to review.
//!
//! Herdr recognizes most agents by *process detection* + screen scraping, which requires a Herdr
//! binary update per new agent. Instead Luxe acts as a **custom socket integration** (Herdr's
//! supported extension path): when Luxe runs inside a Herdr pane (`HERDR_ENV=1`), this hook reports
//! Luxe's own lifecycle to Herdr via the `herdr pane report-agent` CLI. That makes Luxe a
//! first-class, correctly-tracked agent in the sidebar without Herdr needing to know the `luxe`
//! process at all.
//!
//! It's deliberately **best-effort and side-effect-only** (no telemetry, offline-safe,
//! never block loop work): reports are fire-and-forget, failures (Herdr absent, CLI error) are
//! swallowed, the hook always returns [`HookOutcome::Continue`], and it self-disables outside a
//! Herdr pane. Opt out entirely with `LUXE_HERDR=0`.
//!
//! Lifecycle mapping (`Action → … → Event`, observed at hook points):
//! `SessionStart → idle`, `AgentStart → working`, `AgentEnd → idle`; the two ways a turn waits on a
//! human both flip to `blocked` — an interactive permission prompt (`PermissionRequested` /
//! `PermissionResolved`) and the `question` tool (`BeforeToolUse` / `AfterToolUse`); and
//! `SessionEnd → release`.

use crate::hook::{Hook, HookCtx, HookEvent, HookOutcome, HookPoint};
use async_trait::async_trait;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// The tool whose invocation means "the agent is waiting on a human answer".
const BLOCKING_TOOL: &str = "question";

/// Semantic agent state Herdr understands. Maps 1:1 to `herdr pane report-agent --state`.
///
/// Note there is no `done`: Herdr *derives* `done` from an `idle` report that lands while the
/// pane is unseen, so an integration only ever reports these three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HerdrState {
    /// A turn is in flight (calling the provider / running tools).
    Working,
    /// Waiting on a required human decision (e.g. the `question` tool).
    Blocked,
    /// At the prompt, turn complete — nothing in flight.
    Idle,
}

impl HerdrState {
    /// The `--state` token Herdr expects.
    fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Idle => "idle",
        }
    }
}

/// Sink for state reports. A trait so the hook is testable headless: the real impl shells out to
/// `herdr`; tests record calls in memory.
pub trait HerdrReporter: Send + Sync {
    /// Report the current semantic state to Herdr.
    fn report(&self, state: HerdrState);
    /// Relinquish this source's authority over the pane's agent state (on session end).
    fn release(&self);
}

/// The production reporter: spawns `herdr pane report-agent` for the current pane.
///
/// Each report runs in a detached thread that reaps its child, so a report never blocks a turn and
/// never leaks a zombie. Because those threads can finish out of order, every report carries a
/// monotonically increasing `--seq` so Herdr orders them correctly regardless of completion order.
pub struct CliHerdrReporter {
    /// Public pane id Herdr injected as `$HERDR_PANE_ID` (e.g. `w1:p1`).
    pane_id: String,
    /// Source identity that owns these reports (`--source`).
    source: String,
    /// Agent label shown in the sidebar (`--agent`).
    agent: String,
    /// Monotonic sequence so out-of-order subprocess completion can't reorder state.
    seq: AtomicU64,
}

impl CliHerdrReporter {
    /// Build a reporter targeting `pane_id`, reporting as source/agent `luxe`.
    pub fn new(pane_id: String) -> Self {
        Self {
            pane_id,
            source: "luxe".to_string(),
            agent: "luxe".to_string(),
            seq: AtomicU64::new(0),
        }
    }

    /// Spawn a detached `herdr pane <args…>` invocation, silencing its stdio so it can't corrupt
    /// the TUI. Errors (Herdr missing, non-zero exit) are intentionally ignored.
    fn spawn(&self, extra: &[&str]) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed).to_string();
        let mut args: Vec<String> = vec!["pane".to_string()];
        args.extend(extra.iter().map(|s| s.to_string()));
        args.extend(["--seq".to_string(), seq]);
        std::thread::spawn(move || {
            let _ = std::process::Command::new("herdr")
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        });
    }
}

impl HerdrReporter for CliHerdrReporter {
    fn report(&self, state: HerdrState) {
        self.spawn(&[
            "report-agent",
            &self.pane_id,
            "--source",
            &self.source,
            "--agent",
            &self.agent,
            "--state",
            state.as_str(),
        ]);
    }

    fn release(&self) {
        self.spawn(&[
            "release-agent",
            &self.pane_id,
            "--source",
            &self.source,
            "--agent",
            &self.agent,
        ]);
    }
}

/// Reports Luxe's lifecycle to Herdr. Registered only when running inside a Herdr pane.
pub struct HerdrHook {
    reporter: Box<dyn HerdrReporter>,
    /// Last state sent, to suppress redundant reports (a turn fires `AgentStart` many times).
    last: Mutex<Option<HerdrState>>,
}

impl HerdrHook {
    /// Auto-detect the Herdr environment and build the hook, or `None` when not applicable.
    ///
    /// Requires `HERDR_ENV=1` and a non-empty `HERDR_PANE_ID` (both injected into every managed
    /// pane). Honors the `LUXE_HERDR=0`/`false`/`no`/`off` opt-out.
    pub fn detect() -> Option<Self> {
        if env_opted_out("LUXE_HERDR") {
            return None;
        }
        if std::env::var("HERDR_ENV").ok().as_deref() != Some("1") {
            return None;
        }
        let pane_id = std::env::var("HERDR_PANE_ID").ok().filter(|s| !s.trim().is_empty())?;
        Some(Self::with_reporter(Box::new(CliHerdrReporter::new(pane_id))))
    }

    /// Build a hook over an explicit reporter (used by tests).
    pub fn with_reporter(reporter: Box<dyn HerdrReporter>) -> Self {
        Self { reporter, last: Mutex::new(None) }
    }

    /// Report `state`, skipping the CLI round-trip when it hasn't changed.
    fn set_state(&self, state: HerdrState) {
        let mut last = self.last.lock().expect("herdr last-state lock poisoned");
        if *last == Some(state) {
            return;
        }
        *last = Some(state);
        self.reporter.report(state);
    }
}

#[async_trait]
impl Hook for HerdrHook {
    fn name(&self) -> &'static str {
        "herdr"
    }

    fn interest(&self) -> &[HookPoint] {
        &[
            HookPoint::SessionStart,
            HookPoint::SessionEnd,
            HookPoint::AgentStart,
            HookPoint::AgentEnd,
            HookPoint::BeforeToolUse,
            HookPoint::AfterToolUse,
            HookPoint::PermissionRequested,
            HookPoint::PermissionResolved,
        ]
    }

    async fn on(&self, point: HookPoint, ev: &HookEvent, _cx: &HookCtx) -> HookOutcome {
        // The `question` tool is the one built-in that suspends the turn waiting on a human, so it
        // (and only it) toggles blocked; every other tool keeps the turn in `working`.
        let is_question = matches!(ev, HookEvent::ToolUse { tool_name, .. } if tool_name == BLOCKING_TOOL)
            || matches!(ev, HookEvent::ToolResult { tool_name, .. } if tool_name == BLOCKING_TOOL);

        let state = match point {
            HookPoint::SessionStart | HookPoint::AgentEnd => HerdrState::Idle,
            HookPoint::AgentStart | HookPoint::PermissionResolved => HerdrState::Working,
            // An interactive permission prompt, or the `question` tool, blocks on a human.
            HookPoint::PermissionRequested => HerdrState::Blocked,
            HookPoint::BeforeToolUse if is_question => HerdrState::Blocked,
            HookPoint::AfterToolUse if is_question => HerdrState::Working,
            HookPoint::SessionEnd => {
                *self.last.lock().expect("herdr last-state lock poisoned") = None;
                self.reporter.release();
                return HookOutcome::Continue;
            }
            _ => return HookOutcome::Continue, // non-blocking tool use: no state change
        };
        self.set_state(state);
        HookOutcome::Continue
    }
}

/// Whether an env var is explicitly set to a falsy value (`0`/`false`/`no`/`off`).
fn env_opted_out(key: &str) -> bool {
    std::env::var(key)
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Records every report/release so a test can assert the exact transition sequence.
    #[derive(Default)]
    struct MockReporter {
        events: Mutex<Vec<String>>,
    }

    impl HerdrReporter for MockReporter {
        fn report(&self, state: HerdrState) {
            self.events.lock().unwrap().push(state.as_str().to_string());
        }
        fn release(&self) {
            self.events.lock().unwrap().push("release".to_string());
        }
    }

    /// A reporter wrapper so the hook and the test can share one recording sink.
    struct SharedReporter(Arc<MockReporter>);
    impl HerdrReporter for SharedReporter {
        fn report(&self, state: HerdrState) {
            self.0.report(state);
        }
        fn release(&self) {
            self.0.release();
        }
    }

    fn ctx() -> HookCtx {
        HookCtx {
            session_id: "s".to_string(),
            event_tx: tokio::sync::mpsc::channel(1).0,
            hook_registry: crate::hook::HookRegistry::new(),
        }
    }

    fn other() -> HookEvent {
        HookEvent::Other(serde_json::Value::Null)
    }

    #[tokio::test]
    async fn maps_lifecycle_points_to_states() {
        let sink = Arc::new(MockReporter::default());
        let hook = HerdrHook::with_reporter(Box::new(SharedReporter(sink.clone())));
        let cx = ctx();

        hook.on(HookPoint::SessionStart, &other(), &cx).await;
        hook.on(HookPoint::AgentStart, &other(), &cx).await;
        // A non-question tool must NOT move state off `working`.
        let read = HookEvent::ToolUse { tool_name: "read".into(), args: serde_json::json!({}) };
        hook.on(HookPoint::BeforeToolUse, &read, &cx).await;
        // The question tool blocks, then resumes.
        let q = HookEvent::ToolUse { tool_name: "question".into(), args: serde_json::json!({}) };
        hook.on(HookPoint::BeforeToolUse, &q, &cx).await;
        let qr = HookEvent::ToolResult {
            tool_name: "question".into(),
            result: "ok".into(),
            is_error: false,
        };
        hook.on(HookPoint::AfterToolUse, &qr, &cx).await;
        hook.on(HookPoint::AgentEnd, &other(), &cx).await;
        hook.on(HookPoint::SessionEnd, &other(), &cx).await;

        let events = sink.events.lock().unwrap().clone();
        assert_eq!(events, vec!["idle", "working", "blocked", "working", "idle", "release"]);
    }

    #[tokio::test]
    async fn permission_prompt_toggles_blocked_then_working() {
        let sink = Arc::new(MockReporter::default());
        let hook = HerdrHook::with_reporter(Box::new(SharedReporter(sink.clone())));
        let cx = ctx();

        // Mid-turn a tool needs approval: blocked while the prompt is up, working once resolved.
        // The hook keys off the point, not the payload, so a bare event is fine here.
        hook.on(HookPoint::AgentStart, &other(), &cx).await;
        hook.on(HookPoint::PermissionRequested, &other(), &cx).await;
        hook.on(HookPoint::PermissionResolved, &other(), &cx).await;
        hook.on(HookPoint::AgentEnd, &other(), &cx).await;

        assert_eq!(*sink.events.lock().unwrap(), vec!["working", "blocked", "working", "idle"]);
    }

    #[tokio::test]
    async fn dedupes_repeated_states() {
        let sink = Arc::new(MockReporter::default());
        let hook = HerdrHook::with_reporter(Box::new(SharedReporter(sink.clone())));
        let cx = ctx();

        // AgentStart fires once per provider call within a turn; only the first should report.
        hook.on(HookPoint::AgentStart, &other(), &cx).await;
        hook.on(HookPoint::AgentStart, &other(), &cx).await;
        hook.on(HookPoint::AgentStart, &other(), &cx).await;

        assert_eq!(*sink.events.lock().unwrap(), vec!["working"]);
    }

    #[tokio::test]
    async fn always_continues() {
        let sink = Arc::new(MockReporter::default());
        let hook = HerdrHook::with_reporter(Box::new(SharedReporter(sink)));
        let outcome = hook.on(HookPoint::AgentStart, &other(), &ctx()).await;
        assert!(matches!(outcome, HookOutcome::Continue));
    }
}
