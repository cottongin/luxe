use super::*;
#[cfg(unix)]
use crate::program::run_bounded_with_input;
#[cfg(unix)]
use crate::program::tests::still_running;
use crate::program::{run_bounded, BoundedRunError};

/// A process started after [`take_stdin`] reads the null device, and the pipe stays the
/// taker's.
///
/// Three processes, all this test binary: this one holds the write end of a pipe the way an RPC
/// client does, the taker takes it as stdin and keeps a read pending on it, and a child the
/// taker starts reads whatever stdin it inherited. Nothing is written until the child has
/// finished, so a child that got the pipe never does: on Unix it waits for data, on Windows it
/// stalls behind the pending read.
#[test]
fn a_child_cannot_read_the_stdin_luxe_took() {
    use std::io::{BufRead, Read, Write};
    use std::process::{Command, Stdio};
    const ROLE: &str = "LUXE_TAKE_STDIN_ROLE";
    let rerun = |role: &str| {
        let mut command = Command::new(std::env::current_exe().expect("the test binary"));
        command
            .args(["--exact", "program::process::tests::a_child_cannot_read_the_stdin_luxe_took"])
            .args(["--nocapture", "--test-threads=1"])
            .env(ROLE, role);
        command
    };
    match std::env::var(ROLE).as_deref() {
        Ok("child") => {
            let mut read = Vec::new();
            let _ = std::io::stdin().read_to_end(&mut read);
            println!("child read {} bytes", read.len());
            return;
        }
        Ok("taker") => {
            let private = take_stdin().expect("stdin can be taken");
            let reader = std::thread::spawn(move || {
                let mut line = String::new();
                std::io::BufReader::new(private).read_line(&mut line).map(|_| line)
            });
            // the read is pending before the child starts, as the protocol's always is
            std::thread::sleep(std::time::Duration::from_millis(200));
            let child = rerun("child").stdin(Stdio::inherit()).output().expect("child runs");
            print!("{}", String::from_utf8_lossy(&child.stdout));
            std::io::stdout().flush().expect("flush");
            println!("taker read {:?}", reader.join().expect("reader"));
            return;
        }
        _ => {}
    }

    let mut taker = rerun("taker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("taker runs");
    let mut input = taker.stdin.take();
    let output = taker.stdout.take().expect("taker stdout");
    let (lines_tx, lines) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(output).lines().map_while(Result::ok) {
            let _ = lines_tx.send(line);
        }
    });
    let mut wait_for = |prefix: &str, input: &mut Option<std::process::ChildStdin>| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match lines.recv_timeout(left) {
                // libtest's own `test … ... ` has no newline, so a line can start with it
                Ok(line) if line.contains(prefix) => {
                    return line[line.find(prefix).unwrap_or_default()..].to_string();
                }
                Ok(_) => {}
                Err(_) => {
                    // closing the pipe frees a child that got it, so nothing outlives the test
                    input.take();
                    let _ = taker.kill();
                    let _ = taker.wait();
                    panic!("no `{prefix}` line: the child is still waiting on the taken stdin");
                }
            }
        }
    };
    assert_eq!(wait_for("child read", &mut input), "child read 0 bytes");
    let mut pipe = input.take().expect("the pipe is still open");
    pipe.write_all(b"frame\n").expect("write a frame");
    drop(pipe);
    assert_eq!(wait_for("taker read", &mut None), r#"taker read Ok("frame\n")"#);
    assert!(taker.wait().expect("the taker exits").success());
}

/// Rows copied from procps on Linux and from macOS's `ps`, which pad differently and put
/// modifiers after the state letter.
#[test]
fn a_ps_listing_is_cut_down_to_one_group() {
    let listing = "\
2529300 2529300 Ss   /bin/bash -c sh -c 'sleep 30 & kill -STOP $!'
2529302 2529301 T    sleep 30
2529301 2529301 S    sh -c sleep 30 & kill -STOP $!;  echo   two  spaces
  501   501 Ss   /sbin/launchd
  812   2529301 T+   stty -echo
not a row at all
";
    let members = parse_group(listing, 2_529_301);
    let rows: Vec<(u32, &str, &str)> = members
        .iter()
        .map(|member| (member.pid, member.state.as_str(), member.command.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            (812, "T+", "stty -echo"),
            (2_529_301, "S", "sh -c sleep 30 & kill -STOP $!;  echo   two  spaces"),
            (2_529_302, "T", "sleep 30"),
        ]
    );
    assert_eq!(members.iter().filter(|member| member.is_stopped()).count(), 2);
}

/// What every detached spawn has to be: the leader of its own session, so it has no terminal,
/// and of its own group, so [`kill_tree`] reaches everything it started.
#[cfg(unix)]
#[tokio::test]
async fn a_spawned_command_leads_its_own_session_and_group() {
    let mut command = captured_command("sh", &["-c", "sleep 30"], Path::new("."));
    command.kill_on_drop(true);
    let child = command.spawn().expect("sh runs");
    let pid = rustix::process::Pid::from_raw(child.id().expect("a pid") as i32).expect("pid");
    let session = rustix::process::getsid(Some(pid)).expect("its session");
    let group = rustix::process::getpgid(Some(pid)).expect("its group");
    let ours = rustix::process::getsid(None).expect("our session");
    kill_tree(pid.as_raw_nonzero().get() as u32);
    assert_eq!(session, pid, "a session of its own");
    assert_eq!(group, pid, "and a group of its own");
    assert_ne!(session, ours, "not Luxe's");
}

#[cfg(unix)]
#[tokio::test]
async fn a_live_group_reports_its_stopped_member() {
    let mut command =
        captured_command("sh", &["-c", "sleep 30 & kill -STOP $!; sleep 30"], Path::new("."));
    command.kill_on_drop(true);
    let child = command.spawn().expect("sh runs");
    let pgid = child.id().expect("a pid");
    let mut members = Vec::new();
    for _ in 0..50 {
        members = group_members(pgid).await;
        if members.iter().any(GroupMember::is_stopped) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    kill_tree(pgid);
    assert!(members.iter().any(|member| member.pid == pgid), "the shell: {members:?}");
    assert!(
        members.iter().any(|member| member.is_stopped() && member.command.contains("sleep")),
        "{members:?}"
    );
}

/// The process asking is alive; a process id nothing can be using is not.
///
/// What this holds is `luxe loop status`: a state file saying `running: true` is a record of
/// intent, and only the operating system can say whether the run behind it still exists.
#[test]
fn a_live_process_is_told_from_a_dead_one() {
    assert!(process_is_alive(std::process::id()), "this very process");
    assert!(!process_is_alive(0), "there is no process 0 to ask about");
    // Well past any system's `pid_max`, so it cannot have been reused under us.
    assert!(!process_is_alive(u32::MAX - 1));
}

/// Both streams come back to the caller. `spawn` inherits them unless told otherwise, which is
/// how a `gh` error once went to the TUI's screen instead of the model.
#[cfg(unix)]
#[tokio::test]
async fn a_bounded_run_captures_both_streams() {
    let dir = tempfile::tempdir().unwrap();
    let command = captured_command("sh", &["-c", "echo out; echo err >&2"], dir.path());
    let output = run_bounded(command, std::time::Duration::from_secs(10)).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout), "out\n");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "err\n");
}

/// A timeout reaps what the program started, not only the program. The grandchild here is the
/// `gpg` pinentry or pre-commit hook of a real run — the thing left holding `.git/index.lock`.
#[cfg(unix)]
#[tokio::test]
async fn a_bounded_run_past_its_deadline_takes_its_children_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("grandchild.pid");
    let script = format!("sleep 30 & echo $! > '{}'; wait", pidfile.display());
    let command = captured_command("sh", &["-c", &script], dir.path());

    let started = std::time::Instant::now();
    let result = run_bounded(command, std::time::Duration::from_millis(500)).await;
    assert!(
        matches!(result, Err(BoundedRunError::Timeout { after }) if after.as_millis() == 500),
        "{result:?}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "the deadline held");

    let grandchild: u32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while process_is_alive(grandchild) && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(!process_is_alive(grandchild), "the backgrounded sleep outlived the timeout");
}

/// A run whose caller stops waiting takes what it started with it, as a cancelled turn does.
///
/// The break this catches: only the deadline killed the tree, so a dropped run left the
/// backgrounded grandchild of every abandoned fetch or capture running.
#[cfg(unix)]
#[tokio::test]
async fn a_bounded_run_dropped_midway_takes_its_children_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("grandchild.pid");
    let script = format!("sleep 30 & echo $! > '{}'; wait", pidfile.display());
    let command = captured_command("sh", &["-c", &script], dir.path());
    let run = tokio::spawn(run_bounded(command, std::time::Duration::from_secs(60)));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !pidfile.exists() && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    run.abort();
    let _ = run.await;
    let grandchild: u32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while still_running(grandchild) && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(!still_running(grandchild), "the backgrounded sleep outlived its caller");
}

/// The input reaches the program, all of it, even when it is bigger than a pipe holds.
#[cfg(unix)]
#[tokio::test]
async fn a_bounded_run_feeds_its_input_through_a_full_pipe() {
    let dir = tempfile::tempdir().unwrap();
    let input = vec![b'x'; 1 << 20];
    let command = captured_command("wc", &["-c"], dir.path());
    let output =
        run_bounded_with_input(command, &input, std::time::Duration::from_secs(10)).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "1048576");
}

/// A program that never reads its input cannot hold the run past the deadline.
///
/// The break this catches: the write to its stdin was a step of its own, so a program that stalled
/// without draining a patch bigger than the pipe kept the caller waiting on the write for good.
#[cfg(unix)]
#[tokio::test]
async fn a_bounded_run_whose_program_ignores_its_input_times_out() {
    let dir = tempfile::tempdir().unwrap();
    let input = vec![b'x'; 4 << 20];
    let command = captured_command("sleep", &["30"], dir.path());
    let started = std::time::Instant::now();
    let result =
        run_bounded_with_input(command, &input, std::time::Duration::from_millis(500)).await;
    assert!(matches!(result, Err(BoundedRunError::Timeout { .. })), "{result:?}");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}

/// A program that exits without reading its input is answered by its status, not by a failed write.
#[cfg(unix)]
#[tokio::test]
async fn a_program_that_stops_reading_early_still_reports_its_status() {
    let dir = tempfile::tempdir().unwrap();
    let input = vec![b'x'; 4 << 20];
    let command = captured_command("sh", &["-c", "echo no >&2; exit 3"], dir.path());
    let output =
        run_bounded_with_input(command, &input, std::time::Duration::from_secs(10)).await.unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(String::from_utf8_lossy(&output.stderr), "no\n");
}

/// A program that is not there is a spawn failure, not a timeout or an empty success.
#[tokio::test]
async fn a_missing_program_fails_to_spawn() {
    let dir = tempfile::tempdir().unwrap();
    let command = captured_command("luxe-no-such-program-anywhere", &[], dir.path());
    let result = run_bounded(command, std::time::Duration::from_secs(5)).await;
    assert!(matches!(result, Err(BoundedRunError::Spawn(_))), "{result:?}");
}

/// A hostname is always *something*, so a record can always say where it was written.
#[test]
fn the_machine_always_has_a_name() {
    let name = hostname();
    assert!(!name.is_empty());
    assert_eq!(name, name.trim(), "no stray newline from the syscall");
}
