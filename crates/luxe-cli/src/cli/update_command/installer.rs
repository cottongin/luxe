//! Installer update — how `luxe update` updates a copy luxe's own installer script put there.
//!
//! It downloads the release's installer script, the one the install instructions pipe into a
//! shell, and runs it over this copy's directory. The script writes the binary, as it did on the
//! first install; luxe only picks the script and the directory ([`luxe_core::update::installer`]).
//!
//! Two things it does that the script alone would not. The script works in a scratch folder luxe
//! makes and removes, because dist's PowerShell installer never deletes its own and would leave a
//! folder holding the downloaded zip in `%TEMP%` on every update. And everything the script prints
//! is copied to a log as well as shown, so a failure can be read after the console scrolls away.

use anyhow::{bail, Context};
use luxe_core::update::{installer, InstallReceipt, InstallTarget, LEGACY_UPDATER};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::mpsc;

/// How long the script's download may take. It is a few dozen kilobytes; a minute covers a slow
/// link without leaving someone staring at a hung `luxe update`.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);

/// The most script luxe will accept. The real one is about 25 KB; this is remote content that is
/// about to run, so anything far larger is not it.
const MAX_SCRIPT_BYTES: usize = 1024 * 1024;

/// How often and how far apart removing the scratch folder is tried. Windows can hold a file that
/// was just written (an antivirus scan of the downloaded zip), so one failure is not final.
const SCRATCH_REMOVE_TRIES: u32 = 3;
const SCRATCH_REMOVE_PAUSE: Duration = Duration::from_millis(500);

/// Runs the installer script for `latest` over the copy at `exe`.
pub(super) async fn run(latest: &str, exe: &Path) -> anyhow::Result<()> {
    let target = InstallTarget::for_binary(exe, InstallReceipt::load().as_ref())
        .with_context(|| format!("could not tell which folder {} is in", exe.display()))?;
    let scratch = tempfile::Builder::new()
        .prefix("luxe-update-")
        .tempdir()
        .context("could not make a scratch folder for the installer")?;
    let url = installer::installer_script_url(latest);
    println!("Downloading the luxe {latest} installer from {url}");
    let ran = match download(&url, scratch.path()).await {
        Ok(script) => run_script(&script, &target, scratch.path(), &log_path()).await,
        Err(error) => Err(error),
    };
    remove_scratch(scratch.keep()).await;
    ran
}

/// Removes the updater older installers put beside the binary, once an update has made it
/// redundant. A failure only costs a stale file, so it is reported and the update still counts.
pub(super) fn remove_legacy_updater(exe: &Path) {
    let name = format!("{LEGACY_UPDATER}{}", std::env::consts::EXE_SUFFIX);
    let updater = exe.with_file_name(name);
    match std::fs::remove_file(&updater) {
        Ok(()) => println!("Removed {}, which `luxe update` replaces.", updater.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            println!("Couldn't remove {} ({error}); it's no longer used.", updater.display())
        }
    }
}

/// Where the script's output is kept: one file, replaced each run, beside luxe's other logs.
fn log_path() -> PathBuf {
    luxe_core::user_dirs::luxe_state_dir().join("logs").join("update.log")
}

/// Downloads the script at `url` into `folder` and returns where it landed.
async fn download(url: &str, folder: &Path) -> anyhow::Result<PathBuf> {
    // The same bare user agent as the release check: the request says nothing about this machine
    let client = reqwest::Client::builder().timeout(DOWNLOAD_TIMEOUT).user_agent("luxe").build()?;
    let response =
        client.get(url).send().await.with_context(|| format!("could not download {url}"))?;
    if !response.status().is_success() {
        bail!("could not download {url}: it returned {}", response.status());
    }
    let body = crate::update_check::bounded_bytes(response, MAX_SCRIPT_BYTES)
        .await
        .map_err(|error| anyhow::anyhow!("could not download {url}: {error}"))?;
    let script = folder.join(installer::installer_script_name());
    std::fs::write(&script, body)
        .with_context(|| format!("could not save the installer to {}", script.display()))?;
    Ok(script)
}

/// Runs `script` for `target`, showing and logging what it prints. `scratch` is where it keeps its
/// temporary files.
async fn run_script(
    script: &Path,
    target: &InstallTarget,
    scratch: &Path,
    log: &Path,
) -> anyhow::Result<()> {
    let mut command = script_command(script);
    command
        .envs(target.script_variables())
        .envs(scratch_variables(scratch))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().context("could not start the installer")?;
    let mut log_file = open_log(log, script, target);
    let status = relay(&mut child, &mut log_file).await?;
    append(&mut log_file, &format!("exit: {status}"));
    if !status.success() {
        bail!(
            "the installer failed ({status}), so luxe is unchanged; its output is in {}",
            log.display()
        );
    }
    Ok(())
}

/// The command that runs the script: Windows PowerShell for the `.ps1`, `sh` for the rest.
fn script_command(script: &Path) -> tokio::process::Command {
    if !cfg!(windows) {
        let mut command = tokio::process::Command::new("sh");
        command.arg(script);
        return command;
    }
    let shell = luxe_core::program::resolve("powershell").unwrap_or_else(|| "powershell".into());
    let mut command = tokio::process::Command::new(shell);
    // No profile, so nothing of the reader's own runs inside the install. And no `PSModulePath`:
    // inherited from PowerShell 7 it points Windows PowerShell at modules it cannot load
    // (PowerShell/PowerShell#18530)
    command.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(script);
    command.env_remove("PSModulePath");
    command
}

/// Points every temp-folder variable the scripts consult at `scratch`: `TMP` and `TEMP` for
/// PowerShell's `GetTempPath`, `TMPDIR` for `mktemp`.
fn scratch_variables(scratch: &Path) -> [(&'static str, &Path); 3] {
    [("TMP", scratch), ("TEMP", scratch), ("TMPDIR", scratch)]
}

/// Opens the log fresh and heads it with what ran, or `None` when it cannot be written, which
/// costs the record but not the update.
fn open_log(log: &Path, script: &Path, target: &InstallTarget) -> Option<std::fs::File> {
    let opened = log.parent().map_or(Ok(()), std::fs::create_dir_all);
    let file = match opened.and_then(|()| std::fs::File::create(log)) {
        Ok(file) => file,
        Err(error) => {
            tracing::warn!(%error, path = %log.display(), "could not open the update log");
            return None;
        }
    };
    let header = format!(
        "luxe {} running {}\ninstall folder: {}\n",
        luxe_core::version::VERSION,
        script.display(),
        target.dir.display()
    );
    let mut log = Some(file);
    append(&mut log, &header);
    log
}

/// Adds `line` to the log. A log that stops taking writes is dropped with one warning rather than
/// failing the update it only records.
fn append(log: &mut Option<std::fs::File>, line: &str) {
    let Some(file) = log.as_mut() else { return };
    if let Err(error) = writeln!(file, "{line}") {
        tracing::warn!(%error, "the update log stopped taking writes");
        *log = None;
    }
}

/// One line the script printed, and whether it came on stderr.
type ScriptLine = (bool, String);

/// Shows each line the child prints, as it prints it, and copies it to `log`, then waits for it.
async fn relay(
    child: &mut tokio::process::Child,
    log: &mut Option<std::fs::File>,
) -> anyhow::Result<ExitStatus> {
    let (sender, mut lines) = mpsc::unbounded_channel::<ScriptLine>();
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(forward_lines(stdout, false, sender.clone()));
    }
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(forward_lines(stderr, true, sender.clone()));
    }
    drop(sender);
    while let Some((is_stderr, line)) = lines.recv().await {
        match is_stderr {
            true => eprintln!("{line}"),
            false => println!("{line}"),
        }
        append(log, &line);
    }
    child.wait().await.context("lost track of the installer")
}

/// Sends `stream` line by line. Lossy rather than strict UTF-8: Windows PowerShell writes in the
/// console's code page, and one accented folder name must not end the relay.
async fn forward_lines(
    stream: impl AsyncRead + Unpin,
    is_stderr: bool,
    to: mpsc::UnboundedSender<ScriptLine>,
) {
    let mut reader = BufReader::new(stream);
    let mut buffer = Vec::new();
    while matches!(reader.read_until(b'\n', &mut buffer).await, Ok(read) if read > 0) {
        let line = String::from_utf8_lossy(&buffer).trim_end_matches(['\r', '\n']).to_string();
        buffer.clear();
        if to.send((is_stderr, line)).is_err() {
            return;
        }
    }
}

/// Deletes the scratch folder, trying again briefly before saying it is left over.
async fn remove_scratch(folder: PathBuf) {
    for attempt in 1..=SCRATCH_REMOVE_TRIES {
        match std::fs::remove_dir_all(&folder) {
            Ok(()) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) if attempt < SCRATCH_REMOVE_TRIES => {
                tokio::time::sleep(SCRATCH_REMOVE_PAUSE).await
            }
            Err(error) => println!(
                "Couldn't remove the installer's scratch folder {} ({error}). Nothing in it is \
                 needed, so it's safe to delete.",
                folder.display()
            ),
        }
    }
}

#[cfg(test)]
mod tests;
