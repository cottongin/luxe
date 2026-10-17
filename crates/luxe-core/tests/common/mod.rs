//! Shared fixtures for the integration tests.
//!
//! Small on purpose: things that are genuinely the same question in several test binaries, where a
//! per-file copy is how one platform gets forgotten.
//!
//! Every file under `tests/` compiles as its own binary and gets its own copy of this module, so a
//! helper only one of them needs reads as dead code in all the others. Hence the allow — it is a
//! property of the layout, not a sign the helper is unused.
#![allow(dead_code)]

/// Run this test where no instruction file reaches it: an empty working directory, and an empty
/// config directory for the global ones.
///
/// The engine reads `AGENTS.md` from the process's working directory up to the top of its checkout,
/// which in a test is this repository; one line too many in the real file put it over the prompt's
/// cap and turned unrelated tests red with a startup notice. Safe because nextest gives each test its
/// own process. Keep the guard alive for the test's length.
pub(crate) fn outside_any_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("a scratch directory");
    std::env::set_current_dir(dir.path()).expect("enter it");
    std::env::set_var("LUXE_CONFIG_DIR", dir.path().join("config"));
    dir
}

/// Run this test as if no review gate were running around it.
///
/// A gate marks every acceptance command it spawns with the recursion guard, so a suite run by the
/// loop's gate inherits it, and every `ReviewGateHook` inside then skips its commands. A test of
/// the hook itself has to clear it, or it passes outside the loop and fails inside it. Safe because
/// nextest gives each test its own process, and these tests only ever run harmless commands.
pub(crate) fn outside_any_review_gate() {
    std::env::remove_var(luxe_core::hooks::review_gate::RECURSION_GUARD_ENV);
}

/// A command that succeeds and does nothing, spelled for this platform.
///
/// The review gate spawns its acceptance commands directly rather than through a shell, so `true` is
/// not a universal answer — it is a unix *program*, and Windows has no such file. A gate that cannot
/// spawn its command reports FAIL, which in the capstone tests looked like the loop refusing to
/// advance a phase rather than like a fixture naming a program that isn't there.
pub(crate) fn always_succeeds() -> Vec<String> {
    if cfg!(windows) {
        ["cmd", "/C", "exit", "0"].iter().map(|part| (*part).to_string()).collect()
    } else {
        vec!["true".to_string()]
    }
}

/// A stand-in for the `loop_control` tool, for tests in the crate that tool depends on.
///
/// `loop_control` lives in `luxe-tools`, which depends on `luxe-core` — so a core test cannot reach
/// the real one, and a loop test that needs to *claim a phase* has nothing to call. This does the
/// one thing that matters for the loop's wiring, and does it the same way: invoke
/// [`HookPoint::LoopControl`] with the directive, and hand the hooks' message back as the tool
/// result. Keeping those two behaviors in step with `luxe_tools::loop_control` is what makes a test
/// built on this evidence about the real path.
pub(crate) struct LoopControlStub;

#[async_trait::async_trait]
impl luxe_core::tool::Tool for LoopControlStub {
    fn name(&self) -> &str {
        "loop_control"
    }

    fn schema(&self) -> luxe_core::provider::ToolSchema {
        luxe_core::provider::ToolSchema {
            name: "loop_control".to_string(),
            description: "Report loop progress (test stand-in).".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "action": { "type": "string" } },
                "required": ["action"],
            }),
        }
    }

    async fn run(
        &self,
        args: serde_json::Value,
        cx: &luxe_core::tool::ToolContext,
    ) -> luxe_core::tool::ToolResultData {
        let action = args.get("action").and_then(|value| value.as_str()).unwrap_or("progress");
        let directive = match action {
            "complete" => luxe_core::hook::LoopControl::Complete,
            "blocked" => luxe_core::hook::LoopControl::Blocked { reason: "test".to_string() },
            _ => luxe_core::hook::LoopControl::Progress { phase: None },
        };
        let text = match &cx.hook_ctx {
            Some(hook_ctx) => {
                let outcome = hook_ctx
                    .hook_registry
                    .invoke(
                        luxe_core::hook::HookPoint::LoopControl,
                        &luxe_core::hook::HookEvent::LoopControl(directive),
                        hook_ctx,
                    )
                    .await;
                match outcome {
                    luxe_core::hook::HookOutcome::ContinueLoop { message } => message,
                    _ => format!("Loop control: {action}"),
                }
            }
            None => format!("Loop control: {action}"),
        };
        luxe_core::tool::ToolResultData {
            content: vec![luxe_core::ContentBlock::Text(text)],
            is_error: false,
        }
    }
}

/// A command that fails and does nothing, spelled for this platform.
///
/// The counterpart to [`always_succeeds`], and it has to be a real program for the same reason: a
/// gate that *cannot spawn* its command also reports FAIL, so a fixture naming a nonexistent binary
/// would pass a "the gate refused" test for entirely the wrong reason.
pub(crate) fn always_fails() -> Vec<String> {
    if cfg!(windows) {
        ["cmd", "/C", "exit", "1"].iter().map(|part| (*part).to_string()).collect()
    } else {
        vec!["false".to_string()]
    }
}
