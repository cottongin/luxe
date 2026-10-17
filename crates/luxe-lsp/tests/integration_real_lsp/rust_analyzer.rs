//! Rust-analyzer — diagnostics, navigation, rename and code actions against the real server.

use super::*;

#[tokio::test]
#[ignore = "spawns real rust-analyzer; run with --run-ignored"]
async fn real_ra_diagnostics_report_error_and_clean_is_empty() {
    if !have_rust_analyzer() {
        eprintln!("rust-analyzer not found; skipping");
        return;
    }
    let dir = fixture("diag");
    let service = Arc::new(WorkspaceLspClient::new());
    let tool = LspDiagnosticsTool::new(service);
    let cx = ctx(&dir);

    // The type-error file must eventually report a real error (not an empty pre-index snapshot).
    let deadline = Instant::now() + Duration::from_secs(90);
    let reported = poll_until(deadline, || async {
        let out = text(
            &tool.run(serde_json::json!({ "file": "src/bad.rs", "waitMs": 15000 }), &cx).await,
        );
        accept(out.contains("ERROR"), out)
    })
    .await
    .unwrap_or_else(|last| {
        panic!("rust-analyzer should report the type error in bad.rs — got:\n{last}")
    });
    eprintln!("DIAG(bad.rs):\n{reported}");
    assert!(reported.contains("ERROR"), "real error surfaced");
    assert!(
        reported.to_lowercase().contains("mismatch")
            || reported.to_lowercase().contains("expected"),
        "error mentions the type mismatch: {reported}"
    );

    // A clean file returns "No diagnostics" quickly — no 10s hang (server is warm now).
    let started = Instant::now();
    let clean =
        text(&tool.run(serde_json::json!({ "file": "src/good.rs", "waitMs": 10000 }), &cx).await);
    let elapsed = started.elapsed();
    eprintln!("DIAG(good.rs) in {elapsed:?}:\n{clean}");
    assert!(clean.contains("No diagnostics"), "clean file reports none: {clean}");
}

#[tokio::test]
#[ignore = "spawns real rust-analyzer; run with --run-ignored"]
async fn real_ra_go_to_definition_resolves_location_link() {
    if !have_rust_analyzer() {
        eprintln!("rust-analyzer not found; skipping");
        return;
    }
    let dir = fixture("def");
    let service = Arc::new(WorkspaceLspClient::new());
    let nav = LspNavigationTool::new(service);
    let cx = ctx(&dir);

    // `helper()` is called on line 6 (0-based 5); go to its definition on line 1 (0-based 0).
    let deadline = Instant::now() + Duration::from_secs(90);
    let resolved = poll_until(deadline, || async {
        let out = text(
            &nav.run(
                serde_json::json!({
                    "action": "go_to_definition",
                    "file": "src/nav.rs",
                    "line": 6,
                    "character": 5
                }),
                &cx,
            )
            .await,
        );
        // Resolves to nav.rs (the definition site), not an error/empty.
        accept(out.contains("nav.rs") && !out.to_lowercase().contains("no "), out)
    })
    .await
    .unwrap_or_else(|last| panic!("go_to_definition should resolve helper() — got:\n{last}"));
    eprintln!("DEF:\n{resolved}");
    assert!(resolved.contains("nav.rs"), "definition points into nav.rs: {resolved}");
}

#[tokio::test]
#[ignore = "spawns real rust-analyzer; run with --run-ignored"]
async fn real_ra_document_symbols_flatten_hierarchy() {
    if !have_rust_analyzer() {
        eprintln!("rust-analyzer not found; skipping");
        return;
    }
    let dir = fixture("docsym");
    let service = Arc::new(WorkspaceLspClient::new());
    let nav = LspNavigationTool::new(service);
    let cx = ctx(&dir);

    let deadline = Instant::now() + Duration::from_secs(90);
    let symbols = poll_until(deadline, || async {
        let out = text(
            &nav.run(
                serde_json::json!({ "action": "document_symbols", "file": "src/nav.rs" }),
                &cx,
            )
            .await,
        );
        // rust-analyzer returns a hierarchical DocumentSymbol; flattened output should carry the
        // struct, its method, and the free functions.
        accept(out.contains("Widget") && out.contains("area") && out.contains("helper"), out)
    })
    .await
    .unwrap_or_else(|last| {
        panic!("document_symbols should list Widget/area/helper — got:\n{last}")
    });
    eprintln!("DOCSYM:\n{symbols}");
    assert!(symbols.contains("Widget") && symbols.contains("area"), "nested method flattened");
}

#[tokio::test]
#[ignore = "spawns real rust-analyzer; run with --run-ignored"]
async fn real_ra_references_and_incoming_call_hierarchy() {
    if !have_rust_analyzer() {
        eprintln!("rust-analyzer not found; skipping");
        return;
    }
    let dir = fixture("refs");
    let service = Arc::new(WorkspaceLspClient::new());
    let nav = LspNavigationTool::new(service);
    let cx = ctx(&dir);

    // helper() is called twice by caller(); references should find both call sites.
    let deadline = Instant::now() + Duration::from_secs(90);
    let refs = poll_until(deadline, || async {
        let out = text(
            &nav.run(
                serde_json::json!({ "action": "find_references", "file": "src/nav.rs", "symbol": "helper" }),
                &cx,
            )
            .await,
        );
        // Two call sites in nav.rs (declaration excluded).
        accept(out.contains("nav.rs") && out.contains("2 "), out)
    })
    .await
    .unwrap_or_else(|last| {
        panic!("find_references should locate both helper() call sites — got:\n{last}")
    });
    eprintln!("REFS:\n{refs}");
    assert!(refs.contains("nav.rs"), "references in nav.rs: {refs}");

    // Incoming call hierarchy: helper is called by caller.
    let deadline = Instant::now() + Duration::from_secs(60);
    let calls = poll_until(deadline, || async {
        let out = text(
            &nav.run(
                serde_json::json!({ "action": "call_hierarchy", "file": "src/nav.rs", "symbol": "helper" }),
                &cx,
            )
            .await,
        );
        accept(out.contains("caller"), out)
    })
    .await
    .unwrap_or_else(|last| {
        panic!("call_hierarchy should report helper is called by caller — got:\n{last}")
    });
    eprintln!("CALLH:\n{calls}");
    assert!(calls.to_lowercase().contains("called by"), "incoming direction: {calls}");
    assert!(calls.contains("caller"), "caller listed: {calls}");
}

#[tokio::test]
#[ignore = "spawns real rust-analyzer; run with --run-ignored"]
async fn real_ra_rename_applies_across_files_with_utf16_offsets() {
    if !have_rust_analyzer() {
        eprintln!("rust-analyzer not found; skipping");
        return;
    }
    // Two-file rename with an astral emoji (surrogate pair) on the same line as a use site, to
    // stress UTF-16 -> byte offset conversion in apply_text_edits.
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), CARGO_TOML).unwrap();
    std::fs::write(dir.join("src/lib.rs"), "pub mod a;\npub mod b;\n").unwrap();
    std::fs::write(dir.join("src/a.rs"), "pub fn greet() -> &'static str {\n    \"hi\"\n}\n")
        .unwrap();
    let b_src = "use crate::a::greet;\n\npub fn run() -> String {\n    format!(\"\u{1F600} {}\", greet())\n}\n";
    std::fs::write(dir.join("src/b.rs"), b_src).unwrap();

    let service = Arc::new(WorkspaceLspClient::new());
    let nav = LspNavigationTool::new(service);
    let cx = ctx(&dir);

    // Poll a preview until rust-analyzer resolves the cross-file usages (edits in both files).
    let deadline = Instant::now() + Duration::from_secs(90);
    let preview_ok = poll_until(deadline, || async {
        let out = text(
            &nav.run(
                serde_json::json!({ "action": "rename", "file": "src/a.rs", "symbol": "greet", "newName": "salutation" }),
                &cx,
            )
            .await,
        );
        accept(out.contains("a.rs") && out.contains("b.rs"), out)
    })
    .await
    .unwrap_or_else(|last| panic!("rename preview should span a.rs and b.rs — got:\n{last}"));
    eprintln!("RENAME PREVIEW:\n{preview_ok}");

    // Apply it.
    let applied = text(
        &nav.run(
            serde_json::json!({ "action": "rename", "file": "src/a.rs", "symbol": "greet", "newName": "salutation", "apply": true }),
            &cx,
        )
        .await,
    );
    eprintln!("RENAME APPLIED:\n{applied}");
    assert!(applied.contains("Applied"), "apply reported: {applied}");

    let a = std::fs::read_to_string(dir.join("src/a.rs")).unwrap();
    let b = std::fs::read_to_string(dir.join("src/b.rs")).unwrap();
    eprintln!("A:\n{a}\nB:\n{b}");

    assert!(a.contains("pub fn salutation"), "definition renamed: {a}");
    assert!(!a.contains("greet"), "no stale greet in a.rs: {a}");
    assert!(b.contains("use crate::a::salutation"), "import renamed: {b}");
    assert!(b.contains("salutation()"), "call site renamed: {b}");
    assert!(!b.contains("greet"), "no stale greet in b.rs: {b}");
    // The emoji must survive intact — a UTF-16 offset bug would corrupt this line.
    assert!(b.contains("\u{1F600} {}"), "emoji line intact (UTF-16 offsets correct): {b}");
}

const TRAIT_RS: &str = "pub trait Greeter {\n    fn greet(&self) -> u32;\n}\n\npub struct En;\n\nimpl Greeter for En {\n    fn greet(&self) -> u32 {\n        1\n    }\n}\n\npub fn use_it() -> u32 {\n    let g: En = En;\n    g.greet()\n}\n";

#[tokio::test]
#[ignore = "spawns real rust-analyzer; run with --run-ignored"]
async fn real_ra_implementation_and_type_definition() {
    if !have_rust_analyzer() {
        eprintln!("rust-analyzer not found; skipping");
        return;
    }
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), CARGO_TOML).unwrap();
    std::fs::write(dir.join("src/lib.rs"), "pub mod t;\n").unwrap();
    std::fs::write(dir.join("src/t.rs"), TRAIT_RS).unwrap();

    let service = Arc::new(WorkspaceLspClient::new());
    let nav = LspNavigationTool::new(service);
    let cx = ctx(&dir);

    // implementation on the `Greeter` trait (line 1) resolves to the `impl ... for En` in t.rs.
    let deadline = Instant::now() + Duration::from_secs(90);
    let impls = poll_until(deadline, || async {
        let out = text(
            &nav.run(
                serde_json::json!({ "action": "implementation", "file": "src/t.rs", "line": 1, "character": 11 }),
                &cx,
            )
            .await,
        );
        accept(out.contains("t.rs") && !out.to_lowercase().contains("no "), out)
    })
    .await
    .unwrap_or_else(|last| {
        panic!("implementation should resolve to the impl block — got:\n{last}")
    });
    eprintln!("IMPL:\n{impls}");
    assert!(impls.contains("t.rs"), "impl located in t.rs: {impls}");

    // type_definition on `g` (line 14: `    let g: En = En;`) resolves to the `En` struct.
    let deadline = Instant::now() + Duration::from_secs(60);
    let type_def = poll_until(deadline, || async {
        let out = text(
            &nav.run(
                serde_json::json!({ "action": "type_definition", "file": "src/t.rs", "line": 14, "character": 9 }),
                &cx,
            )
            .await,
        );
        accept(out.contains("t.rs") && !out.to_lowercase().contains("no "), out)
    })
    .await
    .unwrap_or_else(|last| panic!("type_definition of g should resolve to En — got:\n{last}"));
    eprintln!("TYPEDEF:\n{type_def}");
    assert!(type_def.contains("t.rs"), "type def located in t.rs: {type_def}");
}

/// The four navigation actions nothing exercised, against a real server.
///
/// `hover`, `signature_help`, `workspace_symbol` and `code_action` were declared in the schema,
/// reachable by the model on every request, and covered by no test at any level — a third of this
/// tool's surface resting on the assumption that the same code path works for all twelve actions.
#[tokio::test]
#[ignore = "spawns real rust-analyzer; run with --run-ignored"]
async fn real_ra_hover_signature_help_and_workspace_symbol() {
    if !have_rust_analyzer() {
        eprintln!("rust-analyzer not found; skipping");
        return;
    }
    let root = fixture("readonly-actions");
    let cx = ctx(&root);
    let service = Arc::new(WorkspaceLspClient::new());
    let nav = LspNavigationTool::new(service);
    let deadline = Instant::now() + Duration::from_secs(90);

    let hover = poll_until(deadline, || async {
        let out = text(
            &nav.run(
                serde_json::json!({ "action": "hover", "file": "src/nav.rs", "symbol": "helper" }),
                &cx,
            )
            .await,
        );
        accept(out.contains("u32"), out)
    })
    .await
    .unwrap_or_else(|last| panic!("hover should describe helper's signature — got:\n{last}"));
    eprintln!("HOVER:\n{hover}");

    let symbols = poll_until(deadline, || async {
        let out = text(
            &nav.run(
                serde_json::json!({
                    "action": "workspace_symbol", "file": "src/nav.rs", "query": "helper"
                }),
                &cx,
            )
            .await,
        );
        accept(out.contains("helper"), out)
    })
    .await
    .unwrap_or_else(|last| panic!("workspace_symbol should find helper by name — got:\n{last}"));
    eprintln!("WORKSPACE SYMBOL:\n{symbols}");

    // Inside `helper() + helper()`, between the parens of the first call.
    let signature = text(
        &nav.run(
            serde_json::json!({
                "action": "signature_help", "file": "src/nav.rs", "line": 6, "character": 12
            }),
            &cx,
        )
        .await,
    );
    eprintln!("SIGNATURE HELP:\n{signature}");
    assert!(!signature.starts_with("Error:"), "signature_help must not error: {signature}");

    // Outgoing is the direction nothing covered; incoming is tested elsewhere.
    let outgoing = text(
        &nav.run(
            serde_json::json!({
                "action": "call_hierarchy", "file": "src/nav.rs", "symbol": "caller",
                "direction": "outgoing"
            }),
            &cx,
        )
        .await,
    );
    eprintln!("OUTGOING CALLS:\n{outgoing}");
    assert!(outgoing.contains("helper"), "caller's outgoing calls must include helper: {outgoing}");
}

/// `code_action` previews by default and writes only when asked, and `title` picks which one.
///
/// The write path of a tool that edits files on the model's behalf, and it had no test at all.
#[tokio::test]
#[ignore = "spawns real rust-analyzer; run with --run-ignored"]
async fn real_ra_code_action_previews_then_applies_the_one_named() {
    if !have_rust_analyzer() {
        eprintln!("rust-analyzer not found; skipping");
        return;
    }
    let root = fixture("code-action");
    // An unused import: every Rust server offers to remove it, under a stable title.
    std::fs::write(
        root.join("src/fix.rs"),
        "use std::collections::HashMap;\n\npub fn f() -> u32 {\n    1\n}\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "pub mod bad;\npub mod good;\npub mod nav;\npub mod fix;\n",
    )
    .unwrap();
    let cx = ctx(&root);
    let service = Arc::new(WorkspaceLspClient::new());
    let nav = LspNavigationTool::new(service);
    let deadline = Instant::now() + Duration::from_secs(90);
    let before = std::fs::read_to_string(root.join("src/fix.rs")).unwrap();

    let listed = poll_until(deadline, || async {
        let out = text(
            &nav.run(
                serde_json::json!({
                    "action": "code_action", "file": "src/fix.rs", "line": 1, "character": 5
                }),
                &cx,
            )
            .await,
        );
        accept(!out.starts_with("Error:") && out.to_lowercase().contains("remove"), out)
    })
    .await
    // No early-out: an empty list is exactly the failure this test exists for. Undeclared
    // `codeActionLiteralSupport` made rust-analyzer answer every request with zero actions, and a
    // "skip when nothing is offered" branch reported that as a pass — for 91 seconds, once per run.
    .unwrap_or_else(|last| {
        panic!(
            "rust-analyzer must offer a quickfix for an unused import; zero actions means the \
             client failed to declare codeActionLiteralSupport — got:\n{last}"
        )
    });
    eprintln!("CODE ACTIONS:\n{listed}");
    assert_eq!(
        std::fs::read_to_string(root.join("src/fix.rs")).unwrap(),
        before,
        "listing actions must not touch the file"
    );

    // Pick by title, which is the parameter that had no coverage at all.
    let title = listed
        .lines()
        .map(str::trim)
        .find(|line| line.to_lowercase().contains("remove"))
        .map(|line| line.split(" [").next().unwrap_or(line).trim().to_string())
        .expect("a titled action");
    let applied = text(
        &nav.run(
            serde_json::json!({
                "action": "code_action", "file": "src/fix.rs", "line": 1, "character": 5,
                "apply": true, "title": title
            }),
            &cx,
        )
        .await,
    );
    eprintln!("APPLIED ({title}):\n{applied}");
    let after = std::fs::read_to_string(root.join("src/fix.rs")).unwrap();
    assert_ne!(after, before, "apply:true must write the edit: {applied}");
    assert!(!after.contains("HashMap"), "the unused import should be gone:\n{after}");
}
