//! `luxe --rpc` against the real binary — the framing promise, end to end.
//!
//! The unit tests in `cli::rpc` drive `serve` over in-memory pipes, which proves the protocol
//! logic but cannot prove the thing a client actually depends on: that **stdout carries protocol
//! frames and nothing else**. One `println!` anywhere in startup — a warning, a migration notice,
//! a stray debug line — breaks every client, and no amount of testing `serve` in isolation would
//! see it. So this spawns the shipped binary and reads what really comes out.

use std::io::Write;
use std::process::{Command, Stdio};

/// Run `luxe --rpc` in an empty project with an isolated config home, feeding it `input`.
///
/// Isolated because the developer's real `~/.config/luxe` must not decide whether a test passes,
/// and because a run against a real project would emit that project's workspace snapshot.
fn run_rpc(input: &str) -> (String, String) {
    run_rpc_with(&[], input)
}

/// [`run_rpc`] with extra flags after `--rpc`.
fn run_rpc_with(flags: &[&str], input: &str) -> (String, String) {
    let project = tempfile::tempdir().expect("temp project");
    let home = tempfile::tempdir().expect("temp home");

    let mut child = Command::new(env!("CARGO_BIN_EXE_luxe"))
        .arg("--rpc")
        .args(flags)
        .current_dir(project.path())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join(".config"))
        // it shares its session, and the developer's own sessions are listed in the real one
        .env("LUXE_RUNTIME_DIR", home.path().join("runtime"))
        .env_remove("LUXE_CONFIG_DIR")
        .env_remove("LUXE_STATE_DIR")
        // Offline and deterministic: a catalog refresh is best-effort anyway, and a test that
        // reaches the network is a test that fails on a train.
        .env("LUXE_MODELS_DEV_REFRESH", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("luxe --rpc starts");

    child.stdin.as_mut().expect("stdin").write_all(input.as_bytes()).expect("write");
    // Dropping stdin is the EOF a client's exit produces, and the session must end on it.
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("luxe --rpc exits");
    assert!(output.status.success(), "luxe --rpc exited with {}", output.status);
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Every line on stdout parses as one JSON frame, and the handshake leads.
///
/// The break this catches: anything printed to stdout that is not a frame. It fails loudly with
/// the offending line rather than leaving a client to discover it.
#[test]
fn rpc_stdout_carries_only_protocol_frames() {
    let (stdout, stderr) = run_rpc("{\"kind\":\"close\"}\n");

    let frames: Vec<serde_json::Value> = stdout
        .lines()
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|error| {
                panic!("stdout carried a line that is not a protocol frame: {line:?} ({error})")
            })
        })
        .collect();

    assert!(!frames.is_empty(), "the handshake alone should produce a frame; stderr: {stderr}");
    assert_eq!(frames[0]["kind"], "ready", "the handshake must lead the stream");
    assert_eq!(frames[0]["content"]["protocol_version"], 2);
    assert!(
        frames[0]["content"]["session_id"].as_str().is_some_and(|id| !id.is_empty()),
        "a client needs the session it attached to"
    );
    assert!(
        frames.iter().all(|frame| frame["kind"].is_string()),
        "every frame is tagged; a client dispatches on it"
    );
}

/// A refused frame does not stop the session, and says which frame it refused.
///
/// Run against the real binary rather than `serve` alone because the refusal path crosses the
/// translator, which is where the model catalog is reached — the one part that behaves differently
/// with a real config home than with a test double.
#[test]
fn rpc_refuses_a_bad_frame_and_keeps_going() {
    let (stdout, _) = run_rpc("not json\n{\"kind\":\"nonesuch\"}\n{\"kind\":\"close\"}\n");
    let frames: Vec<serde_json::Value> =
        stdout.lines().map(|line| serde_json::from_str(line).expect("frame")).collect();

    let refusals: Vec<&serde_json::Value> =
        frames.iter().filter(|frame| frame["kind"] == "action_rejected").collect();
    assert_eq!(refusals.len(), 2, "both bad frames are refused: {frames:#?}");
    assert!(
        refusals[0]["content"].get("kind").is_none(),
        "a line that is not JSON has no kind to report"
    );
    assert_eq!(
        refusals[1]["content"]["kind"], "nonesuch",
        "a frame that named itself is refused by that name"
    );
}

/// GitHub writes are refused unless the launch allowed them, and `ready` says which it is.
///
/// Against the real binary because the gate's inputs are launch-time: the flag, and the permission
/// mode resolved from the command line and the config home. The frame is a write the refusal must
/// stop *before* it reaches GitHub; the test environment has no `gh` session, so a gate that let it
/// through would show up as a `github_comment_posted` carrying an error rather than a refusal.
#[test]
fn rpc_refuses_github_writes_unless_the_launch_allowed_them() {
    let post = "{\"kind\":\"post_github_comment\",\"content\":{\"number\":1,\"body\":\"hi\"}}\n";
    let frames = |flags: &[&str]| -> Vec<serde_json::Value> {
        let (stdout, _) = run_rpc_with(flags, &format!("{post}{{\"kind\":\"close\"}}\n"));
        stdout.lines().map(|line| serde_json::from_str(line).expect("frame")).collect()
    };
    let refusal = |frames: &[serde_json::Value]| -> Option<String> {
        frames
            .iter()
            .find(|frame| frame["kind"] == "action_rejected")
            .map(|frame| frame["content"]["reason"].as_str().unwrap_or_default().to_string())
    };

    let default = frames(&[]);
    assert_eq!(default[0]["content"]["github_writes"], false);
    let said = refusal(&default).expect("a write is refused by default");
    assert!(said.contains("--github-writes"), "the refusal names the switch: {said}");

    let read_only = frames(&["--github-writes", "--read-only"]);
    assert_eq!(read_only[0]["content"]["github_writes"], false, "read-only wins over the flag");
    assert!(refusal(&read_only).is_some_and(|said| said.contains("read-only")));

    let allowed = frames(&["--github-writes"]);
    assert_eq!(allowed[0]["content"]["github_writes"], true);
    assert_eq!(refusal(&allowed), None, "an allowed write is not refused at the gate");
}

/// A live `luxe --rpc` session: frames go in one at a time, and each answer is read before the
/// next is written, which is what a client driving a review has to do.
struct Live {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    frames: std::sync::mpsc::Receiver<serde_json::Value>,
    /// Every frame kind read so far, for a timeout to say how far luxe got
    seen: std::cell::RefCell<Vec<String>>,
    /// Where luxe writes its log, which a timeout prints
    project: std::path::PathBuf,
    home: tempfile::TempDir,
}

impl Live {
    fn start(project: &std::path::Path, flags: &[&str]) -> Self {
        let home = tempfile::tempdir().expect("temp home");
        let mut child = Command::new(env!("CARGO_BIN_EXE_luxe"))
            .arg("--rpc")
            .args(flags)
            .current_dir(project)
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join(".config"))
            .env("LUXE_RUNTIME_DIR", home.path().join("runtime"))
            .env("LUXE_MODELS_DEV_REFRESH", "0")
            .env("RUST_LOG", "debug")
            // not a terminal multiplexer's pane, whatever the one running the tests is
            .env_remove("HERDR_ENV")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("luxe --rpc starts");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");
        let (tx, frames) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let frame = serde_json::from_str(&line)
                    .unwrap_or_else(|error| panic!("not a frame: {line:?} ({error})"));
                if tx.send(frame).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            frames,
            seen: Default::default(),
            project: project.to_path_buf(),
            home,
        }
    }

    /// What this luxe published about its session for others to join, if anything.
    fn published(&self) -> Option<serde_json::Value> {
        let record = self.home.path().join("runtime").join(format!("{}.json", self.child.id()));
        let text = std::fs::read_to_string(record).ok()?;
        Some(serde_json::from_str(&text).expect("the record is JSON"))
    }

    fn send(&mut self, frame: serde_json::Value) {
        writeln!(self.stdin, "{frame}").expect("write a frame");
        self.stdin.flush().expect("flush");
    }

    /// The first frame of `kind` that `matches` accepts, within a deadline.
    fn wait(&self, kind: &str, matches: impl Fn(&serde_json::Value) -> bool) -> serde_json::Value {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let Ok(frame) = self.frames.recv_timeout(left) else {
                let log = std::fs::read_to_string(self.project.join(".luxe/logs/luxe.log"));
                panic!(
                    "no `{kind}` arrived; it had sent {:?}; luxe said:\n{}\nstill running:\n{}",
                    self.seen.borrow(),
                    log.unwrap_or_default(),
                    running_children()
                );
            };
            self.seen.borrow_mut().push(frame["kind"].as_str().unwrap_or_default().to_string());
            if frame["kind"] == "action_rejected" {
                panic!("refused: {frame}");
            }
            if frame["kind"] == kind && matches(&frame["content"]) {
                return frame["content"].clone();
            }
        }
    }

    fn close(mut self) {
        self.send(serde_json::json!({ "kind": "close" }));
        let status = self.child.wait().expect("luxe exits");
        assert!(status.success(), "luxe --rpc exited with {status}");
    }
}

/// The git, gh and luxe processes still running, with their command lines: what a stalled luxe is
/// waiting on. Only Windows has needed asking.
fn running_children() -> String {
    if !cfg!(windows) {
        return String::new();
    }
    let script = "Get-CimInstance Win32_Process | Where-Object { $_.Name -match 'git|gh|luxe' } \
                  | ForEach-Object { \"$($_.ProcessId) <- $($_.ParentProcessId): $($_.CommandLine)\" }";
    Command::new("powershell")
        .args(["-NoProfile", "-Command", script])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_else(|error| format!("(could not list processes: {error})"))
}

impl Drop for Live {
    /// A test that failed partway leaves no luxe behind; after `close` this is a no-op.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A scratch repository with one commit and a change on top of it, for a working-tree review.
fn changed_repository() -> tempfile::TempDir {
    let project = tempfile::tempdir().expect("temp project");
    let git = |args: &[&str]| {
        let status = Command::new("git").args(args).current_dir(project.path()).status();
        assert!(status.expect("git runs").success(), "git {args:?}");
    };
    git(&["init", "--quiet", "--initial-branch=main"]);
    git(&["config", "user.email", "tests@example.invalid"]);
    git(&["config", "user.name", "luxe Tests"]);
    std::fs::write(project.path().join("total.rs"), "fn total() -> u32 {\n    1\n}\n").unwrap();
    git(&["add", "total.rs"]);
    git(&["commit", "--quiet", "-m", "initial"]);
    std::fs::write(project.path().join("total.rs"), "fn total() -> u32 {\n    1 + 1\n}\n").unwrap();
    project
}

/// A review driven entirely over the wire: loaded, commented on by naming a line, resolved, read,
/// and committed, each answered as `rpc.md` says. No model is needed for any of it.
#[test]
fn rpc_drives_a_review_from_comment_to_commit() {
    let project = changed_repository();
    let mut luxe = Live::start(project.path(), &[]);
    luxe.wait("ready", |_| true);

    luxe.send(serde_json::json!({ "kind": "load_guided_review", "content": {
        "request_id": "load-1", "target": { "kind": "working_tree" } } }));
    let review =
        luxe.wait("guided_review_loaded", |c| c["request_id"] == "load-1")["review"].clone();
    let restored = luxe.wait("review_session_restored", |_| true);
    assert_eq!(restored["session"]["id"], review["id"], "the session follows its review");
    let review_id = review["id"].clone();
    let file = &review["files"][0];
    let hunk = &file["hunks"][0];
    let added = hunk["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|line| line["kind"] == "addition")
        .expect("an added line");

    // A line named by number, without the digest no client can compute
    luxe.send(serde_json::json!({ "kind": "edit_review", "content": {
        "request_id": "edit-1", "review_id": review_id, "edit": "comment",
        "anchor": { "kind": "line", "file_id": file["id"], "hunk_id": hunk["id"],
                    "side": "new", "line": added["new_line"] },
        "body": "Why add one here?\nIt changes every total." } }));
    let saved = luxe.wait("review_session_saved", |c| c["request_id"] == "edit-1");
    let finding = &saved["session"]["findings"][0];
    assert_eq!(finding["label"], "R1");
    assert_eq!(finding["title"], "Why add one here?");
    assert!(finding["anchor"]["context_hash"].as_str().is_some_and(|hash| !hash.is_empty()));

    luxe.send(serde_json::json!({ "kind": "edit_review", "content": {
        "request_id": "edit-2", "review_id": review_id, "edit": "resolve",
        "finding_id": finding["id"] } }));
    let saved = luxe.wait("review_session_saved", |c| c["request_id"] == "edit-2");
    assert_eq!(saved["session"]["findings"][0]["state"], "resolved");

    // A comment on a line the review does not have is refused, and says why
    luxe.send(serde_json::json!({ "kind": "edit_review", "content": {
        "request_id": "edit-3", "review_id": review_id, "edit": "comment",
        "anchor": { "kind": "line", "file_id": file["id"], "hunk_id": hunk["id"],
                    "side": "new", "line": 999 },
        "body": "Nowhere." } }));
    let refused = luxe.wait("review_session_save_failed", |c| c["request_id"] == "edit-3");
    assert!(refused["message"].as_str().unwrap().contains("not in the review"), "{refused}");

    let chapter = review["chapters"][0]["id"].clone();
    luxe.send(serde_json::json!({ "kind": "record_review_progress", "content": {
        "review_id": review_id, "walked_chapter_ids": [chapter] } }));
    let saved = luxe.wait("review_session_saved", |c| {
        c["session"]["progress"]["walked_chapter_ids"].as_array().is_some_and(|ids| !ids.is_empty())
    });
    assert_eq!(saved["session"]["progress"]["walked_chapter_ids"][0], chapter);

    luxe.send(serde_json::json!({ "kind": "commit_review_chapters", "content": {
        "request_id": "commit-1", "review_id": review_id,
        "revision_id": review["revision_id"], "squash": true } }));
    let committed = luxe.wait("review_committed", |c| c["request_id"] == "commit-1");
    assert_eq!(committed["commits"].as_array().map(Vec::len), Some(1));
    luxe.close();

    let log = Command::new("git")
        .args(["log", "--format=%s"])
        .current_dir(project.path())
        .output()
        .expect("git log");
    assert_eq!(String::from_utf8_lossy(&log.stdout).lines().count(), 2, "the review's commit");
}

/// A session started over `--rpc` is shared like the terminal's, so the bridge can start one for a
/// phone and the phone can join it.
///
/// The break this catches: `run_rpc` not offering its session, which leaves a session started from
/// the phone running where nothing can reach it.
#[test]
fn rpc_shares_its_session() {
    let project = tempfile::tempdir().expect("temp project");
    let luxe = Live::start(project.path(), &[]);
    let ready = luxe.wait("ready", |_| true);
    let record = luxe.published().expect("a record in the runtime folder");
    assert_eq!(record["session_id"], ready["session_id"]);
    luxe.close();
}

/// A read-only session reviews and comments, but writes no history.
#[test]
fn rpc_read_only_refuses_to_commit_a_review() {
    let project = changed_repository();
    let mut luxe = Live::start(project.path(), &["--read-only"]);
    luxe.wait("ready", |_| true);
    luxe.send(serde_json::json!({ "kind": "load_guided_review", "content": {
        "request_id": "load-1", "target": { "kind": "working_tree" } } }));
    let review = luxe.wait("guided_review_loaded", |_| true)["review"].clone();
    luxe.send(serde_json::json!({ "kind": "commit_review_chapters", "content": {
        "request_id": "commit-1", "review_id": review["id"], "revision_id": review["revision_id"] } }));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let refused = loop {
        let frame = luxe
            .frames
            .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            .expect("an answer");
        if frame["kind"] == "action_rejected" {
            break frame["content"].clone();
        }
        assert_ne!(frame["kind"], "review_committed", "read-only committed");
    };
    assert_eq!(refused["kind"], "commit_review_chapters");
    assert!(refused["reason"].as_str().unwrap().contains("read-only"), "{refused}");
    luxe.close();
}
