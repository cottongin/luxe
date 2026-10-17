use super::*;
use clap::Parser;

/// What a command line classifies as, for the two questions startup asks before dispatch.
///
/// The classification is an exhaustive match, so a new [`SubCommand`] variant does not compile
/// until somebody answers for it; this pins the answers that are already given. The stakes are
/// asymmetric: a session wrongly classed as reporting loses its project log, and a reporting
/// command wrongly classed as a session leaves a `.luxe/` that silently re-roots every
/// repository beneath it.
#[test]
fn only_the_commands_that_own_project_state_may_create_one() {
    let parse = |line: &[&str]| Args::try_parse_from(line).expect("a valid command line");

    for session in [
        vec!["luxe"],
        vec!["luxe", "--headless", "-p", "hi"],
        vec!["luxe", "--rpc"],
        vec!["luxe", "loop", "start"],
        vec!["luxe", "loop", "status"],
        vec!["luxe", "mcp", "add", "--id", "x", "--name", "x", "--command", "x"],
        vec!["luxe", "mcp", "remove", "--id", "x"],
    ] {
        assert!(parse(&session).owns_project_state(), "{session:?} writes into the project");
    }

    for reporting in [
        vec!["luxe", "auth", "list"],
        vec!["luxe", "doctor"],
        vec!["luxe", "init"],
        vec!["luxe", "models", "list"],
        vec!["luxe", "sessions", "list"],
        vec!["luxe", "wire", "/tmp/log"],
        vec!["luxe", "mcp", "list"],
        vec!["luxe", "mcp", "tools"],
    ] {
        assert!(
            !parse(&reporting).owns_project_state(),
            "{reporting:?} only reads, so it must leave no .luxe/ behind"
        );
    }
}

/// `--github-writes` is a grant to an RPC client, so it means nothing anywhere else.
///
/// Accepting it on the TUI or headless would read as "the agent may now publish", which it
/// does not say and does not do.
#[test]
fn github_writes_is_only_for_an_rpc_client() {
    let parse = |line: &[&str]| Args::try_parse_from(line).expect("a valid command line");
    let granted = parse(&["luxe", "--rpc", "--github-writes"]);
    assert!(granted.github_writes && !granted.github_writes_misplaced());
    assert!(!parse(&["luxe", "--rpc"]).github_writes, "off by default");
    assert!(!parse(&["luxe"]).github_writes_misplaced(), "absent is never misplaced");
    for elsewhere in [
        vec!["luxe", "--github-writes"],
        vec!["luxe", "--headless", "-p", "hi", "--github-writes"],
        vec!["luxe", "--rpc", "--github-writes", "doctor"],
    ] {
        assert!(parse(&elsewhere).github_writes_misplaced(), "{elsewhere:?}");
    }
}

/// `--on-ask` settles the answer for a run with nobody to ask, so the surfaces that *can* ask
/// have to be told apart from the ones that cannot — on the TUI and RPC the flag would read as
/// a policy and do nothing, which is the shape of mistake nobody finds until an agent has been
/// running unwatched.
#[test]
fn only_a_run_with_nobody_to_ask_can_be_told_what_an_ask_becomes() {
    let parse = |line: &[&str]| Args::try_parse_from(line).expect("a valid command line");

    for interactive in [vec!["luxe"], vec!["luxe", "--rpc"]] {
        assert!(parse(&interactive).prompts_a_human(), "{interactive:?}");
    }
    for unattended in [
        vec!["luxe", "--headless", "-p", "hi"],
        vec!["luxe", "loop", "start"],
        vec!["luxe", "doctor"],
    ] {
        assert!(!parse(&unattended).prompts_a_human(), "{unattended:?}");
    }

    // A misspelled policy stops the run rather than being ignored — unlike `--thinking`, which
    // warns and carries on, because that is a preference and this is not.
    assert!(
        Args::try_parse_from(["luxe", "--headless", "-p", "hi", "--on-ask", "dney"]).is_err(),
        "an unreadable policy must not resolve to the permissive answer"
    );
    assert_eq!(
        parse(&["luxe", "--headless", "-p", "hi", "--on-ask", "deny"]).on_ask,
        Some(OnAskArg::Deny)
    );

    // The two flags are opposite answers to one question; asking for both is a mistake.
    assert!(
        Args::try_parse_from(["luxe", "--auto-approve", "--on-ask", "allow"]).is_err(),
        "--auto-approve and --on-ask cannot both be in force"
    );
}

/// A notice reaches the reader through the engine where there is one, and is printed where
/// there is not. Getting this wrong writes on the TUI's drawing surface.
#[test]
fn a_startup_notice_goes_through_the_engine_only_where_one_runs() {
    let parse = |line: &[&str]| Args::try_parse_from(line).expect("a valid command line");

    for engine in
        [vec!["luxe"], vec!["luxe", "--headless", "-p", "hi"], vec!["luxe", "loop", "start"]]
    {
        assert!(parse(&engine).starts_an_engine(), "{engine:?}");
    }
    for printed in
        [vec!["luxe", "doctor"], vec!["luxe", "loop", "status"], vec!["luxe", "auth", "list"]]
    {
        assert!(!parse(&printed).starts_an_engine(), "{printed:?}");
    }
}

/// Environment and the real config directory are process-global; serialize what touches them.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A project whose config starts sessions in plan mode, and an isolated global config.
fn project_that_plans() -> (tempfile::TempDir, tempfile::TempDir) {
    let home = tempfile::tempdir().expect("temp home");
    std::env::set_var("XDG_CONFIG_HOME", home.path());
    std::env::set_var("XDG_STATE_HOME", home.path().join("state"));
    for key in ["LUXE_PROVIDER", "LUXE_MODEL", "LUXE_API_KEY", "ANTHROPIC_API_KEY"] {
        std::env::remove_var(key);
    }
    let project = tempfile::tempdir().expect("temp project");
    std::fs::create_dir_all(project.path().join(".luxe")).expect("project config dir");
    std::fs::write(
        project.path().join(".luxe").join("config.json"),
        r#"{"agent":{"default_mode":"plan"}}"#,
    )
    .expect("write project config");
    (home, project)
}

/// `agent.default_mode` decides where a session starts; `build` is not worth sending.
#[test]
fn the_configured_default_mode_is_where_a_session_starts() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (_home, project) = project_that_plans();
    let settings = crate::engine_setup::AgentSettings::resolve(
        project.path().to_path_buf(),
        crate::engine_setup::CliOverrides {
            provider: Some("lmstudio".into()),
            ..Default::default()
        },
    );
    assert_eq!(settings.initial_mode.as_deref(), Some(luxe_core::mode::PLAN));
    assert!(!settings.review_engine, "an ordinary session is not a reviewer");

    std::fs::write(
        project.path().join(".luxe").join("config.json"),
        r#"{"agent":{"default_mode":"build"}}"#,
    )
    .unwrap();
    let settings = crate::engine_setup::AgentSettings::resolve(
        project.path().to_path_buf(),
        crate::engine_setup::CliOverrides::default(),
    );
    assert_eq!(settings.initial_mode, None, "the default needs no switch");
}

#[test]
fn plan_is_a_flag_every_command_line_takes() {
    let parse = |line: &[&str]| Args::try_parse_from(line).expect("a valid command line");
    assert!(parse(&["luxe", "--plan"]).plan);
    assert!(parse(&["luxe", "--headless", "--plan", "-p", "plan it"]).plan);
    assert!(!parse(&["luxe"]).plan);
    assert!(Args::try_parse_from(["luxe", "--role", "planner"]).is_err(), "--role is gone");
}

/// `--from-pr` names a pull request, and it is one way of choosing a session among three: it
/// cannot be combined with naming a session or continuing the latest.
#[test]
fn from_pr_resumes_by_pull_request_and_excludes_the_other_resumes() {
    use crate::engine_setup::ResumeMode;
    let parse = |line: &[&str]| Args::try_parse_from(line).expect("a valid command line");
    assert!(matches!(
        resume_from_args(&parse(&["luxe", "--from-pr", "146"])),
        ResumeMode::PullRequest(146)
    ));
    assert!(matches!(resume_from_args(&parse(&["luxe"])), ResumeMode::New));
    assert!(Args::try_parse_from(["luxe", "--from-pr", "146", "--continue"]).is_err());
    assert!(Args::try_parse_from(["luxe", "--from-pr", "146", "--resume", "abc"]).is_err());
    assert!(Args::try_parse_from(["luxe", "--from-pr", "#146"]).is_err(), "a number, no hash");
}

/// `--pr` starts new work on a pull request, so it goes with nothing that picks a session, an
/// issue, or a branch of its own.
#[test]
fn pr_starts_new_work_and_excludes_every_other_start() {
    let parsed = Args::try_parse_from(["luxe", "--pr", "142"]).expect("valid");
    assert_eq!(parsed.pr, Some(142));
    assert!(matches!(resume_from_args(&parsed), crate::engine_setup::ResumeMode::New));
    for other in [
        &["--from-pr", "142"][..],
        &["--continue"],
        &["--resume", "abc"],
        &["--issue", "7"],
        &["--worktree"],
    ] {
        let mut line = vec!["luxe", "--pr", "142"];
        line.extend_from_slice(other);
        assert!(Args::try_parse_from(line).is_err(), "{other:?}");
    }
}

/// What a flag combination is refused with, or `None` when it is allowed.
fn refusal(line: &[&str]) -> Option<String> {
    let args = Args::try_parse_from(line).expect("clap accepts the line");
    args.validate().err().map(|error| {
        let usage = error.downcast_ref::<UsageError>().expect("a refusal is a usage error");
        usage.to_string()
    })
}

/// `--on-ask` settles what an `ask` becomes where nobody can answer it, so on the two surfaces
/// where somebody does it would read as a policy and do nothing.
#[test]
fn on_ask_is_refused_where_a_person_answers() {
    for line in [vec!["luxe", "--on-ask", "deny"], vec!["luxe", "--rpc", "--on-ask", "deny"]] {
        let said = refusal(&line).expect("refused");
        assert!(said.contains("--on-ask settles what an `ask` becomes"), "{line:?}: {said}");
    }
    for line in [
        vec!["luxe", "--headless", "-p", "hi", "--on-ask", "deny"],
        vec!["luxe", "loop", "start", "--on-ask", "deny"],
    ] {
        assert_eq!(refusal(&line), None, "{line:?} has nobody to answer");
    }
}

/// `--github-writes` lets an `--rpc` client publish; anywhere else it would read as the agent being
/// allowed to.
#[test]
fn github_writes_is_refused_outside_rpc() {
    let said = refusal(&["luxe", "--github-writes"]).expect("refused");
    assert!(said.contains("--github-writes lets an --rpc client publish"), "{said}");
    assert!(refusal(&["luxe", "--headless", "-p", "x", "--github-writes"]).is_some());
    assert_eq!(refusal(&["luxe", "--rpc", "--github-writes"]), None);
}

/// A headless run with nothing to run is refused before anything else happens, a worktree made for
/// it included.
#[test]
fn a_headless_run_needs_a_prompt() {
    let said = refusal(&["luxe", "--headless"]).expect("refused");
    assert_eq!(said, "--headless requires --prompt <text>");
    assert_eq!(refusal(&["luxe", "--headless", "-p", "hi"]), None);
    // a subcommand is not a headless run, nor is a session with a person at it
    assert_eq!(refusal(&["luxe", "doctor"]), None);
    assert_eq!(refusal(&["luxe"]), None);
}
