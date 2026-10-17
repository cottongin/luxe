use super::failure::{explain_failure, explain_gh_failure, GIT_ERROR_MAX_LINES};
use super::*;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::{mpsc, watch};

fn test_context(tmp_dir: &std::path::Path) -> ToolContext {
    let (event_tx, _event_rx) = mpsc::channel(64);
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    ToolContext {
        cwd: tmp_dir.to_path_buf(),
        session_id: "test-session".to_string(),
        cancelled: Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    }
}

#[tokio::test]
async fn test_git_missing_action() {
    let tool = GitTool::new();
    let tmp_dir = TempDir::new().unwrap();
    let ctx = test_context(tmp_dir.path());

    let result = tool.run(serde_json::json!({}), &ctx).await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_git_unknown_action() {
    let tool = GitTool::new();
    let tmp_dir = TempDir::new().unwrap();
    let ctx = test_context(tmp_dir.path());

    let result = tool.run(serde_json::json!({"action": "invalid"}), &ctx).await;
    assert!(result.is_error);
}

/// The reply to an action nobody knows lists every action the schema offers.
///
/// The break this catches: the list was typed out by hand and stopped at `pr`, so a model that
/// misspelled `checks` was told `show`, `issue` and `checks` did not exist.
#[tokio::test]
async fn an_unknown_action_names_every_action_the_schema_offers() {
    let tool = GitTool::new();
    let tmp_dir = TempDir::new().unwrap();
    let ctx = test_context(tmp_dir.path());

    let result = tool.run(serde_json::json!({"action": "invalid"}), &ctx).await;
    let luxe_core::ContentBlock::Text(said) = &result.content[0] else { panic!("text") };
    let schema = tool.schema();
    let offered = schema.parameters["properties"]["action"]["enum"].as_array().unwrap();
    for action in offered {
        let name = action.as_str().unwrap();
        assert!(said.contains(name), "{name} is missing from: {said}");
    }
}

/// The schema and the dispatcher cannot drift: every action the schema offers is one the tool
/// parses, and every action it parses is offered.
#[test]
fn every_action_in_the_schema_parses_and_every_parsed_action_is_in_the_schema() {
    let schema = GitTool::new().schema();
    let offered: Vec<&str> = schema.parameters["properties"]["action"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|action| action.as_str().unwrap())
        .collect();
    for name in &offered {
        let parsed = GitAction::parse(name).unwrap_or_else(|| panic!("`{name}` does not parse"));
        assert_eq!(parsed.name(), *name);
    }
    let parsed: Vec<&str> = GitAction::ALL.map(GitAction::name).to_vec();
    assert_eq!(parsed, offered, "the same actions, in the schema's order");
}

/// What each action runs, bare and with arguments: the defaults a caller relies on, and that the
/// arguments it gives are the ones git gets.
#[test]
fn each_action_plans_the_git_command_it_names() {
    let planned = |action: GitAction, request: &[&str]| {
        let request: Vec<String> = request.iter().map(|arg| arg.to_string()).collect();
        let plan = plan(action, &serde_json::json!({}), &request)
            .unwrap_or_else(|_| panic!("{action:?} plans"));
        plan.argv.join(" ")
    };
    assert_eq!(planned(GitAction::Status, &[]), "status --short");
    assert_eq!(planned(GitAction::Diff, &[]), "diff --unified=3");
    assert_eq!(planned(GitAction::Diff, &["--stat"]), "diff --stat");
    assert_eq!(planned(GitAction::Show, &["HEAD", "--stat"]), "show HEAD --stat");
    assert_eq!(planned(GitAction::Log, &[]), "log --oneline -11");
    assert_eq!(planned(GitAction::Log, &["-n", "3"]), "log -n 3");
    assert_eq!(planned(GitAction::Add, &["a.rs", "b.rs"]), "add a.rs b.rs");
    assert_eq!(planned(GitAction::Branch, &[]), "branch -a");
    assert_eq!(planned(GitAction::Branch, &["-d", "old"]), "branch -d old");
    assert_eq!(planned(GitAction::Checkout, &["topic"]), "checkout topic");
    assert_eq!(planned(GitAction::Stash, &[]), "stash push");

    // Naming nothing where something is needed is refused before git is asked
    for action in [GitAction::Add, GitAction::Checkout] {
        let Err(refusal) = plan(action, &serde_json::json!({}), &[]) else { panic!("{action:?}") };
        assert!(refusal.is_error);
    }
    // and an action that goes to `gh` is not a git command
    assert!(plan(GitAction::Pr, &serde_json::json!({}), &[]).is_err());
}

/// How each answer is told: a clean tree, an empty diff and history, and a staged add each say so
/// rather than returning nothing.
#[test]
fn an_empty_answer_says_what_it_means() {
    assert_eq!(Shape::OrMessage("clean").tell("  \n"), "clean");
    assert_eq!(Shape::OrMessage("clean").tell(" M a.rs\n"), " M a.rs\n");
    let diff = Shape::Capped { max: 10, noun: "lines", hint: "", empty: Some("no changes") };
    assert_eq!(diff.tell(""), "no changes");
    assert_eq!(Shape::LogDefault.tell(""), "no commits matched");
    assert_eq!(Shape::Said("Files staged.").tell("anything"), "Files staged.");
    assert_eq!(Shape::Trimmed.tell("  x\n"), "x");
    assert_eq!(Shape::Raw.tell("  x\n"), "  x\n");
}

/// A repository with one commit on a branch, and a bare "remote" it can really push to.
///
/// A real remote rather than a mock: the whole point of the preflight is what `git` does about
/// an upstream, and an upstream that no command could ever set is not the thing under test.
///
/// `cfg(unix)` for the same reason its callers are — it silences the global git config with
/// `/dev/null`, so a contributor's own `init.defaultBranch` cannot change what is under test.
#[cfg(unix)]
fn repo_with_remote() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let remote = dir.path().join("remote.git");
    let work = dir.path().join("work");
    let run = |args: &[&str], cwd: &std::path::Path| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .expect("git runs");
        assert!(status.status.success(), "git {args:?}: {:?}", status);
    };
    std::fs::create_dir_all(&work).expect("mkdir");
    run(&["init", "--bare", "-b", "main", remote.to_str().expect("utf8")], dir.path());
    run(&["init", "-b", "main"], &work);
    run(&["config", "user.email", "t@example.com"], &work);
    run(&["config", "user.name", "Test"], &work);
    std::fs::write(work.join("a.txt"), "x").expect("write");
    run(&["add", "."], &work);
    run(&["commit", "-m", "first"], &work);
    run(&["remote", "add", "origin", remote.to_str().expect("utf8")], &work);
    (dir, work)
}

/// A bare `log` shows ten commits and says when there are older ones, rather than letting ten
/// read as the whole history; exactly ten says nothing more.
#[cfg(unix)]
#[tokio::test]
async fn a_bare_log_says_when_history_goes_on() {
    let (_dir, work) = repo_with_remote();
    let commit = |message: &str| {
        let output = std::process::Command::new("git")
            .args(["commit", "--allow-empty", "-q", "-m", message])
            .current_dir(&work)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("git runs");
        assert!(output.status.success(), "{output:?}");
    };
    for n in 2..=10 {
        commit(&format!("commit {n}"));
    }
    let tool = GitTool::new();
    let cx = test_context(&work);
    let log = || tool.run(serde_json::json!({"action": "log"}), &cx);
    let luxe_core::ContentBlock::Text(ten) = &log().await.content[0] else { panic!("text") };
    assert_eq!(ten.lines().count(), 10, "{ten}");
    assert!(!ten.contains("older commits"), "{ten}");
    commit("commit 11");
    let luxe_core::ContentBlock::Text(eleven) = &log().await.content[0] else { panic!("text") };
    assert!(eleven.lines().next().unwrap().ends_with("commit 11"), "{eleven}");
    assert!(!eleven.contains(" first"), "the oldest is the one left out: {eleven}");
    assert!(eleven.ends_with("or a path for more."), "{eleven}");
}

/// The preflight's whole reason for existing: a branch nobody has pushed gets published, so
/// `gh pr create` meets the precondition it will not fix for itself.
#[cfg(unix)]
#[tokio::test]
async fn a_pull_request_publishes_its_branch_first() {
    let (_dir, work) = repo_with_remote();
    let run = |args: &[&str]| {
        std::process::Command::new("git").args(args).current_dir(&work).output().expect("git runs");
    };
    run(&["checkout", "-b", "feature"]);

    let tool = GitTool::new();
    let note = tool.ready_branch_for_pr(&work).await.expect("the branch can be published");
    assert!(
        note.as_deref().is_some_and(|line| line.contains("feature")),
        "a push is never silent: {note:?}"
    );

    // Really published, and a second call is now a no-op rather than a second push.
    let upstream = tool
        .run_git(&work, &["rev-parse", "--abbrev-ref", "@{u}"])
        .await
        .expect("an upstream exists now");
    assert_eq!(upstream.trim(), "origin/feature");
    assert_eq!(tool.ready_branch_for_pr(&work).await.expect("still fine"), None);
}

/// Opening a pull request from trunk cannot work, and `gh`'s own error for it sends people to
/// look at their remote instead of at their branch.
#[cfg(unix)]
#[tokio::test]
async fn a_pull_request_from_trunk_is_refused_with_the_fix() {
    let (_dir, work) = repo_with_remote();
    let error =
        GitTool::new().ready_branch_for_pr(&work).await.expect_err("main cannot target main");
    let said = error.to_string();
    assert!(said.contains("main"), "{said}");
    assert!(said.contains("checkout -b"), "names the fix: {said}");
}

/// A detached HEAD has no branch to publish, and saying "push failed" there would be a
/// diagnosis of the wrong problem.
#[cfg(unix)]
#[tokio::test]
async fn a_detached_head_is_named_as_the_problem() {
    let (_dir, work) = repo_with_remote();
    std::process::Command::new("git")
        .args(["checkout", "--detach", "HEAD"])
        .current_dir(&work)
        .output()
        .expect("git runs");
    let error = GitTool::new().ready_branch_for_pr(&work).await.expect_err("nothing to push");
    assert!(error.to_string().contains("detached"), "{error}");
}

/// Only `create` has a precondition; a read must not push anything.
#[test]
fn only_creating_a_pull_request_triggers_the_preflight() {
    assert!(is_pr_create(&["create".to_string(), "--title".to_string()]));
    for read_only in [vec![], vec!["list".to_string()], vec!["view".to_string()]] {
        assert!(!is_pr_create(&read_only), "{read_only:?}");
    }
}

/// The one `gh` failure a person can actually act on, in words that say what to do.
#[test]
fn a_signed_out_gh_is_explained_rather_than_echoed() {
    for raw in [
        "gh command failed: To get started with GitHub CLI, please run: gh auth login",
        "HTTP 401: Requires authentication",
    ] {
        let explained = explain_gh_failure(raw).expect("a known cause");
        assert!(explained.contains("gh auth login"), "{explained}");
    }
    // Anything unrecognized is passed through rather than guessed at.
    assert_eq!(explain_gh_failure("something nobody anticipated"), None);
}

/// `gh`'s "no GitHub remote" message ends by suggesting `gh auth login`. Read as a sign-in
/// problem, it sent the reader to fix an account that was fine.
#[test]
fn a_repository_without_a_github_remote_is_not_a_sign_in_problem() {
    let explained = explain_gh_failure(
        "gh command failed: none of the git remotes configured for this repository point to a \
             known GitHub host. To tell gh about a new GitHub host, please use `gh auth login`",
    )
    .expect("a known cause");
    assert!(explained.contains("no remote on a GitHub host"), "{explained}");
    assert!(!explained.contains("not signed in"), "{explained}");
}

/// The defect this guards was silent by construction: everything after the subaction was
/// dropped, so a fully-specified `pr create` arrived at `gh` as a bare one and sat waiting for
/// the title it had already been given.
#[test]
fn every_pr_argument_reaches_gh() {
    let requested: Vec<String> = ["create", "--title", "Fix the thing", "--body", "why", "--draft"]
        .iter()
        .map(|part| (*part).to_string())
        .collect();
    assert_eq!(
        gh_args("pr", &requested),
        ["pr", "create", "--title", "Fix the thing", "--body", "why", "--draft"]
    );

    assert_eq!(gh_args("pr", &[]), ["pr", "list"], "no subaction is a listing, not an error");
    assert_eq!(gh_args("pr", &["view".to_string()]), ["pr", "view"]);
    let issue: Vec<String> = ["view", "12", "--comments"].map(String::from).to_vec();
    assert_eq!(gh_args("issue", &issue), ["issue", "view", "12", "--comments"]);
    assert_eq!(gh_args("issue", &[]), ["issue", "list"]);
}

/// The same defect as [`every_pr_argument_reaches_gh`], one arm away and still live: only the
/// subaction survived, so the `-m wip` that names a stash entry was dropped on the floor.
#[test]
fn every_stash_argument_reaches_git() {
    let requested: Vec<String> =
        ["push", "-m", "wip", "--keep-index"].iter().map(|part| (*part).to_string()).collect();
    assert_eq!(stash_args(&requested), ["stash", "push", "-m", "wip", "--keep-index"]);

    assert_eq!(stash_args(&[]), ["stash", "push"], "no subaction stashes, as it always did");
    assert_eq!(stash_args(&["pop".to_string()]), ["stash", "pop"]);
}

#[tokio::test]
async fn test_git_status_not_repo() {
    let tool = GitTool::new();
    let tmp_dir = TempDir::new().unwrap();
    let ctx = test_context(tmp_dir.path());

    let result = tool.run(serde_json::json!({"action": "status"}), &ctx).await;
    // Should fail because tmp_dir is not a git repo
    assert!(result.is_error);
}

#[tokio::test]
async fn test_git_commit_missing_message() {
    let tool = GitTool::new();
    let tmp_dir = TempDir::new().unwrap();
    let ctx = test_context(tmp_dir.path());

    let result = tool.run(serde_json::json!({"action": "commit"}), &ctx).await;
    assert!(result.is_error);
    let luxe_core::ContentBlock::Text(text) = &result.content[0] else { panic!("expected text") };
    assert!(text.contains("\"message\""), "a refusal shows the call that works: {text}");
}

/// No refusal may demand an argument the schema never offered.
///
/// The defect this guards was invisible from either side on its own. `commit` read a top-level
/// `message`; `schema()` advertised `action` and `args` and nothing else. Both halves read fine
/// in isolation, and together they meant the only call a model could build from the schema was
/// the one call the tool rejected — which a real session hit three times before giving up and
/// shelling out. So: hand every action everything the schema offers, and nothing may come back
/// saying something is missing.
#[tokio::test]
async fn no_action_demands_an_argument_the_schema_never_offered() {
    let tool = GitTool::new();
    let tmp_dir = TempDir::new().unwrap();
    if !init_repo(tmp_dir.path()) {
        eprintln!("skipping: git unavailable");
        return;
    }
    let ctx = test_context(tmp_dir.path());
    let schema = tool.schema();
    let properties = schema.parameters["properties"].as_object().expect("an object schema");
    let actions =
        properties["action"]["enum"].as_array().expect("the actions are enumerated").clone();

    for action in actions {
        let mut call = serde_json::Map::new();
        call.insert("action".to_string(), action.clone());
        // Everything else the schema offers, filled with the most harmless value that fits, so
        // any complaint left is about something the schema never mentioned.
        for (name, spec) in properties.iter().filter(|(name, _)| *name != "action") {
            let value = match spec["type"].as_str() {
                Some("array") => serde_json::json!(["."]),
                _ => serde_json::json!("placeholder"),
            };
            call.insert(name.clone(), value);
        }

        let result = tool.run(serde_json::Value::Object(call), &ctx).await;
        let luxe_core::ContentBlock::Text(text) = &result.content[0] else {
            panic!("expected text")
        };
        let lowered = text.to_ascii_lowercase();
        assert!(
            !lowered.contains("is required") && !lowered.contains("must include"),
            "`{action}` was handed every declared argument and still reports one missing: \
                 {text}"
        );
    }
}

/// Every spelling of a commit message reaches a real repository as a real commit.
///
/// [`crate::git::args`] proves the argument list; this proves the wiring, against git itself.
#[tokio::test]
async fn a_commit_message_lands_however_it_was_spelled() {
    let tool = GitTool::new();
    let tmp_dir = TempDir::new().unwrap();
    if !init_repo(tmp_dir.path()) {
        eprintln!("skipping: git unavailable");
        return;
    }
    let ctx = test_context(tmp_dir.path());

    let spellings = [
        serde_json::json!({"action": "commit", "message": "canonical"}),
        serde_json::json!({"action": "commit", "args": ["-m", "inside args"]}),
        serde_json::json!({"action": "commit", "args": ["a bare argument"]}),
    ];
    for (nth, call) in spellings.iter().enumerate() {
        std::fs::write(tmp_dir.path().join(format!("{nth}.txt")), "x").unwrap();
        let staged = tool.run(serde_json::json!({"action": "add", "args": ["."]}), &ctx).await;
        assert!(!staged.is_error, "staging failed");

        let result = tool.run(call.clone(), &ctx).await;
        let luxe_core::ContentBlock::Text(text) = &result.content[0] else {
            panic!("expected text")
        };
        assert!(!result.is_error, "{call} was refused: {text}");
    }

    let log = tool.run(serde_json::json!({"action": "log", "args": ["--format=%s"]}), &ctx).await;
    let luxe_core::ContentBlock::Text(text) = &log.content[0] else { panic!("expected text") };
    let subjects: Vec<&str> = text.lines().collect();
    assert_eq!(subjects, ["a bare argument", "inside args", "canonical"], "{text}");
}

/// `show` reads a file as a revision has it, which is how a pull request that is not checked
/// out is read without the shell.
#[tokio::test]
async fn show_reads_a_file_at_a_revision() {
    let tool = GitTool::new();
    let tmp_dir = TempDir::new().unwrap();
    if !init_repo(tmp_dir.path()) {
        eprintln!("skipping: git unavailable");
        return;
    }
    let ctx = test_context(tmp_dir.path());
    let file = tmp_dir.path().join("a.txt");
    std::fs::write(&file, "as committed\n").unwrap();
    tool.run(serde_json::json!({"action": "add", "args": ["."]}), &ctx).await;
    tool.run(serde_json::json!({"action": "commit", "message": "base"}), &ctx).await;
    std::fs::write(&file, "since\n").unwrap();

    let shown = tool.run(serde_json::json!({"action": "show", "args": ["HEAD:a.txt"]}), &ctx).await;
    let luxe_core::ContentBlock::Text(text) = &shown.content[0] else { panic!("text") };
    assert!(!shown.is_error, "{text}");
    assert_eq!(text.trim(), "as committed");
}

/// The defect end to end, against git itself: an unstaged commit must come back saying why.
///
/// Pinned here as well as at [`explain_failure`] because the bug was neither in the wording
/// nor in git — it was the one stream the runner threw away, and only a real invocation has
/// two streams to throw away.
#[tokio::test]
async fn committing_with_nothing_staged_says_what_to_stage() {
    let tool = GitTool::new();
    let tmp_dir = TempDir::new().unwrap();
    if !init_repo(tmp_dir.path()) {
        eprintln!("skipping: git unavailable");
        return;
    }
    let ctx = test_context(tmp_dir.path());
    let file = tmp_dir.path().join("a.txt");

    std::fs::write(&file, "one").unwrap();
    tool.run(serde_json::json!({"action": "add", "args": ["."]}), &ctx).await;
    tool.run(serde_json::json!({"action": "commit", "message": "base"}), &ctx).await;
    std::fs::write(&file, "two").unwrap();

    let refused =
        tool.run(serde_json::json!({"action": "commit", "message": "unstaged"}), &ctx).await;
    let luxe_core::ContentBlock::Text(text) = &refused.content[0] else { panic!("expected text") };
    assert!(refused.is_error, "an unstaged commit is a failure: {text}");
    assert!(!text.contains("no output"), "git said plenty, on stdout: {text}");
    assert!(text.contains("Nothing is staged"), "{text}");

    // And the fix it names actually works, through this tool.
    tool.run(serde_json::json!({"action": "add", "args": ["."]}), &ctx).await;
    let committed =
        tool.run(serde_json::json!({"action": "commit", "message": "staged"}), &ctx).await;
    assert!(!committed.is_error, "{:?}", committed.content);
}

/// A commit that reuses an existing message needs no `message`, and used to be unreachable.
#[tokio::test]
async fn amending_with_no_edit_keeps_the_message_it_already_had() {
    let tool = GitTool::new();
    let tmp_dir = TempDir::new().unwrap();
    if !init_repo(tmp_dir.path()) {
        eprintln!("skipping: git unavailable");
        return;
    }
    let ctx = test_context(tmp_dir.path());

    std::fs::write(tmp_dir.path().join("a.txt"), "one").unwrap();
    tool.run(serde_json::json!({"action": "add", "args": ["."]}), &ctx).await;
    tool.run(serde_json::json!({"action": "commit", "message": "the subject"}), &ctx).await;

    std::fs::write(tmp_dir.path().join("a.txt"), "two").unwrap();
    tool.run(serde_json::json!({"action": "add", "args": ["."]}), &ctx).await;
    let amended = tool
        .run(serde_json::json!({"action": "commit", "args": ["--amend", "--no-edit"]}), &ctx)
        .await;
    let luxe_core::ContentBlock::Text(text) = &amended.content[0] else { panic!("expected text") };
    assert!(!amended.is_error, "{text}");

    let log = tool.run(serde_json::json!({"action": "log", "args": ["--format=%s"]}), &ctx).await;
    let luxe_core::ContentBlock::Text(text) = &log.content[0] else { panic!("expected text") };
    assert_eq!(text.lines().collect::<Vec<_>>(), ["the subject"], "one commit, same subject");
}

/// The `stash` action forwards what it was given, proven against git rather than against
/// [`stash_args`] alone — the argument list was already right in isolation, and the arm calling
/// it is where the label used to go missing.
#[tokio::test]
async fn a_named_stash_keeps_its_name() {
    let tool = GitTool::new();
    let tmp_dir = TempDir::new().unwrap();
    if !init_repo(tmp_dir.path()) {
        eprintln!("skipping: git unavailable");
        return;
    }
    let ctx = test_context(tmp_dir.path());
    let file = tmp_dir.path().join("a.txt");

    std::fs::write(&file, "one").unwrap();
    tool.run(serde_json::json!({"action": "add", "args": ["."]}), &ctx).await;
    tool.run(serde_json::json!({"action": "commit", "message": "base"}), &ctx).await;
    std::fs::write(&file, "two").unwrap();

    let stashed = tool
        .run(
            serde_json::json!({"action": "stash", "args": ["push", "-m", "the named entry"]}),
            &ctx,
        )
        .await;
    let luxe_core::ContentBlock::Text(text) = &stashed.content[0] else { panic!("expected text") };
    assert!(!stashed.is_error, "{text}");

    let listed = std::process::Command::new("git")
        .args(["stash", "list"])
        .current_dir(tmp_dir.path())
        .output()
        .expect("git runs");
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(listed.contains("the named entry"), "the label reached git: {listed:?}");
}

/// Git asking for an editor is a hang everywhere else; here it is closed off, and the reply
/// names the two arguments that would have worked instead of git's `-m`/`-F`.
#[test]
fn an_editor_prompt_is_answered_with_the_arguments_this_tool_takes() {
    let explained = explain_failure(
        "error: there was a problem with the editor 'false'\n\
             Please supply the message using either -m or -F option.",
        "",
        std::path::Path::new("/tmp/work"),
    );
    assert!(explained.contains("`message`"), "{explained}");
    assert!(explained.contains("--no-edit"), "{explained}");
}

/// A repository that can commit: `init` plus the identity git refuses to invent.
fn init_repo(path: &std::path::Path) -> bool {
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(path)
            .output()
            .is_ok_and(|output| output.status.success())
    };
    run(&["init", "--quiet"])
        && run(&["config", "user.email", "test@luxe.invalid"])
        && run(&["config", "user.name", "Luxe Test"])
        // A signing key the machine may not have would fail every commit here.
        && run(&["config", "commit.gpgsign", "false"])
}

#[tokio::test]
async fn a_clean_tree_says_so_instead_of_returning_nothing() {
    let tool = GitTool::new();
    let tmp_dir = TempDir::new().unwrap();
    // A real (empty) repo: `git status --short` prints nothing at all.
    let init = tokio::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(tmp_dir.path())
        .status()
        .await;
    if !matches!(init, Ok(status) if status.success()) {
        eprintln!("skipping: git unavailable");
        return;
    }
    let ctx = test_context(tmp_dir.path());

    let result = tool.run(serde_json::json!({"action": "status"}), &ctx).await;
    assert!(!result.is_error);
    let luxe_core::ContentBlock::Text(text) = &result.content[0] else {
        panic!("expected text");
    };
    assert!(text.contains("clean"), "an empty status explains itself: {text:?}");

    let diff = tool.run(serde_json::json!({"action": "diff"}), &ctx).await;
    let luxe_core::ContentBlock::Text(text) = &diff.content[0] else { panic!("expected text") };
    assert!(text.contains("no changes"), "an empty diff explains itself: {text:?}");
}

/// A failing `gh` hands its own stderr back, rather than an empty complaint.
///
/// The break this catches, measured against the installed binary: `spawn` leaves stdout and
/// stderr **inherited** — only `Command::output` pipes them — so this call reported
/// "gh command failed:" with nothing after the colon while `gh`'s actual message ("no git
/// remotes found") went to the terminal, on top of the TUI. Runs only where `gh` is installed,
/// in a directory with no remote, which is a reliable way to make it fail without a network.
#[tokio::test]
async fn a_failing_gh_reports_what_gh_said() {
    if !luxe_core::program::is_available("gh") {
        return;
    }
    let tmp_dir = TempDir::new().unwrap();
    // A repository with no remote: `gh pr list` refuses immediately and says why.
    std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(tmp_dir.path())
        .output()
        .expect("git runs");

    let tool = GitTool::new();
    let ctx = test_context(tmp_dir.path());
    let result = tool.run(serde_json::json!({"action": "pr"}), &ctx).await;

    assert!(result.is_error, "a repository with no remote has no pull requests");
    let luxe_core::ContentBlock::Text(text) = &result.content[0] else {
        panic!("Expected Text content block")
    };

    // The property, not the particular complaint. `gh` fails here for whatever reason the
    // machine supplies — "no git remotes found" on a developer's box, "set the GH_TOKEN
    // environment variable" on a CI runner — and asserting either one pins the test to one
    // environment, which is how the first version of it passed locally and failed in CI. What
    // has to hold everywhere is that a reason comes back: gh's own words, or the sentence a
    // recognized cause is rewritten into. The bug was an empty tail.
    let reason = text
        .strip_prefix("gh pr failed:")
        .map(|tail| tail.trim().trim_start_matches("gh command failed:").trim())
        .unwrap_or_default();
    assert!(!reason.is_empty(), "a reason must come back, got {text:?}");
    assert!(!reason.contains("(no output)"), "a reason must come back, got {text:?}");
}

/// Without `gh` on PATH, `pr` says so and says what to do about it.
///
/// The break this catches: the old answer was "GitHub CLI integration is not enabled", gated on
/// a `GitConfig` flag that nothing could set — `GitConfig::default()` is what registers the tool
/// — so the action was unreachable and its error named an internal switch instead of the program
/// the user has to install. Skipped where `gh` really is installed, since then there is nothing
/// to report.
#[tokio::test]
async fn pr_without_the_github_cli_names_the_missing_program() {
    if luxe_core::program::is_available("gh") {
        return;
    }
    let tool = GitTool::new();
    let tmp_dir = TempDir::new().unwrap();
    let ctx = test_context(tmp_dir.path());

    let result = tool.run(serde_json::json!({"action": "pr"}), &ctx).await;
    assert!(result.is_error);
    let luxe_core::ContentBlock::Text(text) = &result.content[0] else {
        panic!("Expected Text content block")
    };
    assert!(text.contains("`gh`"), "name the program: {text}");
    assert!(text.contains("auth login"), "and what it needs after installing: {text}");
}

/// Outside a repo, git answers `status` with a filesystem-boundary note and `diff` with its
/// entire usage screen. Neither states the fact that matters.
#[test]
fn outside_a_repository_the_error_says_so_in_one_line() {
    let stderr = "fatal: not a git repository (or any parent up to mount point /)\n\
                      Stopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set).";
    let explained = explain_failure(stderr, "", std::path::Path::new("/tmp/work"));

    assert!(explained.starts_with("Not a git repository"), "{explained}");
    assert!(explained.contains("/tmp/work"), "names the directory: {explained}");
    assert!(explained.contains("read"), "and says what to do instead: {explained}");
    assert!(!explained.contains("GIT_DISCOVERY"), "plumbing noise is dropped: {explained}");
}

#[test]
fn a_usage_dump_is_bounded_rather_than_pasted_whole() {
    let usage = vec!["    -p, --patch  generate patch"; 40].join("\n");
    let explained = explain_failure(&usage, "", std::path::Path::new("/tmp/work"));
    assert!(explained.lines().count() <= GIT_ERROR_MAX_LINES + 3, "{explained}");
    assert!(explained.contains("more lines not shown"));
}

#[test]
fn a_real_git_error_keeps_gits_own_words() {
    let explained =
        explain_failure("error: pathspec 'nope' did not match", "", std::path::Path::new("."));
    assert!(explained.contains("pathspec 'nope' did not match"), "{explained}");
}

#[test]
fn an_empty_stderr_still_says_something() {
    assert!(explain_failure("   ", "", std::path::Path::new(".")).contains("no output"));
}

/// Git says why a commit was refused on **stdout**, and exits 1 with an empty stderr. Reading
/// one stream reported "no output" for the single most common commit failure there is.
#[test]
fn a_refusal_git_wrote_to_stdout_is_not_reported_as_no_output() {
    let refused = "On branch master\nChanges not staged for commit:\n\tmodified: f.txt\n\n\
                       no changes added to commit (use \"git add\" and/or \"git commit -a\")";
    let explained = explain_failure("", refused, std::path::Path::new("."));

    assert!(!explained.contains("no output"), "{explained}");
    assert!(explained.contains("Nothing is staged"), "names the cause: {explained}");
    assert!(explained.contains("\"action\": \"add\""), "and this tool's fix: {explained}");
    assert!(explained.contains("-a"), "and the one-call alternative: {explained}");
}

/// The three refusals are corrected differently, so each gets its own answer: `-a` picks up a
/// tracked modification and will never pick up an untracked file.
#[test]
fn each_reason_a_commit_is_refused_gets_its_own_answer() {
    let untracked = explain_failure(
        "",
        "nothing added to commit but untracked files present (use \"git add\" to track)",
        std::path::Path::new("."),
    );
    assert!(untracked.contains("never tracked"), "{untracked}");
    assert!(untracked.contains("`-a` does not"), "and why -a will not do: {untracked}");

    let clean =
        explain_failure("", "nothing to commit, working tree clean", std::path::Path::new("."));
    assert!(clean.contains("already committed"), "{clean}");
    assert!(!clean.contains("Nothing to commit:"), "not two colons in one line: {clean}");
    assert!(!clean.contains("Stage them"), "there is nothing to stage: {clean}");
}

/// When git splits its report across both streams, neither half is dropped.
#[test]
fn both_streams_reach_the_caller() {
    let explained = explain_failure(
        "Recorded preimage for 'f.txt'",
        "CONFLICT (content): Merge conflict in f.txt",
        std::path::Path::new("."),
    );
    assert!(explained.contains("CONFLICT (content)"), "{explained}");
    assert!(explained.contains("Recorded preimage"), "{explained}");
}
