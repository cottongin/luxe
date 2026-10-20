//! Real-server LSP validation (validation-backlog §A) against an installed `rust-analyzer`.
//!
//! These `#[ignore]` tests spawn the real server over a tiny Cargo project and confirm the paths
//! that were previously only stub-tested: diagnostics settle logic, `LocationLink` definitions,
//! hierarchical `documentSymbol` flattening, and call hierarchy. Run explicitly:
//!
//! ```text
//! cargo nextest run -p luxe-lsp --run-ignored all -E 'test(real_ra)' --no-capture
//! ```

use luxe_core::{ContentBlock, Tool, ToolContext, ToolResultData};
use luxe_lsp::{LspDiagnosticsTool, LspNavigationTool, WorkspaceLspClient};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

const CARGO_TOML: &str =
    "[package]\nname = \"real_ra_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const LIB_RS: &str = "pub mod bad;\npub mod good;\npub mod nav;\n";
const BAD_RS: &str = "pub fn bad() -> u32 {\n    \"not a number\"\n}\n";
const GOOD_RS: &str = "pub fn good() -> u32 {\n    42\n}\n";
// A definition + two call sites, plus a struct with a method (for documentSymbol nesting).
const NAV_RS: &str = "pub fn helper() -> u32 {\n    7\n}\n\npub fn caller() -> u32 {\n    helper() + helper()\n}\n\npub struct Widget {\n    pub size: u32,\n}\n\nimpl Widget {\n    pub fn area(&self) -> u32 {\n        self.size * self.size\n    }\n}\n";

fn ctx(cwd: &Path) -> ToolContext {
    let (event_tx, _rx) = mpsc::channel(8);
    let (_c, cancel_rx) = watch::channel(false);
    ToolContext {
        cwd: cwd.to_path_buf(),
        session_id: "real-ra".to_string(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    }
}

fn text(result: &ToolResultData) -> String {
    match &result.content[0] {
        ContentBlock::Text(t) => t.clone(),
        _ => panic!("expected text"),
    }
}

/// A tool result as the model receives it: its text, and whether luxe called the call a failure.
fn answer(result: &ToolResultData) -> (bool, String) {
    (result.is_error, text(result))
}

/// Whether luxe declined the request by naming the server that would not run.
///
/// A host is entitled to have no *working* server for a language even though one resolves on
/// `PATH` — a version-manager shim with no version set is exactly that, and [`have`] cannot see
/// it. luxe failing the call and naming the server, in the server's own words, is this contract
/// holding rather than breaking. What must never pass is the silent empty answer, which a reader
/// cannot tell apart from "your file is fine".
fn declined(failed: bool, out: &str, server: &str) -> bool {
    failed && out.contains(server)
}

/// Accept this answer, or hand it back so the failure can quote what actually arrived.
fn accept(good: bool, out: String) -> Result<String, String> {
    if good {
        Ok(out)
    } else {
        Err(out)
    }
}

fn have_rust_analyzer() -> bool {
    Path::new("/usr/bin/rust-analyzer").exists()
        || std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join("rust-analyzer").exists())
        })
}

/// A directory of this test's own, deleted when it is dropped; it reads as the path it holds.
struct Scratch(tempfile::TempDir);

impl std::ops::Deref for Scratch {
    type Target = std::path::Path;

    fn deref(&self) -> &Self::Target {
        self.0.path()
    }
}

/// Write the fixture Cargo project into a fresh temp dir and return its root.
fn fixture(tag: &str) -> Scratch {
    let scratch = Scratch(
        tempfile::Builder::new()
            .prefix(&format!("luxe-real-ra-{tag}-"))
            .tempdir()
            .expect("temp dir"),
    );
    let dir: &Path = &scratch;
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), CARGO_TOML).unwrap();
    std::fs::write(dir.join("src/lib.rs"), LIB_RS).unwrap();
    std::fs::write(dir.join("src/bad.rs"), BAD_RS).unwrap();
    std::fs::write(dir.join("src/good.rs"), GOOD_RS).unwrap();
    std::fs::write(dir.join("src/nav.rs"), NAV_RS).unwrap();
    scratch
}

/// Poll `f` until it accepts an answer, or the deadline passes.
///
/// The rejected answers are not thrown away — the last one comes back with the failure. These
/// tests spend up to ninety seconds before they give up, and "must surface a type error" on its
/// own says nothing about *what* arrived instead, which is the whole of the diagnosis.
async fn poll_until<F, Fut>(deadline: Instant, mut f: F) -> Result<String, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    let mut last = String::from("(the tool was never reached)");
    while Instant::now() < deadline {
        match f().await {
            Ok(value) => return Ok(value),
            Err(rejected) => last = rejected,
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    Err(last)
}

fn have(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(cmd).exists()))
}

mod other_servers;
mod rust_analyzer;
