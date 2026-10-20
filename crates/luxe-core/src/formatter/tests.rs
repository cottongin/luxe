use super::*;

/// rustfmt and gofmt always run; biome, ruff and taplo run only under the project's own config
/// for them, found beside the file or above it, but never above the project root.
#[test]
fn a_formatter_beyond_the_language_standard_runs_only_where_the_project_chose_it() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("app");
    std::fs::create_dir_all(project.join("web/src")).unwrap();
    let registry = FormatterRegistry::for_project(&project, false);
    let ts = project.join("web/src/a.ts");

    assert_eq!(registry.formatter_for(&project.join("src/main.rs")).as_deref(), Some("rust"));
    assert_eq!(registry.formatter_for(&project.join("main.go")).as_deref(), Some("go"));
    assert_eq!(registry.formatter_for(&ts), None, "no biome config, no biome");
    assert_eq!(registry.formatter_for(&project.join("a.py")), None);
    assert_eq!(registry.formatter_for(&project.join("Cargo.toml")), None);
    assert_eq!(registry.formatter_for(&project.join("a.yaml")), None, "nothing formats YAML");

    // a config above the project is someone else's
    std::fs::write(root.path().join("biome.json"), "{}").unwrap();
    assert_eq!(registry.formatter_for(&ts), None);
    // one in a package between the file and the root counts
    std::fs::write(project.join("web/biome.json"), "{}").unwrap();
    assert_eq!(registry.formatter_for(&ts).as_deref(), Some("typescript"));
    assert_eq!(registry.formatter_for(&project.join("a.js")), None, "but not beside it");

    // a shared pyproject.toml counts only when it configures ruff
    std::fs::write(project.join("pyproject.toml"), "[tool.black]\n").unwrap();
    assert_eq!(registry.formatter_for(&project.join("a.py")), None);
    std::fs::write(project.join("pyproject.toml"), "[tool.ruff]\nline-length = 100\n").unwrap();
    assert_eq!(registry.formatter_for(&project.join("a.py")).as_deref(), Some("python"));
    std::fs::write(project.join(".taplo.toml"), "").unwrap();
    assert_eq!(registry.formatter_for(&project.join("Cargo.toml")).as_deref(), Some("toml"));
}

#[test]
fn default_registry_has_rust() {
    let reg = FormatterRegistry::default();
    assert!(reg.get("rust").is_some());
    assert_eq!(reg.language_for_path(Path::new("src/main.rs")), Some("rust".into()));
}

#[test]
fn default_registry_has_python() {
    let reg = FormatterRegistry::default();
    assert!(reg.get("python").is_some());
    assert_eq!(reg.language_for_path(Path::new("app.py")), Some("python".into()));
}

#[test]
fn default_registry_has_typescript() {
    let reg = FormatterRegistry::default();
    assert!(reg.get("typescript").is_some());
    assert_eq!(reg.language_for_path(Path::new("index.ts")), Some("typescript".into()));
    assert_eq!(reg.language_for_path(Path::new("app.tsx")), Some("typescript".into()));
}

#[test]
fn unknown_extension_returns_none() {
    let reg = FormatterRegistry::default();
    assert_eq!(reg.language_for_path(Path::new("file.xyz123")), None);
    assert_eq!(reg.language_for_path(Path::new("no_extension")), None);
}

#[test]
fn custom_formatter_registration() {
    let mut reg = FormatterRegistry::default();
    reg.register(
        "zig",
        FormatterSpec {
            command: "zig".to_string(),
            args: vec!["fmt".to_string()],
            via_stdin: false,
        },
    );
    assert!(reg.get("zig").is_some());
    assert_eq!(reg.language_for_path(Path::new("main.zig")), Some("zig".into()));
}

#[test]
fn registry_serialization_roundtrip() {
    let reg = FormatterRegistry::default();
    let json = serde_json::to_string(&reg).expect("serialize");
    let decoded: FormatterRegistry = serde_json::from_str(&json).expect("deserialize");
    let mut before = reg.languages();
    let mut after = decoded.languages();
    before.sort_unstable();
    after.sort_unstable();
    assert_eq!(before, after);
    assert!(before.contains(&"rust") && before.contains(&"python") && before.contains(&"go"));
}

/// A filter formatter's whole job is the bytes it hands back — which the file has to end up
/// holding, inside the one deadline that spans writing to it and reading from it.
///
/// Unix-only: the stub is a shell pipeline, the same reason the stdio integration test is.
#[cfg(unix)]
#[tokio::test]
async fn a_filter_formatters_output_becomes_the_file() {
    use std::os::unix::fs::PermissionsExt;
    let project = tempfile::tempdir().expect("temp dir");
    let stub = project.path().join("luxe-fixture-filter");
    std::fs::write(&stub, "#!/bin/sh\ntr 'a-z' 'A-Z'\n").expect("write stub");
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let mut registry = FormatterRegistry::default();
    registry.register(
        "fixturefilter",
        FormatterSpec { command: crate::paths::display(&stub), args: Vec::new(), via_stdin: true },
    );
    let target = project.path().join("sample.fixturefilter");
    std::fs::write(&target, "shout\n").expect("write");

    run_formatter(&target, &registry).await.expect("the filter runs");
    assert_eq!(std::fs::read_to_string(&target).expect("read back"), "SHOUT\n");
}

#[test]
fn rustfmt_spec_is_correct() {
    let reg = FormatterRegistry::default();
    let spec = reg.get("rust").unwrap();
    assert_eq!(spec.command, "rustfmt");
    assert!(!spec.via_stdin);
    assert!(spec.args.contains(&"--edition".to_string()));
}

/// The edition on the command line overrides the project's own `rustfmt.toml`, so a constant
/// there is not a default — it is luxe overruling the repository about its own formatting.
#[test]
fn rust_is_formatted_against_the_edition_the_project_declares() {
    let project = tempfile::tempdir().expect("temp dir");
    let edition_of = |reg: &FormatterRegistry| {
        let args = reg.get("rust").expect("a rust formatter").args.clone();
        args.last().cloned().expect("an edition")
    };

    assert_eq!(edition_of(&FormatterRegistry::for_project(project.path(), false)), "2021");

    std::fs::write(project.path().join("Cargo.toml"), "[package]\nedition = \"2024\"\n")
        .expect("write");
    assert_eq!(edition_of(&FormatterRegistry::for_project(project.path(), false)), "2024");

    // A member that inherits carries no value of its own; the file that answers is the next one.
    std::fs::write(
        project.path().join("Cargo.toml"),
        "[package]\nedition.workspace = true\n\n[workspace.package]\nedition = \"2018\"\n",
    )
    .expect("write");
    assert_eq!(edition_of(&FormatterRegistry::for_project(project.path(), false)), "2018");

    // rustfmt reads `rustfmt.toml` itself, so what it says has to win over the manifest —
    // otherwise the flag luxe passes silently overrules it.
    std::fs::write(project.path().join("rustfmt.toml"), "edition = \"2015\"\n").expect("write");
    assert_eq!(edition_of(&FormatterRegistry::for_project(project.path(), false)), "2015");
}

/// Only a real `edition = "20xx"` counts. Everything else falls through to the next candidate.
#[test]
fn an_edition_is_read_only_where_a_project_really_states_one() {
    assert_eq!(edition_in("edition = \"2024\"").as_deref(), Some("2024"));
    assert_eq!(edition_in("  edition='2018'  ").as_deref(), Some("2018"));
    assert_eq!(edition_in("[package]\nname = \"x\"\nedition = \"2021\"").as_deref(), Some("2021"));
    for absent in [
        "edition.workspace = true",
        "style_edition = \"2024\"",
        "name = \"edition\"",
        "edition = \"next\"",
        "",
    ] {
        assert_eq!(edition_in(absent), None, "{absent}");
    }
}

/// The formatter a project installed for itself is the one whose version matches it — and it is
/// invisible to a `PATH` lookup, which is how format-on-save ended up running a stranger.
#[tokio::test]
async fn the_projects_own_formatter_is_used_only_when_the_run_allows_it() {
    let project = tempfile::tempdir().expect("temp dir");
    let bin = project.path().join("node_modules").join(".bin");
    std::fs::create_dir_all(&bin).expect("mkdir");
    let marker = project.path().join("formatted-by-the-project");
    let _ = write_stub(&bin, "luxe-fixture-fmt", &marker);

    let target = project.path().join("sample.fixturefmt");
    std::fs::write(&target, "unformatted\n").expect("write");

    let registry = |project_local: bool| {
        let mut reg = FormatterRegistry::for_project(project.path(), project_local);
        reg.register(
            "fixturefmt",
            FormatterSpec {
                command: "luxe-fixture-fmt".to_string(),
                args: Vec::new(),
                via_stdin: false,
            },
        );
        reg
    };

    let refused = run_formatter(&target, &registry(false)).await;
    assert!(
        matches!(refused, Err(FormatterError::NotFound { .. })),
        "a binary the repository supplied is not run on this run's say-so: {refused:?}"
    );
    assert!(!marker.exists(), "and it really did not run");

    run_formatter(&target, &registry(true)).await.expect("allowed, the project's copy runs");
    assert!(marker.exists(), "the copy that ran was the project's own");
}

/// gofmt has no `--version`. A liveness probe that assumed every formatter answers one turned
/// format-on-save off for every Go file, while the formatter itself worked perfectly.
#[tokio::test]
async fn a_formatter_that_refuses_version_is_still_run() {
    let project = tempfile::tempdir().expect("temp dir");
    let marker = project.path().join("formatted");
    let stub = write_stub(project.path(), "luxe-fixture-gofmt", &marker);

    let mut registry = FormatterRegistry::default();
    registry.register(
        "fixturefmt",
        FormatterSpec { command: crate::paths::display(&stub), args: Vec::new(), via_stdin: false },
    );
    let target = project.path().join("sample.fixturefmt");
    std::fs::write(&target, "unformatted\n").expect("write");

    run_formatter(&target, &registry).await.expect("it runs");
    assert!(marker.exists(), "a formatter is judged by running it, not by interviewing it");
}

/// A formatter stub that records the fact it ran — and, like `gofmt`, rejects `--version`.
#[cfg(unix)]
fn write_stub(dir: &Path, name: &str, marker: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    let script = format!(
        "#!/bin/sh\n\
             if [ \"$1\" = \"--version\" ]; then echo 'flag provided but not defined' >&2; exit 2; \
             fi\ntouch '{}'\n",
        marker.display()
    );
    std::fs::write(&path, script).expect("write stub");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

#[cfg(not(unix))]
fn write_stub(dir: &Path, name: &str, marker: &Path) -> PathBuf {
    let path = dir.join(format!("{name}.cmd"));
    let script = format!(
        "@echo off\r\nif \"%1\"==\"--version\" exit /b 2\r\ntype nul > \"{}\"\r\n",
        marker.display()
    );
    std::fs::write(&path, script).expect("write stub");
    path
}

/// A registry for one extension, `.fmt`, formatted by `command args…` through stdin.
#[cfg(unix)]
fn filter_registry(command: &str, args: &[&str]) -> FormatterRegistry {
    let mut registry = FormatterRegistry::new();
    let args = args.iter().map(|arg| arg.to_string()).collect();
    registry.register("fmt", FormatterSpec { command: command.into(), args, via_stdin: true });
    registry
}

/// A formatter that answers a non-empty file with nothing is refused, not written: that is a
/// misconfigured formatter, and the file it would have emptied is the agent's work.
#[cfg(unix)]
#[tokio::test]
async fn an_empty_formatter_answer_does_not_empty_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.fmt");
    std::fs::write(&file, "keep me\n").unwrap();
    let error = run_formatter(&file, &filter_registry("true", &[])).await.unwrap_err();
    assert!(matches!(error, FormatterError::EmptyOutput { .. }), "{error}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep me\n");
}

/// A file that changed while its formatter ran keeps the change; the output was for the old text.
#[cfg(unix)]
#[tokio::test]
async fn a_file_changed_during_formatting_is_not_written_over() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.fmt");
    std::fs::write(&file, "before\n").unwrap();
    let script = format!("printf 'meanwhile\\n' >> '{}'; cat", file.display());
    let error = run_formatter(&file, &filter_registry("sh", &["-c", &script])).await.unwrap_err();
    assert!(matches!(error, FormatterError::Changed { .. }), "{error}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "before\nmeanwhile\n");
}

/// A file that cannot be read is an IO failure with what was attempted, not "formatter not
/// found", which sends the reader off to install a program they have.
#[cfg(unix)]
#[tokio::test]
async fn a_read_failure_is_not_reported_as_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("gone.fmt");
    let error = run_formatter(&missing, &filter_registry("cat", &[])).await.unwrap_err();
    assert!(matches!(error, FormatterError::Io { action: "read the file", .. }), "{error}");
}

/// A registry read back with an extension but no formatter for it is an error, not a panic.
#[tokio::test]
async fn an_extension_without_a_formatter_is_an_error() {
    let registry: FormatterRegistry =
        serde_json::from_str(r#"{"formatters":{},"extensions":{"zz":"zz"}}"#).unwrap();
    let error = run_formatter(Path::new("a.zz"), &registry).await.unwrap_err();
    assert!(matches!(error, FormatterError::NoFormatter { .. }), "{error}");
}
