//! Other servers — TypeScript, Python, Go and the rest answer the same navigation questions.

use super::*;

#[tokio::test]
#[ignore = "spawns real typescript-language-server; run with --run-ignored"]
async fn real_ts_diagnostics_and_symbols() {
    if !have("typescript-language-server") {
        eprintln!("typescript-language-server not found; skipping");
        return;
    }
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("package.json"),
        "{\n  \"name\": \"t\",\n  \"version\": \"1.0.0\"\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("tsconfig.json"),
        "{\n  \"compilerOptions\": { \"strict\": true }\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("index.ts"),
        "export function bad(): number {\n  return \"not a number\";\n}\n\nexport class Widget {\n  size = 1;\n  area(): number { return this.size; }\n}\n",
    )
    .unwrap();

    let service = Arc::new(WorkspaceLspClient::new());
    let diag = LspDiagnosticsTool::new(service);
    let cx = ctx(&dir);

    // Which server answers depends on the host, and all three outcomes are Luxe working. A
    // TypeScript 5.x tsserver pushes the type error; TypeScript 7's native LSP pushes nothing at
    // all and has to be *asked* for it; a host with neither gets a refusal naming the servers that
    // would not run. The one result that is a defect is a bare "No diagnostics", which says the
    // file is fine — and that is what shipped for every `.ts` file against TypeScript 7.
    let deadline = Instant::now() + Duration::from_secs(40);
    let out = poll_until(deadline, || async {
        let (failed, out) = answer(
            &diag.run(serde_json::json!({ "file": "index.ts", "waitMs": 12000 }), &cx).await,
        );
        let explained = out.contains("note: language server reported")
            || declined(failed, &out, "typescript-language-server");
        accept(out.contains("ERROR") || explained, out)
    })
    .await
    .unwrap_or_else(|last| {
        panic!("luxe must surface the type error, or say why it cannot — got:\n{last}")
    });
    eprintln!("TS DIAG:\n{out}");
    if !out.contains("ERROR") {
        eprintln!("no usable TypeScript server on this host, and Luxe said so — nothing to check");
    }
}

#[tokio::test]
#[ignore = "spawns real pyright-langserver; run with --run-ignored"]
async fn real_pyright_diagnostics_and_symbols() {
    if !have("pyright-langserver") {
        eprintln!("pyright-langserver not found; skipping");
        return;
    }
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("pyproject.toml"), "[tool.pyright]\ntypeCheckingMode = \"basic\"\n")
        .unwrap();
    std::fs::write(
        dir.join("app.py"),
        "def bad() -> int:\n    return \"not a number\"\n\n\nclass Widget:\n    def area(self) -> int:\n        return 1\n",
    )
    .unwrap();

    let service = Arc::new(WorkspaceLspClient::new());
    let diag = LspDiagnosticsTool::new(service.clone());
    let nav = LspNavigationTool::new(service);
    let cx = ctx(&dir);

    let deadline = Instant::now() + Duration::from_secs(60);
    let reported = poll_until(deadline, || async {
        let (failed, out) =
            answer(&diag.run(serde_json::json!({ "file": "app.py", "waitMs": 12000 }), &cx).await);
        let declined = declined(failed, &out, "pyright-langserver");
        accept(out.contains("ERROR") || declined, out)
    })
    .await
    .unwrap_or_else(|last| {
        panic!("pyright should report the return-type error, or say why it cannot — got:\n{last}")
    });
    eprintln!("PY DIAG:\n{reported}");
    if !reported.contains("ERROR") {
        eprintln!("pyright cannot run on this host, and Luxe said so — skipping the rest");
        return;
    }

    let symbols = text(
        &nav.run(serde_json::json!({ "action": "document_symbols", "file": "app.py" }), &cx).await,
    );
    eprintln!("PY SYMBOLS:\n{symbols}");
    assert!(symbols.contains("bad") && symbols.contains("Widget"), "symbols listed: {symbols}");
}

#[tokio::test]
#[ignore = "spawns real gopls; run with --run-ignored"]
async fn real_gopls_diagnostics_and_symbols() {
    if !have("gopls") {
        eprintln!("gopls not found; skipping");
        return;
    }
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("go.mod"), "module gofixture\n\ngo 1.21\n").unwrap();
    std::fs::write(
        dir.join("main.go"),
        "package main\n\nfunc bad() int {\n\treturn \"not a number\"\n}\n\nfunc Area(size int) int {\n\treturn size * size\n}\n\nfunc main() {\n\t_ = bad()\n\t_ = Area(2)\n}\n",
    )
    .unwrap();

    let service = Arc::new(WorkspaceLspClient::new());
    let diag = LspDiagnosticsTool::new(service.clone());
    let nav = LspNavigationTool::new(service);
    let cx = ctx(&dir);

    let deadline = Instant::now() + Duration::from_secs(90);
    let reported = poll_until(deadline, || async {
        let (failed, out) =
            answer(&diag.run(serde_json::json!({ "file": "main.go", "waitMs": 15000 }), &cx).await);
        let declined = declined(failed, &out, "gopls");
        accept(out.contains("ERROR") || declined, out)
    })
    .await
    .unwrap_or_else(|last| {
        panic!("gopls should report the return-type error, or say why it cannot — got:\n{last}")
    });
    eprintln!("GO DIAG:\n{reported}");
    if !reported.contains("ERROR") {
        eprintln!("gopls cannot run on this host, and Luxe said so — skipping the rest");
        return;
    }

    let symbols = text(
        &nav.run(serde_json::json!({ "action": "document_symbols", "file": "main.go" }), &cx).await,
    );
    eprintln!("GO SYMBOLS:\n{symbols}");
    assert!(symbols.contains("bad") && symbols.contains("Area"), "symbols listed: {symbols}");
}

/// Navigation, not just diagnostics, on the servers that are not rust-analyzer.
///
/// Every other language was covered by "does it report an error and list symbols", which exercises
/// `publishDiagnostics` and `documentSymbol` and nothing else. Definition, references and hover go
/// through different requests, different response shapes, and the position translation that turns
/// a 1-based line into a UTF-16 offset — so a server that disagreed about any of those went
/// unnoticed on every language but Rust.
///
/// One test per language, deliberately: five servers sharing one test share its deadline, and the
/// last of them starves. A failure should name the language rather than the batch.
struct Case {
    server: &'static str,
    files: &'static [(&'static str, &'static str)],
    file: &'static str,
    /// A call site of the symbol, 1-based, as `(line, character)`.
    ///
    /// Definition is asked from here rather than from the symbol's own name: servers disagree
    /// about answering "go to definition" when you are already standing on it (clangd returns
    /// nothing, tsserver returns the definition), and that disagreement is not Luxe's to assert.
    /// A call site is the question every server answers the same way — and it exercises the
    /// line/character path, which the `symbol` shorthand does not.
    call_site: (u32, u32),
    /// A word the hover text must contain, in that language's own vocabulary.
    hover_says: &'static str,
}

/// Drive one language's server through definition, references and hover.
///
/// The contract asserted is Luxe's, not the server's: **navigate, or say why not.** A server that
/// cannot answer is a real situation — a `typescript-language-server` with no usable `typescript`
/// to drive answers everything with nothing, and a version-manager shim resolves on `PATH` and
/// then refuses to run at all — and the failure that matters is Luxe passing either off as "no
/// definition exists". Any of the three is a pass; a bare empty answer is not.
async fn navigates(case: Case) {
    if !have(case.server) {
        eprintln!("{} not found; skipping", case.server);
        return;
    }
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    for (name, body) in case.files {
        std::fs::write(dir.join(name), body).unwrap();
    }
    let service = Arc::new(WorkspaceLspClient::new());
    let nav = LspNavigationTool::new(service);
    let cx = ctx(&dir);
    let deadline = Instant::now() + Duration::from_secs(45);
    let at = serde_json::json!({
        "file": case.file, "line": case.call_site.0, "character": case.call_site.1
    });
    let ask = |action: &'static str| {
        let (nav, cx, at) = (&nav, &cx, at.clone());
        async move {
            let mut args = at;
            args["action"] = serde_json::Value::String(action.to_string());
            answer(&nav.run(args, cx).await)
        }
    };

    let definition = poll_until(deadline, || async {
        let (failed, out) = ask("go_to_definition").await;
        // `Definition:` and nothing else — the note quotes the file's own URI, so matching the
        // filename would match the failure too.
        let answered = out.starts_with("Definition:");
        let explained =
            out.contains("note: language server reported") || declined(failed, &out, case.server);
        accept(answered || explained, out)
    })
    .await
    .unwrap_or_else(|last| {
        panic!("{}: go_to_definition must resolve or say why not — got:\n{last}", case.server)
    });
    eprintln!("{} DEFINITION: {}", case.server, definition.trim());
    if !definition.starts_with("Definition:") {
        eprintln!("{}: cannot navigate on this host, and said so — skipping the rest", case.server);
        return;
    }

    let (_, references) = ask("find_references").await;
    eprintln!("{} REFERENCES:\n{}", case.server, references.trim());
    assert!(!references.starts_with("Error:"), "{}: {references}", case.server);
    assert!(
        references.matches(case.file).count() >= 2,
        "{}: two call sites plus the definition: {references}",
        case.server
    );

    let (_, hover) = ask("hover").await;
    eprintln!("{} HOVER: {}", case.server, hover.trim());
    assert!(hover.contains(case.hover_says), "{}: {hover}", case.server);
}

#[tokio::test]
#[ignore = "spawns a real typescript-language-server; run with --run-ignored"]
async fn real_ts_navigates() {
    navigates(Case {
        server: "typescript-language-server",
        files: &[
            ("package.json", "{\"name\":\"t\",\"version\":\"1.0.0\"}\n"),
            ("tsconfig.json", "{\"compilerOptions\":{\"strict\":true}}\n"),
            ("index.ts", "export function helper(): number {\n  return 7;\n}\n\nexport function caller(): number {\n  return helper() + helper();\n}\n"),
        ],
        file: "index.ts",
        call_site: (6, 10),
        hover_says: "number",
    })
    .await;
}

#[tokio::test]
#[ignore = "spawns a real pyright; run with --run-ignored"]
async fn real_pyright_navigates() {
    navigates(Case {
        server: "pyright-langserver",
        files: &[
            ("pyproject.toml", "[tool.pyright]\ntypeCheckingMode = \"basic\"\n"),
            ("app.py", "def helper() -> int:\n    return 7\n\n\ndef caller() -> int:\n    return helper() + helper()\n"),
        ],
        file: "app.py",
        call_site: (6, 12),
        hover_says: "int",
    })
    .await;
}

#[tokio::test]
#[ignore = "spawns a real gopls; run with --run-ignored"]
async fn real_gopls_navigates() {
    navigates(Case {
        server: "gopls",
        files: &[
            ("go.mod", "module nav\n\ngo 1.21\n"),
            ("main.go", "package main\n\nfunc helper() int {\n\treturn 7\n}\n\nfunc caller() int {\n\treturn helper() + helper()\n}\n"),
        ],
        file: "main.go",
        call_site: (8, 10),
        hover_says: "int",
    })
    .await;
}

#[tokio::test]
#[ignore = "spawns a real bash-language-server; run with --run-ignored"]
async fn real_bash_navigates() {
    navigates(Case {
        server: "bash-language-server",
        files: &[
            (".git", ""),
            ("script.sh", "#!/usr/bin/env bash\n\nhelper() {\n  echo 7\n}\n\ncaller() {\n  helper\n  helper\n}\n"),
        ],
        file: "script.sh",
        call_site: (8, 4),
        hover_says: "helper",
    })
    .await;
}

#[tokio::test]
#[ignore = "spawns a real clangd; run with --run-ignored"]
async fn real_clangd_navigates() {
    navigates(Case {
        server: "clangd",
        files: &[
            ("compile_flags.txt", "-std=c11\n"),
            ("main.c", "int helper(void) {\n    return 7;\n}\n\nint caller(void) {\n    return helper() + helper();\n}\n"),
        ],
        file: "main.c",
        call_site: (6, 12),
        hover_says: "int",
    })
    .await;
}

/// The config languages: a malformed file has to be reported like any other error.
///
/// JSON and YAML carry the settings an agent edits most often and gets subtly wrong — a trailing
/// comma, a tab where the format forbids one. Both servers are npm packages and neither is
/// interesting beyond "does the registry entry name it correctly and does the error arrive", which
/// is exactly what an unverified entry cannot promise.
#[tokio::test]
#[ignore = "spawns real json/yaml language servers; run with --run-ignored"]
async fn real_config_language_servers_report_malformed_files() {
    let cases: &[(&str, &str, &str)] = &[
        ("vscode-json-language-server", "settings.json", "{\n  \"a\": 1,\n}\n"),
        ("yaml-language-server", "config.yaml", "root:\n  good: 1\n\tbad: 2\n"),
    ];
    for (server, file, body) in cases {
        if !have(server) {
            eprintln!("{server} not found; skipping");
            continue;
        }
        let dir_guard = tempfile::tempdir().expect("temp dir");
        let dir = dir_guard.path().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".git"), "").unwrap();
        std::fs::write(dir.join(file), body).unwrap();

        let service = Arc::new(WorkspaceLspClient::new());
        let diag = LspDiagnosticsTool::new(service);
        let cx = ctx(&dir);
        let deadline = Instant::now() + Duration::from_secs(45);
        let reported = poll_until(deadline, || async {
            let (failed, out) =
                answer(&diag.run(serde_json::json!({ "file": file, "waitMs": 8000 }), &cx).await);
            let found = out.contains("ERROR") || out.contains("WARN");
            accept(found || declined(failed, &out, server), out)
        })
        .await
        .unwrap_or_else(|last| {
            panic!(
                "{server} should report the malformed {file}, or say why it cannot — got:\n{last}"
            )
        });
        eprintln!("{server} DIAG:\n{}", reported.trim());
    }
}

/// Ruby and Lua: diagnostics and navigation, like every other entry in the registry.
///
/// Added last and verified the same way as the rest — a registry entry nobody has watched answer a
/// real request is a claim, not support.
#[tokio::test]
#[ignore = "spawns a real ruby-lsp; run with --run-ignored"]
async fn real_ruby_navigates() {
    navigates(Case {
        server: "ruby-lsp",
        files: &[
            ("Gemfile", "source 'https://rubygems.org'\n"),
            ("app.rb", "def helper\n  7\nend\n\ndef caller_method\n  helper + helper\nend\n"),
        ],
        file: "app.rb",
        call_site: (6, 3),
        hover_says: "helper",
    })
    .await;
}

#[tokio::test]
#[ignore = "spawns a real lua-language-server; run with --run-ignored"]
async fn real_lua_navigates() {
    navigates(Case {
        server: "lua-language-server",
        files: &[
            (".luarc.json", "{\n  \"runtime.version\": \"Lua 5.4\"\n}\n"),
            (
                "main.lua",
                "local function helper()\n  return 7\nend\n\nlocal function caller()\n  return helper() + helper()\nend\n\nreturn caller\n",
            ),
        ],
        file: "main.lua",
        call_site: (6, 11),
        hover_says: "helper",
    })
    .await;
}

/// Svelte: a component is two languages in one file, and only the server knows which is which.
///
/// The reason this needs its own case rather than riding on TypeScript's: `svelteserver` does not
/// answer about the file on disk. It runs the component through `svelte2tsx` and asks a TypeScript
/// service about the *generated* module, then maps every position back — so a definition, a
/// reference and a hover each make a round trip through a source map that the `.ts` path never
/// touches. A registry entry that merely names the binary proves none of that survived.
#[tokio::test]
#[ignore = "spawns a real svelteserver; run with --run-ignored"]
async fn real_svelte_navigates() {
    navigates(Case {
        server: "svelteserver",
        files: &[
            ("package.json", "{\"name\":\"s\",\"version\":\"1.0.0\",\"type\":\"module\"}\n"),
            ("svelte.config.js", "export default {};\n"),
            (
                "App.svelte",
                "<script lang=\"ts\">\n  function helper(): number {\n    return 7;\n  }\n\n  function caller(): number {\n    return helper() + helper();\n  }\n</script>\n\n<p>{caller()}</p>\n",
            ),
        ],
        file: "App.svelte",
        call_site: (7, 12),
        hover_says: "number",
    })
    .await;
}

/// The type error inside a component's `<script>` reaches the model.
///
/// Separate from navigation because it is the failure that matters most and the one a source map
/// most easily loses: a diagnostic reported against the generated module lands on the wrong line of
/// the component, or on no line at all. A wrong line is worse than none — the model edits what the
/// number pointed at.
#[tokio::test]
#[ignore = "spawns a real svelteserver; run with --run-ignored"]
async fn real_svelte_diagnostics_land_on_the_component_line() {
    if !have("svelteserver") {
        eprintln!("svelteserver not found; skipping");
        return;
    }
    let dir_guard = tempfile::tempdir().expect("temp dir");
    let dir = dir_guard.path().to_path_buf();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("package.json"), "{\"name\":\"s\",\"version\":\"1.0.0\"}\n").unwrap();
    std::fs::write(dir.join("svelte.config.js"), "export default {};\n").unwrap();
    // The bad return is on line 3 of the component, and nowhere near line 3 of what `svelte2tsx`
    // hands the TypeScript service.
    std::fs::write(
        dir.join("Bad.svelte"),
        "<script lang=\"ts\">\n  function bad(): number {\n    return \"not a number\";\n  }\n</script>\n\n<p>{bad()}</p>\n",
    )
    .unwrap();

    let service = Arc::new(WorkspaceLspClient::new());
    let diag = LspDiagnosticsTool::new(service);
    let cx = ctx(&dir);

    let deadline = Instant::now() + Duration::from_secs(60);
    let out = poll_until(deadline, || async {
        let (failed, out) = answer(
            &diag.run(serde_json::json!({ "file": "Bad.svelte", "waitMs": 15000 }), &cx).await,
        );
        let explained = out.contains("note: language server reported")
            || declined(failed, &out, "svelteserver");
        accept(out.contains("ERROR") || explained, out)
    })
    .await
    .unwrap_or_else(|last| {
        panic!("luxe must surface the component's type error, or say why it cannot — got:\n{last}")
    });
    eprintln!("SVELTE DIAG:\n{out}");
    if out.contains("ERROR") {
        assert!(
            out.contains("] 3:"),
            "the error belongs to line 3 of the component, not of the generated module:\n{out}"
        );
    } else {
        eprintln!("no usable Svelte server on this host, and Luxe said so — nothing to check");
    }
}
