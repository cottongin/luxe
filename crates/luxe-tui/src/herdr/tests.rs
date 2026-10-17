use super::*;

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let pairs: Vec<(String, String)> =
        pairs.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect();
    move |name: &str| pairs.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone())
}

#[test]
fn only_a_whole_herdr_environment_is_a_pane() {
    let whole =
        [("HERDR_ENV", "1"), ("HERDR_PANE_ID", "w1:p2"), ("HERDR_BIN_PATH", "/usr/bin/herdr")];
    assert_eq!(
        pane(&env(&whole)),
        Some(Pane { bin: PathBuf::from("/usr/bin/herdr"), id: "w1:p2".into() })
    );
    for missing in 0..whole.len() {
        let partial: Vec<_> =
            whole.iter().enumerate().filter(|(i, _)| *i != missing).map(|(_, p)| *p).collect();
        assert_eq!(pane(&env(&partial)), None, "without {}", whole[missing].0);
    }
    let off = [("HERDR_ENV", "0"), ("HERDR_PANE_ID", "p"), ("HERDR_BIN_PATH", "/h")];
    assert_eq!(pane(&env(&off)), None, "HERDR_ENV must be 1");
    let blank = [("HERDR_ENV", "1"), ("HERDR_PANE_ID", " "), ("HERDR_BIN_PATH", "/h")];
    assert_eq!(pane(&env(&blank)), None, "a blank pane id is no pane");
}

/// Pinned against herdr's CLI: `pane report-agent <PANE> --source --agent --state --seq
/// [--message] [--agent-session-id] [-- RESUME_ARG...]`.
#[test]
fn a_report_is_written_the_way_herdr_reads_it() {
    let idle = Report { state: AgentState::Idle, session: None, seq: 7 };
    assert_eq!(
        report_args("w1:p2", &idle).join(" "),
        "pane report-agent w1:p2 --source luxe --agent luxe --state idle --seq 7"
    );
    let blocked = Report {
        state: AgentState::Blocked("Approve bash\nrm -rf build".into()),
        session: Some("0b5c-41d2".into()),
        seq: 8,
    };
    assert_eq!(
        report_args("w1:p2", &blocked),
        [
            "pane",
            "report-agent",
            "w1:p2",
            "--source",
            "luxe",
            "--agent",
            "luxe",
            "--state",
            "blocked",
            "--seq",
            "8",
            "--message",
            "Approve bash rm -rf build",
            "--agent-session-id",
            "0b5c-41d2",
            "--",
            "luxe",
            "--resume",
            "0b5c-41d2",
        ]
    );
    assert_eq!(
        release_args("w1:p2", 9).join(" "),
        "pane release-agent w1:p2 --source luxe --agent luxe --seq 9"
    );
}

/// herdr rejects the whole report over a bad resume argument, so a session id that could be one
/// is left off and the state still lands.
#[test]
fn a_session_id_that_could_break_the_report_is_left_off() {
    let report = Report { state: AgentState::Working, session: Some("it's".into()), seq: 1 };
    let args = report_args("p", &report);
    assert!(!args.iter().any(|arg| arg == "--"), "{args:?}");
    assert!(args.contains(&"working".to_string()));
    assert!(!resumable(""));
    assert!(resumable("d176e3f4-f698-47f3-b52f-28879c8d01b8"));
}

#[test]
fn a_long_message_is_cut_to_one_short_line() {
    let message = message_text(&format!("Question: {}", "word ".repeat(40)));
    assert_eq!(message.chars().count(), MAX_MESSAGE_CHARS);
    assert!(message.ends_with('…'));
    assert_eq!(message_text("a\u{7}b\tc"), "ab c");
    let exactly = "x".repeat(MAX_MESSAGE_CHARS);
    assert_eq!(message_text(&exactly), exactly, "one that fits is kept whole");
}

/// The whole path, against a stand-in for herdr that writes down what it was told: nothing
/// without the switch, a report per change once started, and the release on the way out.
#[cfg(unix)]
#[tokio::test]
async fn reports_and_the_release_reach_herdr() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("herdr");
    let log = dir.path().join("calls.log");
    std::fs::write(&bin, format!("#!/bin/sh\necho \"$@\" >> '{}'\n", log.display())).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let bin_path = bin.display().to_string();
    let lookup = |name: &str| match name {
        "HERDR_ENV" => Some("1".to_string()),
        "HERDR_PANE_ID" => Some("w1:p1".to_string()),
        "HERDR_BIN_PATH" => Some(bin_path.clone()),
        _ => None,
    };
    assert!(Reporter::start_in(false, &lookup).is_none(), "switched off: nothing starts");
    let mut reporter = Reporter::start_in(true, &lookup).expect("inside herdr");

    let calls = || std::fs::read_to_string(&log).unwrap_or_default();
    let wait_for = |needle: &'static str| {
        let calls = &calls;
        async move {
            for _ in 0..200 {
                if calls().contains(needle) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            panic!("herdr never heard {needle:?}: {}", calls());
        }
    };
    reporter.report(AgentState::Working, Some("abc-1"));
    wait_for("--state working").await;
    assert!(calls().contains("pane report-agent w1:p1 --source luxe --agent luxe"));
    assert!(calls().contains("-- luxe --resume abc-1"), "{}", calls());
    reporter.report(AgentState::Blocked("Approve bash".into()), Some("abc-1"));
    wait_for("--message Approve bash").await;
    reporter.release().await;
    assert!(calls().contains("pane release-agent w1:p1"), "{}", calls());
}

/// `LUXE_HERDR=0` stops the reports, as the guide says, while the pane itself is still a pane.
#[test]
fn the_opt_out_variable_starts_no_reporter() {
    for off in ["0", "false", "no", "off", " OFF "] {
        let lookup = env(&[
            ("HERDR_ENV", "1"),
            ("HERDR_PANE_ID", "w1:p1"),
            ("HERDR_BIN_PATH", "/bin/herdr"),
            ("LUXE_HERDR", off),
        ]);
        assert!(Reporter::start_in(true, &lookup).is_none(), "LUXE_HERDR={off:?}");
        assert!(pane(&lookup).is_some(), "the variable silences the reports, not the pane");
    }
}

#[test]
fn report_numbers_only_rise() {
    let mut sequence = Sequence::default();
    let first = sequence.next();
    let second = sequence.next();
    assert!(second > first);
    sequence.last = u64::MAX - 1;
    assert_eq!(sequence.next(), u64::MAX, "a clock behind the last number still rises");
}

/// The reporter's job between frames: one report per change, none for the same state twice.
#[tokio::test]
async fn only_a_change_is_handed_on() {
    let (newest, mut receiver) = watch::channel(None);
    let mut reporter = Reporter {
        pane: Pane { bin: PathBuf::from("/nonexistent/herdr"), id: "p".into() },
        last: None,
        sequence: Sequence::default(),
        newest,
        worker: tokio::spawn(async {}),
    };
    reporter.report(AgentState::Working, Some("s"));
    assert!(receiver.has_changed().unwrap());
    let first = receiver.borrow_and_update().clone().expect("a report");
    assert_eq!(first.state, AgentState::Working);
    reporter.report(AgentState::Working, Some("s"));
    assert!(!receiver.has_changed().unwrap(), "the same state is not sent twice");
    reporter.report(AgentState::Blocked("Question".into()), Some("s"));
    let second = receiver.borrow_and_update().clone().expect("a report");
    assert!(second.seq > first.seq);
    reporter.report(AgentState::Blocked("Question".into()), Some("other"));
    assert!(receiver.has_changed().unwrap(), "a new session is a change: it resumes differently");
}
