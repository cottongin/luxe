//! Real-server enrichment check (design-code-graph.md §6, G6b).
//!
//! Validates `graph::verify_callers` against a **real** `rust-analyzer` (not a stub): build the
//! code graph over a tiny Cargo project, then confirm the LSP's `textDocument/references` agrees
//! that the heuristic caller is a real caller. `#[ignore]` because it spawns rust-analyzer and
//! waits for it to index — run it explicitly:
//!
//! ```text
//! cargo nextest run -p luxe-lsp --run-ignored all -E 'test(enrich_verifies_caller_with_real_rust_analyzer)' --no-capture
//! ```

use luxe_lsp::graph::{build_graph, verify_callers, LspReferenceSource, QualifiedName};
use luxe_lsp::WorkspaceLspClient;
use std::sync::Arc;
use std::time::{Duration, Instant};

const AUTH_RS: &str = "pub fn login() -> bool {\n    true\n}\n";
const SESSION_RS: &str = "use crate::auth::login;\n\npub fn start() -> bool {\n    login()\n}\n";
const LIB_RS: &str = "pub mod auth;\npub mod session;\n";

#[tokio::test]
#[ignore = "spawns real rust-analyzer; run explicitly with --run-ignored"]
async fn enrich_verifies_caller_with_real_rust_analyzer() {
    // Skip cleanly if the real server isn't installed.
    if !std::path::Path::new("/usr/bin/rust-analyzer").exists() && which_rust_analyzer().is_none() {
        eprintln!("rust-analyzer not found; skipping");
        return;
    }

    let dir_guard = tempfile::tempdir().expect("temp dir");

    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"enrich_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("src/lib.rs"), LIB_RS).unwrap();
    std::fs::write(dir.join("src/auth.rs"), AUTH_RS).unwrap();
    std::fs::write(dir.join("src/session.rs"), SESSION_RS).unwrap();

    // Heuristic graph: session::start calls auth::login.
    let graph = build_graph(vec![
        ("src/lib.rs".to_string(), LIB_RS.to_string()),
        ("src/auth.rs".to_string(), AUTH_RS.to_string()),
        ("src/session.rs".to_string(), SESSION_RS.to_string()),
    ]);
    let seed = QualifiedName::from_raw("src/auth.rs::login");

    let service = Arc::new(WorkspaceLspClient::new());
    let source = LspReferenceSource::new(service, &dir);

    // rust-analyzer indexes asynchronously; poll until references land (or give up).
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut confirmed = None;
    while Instant::now() < deadline {
        if let Some(callers) = verify_callers(&graph, &seed, &source).await {
            if let Some(session) = callers.iter().find(|c| c.file == "src/session.rs") {
                if session.verified() {
                    confirmed = Some(callers.clone());
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    let callers = confirmed.expect("rust-analyzer should confirm session.rs calls login");
    let session = callers.iter().find(|c| c.file == "src/session.rs").unwrap();
    eprintln!("ENRICH: {callers:?}");
    assert!(session.in_graph, "heuristic graph found the caller");
    assert!(session.verified(), "rust-analyzer verified the caller");
}

fn which_rust_analyzer() -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join("rust-analyzer"))
            .find(|candidate| candidate.exists())
    })
}
