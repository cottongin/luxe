//! The `debug` tool — one action-enum surface over the DAP service.
//!
//! Read actions (threads/stack_trace/scopes/variables/output/sessions) never mutate program state;
//! exec actions (launch/attach/breakpoints/continue/step/pause/evaluate/terminate) run the
//! debuggee and are gated whenever the permission hook is enabled (the `debug` tool is classified
//! mutating). On every stop the result auto-injects the top frame + locals so the model sees where
//! it is without extra round-trips.

use crate::protocol::StoppedBody;
use crate::service::{
    BreakpointRequest, DapService, DapSession, FunctionBreakpoint, SourceBreakpoint,
};
use async_trait::async_trait;
use luxe_core::provider::ToolSchema;
use luxe_core::{Tool, ToolContext, ToolResultData};
use std::path::Path;
use std::sync::Arc;

mod failure;
mod schema;

use failure::Failure;

/// Max locals rendered in an auto stop-summary.
const MAX_LOCALS: usize = 24;

/// The `debug` tool.
pub struct DebugTool {
    service: Arc<DapService>,
}

impl DebugTool {
    /// Create the tool over a shared warm debug service.
    pub fn new(service: Arc<DapService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl Tool for DebugTool {
    fn name(&self) -> &str {
        "debug"
    }

    fn schema(&self) -> ToolSchema {
        schema::schema()
    }

    async fn run(&self, args: serde_json::Value, cx: &ToolContext) -> ToolResultData {
        let Some(action) = args.get("action").and_then(|v| v.as_str()) else {
            return ToolResultData::error("Error: 'action' is required");
        };
        match self.dispatch(action, &args, cx).await {
            Ok(text) => ToolResultData::text(text),
            Err(failure) => ToolResultData::error(failure.to_string()),
        }
    }
}

impl DebugTool {
    async fn dispatch(
        &self,
        action: &str,
        args: &serde_json::Value,
        cx: &ToolContext,
    ) -> Result<String, Failure> {
        match action {
            "launch" => self.launch(args, cx).await,
            "attach" => self.attach(args, cx).await,
            "sessions" => Ok(self.sessions().await),
            _ => {
                // All other actions operate on an existing session.
                let id = str_arg(args, "session").ok_or("Error: 'session' is required")?;
                let session = self.service.get(&id).await.ok_or_else(|| {
                    format!(
                        "Error: no active debug session '{id}'. It may have ended — use \
                         action=sessions to list, or launch/attach to start one."
                    )
                })?;
                // A program that ran to completion keeps its session (is_terminated) but its
                // runtime is gone; give a clear message instead of a low-level adapter error.
                if session.client.is_terminated() && Self::requires_live_program(action) {
                    return Err(Failure::Usage(format!(
                        "Error: debug session '{id}' has ended (program terminated); its runtime \
                         state is gone. Use action=output for remaining output, or launch/attach \
                         again to inspect."
                    )));
                }
                self.session_action(action, &session, args).await
            }
        }
    }

    async fn launch(&self, args: &serde_json::Value, cx: &ToolContext) -> Result<String, Failure> {
        let program = str_arg(args, "program").ok_or("Error: 'program' is required for launch")?;
        let cwd = str_arg(args, "cwd").unwrap_or_else(|| cx.cwd.display().to_string());
        let program_abs = crate::service::resolve(&program, &cwd);
        let outcome = self
            .service
            .launch(
                program_abs,
                string_array(args, "args"),
                cwd,
                args.get("stopOnEntry").and_then(|v| v.as_bool()).unwrap_or(false),
                breakpoint_requests(args),
                str_arg(args, "adapter"),
            )
            .await?;
        Ok(self.report(&outcome).await)
    }

    /// What a launch or attach says: the session, its stop, what did not take, its output.
    async fn report(&self, outcome: &crate::service::LaunchOutcome) -> String {
        let mut out = format!("session {} ({})\n", outcome.session_id, outcome.adapter_id);
        if let Some(session) = self.service.get(&outcome.session_id).await {
            out.push_str(&stop_summary(&session, &outcome.stop).await);
        } else if outcome.terminated {
            out.push_str("Program terminated.\n");
        }
        if !outcome.unverified.is_empty() {
            let unverified = outcome.unverified.join("\n  ");
            out.push_str(&format!("\nBreakpoints not set:\n  {unverified}\n"));
        }
        if !outcome.output.is_empty() {
            out.push_str(&format!("\n--- output ---\n{}", tail(&outcome.output)));
        }
        out
    }

    async fn attach(&self, args: &serde_json::Value, cx: &ToolContext) -> Result<String, Failure> {
        let pid = args.get("pid").and_then(serde_json::Value::as_i64);
        let port = args.get("port").and_then(serde_json::Value::as_i64);
        if pid.is_none() && port.is_none() {
            return Err("Error: attach needs a 'pid' or 'port'".into());
        }
        let cwd = str_arg(args, "cwd").unwrap_or_else(|| cx.cwd.display().to_string());
        let outcome = self
            .service
            .attach(pid, port, cwd, breakpoint_requests(args), str_arg(args, "adapter"))
            .await?;
        Ok(self.report(&outcome).await)
    }

    async fn session_action(
        &self,
        action: &str,
        session: &DapSession,
        args: &serde_json::Value,
    ) -> Result<String, Failure> {
        let thread = args.get("threadId").and_then(serde_json::Value::as_i64);
        match action {
            "set_breakpoints" => set_breakpoints(session, args).await,
            "set_function_breakpoints" => set_function_breakpoints(session, args).await,
            "set_variable" => set_variable(session, args).await,
            "continue" => Ok(stop_summary(session, &session.continue_(thread).await?).await),
            "next" => Ok(stop_summary(session, &session.step("next", thread).await?).await),
            "step_in" => Ok(stop_summary(session, &session.step("stepIn", thread).await?).await),
            "step_out" => Ok(stop_summary(session, &session.step("stepOut", thread).await?).await),
            "pause" => {
                session.pause(thread).await?;
                Ok("pause requested".to_string())
            }
            "threads" => {
                let threads = session.threads().await?;
                Ok(threads
                    .iter()
                    .map(|t| format!("{}: {}", t.id, t.name))
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            "stack_trace" => {
                let frames = session.stack_trace(thread).await?;
                Ok(render_frames(&frames, &session.cwd))
            }
            "scopes" => {
                let frame_id = args
                    .get("frameId")
                    .and_then(serde_json::Value::as_i64)
                    .ok_or("Error: 'frameId' is required")?;
                let scopes = session.scopes(frame_id).await?;
                Ok(scopes
                    .iter()
                    .map(|s| format!("{} (ref {})", s.name, s.variables_reference))
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            "variables" => {
                let vref = args
                    .get("variablesReference")
                    .and_then(serde_json::Value::as_i64)
                    .ok_or("Error: 'variablesReference' is required")?;
                let vars = session.variables(vref).await?;
                Ok(render_variables(&vars))
            }
            "evaluate" => evaluate(session, args).await,
            "output" => Ok(tail(&session.client.output_for_display())),
            "terminate" => {
                self.service.terminate(&session.id).await?;
                Ok(format!("terminated session {}", session.id))
            }
            other => Err(Failure::Usage(format!("Error: unknown action '{other}'"))),
        }
    }

    /// Whether an action needs the debuggee still running (false for output/terminate, which are
    /// meaningful on an already-ended session).
    fn requires_live_program(action: &str) -> bool {
        !matches!(action, "output" | "terminate")
    }

    async fn sessions(&self) -> String {
        let sessions = self.service.list().await;
        if sessions.is_empty() {
            return "No debug sessions.".to_string();
        }
        sessions
            .iter()
            .map(|s| {
                let state = if s.terminated {
                    "terminated".to_string()
                } else if s.stopped {
                    match &s.stop_reason {
                        Some(reason) => format!("stopped: {reason}"),
                        None => "stopped".to_string(),
                    }
                } else {
                    "running".to_string()
                };
                format!("{} [{state}] {} · {}", s.id, s.adapter, s.program)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Renders a stop: reason + top frame location + top-scope locals (auto-injected context).
async fn stop_summary(session: &DapSession, stop: &Option<StoppedBody>) -> String {
    let Some(stop) = stop else {
        return "Program terminated.".to_string();
    };
    let mut out = format!("stopped: {}", stop.reason);
    if let Some(text) = stop.text.as_deref().filter(|t| !t.is_empty()) {
        out.push_str(&format!(" — {text}"));
    }
    out.push('\n');

    let Ok(frames) = session.stack_trace(None).await else {
        return out;
    };
    let Some(top) = frames.first() else {
        return out;
    };
    out.push_str(&format!("at {} ({})\n", top.name, frame_location(top)));

    let scopes = session.scopes(top.id).await.unwrap_or_default();
    for scope in scopes.iter().filter(|scope| shown_on_stop(scope)) {
        if let Ok(vars) = session.variables(scope.variables_reference).await {
            if !vars.is_empty() {
                out.push_str(&format!("{}:\n{}\n", scope.name, render_variables(&vars)));
            }
        }
    }
    out
}

/// Whether a stop shows this scope's variables unasked.
///
/// Every cheap scope but the registers: `gdb` splits a frame into Arguments and Locals, and taking
/// only the first showed the arguments and never the locals, while a frame with no debug info
/// offered its 85 registers in their place.
fn shown_on_stop(scope: &crate::protocol::Scope) -> bool {
    let registers = scope.presentation_hint.as_deref() == Some("registers")
        || scope.name.eq_ignore_ascii_case("registers");
    !scope.expensive && !registers
}

/// Frames this deep or deeper are listed only when they are the program's own.
const FRAMES_ALWAYS_SHOWN: usize = 3;

/// A stack as a list, with runs of frames outside `project` folded into a count.
///
/// A Rust test stopped two frames into its own code sits on 17 more of `core`, `std` and the test
/// runner, 600 tokens that say nothing about the bug. The top frames always show, since a stop
/// inside a library is still where the program is.
fn render_frames(frames: &[crate::protocol::StackFrame], project: &str) -> String {
    let ours = |frame: &crate::protocol::StackFrame| {
        frame
            .source
            .as_ref()
            .and_then(|source| source.path.as_deref())
            .is_some_and(|path| Path::new(path).starts_with(project))
    };
    let mut lines = Vec::new();
    let mut folded: Vec<i64> = Vec::new();
    let fold = |lines: &mut Vec<String>, folded: &mut Vec<i64>| {
        match folded.as_slice() {
            [] => {}
            [only] => lines.push(format!("#{only} (outside the project)")),
            [first, .., last] => lines.push(format!(
                "… {} frames outside the project, #{first} to #{last}",
                folded.len()
            )),
        }
        folded.clear();
    };
    for (depth, frame) in frames.iter().enumerate() {
        if depth < FRAMES_ALWAYS_SHOWN || ours(frame) {
            fold(&mut lines, &mut folded);
            lines.push(format!("#{} {} ({})", frame.id, frame.name, frame_location(frame)));
        } else {
            folded.push(frame.id);
        }
    }
    fold(&mut lines, &mut folded);
    lines.join("\n")
}

fn frame_location(frame: &crate::protocol::StackFrame) -> String {
    let file = frame
        .source
        .as_ref()
        .and_then(|s| s.name.clone().or_else(|| s.path.clone()))
        .unwrap_or_else(|| "?".to_string());
    format!("{file}:{}", frame.line)
}

fn render_variables(vars: &[crate::protocol::Variable]) -> String {
    let mut out: Vec<String> = vars
        .iter()
        .take(MAX_LOCALS)
        .map(|v| {
            // A structured value can render as nothing at all (`gdb` on a slice), which read as
            // a variable without one; the reference is how to look inside
            let value = match (v.value.is_empty(), v.variables_reference) {
                (true, 0) => "\"\"".to_string(),
                (true, reference) => format!("{{…}} (variablesReference {reference})"),
                (false, 0) => v.value.clone(),
                (false, reference) => format!("{} (variablesReference {reference})", v.value),
            };
            match &v.type_name {
                Some(ty) if !ty.is_empty() => format!("  {} : {ty} = {value}", v.name),
                _ => format!("  {} = {value}", v.name),
            }
        })
        .collect();
    if vars.len() > MAX_LOCALS {
        out.push(format!("  … {} more", vars.len() - MAX_LOCALS));
    }
    out.join("\n")
}

/// Keep only the tail of possibly-long program output.
fn tail(output: &str) -> String {
    const MAX: usize = 4000;
    if output.len() <= MAX {
        return output.to_string();
    }
    format!("…{}", &output[luxe_core::output_limit::ceil_boundary(output, output.len() - MAX)..])
}

/// Parse launch/attach breakpoints: `[{path, lines:[..], conditions?:[..]}]` or
/// `[{path, breakpoints:[{line, condition?}]}]`. Per-line conditions align with `lines` by index;
/// a `conditions` list shorter than `lines` leaves the rest unconditional.
fn breakpoint_requests(args: &serde_json::Value) -> Vec<BreakpointRequest> {
    args.get("breakpoints")
        .and_then(|v| v.as_array())
        .map(|entries| entries.iter().filter_map(parse_breakpoint_entry).collect())
        .unwrap_or_default()
}

fn parse_breakpoint_entry(entry: &serde_json::Value) -> Option<BreakpointRequest> {
    let path = entry.get("path")?.as_str()?.to_string();
    // Explicit `breakpoints: [{line, condition?}]` form.
    if let Some(list) = entry.get("breakpoints").and_then(|v| v.as_array()) {
        let breakpoints = list
            .iter()
            .filter_map(|bp| {
                Some(SourceBreakpoint {
                    line: bp.get("line")?.as_i64()?,
                    condition: bp.get("condition").and_then(|c| c.as_str()).map(str::to_string),
                })
            })
            .collect();
        return Some(BreakpointRequest { path, breakpoints });
    }
    // Simple `lines: [..]` (+ optional aligned `conditions: [..]`) form.
    let conditions = entry.get("conditions").and_then(|v| v.as_array());
    let breakpoints = entry
        .get("lines")
        .and_then(|v| v.as_array())
        .map(|lines| {
            lines
                .iter()
                .enumerate()
                .filter_map(|(index, line)| {
                    Some(SourceBreakpoint {
                        line: line.as_i64()?,
                        condition: conditions
                            .and_then(|c| c.get(index))
                            .and_then(|c| c.as_str())
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(BreakpointRequest { path, breakpoints })
}

fn str_arg(args: &serde_json::Value, key: &str) -> Option<String> {
    args.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

fn string_array(args: &serde_json::Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default()
}

fn int_array(args: &serde_json::Value, key: &str) -> Vec<i64> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(serde_json::Value::as_i64).collect())
        .unwrap_or_default()
}

/// Sets the breakpoints for one file's lines.
async fn set_breakpoints(
    session: &DapSession,
    args: &serde_json::Value,
) -> Result<String, Failure> {
    let path = str_arg(args, "path").ok_or("Error: 'path' is required")?;
    let condition = str_arg(args, "condition");
    let breakpoints: Vec<SourceBreakpoint> = int_array(args, "lines")
        .into_iter()
        .map(|line| SourceBreakpoint { line, condition: condition.clone() })
        .collect();
    let path_abs = crate::service::resolve(&path, &session.cwd);
    let bps = crate::service::set_breakpoints(&session.client, &path_abs, &breakpoints).await?;
    let verified = bps.iter().filter(|b| b.verified).count();
    Ok(format!("set {} breakpoint(s) in {path}, {verified} verified", bps.len()))
}

/// Sets breakpoints on functions by name.
async fn set_function_breakpoints(
    session: &DapSession,
    args: &serde_json::Value,
) -> Result<String, Failure> {
    let condition = str_arg(args, "condition");
    let breakpoints: Vec<FunctionBreakpoint> = string_array(args, "functions")
        .into_iter()
        .map(|name| FunctionBreakpoint { name, condition: condition.clone() })
        .collect();
    if breakpoints.is_empty() {
        return Err("Error: 'functions' is required".into());
    }
    let bps = crate::service::set_function_breakpoints(&session.client, &breakpoints).await?;
    let verified = bps.iter().filter(|b| b.verified).count();
    Ok(format!("set {} function breakpoint(s), {verified} verified", bps.len()))
}

/// Changes a variable's value in the stopped frame.
async fn set_variable(session: &DapSession, args: &serde_json::Value) -> Result<String, Failure> {
    let vref = args
        .get("variablesReference")
        .and_then(serde_json::Value::as_i64)
        .ok_or("Error: 'variablesReference' is required")?;
    let name = str_arg(args, "name").ok_or("Error: 'name' is required")?;
    let value = str_arg(args, "value").ok_or("Error: 'value' is required")?;
    let updated = session.set_variable(vref, &name, &value).await?;
    Ok(format!("{name} = {updated}"))
}

/// Evaluates an expression in a frame, the stopped top one unless told otherwise.
async fn evaluate(session: &DapSession, args: &serde_json::Value) -> Result<String, Failure> {
    let expr = str_arg(args, "expression").ok_or("Error: 'expression' is required")?;
    // Default to the stopped top frame so an expression over locals resolves without a
    // manual stack_trace first (evaluating globally can't see locals — a common trap).
    let frame_id = match args.get("frameId").and_then(serde_json::Value::as_i64) {
        Some(id) => Some(id),
        None => session
            .stack_trace(None)
            .await
            .ok()
            .and_then(|frames| frames.first().map(|frame| frame.id)),
    };
    Ok(session.evaluate(&expr, frame_id).await?)
}

#[cfg(test)]
mod tests {
    use super::DebugTool;
    use crate::protocol::{Scope, Source, StackFrame, Variable};

    fn frame(id: i64, path: &str) -> StackFrame {
        StackFrame {
            id,
            name: format!("f{id}"),
            source: Some(Source { name: Some("x.rs".into()), path: Some(path.into()) }),
            line: 1,
            column: 1,
        }
    }

    /// The break this catches: the tail of long output was a byte slice, which panics the tool
    /// call when it lands inside a multibyte char.
    #[test]
    fn a_long_tail_is_cut_on_a_char_boundary() {
        let output = format!("x{}y", "é".repeat(2_500));
        let tail = super::tail(&output);
        assert!(tail.starts_with('…') && tail.len() <= 4_000 + '…'.len_utf8(), "{}", tail.len());
        assert!(tail[..tail.len() - 1].chars().skip(1).all(|c| c == 'é'), "{tail}");
    }

    /// Past the top few, frames outside the project fold into a count, and the project's own
    /// frames still show wherever they sit.
    #[test]
    fn library_frames_fold_into_a_count() {
        let mut frames: Vec<StackFrame> =
            (0..3).map(|id| frame(id, "/rustc/abc/library/core/x.rs")).collect();
        frames.push(frame(3, "/proj/src/lib.rs"));
        frames.extend((4..9).map(|id| frame(id, "/rustc/abc/library/std/x.rs")));
        frames.push(frame(9, "/proj/src/main.rs"));
        frames.push(frame(10, "/home/me/.cargo/registry/tokio/x.rs"));
        let shown = super::render_frames(&frames, "/proj");
        assert_eq!(
            shown,
            "#0 f0 (x.rs:1)\n#1 f1 (x.rs:1)\n#2 f2 (x.rs:1)\n#3 f3 (x.rs:1)\n\
             … 5 frames outside the project, #4 to #8\n#9 f9 (x.rs:1)\n#10 (outside the project)"
        );
        // a sibling directory sharing the prefix is not the project
        assert!(super::render_frames(
            &[frame(0, "/a"), frame(1, "/a"), frame(2, "/a"), frame(3, "/projector/x.rs")],
            "/proj"
        )
        .ends_with("#3 (outside the project)"));
    }

    /// Every cheap scope shows on a stop, so the locals come with the arguments; registers and
    /// expensive scopes do not.
    #[test]
    fn a_stop_shows_every_cheap_scope_but_registers() {
        let scope = |name: &str, hint: Option<&str>, expensive: bool| Scope {
            name: name.into(),
            variables_reference: 1,
            expensive,
            presentation_hint: hint.map(str::to_string),
        };
        assert!(super::shown_on_stop(&scope("Arguments", Some("arguments"), false)));
        assert!(super::shown_on_stop(&scope("Locals", None, false)));
        assert!(!super::shown_on_stop(&scope("Registers", None, false)));
        assert!(!super::shown_on_stop(&scope("CPU", Some("registers"), false)));
        assert!(!super::shown_on_stop(&scope("Globals", None, true)));
    }

    /// A structured value says how to look inside it, and one that renders as nothing still reads
    /// as a value.
    #[test]
    fn a_structured_value_names_its_reference() {
        let var = |name: &str, value: &str, reference: i64| Variable {
            name: name.into(),
            value: value.into(),
            type_name: None,
            variables_reference: reference,
        };
        let shown = super::render_variables(&[
            var("items", "", 4),
            var("item", "0x5555", 5),
            var("sum", "0", 0),
            var("name", "", 0),
        ]);
        assert_eq!(
            shown,
            "  items = {…} (variablesReference 4)\n  item = 0x5555 (variablesReference 5)\n  \
             sum = 0\n  name = \"\""
        );
    }

    #[test]
    fn a_relative_path_resolves_against_the_directory_not_the_program() {
        let base = std::env::temp_dir();
        let base = base.to_string_lossy();
        let joined = std::path::Path::new(&*base).join("src/lib.rs");
        assert_eq!(crate::service::resolve("src/lib.rs", &base), joined.to_string_lossy());
        let absolute = joined.to_string_lossy();
        assert_eq!(crate::service::resolve(&absolute, "/elsewhere"), absolute);
    }

    #[test]
    fn live_program_guard_allows_output_and_terminate() {
        // Actions that still make sense on an ended session.
        assert!(!DebugTool::requires_live_program("output"));
        assert!(!DebugTool::requires_live_program("terminate"));
        // Anything that touches the running program does not.
        for action in ["continue", "next", "variables", "evaluate", "set_variable", "stack_trace"] {
            assert!(DebugTool::requires_live_program(action), "{action} needs a live program");
        }
    }
}
