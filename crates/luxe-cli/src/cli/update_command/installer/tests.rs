use super::*;

/// A folder holding a stand-in installer script with `body`, a scratch folder, and a log path.
#[cfg(unix)]
fn stand_in(body: &str) -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("installer.sh");
    std::fs::write(&script, body).expect("the script");
    let scratch = dir.path().join("scratch");
    std::fs::create_dir(&scratch).expect("scratch");
    let log = dir.path().join("logs").join("update.log");
    (dir, script, scratch, log)
}

/// The script is told where to install and that `PATH` is left alone, keeps its temporary files in
/// the scratch folder, and everything it prints, stderr too, lands in the log.
#[cfg(unix)]
#[tokio::test]
async fn the_script_is_pointed_at_the_target_and_logged() {
    let (_dir, script, scratch, log) = stand_in(
        "echo \"dir=$LUXE_CLI_INSTALL_DIR path=$LUXE_CLI_NO_MODIFY_PATH tmp=$TMPDIR\"\n\
         echo 'a warning' >&2\n",
    );
    let target = InstallTarget { dir: "/home/dev/.local/bin".into(), modify_path: false };
    run_script(&script, &target, &scratch, &log).await.expect("the script succeeds");

    let written = std::fs::read_to_string(&log).expect("the log");
    let expected = format!("dir=/home/dev/.local/bin path=1 tmp={}", scratch.display());
    assert!(written.contains(&expected), "{written}");
    assert!(written.contains("a warning"), "stderr is kept: {written}");
    assert!(written.contains("install folder: /home/dev/.local/bin"), "{written}");
    assert!(written.trim_end().ends_with("exit: exit status: 0"), "{written}");
}

/// A script that fails is an error that says luxe is unchanged and where its output went.
#[cfg(unix)]
#[tokio::test]
async fn a_failed_script_names_its_log() {
    let (_dir, script, scratch, log) = stand_in("echo 'could not download' >&2\nexit 3\n");
    let target = InstallTarget { dir: "/tmp/bin".into(), modify_path: true };
    let error = run_script(&script, &target, &scratch, &log).await.expect_err("it failed");
    let message = error.to_string();
    assert!(
        message.contains("unchanged") && message.contains(&log.display().to_string()),
        "{message}"
    );
    assert!(std::fs::read_to_string(&log).expect("the log").contains("could not download"));
}

/// Every temp variable either script reads points at the scratch folder. Without `TMP` and
/// `TEMP`, PowerShell's `GetTempPath` would put its folder in `%TEMP%` again.
#[test]
fn every_temp_variable_points_at_the_scratch_folder() {
    let scratch = Path::new("/scratch");
    let names: Vec<&str> = scratch_variables(scratch).iter().map(|(name, _)| *name).collect();
    assert_eq!(names, ["TMP", "TEMP", "TMPDIR"]);
    assert!(scratch_variables(scratch).iter().all(|(_, value)| *value == scratch));
}

/// The scratch folder goes, the downloaded archive and what was unpacked from it included.
#[tokio::test]
async fn the_scratch_folder_is_removed_with_everything_in_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scratch = dir.path().join("luxe-update-x");
    let unpacked = scratch.join("6f1b2c");
    std::fs::create_dir_all(&unpacked).expect("nested");
    std::fs::write(unpacked.join("luxe-cli.zip"), b"zip").expect("the archive");
    remove_scratch(scratch.clone()).await;
    assert!(!scratch.exists());
}

/// After an update the legacy updater beside the binary is removed, and nothing else is.
#[test]
fn the_legacy_updater_is_removed_and_nothing_else() {
    let dir = tempfile::tempdir().expect("tempdir");
    let exe = dir.path().join(format!("luxe{}", std::env::consts::EXE_SUFFIX));
    let updater = dir.path().join(format!("{LEGACY_UPDATER}{}", std::env::consts::EXE_SUFFIX));
    for path in [&exe, &updater] {
        std::fs::write(path, b"").expect("a stand-in");
    }
    remove_legacy_updater(&exe);
    assert!(!updater.exists());
    assert!(exe.exists());
    remove_legacy_updater(&exe);
}
