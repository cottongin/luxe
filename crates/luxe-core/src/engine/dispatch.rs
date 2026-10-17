//! Tool dispatch — the guard chain (hook → policy → role → registry) and tool execution.

use super::*;

mod permission;
mod results;
mod test_nudge;

use results::{redact_tool_result, tool_output_from_content};
use test_nudge::is_test_command;

impl Engine {
    /// Runs one call and returns its `ToolResult` block, announcing it on both sides.
    ///
    /// Returns a block **always** — for a call a cancel or a deadline stopped too. That total is
    /// the contract the providers enforce: an assistant message whose calls are not all answered is
    /// rejected on every later request, so a stopped call comes back as a result, not an error.
    ///
    /// `ToolStarted` goes out before the guard chain and `ToolCompleted` the moment the call is
    /// over, with its output, so a card settles when its own call does — not when the last call of
    /// its message does, which is when the results commit and `ToolEnded` follows. The guard chain
    /// and the run live in [`Self::run_single_tool_call`].
    ///
    /// A cancel raised before the call starts answers it as never started. Calls run strictly one at
    /// a time, so a cancel has to end the *batch*: checked only inside the running call, the
    /// interrupt would unwind that call and start the next, a fresh tool card appearing after the
    /// UI already reported the turn idle.
    pub(crate) async fn dispatch_one(
        &self,
        tc: &ToolCallInfo,
        event_tx: &mpsc::Sender<Event>,
    ) -> ContentBlock {
        if self.is_cancelled() {
            debug!(tool = %tc.name, "skipping queued tool call: turn cancelled");
            return unfinished(tc, Unfinished::NeverStarted);
        }
        let _ = event_tx
            .send(Event::ToolStarted { call_id: tc.id.clone(), name: tc.name.clone() })
            .await;

        let result = self.run_single_tool_call(tc, event_tx).await;
        let content = tool_output_from_content(&result.content);
        let output = content
            .iter()
            .filter_map(|part| match part {
                crate::message::ToolOutput::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let _ = event_tx
            .send(Event::ToolCompleted {
                call_id: tc.id.clone(),
                is_error: result.is_error,
                output,
            })
            .await;
        ContentBlock::ToolResult { call_id: tc.id.clone(), content, is_error: result.is_error }
    }

    /// Evaluate the guard chain (hook → permission policy → role allow-list → registry
    /// lookup) and run the tool. Denials and lookup misses produce an explanatory result
    /// instead of running anything, so the model still sees why a call didn't execute.
    async fn run_single_tool_call(
        &self,
        tc: &ToolCallInfo,
        event_tx: &mpsc::Sender<Event>,
    ) -> ToolResultData {
        let PreparedCall { args, hook_ask, hook_cx } =
            match self.prepare_tool_call(tc, event_tx).await {
                Ok(prepared) => prepared,
                Err(denied) => return denied,
            };
        let plan_write =
            match self.judge_tool_call(tc, &args, hook_ask.as_deref(), &hook_cx, event_tx).await {
                Ok(plan_write) => plan_write,
                Err(denied) => return denied,
            };

        let tool = match self.config.tool_registry.get(&tc.name) {
            Some(t) => t,
            None => {
                warn!(tool = %tc.name, "tool not found in registry");
                return ToolResultData::error(format!("Tool '{}' not found", tc.name));
            }
        };

        let tool_cx = ToolContext {
            cwd: ToolContext::working_dir(),
            session_id: hook_cx.session_id.clone(),
            cancelled: Arc::new(self.cancel_tx.subscribe()),
            event_tx: event_tx.clone(),
            hook_ctx: Some(hook_cx.clone()),
            // The `question` tool prompts interactively only where a front-end can answer (an
            // interactive `ask` resolution). Headless/loop runs get `None` and fall back to the
            // question's default.
            question_broker: matches!(self.ask_resolution, AskResolution::Prompt)
                .then(|| self.question_broker.clone()),
        };

        // Classify before `args` is moved into the tool, for the debugger nudge below.
        let test_call = is_test_command(&tc.name, &args);
        let stray = crate::tool::stray_arguments(&tool.schema(), tool.argument_aliases(), &args);
        debug!(tool = %tc.name, "running tool");
        let mut result = match self.run_bounded(tool, args, &tool_cx).await {
            Ok(result) => result,
            Err(stopped) => {
                warn!(tool = %tc.name, reason = ?stopped, "tool call stopped before it finished");
                return ToolResultData::error(stopped.explain(&tc.name));
            }
        };
        // Scrub secret-shaped strings once, at this single choke point, BEFORE the result
        // reaches hooks, the persisted transcript, the event stream, or the model's next turn.
        redact_tool_result(&mut result);
        if let Some(note) = crate::tool::stray_note(&tc.name, &stray) {
            result.content.push(ContentBlock::Text(format!("\n{note}")));
        }
        self.finish_tool_result(tc, &mut result, test_call, plan_write, &hook_cx, event_tx).await;
        result
    }

    /// Parses a call's arguments and lets the `BeforeToolUse` hooks weigh in, or answers why it
    /// will not run.
    ///
    /// A hook's rewrite is applied here, *above* the permission engine, so the arguments the gate
    /// judges are the ones the tool will actually receive. Doing it the other way round — check,
    /// then let a hook edit — is a confused deputy: the rules would clear `rm build` while the
    /// shell ran whatever the hook substituted for it.
    async fn prepare_tool_call(
        &self,
        tc: &ToolCallInfo,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<PreparedCall, ToolResultData> {
        // Never runs on a guess: the `{}` this would parse to is a different call
        if let Some(error) = crate::engine::malformed_arguments(&tc.arguments) {
            warn!(tool = %tc.name, %error, "a tool call's arguments were not valid JSON");
            return Err(denial(format!(
                "This `{}` call did not run: its arguments were not valid JSON ({error}). Send \
                 the call again with its arguments as one JSON object.",
                tc.name
            )));
        }
        let mut args = parse_tool_arguments(&tc.arguments);
        let hook_cx = self.hook_context(event_tx).await;

        let outcome = self
            .config
            .hook_registry
            .invoke(
                HookPoint::BeforeToolUse,
                &HookEvent::ToolUse { tool_name: tc.name.clone(), args: args.clone() },
                &hook_cx,
            )
            .await;
        let hook_ask = match outcome {
            HookOutcome::Deny { reason } => {
                warn!(tool = %tc.name, reason = %reason, "tool use denied by hook");
                return Err(denial(format!("Tool call denied: {reason}")));
            }
            HookOutcome::Modify { args: patch, ask } => {
                debug!(tool = %tc.name, "a hook rewrote the call's arguments");
                args = crate::hook::merge_args(args, patch);
                ask
            }
            HookOutcome::Ask(reason) => Some(reason),
            HookOutcome::Continue => None,
            _ => {
                warn!(tool = %tc.name, "unexpected hook outcome");
                None
            }
        };
        Ok(PreparedCall { args, hook_ask, hook_cx })
    }

    /// Whether the call may run: in a read-only turn, in the session's mode, and under the
    /// permission rules. Answers the plan file it writes when the mode holds writes back, or the
    /// refusal.
    ///
    /// Args: `(tc, args, hook_ask, hook_cx, event_tx)`.
    async fn judge_tool_call(
        &self,
        tc: &ToolCallInfo,
        args: &serde_json::Value,
        hook_ask: Option<&str>,
        hook_cx: &HookCtx,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<Option<String>, ToolResultData> {
        // A read-only turn changes nothing, enforced here rather than asked for
        if let Some(turn) = self.read_only.filter(|_| !super::read_only::allows(&tc.name, args)) {
            warn!(tool = %tc.name, ?turn, "a read-only turn tried a tool that changes things");
            return Err(denial(super::read_only::refusal(turn, &tc.name, args)));
        }
        // And so does a mode that holds writes back, for as long as the session is in it
        let mode = self.mode().await;
        let plan_write =
            match super::read_only::judge_for_mode(&mode, &tc.name, args, &self.project_root()) {
                super::read_only::ModeVerdict::Runs => None,
                super::read_only::ModeVerdict::WritesPlan(path) => Some(path),
                super::read_only::ModeVerdict::Refused(refusal) => {
                    warn!(tool = %tc.name, mode = %mode.name, "the session's mode refused a call");
                    return Err(denial(refusal));
                }
            };
        if let Some(reason) =
            self.check_permission(&tc.name, args, hook_ask, event_tx, hook_cx).await
        {
            warn!(tool = %tc.name, %reason, "tool use denied by permission policy");
            return Err(denial(reason));
        }
        Ok(plan_write)
    }

    /// What follows a tool's run: the `AfterToolUse` hooks, any context they feed back, the
    /// debugger nudge for a failing test, and the plan-written notice.
    ///
    /// Args: `(tc, result, test_call, plan_write, hook_cx, event_tx)`.
    async fn finish_tool_result(
        &self,
        tc: &ToolCallInfo,
        result: &mut ToolResultData,
        test_call: bool,
        plan_write: Option<String>,
        hook_cx: &HookCtx,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let result_str = result
            .content
            .iter()
            .map(|c| match c {
                ContentBlock::Text(t) => t.clone(),
                _ => "[non-text]".to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        let after = self
            .config
            .hook_registry
            .invoke(
                HookPoint::AfterToolUse,
                &HookEvent::ToolResult {
                    tool_name: tc.name.clone(),
                    result: result_str,
                    is_error: result.is_error,
                },
                hook_cx,
            )
            .await;
        // An AfterToolUse hook may feed context back to the model (e.g. a linter/typecheck run):
        // append it to the result the model sees. Redacted like any other tool output.
        if let HookOutcome::InjectContext(feedback) = after {
            let feedback = crate::redact::process_secrets().redact(&feedback);
            result.content.push(ContentBlock::Text(format!("\n[hook] {feedback}")));
        }

        self.maybe_nudge_debugger(test_call, result).await;
        if let Some(path) = plan_write.filter(|_| !result.is_error) {
            let _ = event_tx.send(Event::PlanWritten { path }).await;
        }
    }

    /// Run `tool`, stopping on cancellation or its declared deadline.
    ///
    /// Dropping the future must stop its children, so tools need `kill_on_drop`. A tool that owns a
    /// process group enforces its own deadline; dispatch can only reach the direct child.
    async fn run_bounded(
        &self,
        tool: &dyn Tool,
        args: serde_json::Value,
        cx: &ToolContext,
    ) -> Result<ToolResultData, Stopped> {
        let mut cancel = self.cancel_tx.subscribe();
        let run = async {
            let engine = self.permission.lock().await.clone();
            let read_hook =
                crate::permission::PermissionSettings { engine, ask: self.ask_resolution_now() };
            tool.run_with_read_hook(args, cx, &read_hook).await
        };
        let deadline = crate::tool::effective_timeout(tool);
        tokio::pin!(run);

        // No deadline: still race the cancel, so an unbounded tool is interruptible rather than
        // permanent.
        let Some(deadline) = deadline else {
            return tokio::select! {
                biased;
                result = &mut run => Ok(result),
                () = wait_until_cancelled(&mut cancel) => Err(Stopped::Cancelled),
            };
        };
        tokio::select! {
            biased;
            result = tokio::time::timeout(deadline, &mut run) => match result {
                Ok(result) => Ok(result),
                Err(_) => Err(Stopped::TimedOut(deadline)),
            },
            () = wait_until_cancelled(&mut cancel) => Err(Stopped::Cancelled),
        }
    }
}

/// A call ready to be judged: its arguments as the hooks left them, whether a hook asked for a
/// prompt, and the hook context it ran under.
struct PreparedCall {
    args: serde_json::Value,
    hook_ask: Option<String>,
    hook_cx: HookCtx,
}

/// Why a tool call ended without the tool producing a result.
///
/// Both cases dropped the tool's future mid-flight, so both have to be reported as "this may have
/// half-happened" rather than as a clean failure — see [`Unfinished::Aborted`].
#[derive(Debug, Clone, Copy)]
enum Stopped {
    /// The user interrupted the turn.
    Cancelled,
    /// The tool's declared deadline passed.
    TimedOut(std::time::Duration),
}

impl Stopped {
    /// What the model is told in place of the tool's output.
    ///
    /// A timeout names the tool and the bound, because the useful next move depends on which one it
    /// was: a page that never loads wants a different URL, while a genuinely slow job wants
    /// `LUXE_TOOL_TIMEOUT_SECS` raised, and a model that cannot tell them apart just retries.
    fn explain(self, tool: &str) -> String {
        match self {
            Self::Cancelled => Unfinished::Aborted.message().to_string(),
            Self::TimedOut(after) => format!(
                "`{tool}` was still running after {}s and was stopped. {} Raise \
                 LUXE_TOOL_TIMEOUT_SECS if this work legitimately takes longer.",
                after.as_secs(),
                Unfinished::Aborted.message(),
            ),
        }
    }
}

/// A result block standing in for a call the harness stopped instead of running.
fn unfinished(tc: &ToolCallInfo, state: Unfinished) -> ContentBlock {
    state.result_for(tc.id.clone())
}

/// The result for a call that never started, so it could have had no effect.
pub(crate) fn never_started(tc: &ToolCallInfo) -> ContentBlock {
    unfinished(tc, Unfinished::NeverStarted)
}

/// Await until the cancel watch flips to `true` (or its sender drops), so a pending permission
/// prompt unblocks the moment the turn is cancelled.
async fn wait_until_cancelled(rx: &mut tokio::sync::watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}

/// The result for a call that did not run, carrying why.
///
/// An error, because the call did not do what it asked for. Reported as a success it read as a
/// green "Completed" card for a commit the reader had just rejected, and told Anthropic's models
/// the call had worked.
fn denial(msg: String) -> ToolResultData {
    ToolResultData::error(msg)
}

#[cfg(test)]
mod tests {
    use super::permission::{deny_message, rejected_message};
    use super::{denial, is_test_command, Stopped};
    use serde_json::json;

    /// A call that never ran is an error to the model and a card that says who stopped it, never a
    /// green "Completed" over a commit the reader had just turned down.
    #[test]
    fn a_refused_call_is_an_error_that_names_who_refused_it() {
        let request = crate::permission::build_request(
            "git",
            &json!({"action": "commit", "message": "more"}),
            std::path::Path::new("/tmp/project"),
        );
        let rejected = rejected_message(&request);
        assert!(denial(rejected.clone()).is_error, "it did not run");
        assert_eq!(crate::activity::failure_reason(&rejected), "Rejected");
        assert_eq!(
            crate::activity::failure_reason(&deny_message(&request)),
            "Denied by the permission rules"
        );
    }

    /// A stopped call's explanation has to leave the model able to act: name the tool and the bound it
    /// passed, and never claim a clean slate for work that had already started.
    #[test]
    fn a_stopped_call_explains_itself_without_promising_a_clean_slate() {
        let timed_out = Stopped::TimedOut(std::time::Duration::from_secs(60)).explain("screenshot");
        assert!(timed_out.contains("`screenshot`"), "{timed_out}");
        assert!(timed_out.contains("60s"), "the bound it passed: {timed_out}");
        assert!(timed_out.contains("LUXE_TOOL_TIMEOUT_SECS"), "and the lever: {timed_out}");

        for message in [timed_out, Stopped::Cancelled.explain("bash")] {
            assert!(message.contains("partially completed"), "{message}");
            assert!(!message.contains("Safe to retry"), "it started, so it may have done work");
        }
    }

    #[test]
    fn recognizes_test_calls_for_the_debugger_nudge() {
        // The dedicated test tool is always a test run.
        assert!(is_test_command("test", &json!({})));
        // Bash commands invoking a known runner count.
        assert!(is_test_command("bash", &json!({"command": "cargo nextest run -p luxe-core"})));
        assert!(is_test_command("bash", &json!({"command": "PYTHONPATH=. pytest -x"})));
        assert!(is_test_command("bash", &json!({"command": "npm test"})));
        // Unrelated bash and other tools don't trip the streak.
        assert!(!is_test_command("bash", &json!({"command": "ls -la && cargo build"})));
        assert!(!is_test_command("edit", &json!({"path": "src/x.rs"})));
        assert!(!is_test_command("read", &json!({"path": "tests/t.rs"})));
    }
}
