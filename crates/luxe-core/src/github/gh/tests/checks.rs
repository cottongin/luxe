//! Checks — failing checks, their jobs, and the tail of a failed log.

use super::*;

pub(super) const CHECK_RUNS: &str = include_str!("../../fixtures/check_runs.jsonl");
pub(super) const COMMIT_STATUSES: &str = include_str!("../../fixtures/commit_statuses.jsonl");
pub(super) const LOG_FAILED: &str = include_str!("../../fixtures/log_failed.txt");

#[test]
fn a_branch_with_a_passing_pull_request_reads_its_counts_and_budget() {
    let status = parse_branch_status(STATUS_OPEN, "topic", "nora-quinn").unwrap();
    let pr = status.pull_request.expect("the branch has a pull request");
    assert_eq!(pr.number, 138);
    assert_eq!(pr.state, PullRequestState::Open);
    assert!(!pr.draft);
    assert_eq!(pr.review_decision, None);
    assert_eq!(pr.checks, CheckRollup { passed: 5, ..CheckRollup::default() });
    assert_eq!(pr.unresolved_threads, 0);
    assert_eq!(pr.head_oid, "399286814564627352030020a163bfcbaeafe166");
    assert!(pr.url.ends_with("/pull/138"));
    let limit = status.rate_limit.expect("the query asks for its budget");
    assert!(limit.remaining > 0 && limit.remaining <= 5000);
    assert!(limit.reset_at > 1_780_000_000, "{}", limit.reset_at);
}

#[test]
fn a_branch_without_one_has_none_and_still_reports_the_budget() {
    let status = parse_branch_status(STATUS_NONE, "topic", "nora-quinn").unwrap();
    assert_eq!(status.pull_request, None);
    assert!(status.rate_limit.is_some());
}

/// A closed pull request with a failing check is still the branch's, and says both.
#[test]
fn a_closed_pull_request_with_a_failing_check_says_both() {
    let pr = parse_branch_status(STATUS_FAILING, "topic", "nora-quinn").unwrap().pull_request;
    let pr = pr.expect("the branch had a pull request");
    assert_eq!(pr.state, PullRequestState::Closed);
    assert_eq!(pr.checks, CheckRollup { passed: 4, failed: 1, ..CheckRollup::default() });
}

#[test]
fn no_checks_at_all_counts_nothing() {
    let pr = parse_branch_status(STATUS_NO_CHECKS, "topic", "nora-quinn").unwrap().pull_request;
    assert_eq!(pr.expect("pull request").checks.total(), 0);
}

/// A fork's pull request is the fork owner's, whatever the branch is called: seen from the
/// upstream's own owner it does not exist.
#[test]
fn a_forks_pull_request_belongs_to_the_fork_owner() {
    let pr = parse_branch_status(STATUS_FORK, "topic", "sebtardif").unwrap().pull_request;
    let pr = pr.expect("the fork owner's branch has a pull request");
    assert_eq!(pr.number, 13340);
    assert_eq!(pr.unresolved_threads, 2);
    assert_eq!(pr.review_decision, Some(ReviewDecision::ReviewRequired));
    assert_eq!(parse_branch_status(STATUS_FORK, "topic", "cli").unwrap().pull_request, None);
}

#[test]
fn an_open_pull_request_wins_over_a_newer_closed_one() {
    let body = r#"{"data":{"repository":{"pullRequests":{"nodes":[
            {"number":2,"title":"t","url":"u","state":"CLOSED","isDraft":false,"headRefOid":"b",
             "headRepositoryOwner":{"login":"me"},"reviewDecision":null,"commits":{"nodes":[]},
             "reviewThreads":{"totalCount":0,"nodes":[]}},
            {"number":1,"title":"t","url":"u","state":"OPEN","isDraft":true,"headRefOid":"a",
             "headRepositoryOwner":{"login":"me"},"reviewDecision":"CHANGES_REQUESTED",
             "commits":{"nodes":[]},"reviewThreads":{"totalCount":0,"nodes":[]}}]}},
            "rateLimit":null}}"#;
    let pr = parse_branch_status(body, "topic", "me").unwrap().pull_request.unwrap();
    assert_eq!((pr.number, pr.draft), (1, true));
    assert_eq!(pr.review_decision, Some(ReviewDecision::ChangesRequested));
}

#[test]
fn a_title_is_filtered_flattened_and_bounded() {
    assert_eq!(status_text("Fix\n  the\tthing<!-- hidden -->"), "Fix the thing");
    let long = status_text(&"x".repeat(500));
    assert_eq!(long.chars().count(), STATUS_TEXT_MAX_CHARS);
    assert!(long.ends_with('…'));
}

#[test]
fn github_timestamps_become_epoch_seconds() {
    assert_eq!(rfc3339_seconds("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(rfc3339_seconds("2000-03-01T00:00:00Z"), Some(951_868_800));
    assert_eq!(rfc3339_seconds("2026-09-24T19:05:02Z"), Some(1_790_276_702));
    assert_eq!(rfc3339_seconds("2024-02-29T23:59:59.5Z"), Some(1_709_251_199));
    for bad in [
        "",
        "2026-09-24",
        "2026-13-01T00:00:00Z",
        "2026-09-24T24:00:00Z",
        "2026-09-24T10:60:00Z",
        "2026-09-24T19:05:02+01:00",
    ] {
        assert_eq!(rfc3339_seconds(bad), None, "{bad}");
    }
}

#[test]
fn failing_checks_are_the_failed_runs_and_statuses_with_their_jobs() {
    let failing = parse_failing_checks(CHECK_RUNS, COMMIT_STATUSES).unwrap();
    let names: Vec<_> =
        failing.iter().map(|(check, job)| (check.name.as_str(), job.as_deref())).collect();
    assert_eq!(
        names,
        [
            ("build · clippy · test (macos-latest)", Some("106993218139")),
            ("build · test · clippy · fmt", Some("106993218126")),
            ("build · clippy · test (windows-latest)", Some("106993217890")),
            ("Azure Pipelines CI", None),
        ]
    );
    let (azure, _) = &failing[3];
    assert_eq!(azure.outcome, "failure");
    assert_eq!(azure.summary.as_deref(), Some("#20260924.53 failed"));
    assert!(azure.url.starts_with("https://dev.azure.com/"));
    assert!(failing.iter().all(|(check, _)| check.log == CheckLog::NotActions));
}

/// An empty title is no summary, and does not hide the one the check wrote.
#[test]
fn a_check_summary_skips_an_empty_title() {
    let summary = |title: &str, summary: &str| {
        let line = format!(
            r#"{{"name":"lint","status":"completed","conclusion":"failure","html_url":"h","details_url":null,"app":"other","title":{title},"summary":{summary}}}"#
        );
        parse_failing_checks(&line, "").unwrap()[0].0.summary.clone()
    };
    assert_eq!(summary("\"\"", "\"3 errors\"").as_deref(), Some("3 errors"));
    assert_eq!(summary("\"Lint failed\"", "\"3 errors\"").as_deref(), Some("Lint failed"));
    assert_eq!(summary("null", "\"  \""), None);
}

#[test]
fn check_counts_land_in_the_bucket_their_state_names() {
    let count = |state: &str, count: usize| StateCount { state: state.into(), count };
    let contexts = RollupContexts {
        check_run_counts_by_state: vec![
            count("SUCCESS", 3),
            count("SKIPPED", 1),
            count("NEUTRAL", 1),
            count("FAILURE", 2),
            count("TIMED_OUT", 1),
            count("QUEUED", 1),
            count("IN_PROGRESS", 2),
            count("COMPLETED", 9),
        ],
        status_context_counts_by_state: vec![
            count("SUCCESS", 1),
            count("PENDING", 1),
            count("EXPECTED", 1),
            count("ERROR", 1),
            count("FAILURE", 1),
        ],
    };
    assert_eq!(
        count_checks(&contexts),
        CheckRollup { passed: 4, failed: 5, pending: 5, skipped: 2 }
    );
}

#[test]
fn a_merged_and_approved_pull_request_says_so() {
    let body = r#"{"data":{"repository":{"pullRequests":{"nodes":[
            {"number":7,"title":"t","url":"u","state":"MERGED","isDraft":false,"headRefOid":"a",
             "headRepositoryOwner":{"login":"me"},"reviewDecision":"APPROVED",
             "commits":{"nodes":[]},"reviewThreads":{"totalCount":0,"nodes":[]}}]}},
            "rateLimit":null}}"#;
    let pr = parse_branch_status(body, "topic", "me").unwrap().pull_request.unwrap();
    assert_eq!(pr.state, PullRequestState::Merged);
    assert_eq!(pr.review_decision, Some(ReviewDecision::Approved));
}

#[test]
fn a_running_check_has_not_failed() {
    let runs = r#"{"name":"ci","status":"in_progress","conclusion":"failure","html_url":"h","details_url":null,"app":"github-actions","title":null,"summary":null}"#;
    assert!(parse_failing_checks(runs, "").unwrap().is_empty());
}

#[test]
fn an_actions_job_id_comes_from_its_address() {
    let url = "https://github.com/o/r/actions/runs/35801682338/job/106993218126";
    assert_eq!(actions_job(url).as_deref(), Some("106993218126"));
    assert_eq!(actions_job(&format!("{url}?pr=3")).as_deref(), Some("106993218126"));
    assert_eq!(actions_job("https://github.com/o/r/actions/runs/1"), None);
    assert_eq!(actions_job("https://github.com/o/r/actions/runs/1/job/x1"), None);
    assert_eq!(actions_job("https://ci.example/job/12"), None);
}

/// The log's tail is the error, without the timestamps, color codes or byte-order mark that
/// would otherwise fill the lines around it.
#[test]
fn a_failed_log_is_cut_to_the_tail_of_each_step() {
    let steps = parse_failed_log(LOG_FAILED, 12);
    assert_eq!(steps.len(), 1);
    let step = &steps[0];
    assert_eq!(step.name, "Clippy (deny warnings)");
    assert_eq!(step.tail.len(), 12);
    assert_eq!(step.omitted + step.tail.len(), LOG_FAILED.lines().count());
    assert_eq!(step.tail.last().unwrap(), "##[error]Process completed with exit code 101.");
    assert!(step.tail.iter().any(|line| line.contains("error[E0425]")), "{:#?}", step.tail);
    for line in &step.tail {
        assert!(!line.contains('\u{1b}') && !line.contains("2026-09-23T"), "{line}");
        assert!(!line.contains("^["), "{line}");
    }
    let all = parse_failed_log(LOG_FAILED, usize::MAX);
    assert_eq!(all[0].omitted, 0);
    assert!(all[0].tail[0].starts_with("##[group]Run cargo clippy"), "{}", all[0].tail[0]);
    assert_eq!(all[0].tail[1], "cargo clippy --workspace --all-targets --locked -- -D warnings");
}

/// When `gh` could not split the log by step it prints the whole job, cleanup and all; the
/// tail ends at the last error, and what came after is counted, not shown.
#[test]
fn the_tail_ends_at_the_last_error_not_at_the_cleanup() {
    let log = "job\tUNKNOWN STEP\t2026-01-01T00:00:00Z compiling\n\
                   job\tUNKNOWN STEP\t2026-01-01T00:00:00Z error[E0425]: nope\n\
                   job\tUNKNOWN STEP\t2026-01-01T00:00:00Z ##[error]Process completed with exit code 101.\n\
                   job\tUNKNOWN STEP\t2026-01-01T00:00:00Z Post job cleanup.\n\
                   job\tUNKNOWN STEP\t2026-01-01T00:00:00Z Cleaning up orphan processes\n";
    let steps = parse_failed_log(log, 2);
    assert_eq!(
        steps[0].tail,
        ["error[E0425]: nope", "##[error]Process completed with exit code 101."]
    );
    assert_eq!((steps[0].omitted, steps[0].after), (1, 2));
    let clean = parse_failed_log(LOG_FAILED, 5);
    assert_eq!(clean[0].after, 0, "the error was the last line already");
}

#[test]
fn steps_keep_their_order_and_trailing_blank_lines_go() {
    let log = "job\ta\t2026-01-01T00:00:00Z one\njob\tb\t2026-01-01T00:00:00Z two\n\
                   job\tb\t2026-01-01T00:00:00Z \nnot a log line\n";
    let steps = parse_failed_log(log, 5);
    assert_eq!(steps.len(), 2);
    assert_eq!((steps[0].name.as_str(), steps[0].tail.clone()), ("a", vec!["one".to_string()]));
    assert_eq!((steps[1].name.as_str(), steps[1].tail.clone()), ("b", vec!["two".to_string()]));
    assert_eq!(steps[1].after, 1, "the blank line is counted as left out");
}

#[test]
fn color_codes_go_and_everything_else_stays() {
    assert_eq!(strip_ansi("\u{1b}[1m\u{1b}[91merror\u{1b}[0m: x"), "error: x");
    assert_eq!(strip_ansi("plain [1m"), "plain [1m");
    assert_eq!(strip_ansi("^[[36;1mcargo^[[0m"), "cargo");
    assert_eq!(strip_ansi("x ^ [y] ^ok"), "x ^ [y] ^ok");
    assert_eq!(without_timestamp("2026-09-23T00:21:48Z text here"), "text here");
    assert_eq!(without_timestamp("\u{feff}2026-09-23T00:21:48Z text"), "text");
    assert_eq!(without_timestamp("warning: 2026Z"), "warning: 2026Z");
    assert_eq!(without_timestamp("2026 was a year"), "2026 was a year");
}

/// Through a fake `gh`: the check listing, the statuses, and a log for each failing Actions
/// job up to the limit, the rest marked as not read.
#[cfg(unix)]
#[tokio::test]
async fn failing_checks_read_logs_for_the_first_few_actions_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/github/fixtures");
    let script = format!(
        "case \"$*\" in\n\
             *check-runs*) cat '{runs}' '{runs}' ;;\n\
             *commits/*/status*) cat '{statuses}' ;;\n\
             *106993217890*) echo 'HTTP 410: logs expired' >&2; exit 1 ;;\n\
             *run\\ view*) cat '{log}' ;;\n\
             esac",
        runs = fixtures.join("check_runs.jsonl").display(),
        statuses = fixtures.join("commit_statuses.jsonl").display(),
        log = fixtures.join("log_failed.txt").display(),
    );
    let gh = GhCli { deadline: GH_DEADLINE, program: Some(fake_gh(dir.path(), &script)) };
    let repository = Repository {
        host: "github.com".into(),
        owner: "o".into(),
        name: "r".into(),
        default_branch: String::new(),
        remote: None,
    };

    let checks = gh.failing_checks(&repository, "1ee6ffe", 5).await.unwrap();

    // The listing twice over: six failing Actions jobs, of which the first four are read
    assert_eq!(checks.len(), 7);
    assert!(matches!(&checks[0].log, CheckLog::Steps(steps) if steps[0].tail.len() == 5));
    assert!(matches!(&checks[2].log, CheckLog::Unreadable(reason) if reason.contains("410")));
    assert!(matches!(&checks[3].log, CheckLog::Steps(_)));
    assert_eq!(checks[4].log, CheckLog::NotRead);
    assert_eq!(checks[5].log, CheckLog::NotRead);
    assert_eq!(checks[6].log, CheckLog::NotActions);
}
