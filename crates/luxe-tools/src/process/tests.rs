use super::*;

/// However much a stream prints, a capture holds its head and its tail and counts the rest.
#[test]
fn a_capture_holds_its_bound_and_counts_what_fell_between() {
    let mut capture = Capture::default();
    let chunk = vec![b'x'; 1024 * 1024];
    for _ in 0..200 {
        capture.push(&chunk);
    }
    assert_eq!(capture.kept(), HEAD_BYTES + TAIL_BYTES);
    assert_eq!(capture.dropped(), (200 * chunk.len() - HEAD_BYTES - TAIL_BYTES) as u64);
    assert!(capture.text().contains("bytes of output not kept"));

    let mut small = Capture::default();
    small.push(b"all of it\n");
    assert_eq!(small.text(), "all of it\n", "a short stream is untouched");
}

/// A stream past the head but short of the bound keeps a character that straddles the head's end.
///
/// The break this catches: head and tail were decoded apart, so an `é` cut by the 4 MB mark came
/// back as two replacement characters though nothing was dropped.
#[test]
fn a_character_across_the_heads_end_survives() {
    let mut capture = Capture::default();
    capture.push(&vec![b'a'; HEAD_BYTES - 1]);
    capture.push("é".as_bytes());
    let text = capture.text();
    assert!(text.ends_with('é') && !text.contains('\u{fffd}'));
}

/// A command printing 200 MB is captured within the bound, and says where it lost the middle.
///
/// The break this catches: every chunk was appended to one buffer for the life of the command, so
/// `yes` or a runaway log grew memory until the timeout.
#[cfg(unix)]
#[tokio::test]
async fn two_hundred_megabytes_of_output_stays_within_the_bound() {
    let dir = tempfile::tempdir().unwrap();
    let outcome = run_captured(
        "head -c 200000000 /dev/zero | tr '\\0' a",
        dir.path(),
        Duration::from_secs(120),
    )
    .await
    .unwrap();
    let Outcome::Finished(captured) = outcome else { panic!("it finishes") };
    assert!(captured.stdout.len() <= HEAD_BYTES + TAIL_BYTES + 100, "{}", captured.stdout.len());
    assert!(captured.stdout.contains("bytes of output not kept"));
    assert!(captured.status.success());
}

/// Artifacts written in the same instant get names of their own.
///
/// The break this catches: the name was the millisecond alone, so two calls of one reply that
/// finished together overwrote each other's output.
#[test]
fn artifacts_never_share_a_name() {
    let dir = tempfile::tempdir().unwrap();
    let paths: std::collections::HashSet<PathBuf> = (0..50)
        .map(|index| {
            let stdout = format!("run {index}");
            tee_artifact(dir.path(), "cmd", "true", (&stdout, "", Some(0))).expect("written")
        })
        .collect();
    assert_eq!(paths.len(), 50);
    for path in &paths {
        assert!(std::fs::read_to_string(path).unwrap().contains("run "));
    }
}
