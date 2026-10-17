//! Every parameter a tool advertises has to change what the tool does.
//!
//! A tool's schema is a promise to the model, and the model reads it on every request. A parameter
//! that is declared and then dropped somewhere down the call chain does not fail — it answers a
//! different question, confidently, and neither the model nor the person reading the transcript
//! can tell. `web_search` shipped that way: it advertised `queries`, `domains`, `recency` and
//! `content` while passing only the query and a count to any keyed backend, so configuring an API
//! key silently *downgraded* the tool.
//!
//! The check is behavioral rather than structural, because the structural version passes. Every one
//! of those four names appeared in `web_search`'s own module; they just never reached the vendor. So
//! each case here runs the tool twice against the same fixture — once with the parameter, once
//! without — and asserts the two answers differ. Nothing else can tell the difference between a
//! parameter that is read and one that is honored.
//!
//! Not exhaustive by construction: a parameter needs an input where its presence is observable, and
//! writing that input is the work. What it does guarantee is that no *covered* parameter can quietly
//! stop mattering.

#![allow(clippy::unwrap_used)]

use luxe_core::tool::{Tool, ToolContext, ToolResultData};
use luxe_tools::{edit, glob, grep, read, todo, write};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::{mpsc, watch};

fn context(cwd: &std::path::Path) -> ToolContext {
    let (event_tx, _event_rx) = mpsc::channel(64);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    ToolContext {
        cwd: cwd.to_path_buf(),
        session_id: "schema-audit".to_string(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    }
}

/// The text a tool returned, so two runs can be compared.
fn text(result: &ToolResultData) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            luxe_core::ContentBlock::Text(text) => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A small tree with enough shape for a filter to have something to remove.
fn fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::create_dir_all(root.join("nested/deep")).unwrap();
    std::fs::write(root.join("src/alpha.rs"), "fn alpha() {}\n// needle here\nfn beta() {}\n")
        .unwrap();
    std::fs::write(root.join("src/beta.rs"), "fn gamma() {}\n// needle again\n").unwrap();
    std::fs::write(root.join("docs/notes.md"), "needle in prose\n").unwrap();
    std::fs::write(root.join("nested/deep/buried.rs"), "// needle buried\n").unwrap();
    std::fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
    std::fs::create_dir_all(root.join("ignored")).unwrap();
    std::fs::write(root.join("ignored/hidden.rs"), "// needle hidden\n").unwrap();
    let numbered: String = (1..=40).map(|n| format!("line {n}\n")).collect();
    std::fs::write(root.join("numbered.txt"), numbered).unwrap();
    dir
}

/// Run `tool` with `args` and return its text.
async fn run(tool: &dyn Tool, cwd: &std::path::Path, args: serde_json::Value) -> String {
    text(&tool.run(args, &context(cwd)).await)
}

/// `read`: `offset` and `limit` each have to move the window.
#[tokio::test]
async fn read_honors_its_window() {
    let dir = fixture();
    let root = dir.path();
    let tool = read::ReadTool::new();

    let whole = run(&tool, root, serde_json::json!({"path": "numbered.txt"})).await;
    let from_ten =
        run(&tool, root, serde_json::json!({"path": "numbered.txt", "offset": 10})).await;
    let first_three =
        run(&tool, root, serde_json::json!({"path": "numbered.txt", "limit": 3})).await;

    assert!(whole.contains("line 1\n") || whole.contains("line 1"), "{whole}");
    assert_ne!(whole, from_ten, "offset must move the window");
    assert_ne!(whole, first_three, "limit must bound the window");
    assert!(!from_ten.contains("line 1\n"), "offset 10 must not start at line 1: {from_ten}");
    assert!(first_three.contains("line 3"), "{first_three}");
    assert!(!first_three.contains("line 4"), "limit 3 must stop at three lines: {first_three}");
}

/// `grep`: `path`, `glob`, `context` and `all` each have to change the result.
#[tokio::test]
async fn grep_honors_every_filter_it_advertises() {
    let dir = fixture();
    let root = dir.path();
    let tool = grep::GrepTool::new();

    let everywhere = run(&tool, root, serde_json::json!({"pattern": "needle"})).await;
    let scoped = run(&tool, root, serde_json::json!({"pattern": "needle", "path": "docs"})).await;
    let globbed = run(&tool, root, serde_json::json!({"pattern": "needle", "glob": "*.md"})).await;
    let with_context =
        run(&tool, root, serde_json::json!({"pattern": "needle", "context": 2})).await;

    assert!(everywhere.contains("alpha.rs"), "the unfiltered search finds the tree: {everywhere}");
    assert!(!scoped.contains("alpha.rs"), "`path` must scope the search: {scoped}");
    assert!(scoped.contains("notes.md"), "{scoped}");
    assert!(!globbed.contains("alpha.rs"), "`glob` must filter by name: {globbed}");
    assert!(globbed.contains("notes.md"), "{globbed}");
    assert_ne!(everywhere, with_context, "`context` must add surrounding lines");
    assert!(with_context.contains("fn alpha"), "context 2 reaches the line above: {with_context}");
}

/// `grep` and `glob`'s `all`: the ignore rules are the default, and turning them off has to show.
#[tokio::test]
async fn the_all_flag_reaches_past_the_ignore_rules() {
    let dir = fixture();
    let root = dir.path();

    let respected =
        run(&grep::GrepTool::new(), root, serde_json::json!({"pattern": "needle"})).await;
    let everything =
        run(&grep::GrepTool::new(), root, serde_json::json!({"pattern": "needle", "all": true}))
            .await;
    assert!(!respected.contains("hidden.rs"), "gitignore is honored by default: {respected}");
    assert!(everything.contains("hidden.rs"), "`all` must reach an ignored file: {everything}");

    let globbed = run(&glob::GlobTool, root, serde_json::json!({"pattern": "**/*.rs"})).await;
    let globbed_all =
        run(&glob::GlobTool, root, serde_json::json!({"pattern": "**/*.rs", "all": true})).await;
    assert!(!globbed.contains("hidden.rs"), "{globbed}");
    assert!(globbed_all.contains("hidden.rs"), "{globbed_all}");
}

/// `glob`: `pattern` selects and `path` scopes.
#[tokio::test]
async fn glob_honors_its_pattern_and_root() {
    let dir = fixture();
    let root = dir.path();
    let tool = glob::GlobTool;

    let rust = run(&tool, root, serde_json::json!({"pattern": "**/*.rs"})).await;
    let markdown = run(&tool, root, serde_json::json!({"pattern": "**/*.md"})).await;
    let scoped = run(&tool, root, serde_json::json!({"pattern": "**/*.rs", "path": "src"})).await;

    assert!(rust.contains("alpha.rs"), "{rust}");
    assert!(!rust.contains("notes.md"), "the pattern must select: {rust}");
    assert!(markdown.contains("notes.md"), "{markdown}");
    assert!(scoped.contains("alpha.rs"), "{scoped}");
    assert!(!scoped.contains("buried.rs"), "`path` must scope the walk: {scoped}");
}

/// `edit`: `replaceAll` is the difference between changing one occurrence and every one, and the
/// tool *refuses* an ambiguous edit without it — so a dropped flag is a refusal, not a wrong answer.
#[tokio::test]
async fn edit_honors_replace_all() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let tool = edit::EditTool::new();
    let body = "let x = old;\nlet y = old;\nlet z = old;\n";

    // Without the flag, three occurrences are ambiguous and the edit is refused.
    std::fs::write(root.join("a.rs"), body).unwrap();
    let refused = run(
        &tool,
        root,
        serde_json::json!({
            "path": "a.rs",
            "edits": [{"oldText": "old", "newText": "new"}],
        }),
    )
    .await;
    assert_eq!(std::fs::read_to_string(root.join("a.rs")).unwrap(), body, "{refused}");

    // With it, all three change.
    run(
        &tool,
        root,
        serde_json::json!({
            "path": "a.rs",
            "edits": [{"oldText": "old", "newText": "new", "replaceAll": true}],
        }),
    )
    .await;
    let after = std::fs::read_to_string(root.join("a.rs")).unwrap();
    assert_eq!(after.matches("new").count(), 3, "replaceAll must change every occurrence: {after}");
    assert!(!after.contains("old"), "{after}");
}

/// `write`: both of its arguments, and the round trip through `read`.
#[tokio::test]
async fn write_puts_the_content_at_the_path() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    run(
        &write::WriteTool::new(),
        root,
        serde_json::json!({"path": "nested/made.txt", "content": "first\nsecond\n"}),
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(root.join("nested/made.txt")).unwrap(),
        "first\nsecond\n",
        "`content` must reach the file, at the `path` given"
    );

    let back =
        run(&read::ReadTool::new(), root, serde_json::json!({"path": "nested/made.txt"})).await;
    assert!(back.contains("second"), "{back}");
}

/// `todo`: the control plane's arguments are the ones a model gets wrong most, so each has to land.
#[tokio::test]
async fn todo_honors_the_fields_it_advertises() {
    let dir = TempDir::new().unwrap();
    let tool = todo::TodoTool::new(dir.path().join("todos"));
    let root = dir.path();

    run(&tool, root, serde_json::json!({"action": "add", "subject": "first"})).await;
    run(&tool, root, serde_json::json!({"action": "add", "subject": "second"})).await;
    let listed = run(&tool, root, serde_json::json!({"action": "list"})).await;
    assert!(listed.contains("first") && listed.contains("second"), "{listed}");

    // `status` has to move the task, and `list` has to reflect it.
    run(&tool, root, serde_json::json!({"action": "update", "id": 1, "status": "completed"})).await;
    let after = run(&tool, root, serde_json::json!({"action": "list"})).await;
    assert_ne!(listed, after, "a status update must be visible in the list");

    // `subjects` is the batch spelling of `subject`.
    let batch =
        run(&tool, root, serde_json::json!({"action": "add", "subjects": ["third", "fourth"]}))
            .await;
    let listed = run(&tool, root, serde_json::json!({"action": "list"})).await;
    assert!(listed.contains("third") && listed.contains("fourth"), "{batch}\n{listed}");

    // `complete` is in the schema's own `enum`, so the model will send it.
    let completed = run(&tool, root, serde_json::json!({"action": "complete", "id": 2})).await;
    let listed = run(&tool, root, serde_json::json!({"action": "list"})).await;
    assert!(
        !completed.starts_with("Unknown action"),
        "an action the schema advertises must exist: {completed}"
    );
    assert!(listed.contains("[x] #2"), "`complete` must complete the task: {listed}");
}

/// A truncation hint has to name something the caller can actually do.
///
/// `design-tool-surface.md` states the check: take the hint literally, as a caller with no other
/// context would, and ask whether it can be followed for *every* input that can trigger it. Both
/// tools that drop output promise an artifact file, and both wrote that promise before knowing
/// whether the file existed — `tee_artifact` returns `None` on a read-only tree or a full disk.
///
/// A directory that cannot be written to is how that is reproduced, so this runs on unix only:
/// the mode bits are the mechanism.
#[cfg(unix)]
#[tokio::test]
async fn a_dropped_output_never_promises_an_artifact_it_could_not_write() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().unwrap();
    let root = dir.path();
    // Read-only, so `.luxe/artifacts/` cannot be created.
    let mut mode = std::fs::metadata(root).unwrap().permissions();
    mode.set_mode(0o555);
    std::fs::set_permissions(root, mode).unwrap();

    // Enough lines that the cap fires and the hint is reached.
    let noisy = "for i in $(seq 1 400); do echo \"line $i\"; done";
    let out =
        run(&luxe_tools::bash::BashTool::new(), root, serde_json::json!({"command": noisy})).await;

    // Restore before the tempdir is cleaned up, or the drop leaks the directory.
    let mut mode = std::fs::metadata(root).unwrap().permissions();
    mode.set_mode(0o755);
    std::fs::set_permissions(root, mode).unwrap();

    assert!(!out.contains("Full output: "), "no artifact was written, so none may be named: {out}");
    if out.contains("not shown") || out.contains("compressed") {
        assert!(
            out.contains("could not be saved"),
            "output was dropped with no artifact and no explanation: {out}"
        );
    }
}

/// `screenshot`: the parameters that change the pixels have to change the pixels.
///
/// The least-covered tool in the tree by parameter count, and the one whose output nobody can
/// eyeball in CI. Skipped without a display, and `mode=url` skipped without a browser — but where
/// they exist, the image itself is the assertion: a `width`/`height` that does not reach the
/// browser produces a differently-sized PNG, and a `region` that does not reach the compositor
/// produces a full screen.
#[tokio::test]
async fn screenshot_honors_the_geometry_it_is_given() {
    let display =
        std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some();
    if !display {
        eprintln!("skipped: no display");
        return;
    }
    let dir = TempDir::new().unwrap();
    let tool = luxe_tools::screenshot::ScreenshotTool;

    // `mode=url` is the one that runs everywhere a browser does, and carries three of the nine
    // parameters. The PNG's own dimensions are the evidence.
    let browser = ["chromium", "google-chrome", "chrome", "microsoft-edge", "chromium-browser"]
        .iter()
        .any(|name| luxe_core::program::resolve(name).is_some());
    if !browser {
        eprintln!("skipped: no headless browser");
        return;
    }
    // Over loopback, not `file://`: the tool refuses a local path on purpose (a browser given one
    // can read anything this user can), and loopback is the origin it explicitly allows.
    let (port, _server) = serve_one_page("<html><body style='background:#0af'>hi</body></html>");
    let url = format!("http://127.0.0.1:{port}/");

    for (width, height) in [(400u32, 300u32), (900u32, 600u32)] {
        let result = tool
            .run(
                serde_json::json!({
                    "mode": "url", "url": url, "width": width, "height": height, "waitMs": 200
                }),
                &context(dir.path()),
            )
            .await;
        let png = result
            .content
            .iter()
            .find_map(|block| match block {
                luxe_core::ContentBlock::Image { data, .. } => Some(data.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("a capture returns an image: {}", text(&result)));
        let (got_w, got_h) = png_size(&png);
        assert_eq!(
            (got_w, got_h),
            (width, height),
            "width/height must reach the browser, not just the schema"
        );
    }
}

/// Serve one tiny page on a loopback port until the returned guard is dropped.
fn serve_one_page(body: &'static str) -> (u16, std::thread::JoinHandle<()>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let handle = std::thread::spawn(move || {
        for stream in listener.incoming().take(8) {
            let Ok(mut stream) = stream else { continue };
            let mut scratch = [0u8; 1024];
            let _ = stream.read(&mut scratch);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (port, handle)
}

#[cfg(unix)]
/// A word from the title of some window the compositor currently has, if it will say.
///
/// Hyprland only; elsewhere `window` mode resolves its target differently and there is nothing
/// portable to ask.
fn a_listed_window() -> Option<String> {
    let output = std::process::Command::new("hyprctl").args(["clients", "-j"]).output().ok()?;
    let clients: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    clients
        .as_array()?
        .iter()
        .filter_map(|client| client.get("title")?.as_str())
        .find_map(|title| title.split_whitespace().find(|word| word.len() > 4).map(str::to_string))
}

/// A base64 PNG's pixel dimensions, read straight out of the IHDR chunk.
fn png_size(base64_data: &str) -> (u32, u32) {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_data)
        .expect("the tool returns base64 PNG");
    assert_eq!(&bytes[1..4], b"PNG", "not a PNG");
    let at = |i: usize| u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
    (at(16), at(20))
}

/// `screenshot`'s compositor-backed modes, against the real display.
///
/// `fullscreen`, `region` and `window` all go through the OS's own capture tool, and none of them
/// had ever run in a test — the browser path was the only one reachable without a display. Each is
/// checked by the one thing that cannot be faked: the dimensions of the PNG that comes back.
#[cfg(unix)]
#[tokio::test]
async fn screenshot_captures_the_screen_the_way_each_mode_says() {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() && std::env::var_os("DISPLAY").is_none() {
        eprintln!("skipped: no display");
        return;
    }
    // The capture programs themselves; without one, this mode cannot run at all.
    let have_capture = ["grim", "maim", "scrot", "import", "screencapture"]
        .iter()
        .any(|name| luxe_core::program::resolve(name).is_some());
    if !have_capture {
        eprintln!("skipped: no screen-capture program");
        return;
    }
    let dir = TempDir::new().unwrap();
    let tool = luxe_tools::screenshot::ScreenshotTool;
    // A *desktop* capture runs only with somebody at the keyboard, and a broker is what says there
    // is one. This test stands in for an interactive session, which is the only place the
    // fullscreen/region/window modes are offered at all.
    let interactive = {
        let mut cx = context(dir.path());
        cx.question_broker = Some(Arc::new(luxe_core::question::QuestionBroker::new()));
        cx
    };

    let capture = |args: serde_json::Value| {
        let tool = &tool;
        let interactive = &interactive;
        async move {
            let result = tool.run(args, interactive).await;
            let image = result.content.iter().find_map(|block| match block {
                luxe_core::ContentBlock::Image { data, .. } => Some(data.clone()),
                _ => None,
            });
            (image, text(&result), result.is_error)
        }
    };

    let (full, summary, failed) = capture(serde_json::json!({"mode": "fullscreen"})).await;
    // A locked or sleeping screen draws no frames, so there is nothing to measure — the machine's
    // state, not the tool's. The tool saying so in its own words is the part this still checks.
    if failed && summary.contains("locked or asleep") {
        eprintln!("skipped: the screen is not drawing ({summary})");
        return;
    }
    assert!(!failed, "fullscreen: {summary}");
    let full = full.expect("fullscreen returns an image");
    let (screen_w, screen_h) = png_size(&full);
    assert!(screen_w > 0 && screen_h > 0, "a screen has size: {screen_w}x{screen_h}");
    eprintln!("fullscreen: {screen_w}x{screen_h}");

    // A region has to be *that* region, not the screen — the assertion the geometry exists for.
    let (width, height) = (200u32, 120u32);
    let (region, summary, failed) =
        capture(serde_json::json!({"mode": "region", "region": format!("0,0 {width}x{height}")}))
            .await;
    assert!(!failed, "region: {summary}");
    let region = region.expect("region returns an image");
    let (got_w, got_h) = png_size(&region);
    assert_eq!(
        (got_w, got_h),
        (width, height),
        "the region geometry must reach the compositor (screen is {screen_w}x{screen_h})"
    );

    // `window` needs a window to exist, so it is checked only where the compositor lists one.
    // Its geometry comes from the compositor rather than the arguments, so the assertion is that
    // it captured *a* window and not the whole screen.
    if let Some(title) = a_listed_window() {
        let (window, summary, failed) =
            capture(serde_json::json!({"mode": "window", "window": title})).await;
        if failed {
            eprintln!("window mode unavailable here: {summary}");
        } else {
            let window = window.expect("window mode returns an image");
            let (w, h) = png_size(&window);
            assert!(w > 0 && h > 0, "a window has size: {w}x{h}");
            eprintln!("window: {w}x{h}");
        }
    }

    // `note` is echoed back, and is the only parameter whose whole job is to come back out.
    let (_, summary, failed) =
        capture(serde_json::json!({"mode": "region", "region": "0,0 64x64", "note": "audit mark"}))
            .await;
    assert!(!failed, "note: {summary}");
    assert!(summary.contains("audit mark"), "`note` is echoed back: {summary}");
}
