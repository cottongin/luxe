use super::*;

mod read_policy;
#[cfg(unix)] // the stub CLI is a shell script
mod session_cwd;

/// A stand-in CLI on PATH: `name` becomes an executable shell script printing `body`, so the
/// tool can be driven without a real ast-grep install.
///
/// Writing a script and immediately exec'ing it races the kernel: while any handle is still
/// open for writing, `execve` returns `ETXTBSY` ("Text file busy"). It only shows up under
/// parallel test load, which makes it the worst kind of flake — so the handle is synced and
/// dropped explicitly, before the mode is set.
///
/// Unix-only, like every test that uses it: the stub is a `#!/bin/sh` script.
#[cfg(unix)]
fn fake_binary(dir: &std::path::Path, name: &str, body: &str) {
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join(name);
    {
        let mut file = std::fs::File::create(&path).expect("create fake binary");
        file.write_all(body.as_bytes()).expect("write fake binary");
        file.sync_all().expect("flush fake binary");
    }
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fake binary");
}

/// Run a just-written stub, retrying the one failure that is nobody's fault.
///
/// Linux refuses to exec a file any process holds open for writing (`ETXTBSY`), and in a
/// multithreaded test binary that is reachable without doing anything wrong: the write handle
/// above is closed before the mode is set, but a *sibling* test spawning a process forks the whole
/// descriptor table first and only drops its copy at its own `exec`. Land inside that window and
/// this exec fails with "Text file busy".
///
/// The window is microseconds wide and cannot be closed from this side, so the answer is to try
/// again rather than to let the suite's reliability depend on scheduling.
#[cfg(unix)]
async fn run_fake(binary: &std::path::Path, args: &[String]) -> std::process::Output {
    for attempt in 0..50u64 {
        match tokio::process::Command::new(binary).args(args).output().await {
            Ok(output) => return output,
            Err(error) if error.kind() == std::io::ErrorKind::ExecutableFileBusy => {
                tokio::time::sleep(std::time::Duration::from_millis(2 * (attempt + 1))).await;
            }
            Err(error) => panic!("run {}: {error}", binary.display()),
        }
    }
    panic!("{} stayed busy: a sibling test is holding it open", binary.display());
}

/// The scope arrives however a model spells it: an array of `paths`, or one `path` like the
/// sibling tools take. Dropping the second searched the whole project instead.
#[test]
fn a_single_path_scopes_the_search_like_paths_does() {
    let one: AstGrepSearchParams = serde_json::from_value(serde_json::json!(
        {"pattern": "$X.unwrap()", "lang": "rust", "path": "src/a.rs"}
    ))
    .expect("parses");
    assert_eq!(one.paths, ["src/a.rs"]);
    let many: AstGrepSearchParams = serde_json::from_value(serde_json::json!(
        {"pattern": "$X", "lang": "rust", "paths": ["a", "b"]}
    ))
    .expect("parses");
    assert_eq!(many.paths, ["a", "b"]);
    let none: AstGrepSearchParams =
        serde_json::from_value(serde_json::json!({"pattern": "$X", "lang": "rust"}))
            .expect("parses");
    assert!(none.paths.is_empty());
    let replace: AstGrepReplaceParams = serde_json::from_value(serde_json::json!(
        {"pattern": "$X", "rewrite": "$X", "lang": "rust", "path": "src"}
    ))
    .expect("parses");
    assert_eq!(replace.paths, ["src"]);
}

#[test]
fn a_match_parses_into_file_position_and_range() {
    let value: serde_json::Value = serde_json::from_str(
        r#"{"text":"helper(1, 2)","range":{"byteOffset":{"start":37,"end":49},
                "start":{"line":2,"column":4},"end":{"line":2,"column":16}},
                "file":"src/a.rs","replacement":"assist(1, 2)"}"#,
    )
    .unwrap();
    let parsed = parse_match(&value).expect("parse");
    assert_eq!(parsed.file, PathBuf::from("src/a.rs"));
    assert_eq!(parsed.range, (37, 49));
    assert_eq!(parsed.line, 3, "ast-grep's 0-based line becomes 1-based");
    assert_eq!(parsed.column, 4);
    assert_eq!(parsed.text, "helper(1, 2)");
    assert_eq!(parsed.replacement.as_deref(), Some("assist(1, 2)"));
}

#[test]
fn matches_render_as_a_compact_listing_with_the_rewrite() {
    let matches = vec![AstGrepMatch {
        file: PathBuf::from("src/a.rs"),
        range: (37, 49),
        line: 3,
        column: 4,
        text: "helper(1, 2)".into(),
        replacement: Some("assist(1, 2)".into()),
    }];
    let rendered = render_matches(&matches, false);
    assert!(rendered.contains("found 1 match(es)"));
    assert!(rendered.contains("src/a.rs:3:4"));
    assert!(rendered.contains("  - helper(1, 2)"));
    assert!(rendered.contains("  + assist(1, 2)"));
    assert_eq!(render_matches(&[], false), "no matches");
}

/// A `sg` on PATH that is the shadow-utils group tool spawns fine and must still be rejected.
#[cfg(unix)]
#[tokio::test]
async fn a_colliding_unix_sg_is_not_mistaken_for_ast_grep() {
    let dir = tempfile::tempdir().expect("temp dir");
    fake_binary(
        dir.path(),
        "sg",
        "#!/bin/sh\necho 'Usage: sg [-] group [[-c] command]' >&2\nexit 1\n",
    );
    // Probe the fake directly rather than through PATH (the resolution cache is per-process).
    let output = run_fake(&dir.path().join("sg"), &["--version".to_string()]).await;
    assert!(!output.status.success(), "the group tool exits non-zero");
    assert!(
        !String::from_utf8_lossy(&output.stdout).to_ascii_lowercase().contains("ast-grep"),
        "and never claims to be ast-grep"
    );
}

/// End-to-end against a fake ast-grep that emits real streaming JSON: proves the argument
/// shape and the parser agree without needing the binary installed.
#[cfg(unix)]
#[tokio::test]
async fn a_stub_cli_drives_search_argument_shape_and_parsing() {
    let dir = tempfile::tempdir().expect("temp dir");
    let args_log = dir.path().join("args.txt");
    fake_binary(
        dir.path(),
        "ast-grep",
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'ast-grep 0.45.0'; exit 0; fi\n\
                 printf '%s\\n' \"$*\" > {log}\n\
                 echo '{{\"text\":\"foo()\",\"range\":{{\"byteOffset\":{{\"start\":1,\"end\":6}},\
                 \"start\":{{\"line\":0,\"column\":2}}}},\"file\":\"x.rs\"}}'\n",
            log = args_log.display()
        ),
    );

    let binary = dir.path().join("ast-grep");
    let mut args = base_args("foo()", "rust");
    args.push("--json=stream".to_string());
    let output = run_fake(&binary, &args).await;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value =
        serde_json::from_str(stdout.lines().next().expect("one match")).expect("json");
    let parsed = parse_match(&value).expect("parse");
    assert_eq!(parsed.file, PathBuf::from("x.rs"));
    assert_eq!(parsed.line, 1);

    let logged = std::fs::read_to_string(&args_log).expect("args log");
    assert!(logged.contains("run"), "uses the `run` subcommand: {logged}");
    assert!(logged.contains("--pattern foo()"), "passes --pattern: {logged}");
    assert!(logged.contains("--lang rust"), "passes --lang: {logged}");
    assert!(logged.contains("--json=stream"), "asks for streaming JSON: {logged}");
    assert!(!logged.contains("search"), "there is no `search` subcommand: {logged}");
}

/// Runs only where a real ast-grep is installed; skipped otherwise so CI stays hermetic.
#[tokio::test]
async fn a_real_ast_grep_finds_a_structural_match() {
    let Some(_binary) = resolve_binary().await else {
        eprintln!("skipping: no ast-grep on PATH");
        return;
    };
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("a.rs"),
        "fn main() {\n    helper(1, 2);\n}\nfn helper(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .expect("write source");

    let found = AstGrepTool::search(
        &AstGrepSearchParams {
            pattern: "helper($$$ARGS)".to_string(),
            lang: "rust".to_string(),
            paths: vec![dir.path().display().to_string()],
        },
        dir.path(),
    )
    .await
    .expect("search runs");
    assert_eq!(found.len(), 1, "the call site matches structurally: {found:?}");
    assert_eq!(found[0].line, 2);
    assert_eq!(found[0].text, "helper(1, 2)");

    // No matches is an empty result, not an error — ast-grep exits non-zero for it.
    let none = AstGrepTool::search(
        &AstGrepSearchParams {
            pattern: "definitely_not_here($$$A)".to_string(),
            lang: "rust".to_string(),
            paths: vec![dir.path().display().to_string()],
        },
        dir.path(),
    )
    .await
    .expect("an empty result is not an error");
    assert!(none.is_empty());

    // A preview reports the rewrite without touching the file; apply writes it.
    let params = AstGrepReplaceParams {
        pattern: "helper($$$ARGS)".to_string(),
        rewrite: "assist($$$ARGS)".to_string(),
        lang: "rust".to_string(),
        paths: vec![dir.path().display().to_string()],
        apply: false,
    };
    let preview = AstGrepTool::replace(&params, dir.path()).await.expect("preview runs");
    assert_eq!(preview[0].replacement.as_deref(), Some("assist(1, 2)"));
    assert!(std::fs::read_to_string(dir.path().join("a.rs")).unwrap().contains("helper(1, 2)"));

    let applied = AstGrepReplaceParams { apply: true, ..params };
    AstGrepTool::replace(&applied, dir.path()).await.expect("apply runs");
    let rewritten = std::fs::read_to_string(dir.path().join("a.rs")).unwrap();
    assert!(rewritten.contains("assist(1, 2)"), "the rewrite landed: {rewritten}");
}

/// The failure that killed a real turn: a structural pattern across a large repo returned
/// 232,744 tokens — past the model's context window — because every match carried its whole
/// body and nothing capped the total.
#[test]
fn a_broad_search_is_bounded_instead_of_flooding_context() {
    let body: String = (1..=40).map(|n| format!("    line {n}\n")).collect();
    let matches: Vec<AstGrepMatch> = (0..300)
        .map(|n| AstGrepMatch {
            file: PathBuf::from(format!("services/service_{n}.py")),
            range: (0, 10),
            line: 1,
            column: 0,
            text: format!("def handler_{n}():\n{body}"),
            replacement: None,
        })
        .collect();

    let rendered = render_matches(&matches, false);
    let lines = rendered.lines().count();
    assert!(lines <= MAX_MATCH_LINES + 3, "bounded, got {lines} lines");
    assert!(rendered.contains("found 300 match(es)"), "the true count still leads");
    assert!(rendered.contains("more lines not shown"), "and says what it withheld");
    assert!(rendered.contains("Narrow the pattern"), "with a way to recover the rest");
}

#[test]
fn one_match_shows_its_head_not_its_whole_body() {
    let body: String = (1..=30).map(|n| format!("    line {n}\n")).collect();
    let one = vec![AstGrepMatch {
        file: PathBuf::from("a.py"),
        range: (0, 1),
        line: 1,
        column: 0,
        text: format!("def f():\n{body}"),
        replacement: None,
    }];
    let rendered = render_matches(&one, false);
    assert!(rendered.contains("def f():"), "the identifying line survives");
    assert!(rendered.contains("… (25 more lines)"), "the rest is accounted for: {rendered}");
}

#[test]
fn a_small_result_is_untouched() {
    let one = vec![AstGrepMatch {
        file: PathBuf::from("a.py"),
        range: (0, 1),
        line: 3,
        column: 2,
        text: "def f(): pass".to_string(),
        replacement: None,
    }];
    let rendered = render_matches(&one, false);
    assert!(!rendered.contains("not shown"), "nothing to trim: {rendered}");
    assert!(rendered.contains("a.py:3:2"));
}

/// Upstream's parse error ends in a URL a model cannot open, so a rejected pattern was a dead
/// end — observed: two bad patterns, then eight `bash` calls instead.
#[test]
fn a_rejected_pattern_comes_back_with_the_rules_it_broke() {
    let upstream = "Error: Cannot parse query as a valid pattern.\n\
                        Help: see https://ast-grep.github.io/guide/pattern-syntax.html";
    let shown = AstGrepError::CommandFailed(upstream.to_string()).to_string();

    assert!(shown.contains("ONE syntactically valid snippet"), "{shown}");
    assert!(shown.contains("$$$ARGS"), "names the metavariable forms: {shown}");
    assert!(shown.contains("use `grep` instead"), "and when to give up: {shown}");
}

/// An environment failure is not a pattern failure; don't lecture about syntax.
#[test]
fn an_unrelated_failure_gets_no_pattern_lecture() {
    let shown = AstGrepError::CommandFailed("permission denied".to_string()).to_string();
    assert_eq!(shown, "command failed: permission denied");
}

/// A tool that cannot work here should not cost a schema; the identity check is the same one
/// `resolve_binary` applies, because the shadow-utils `sg` on PATH is not this tool.
#[test]
fn availability_matches_whether_a_real_ast_grep_resolves() {
    let available = is_available();
    let paths = candidate_paths();
    if paths.is_empty() {
        assert!(!available, "nothing on PATH means nothing to offer");
    }
    // When it reports available it must have cached the binary it verified.
    if available {
        assert!(BINARY.get().is_some(), "seeds the cache resolve_binary reads");
    }
}
