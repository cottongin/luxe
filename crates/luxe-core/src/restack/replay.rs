//! Replay — moving one layer onto its new base, and saying exactly why when it conflicts.

use super::*;

/// Replays `start..tip` onto `onto`, answering the new tip, through a scratch branch that is
/// always removed. Nothing is updated: git is asked only to print what it made.
pub(super) async fn replay(
    project: &Path,
    layer: &Layer,
    start: &str,
    onto: &str,
    tip: &str,
) -> Result<String, String> {
    let scratch = format!("{SCRATCH_PREFIX}{}", layer.number);
    git(project, &["update-ref", &scratch, tip]).await?;
    let range = format!("{start}..{scratch}");
    let onto_arg = format!("--onto={onto}");
    // Newer git updates the refs unless told to print; older git only prints, and knows no option
    // to say so, which is asked of it rather than read from how it refuses
    let args: &[&str] = if prints_only_when_told(project).await {
        &["replay", "--ref-action=print", &onto_arg, &range]
    } else {
        &["replay", &onto_arg, &range]
    };
    let said = replay_run(project, args).await;
    let _ = git(project, &["update-ref", "-d", &scratch]).await;
    let out = match said {
        Replayed::Made(out) => out,
        Replayed::Conflict => return Err(conflict_detail(project, start, onto, tip).await),
        Replayed::Failed(detail) => return Err(detail),
    };
    // One ref was named, so one update comes back: `update <ref> <new> <old>`
    out.lines()
        .find_map(|line| {
            let mut words = line.split_whitespace();
            match (words.next(), words.next(), words.next()) {
                (Some("update"), Some(_), Some(new)) => Some(new.to_string()),
                _ => None,
            }
        })
        .ok_or_else(|| format!("git replay said nothing it made: {out}"))
}

/// How a `git replay` went, told apart by exit status rather than by what git printed.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Replayed {
    /// What it printed
    Made(String),
    /// Stopped on a conflict: exit status 1, and nothing said about it
    Conflict,
    /// Failed some other way, in its own words
    Failed(String),
}

pub(super) async fn replay_run(project: &Path, args: &[&str]) -> Replayed {
    let program = resolve("git").unwrap_or_else(|| PathBuf::from("git"));
    let output = match run_bounded(captured_command(program, args, project), GIT_DEADLINE).await {
        Ok(output) => output,
        Err(error) => return Replayed::Failed(error.to_string()),
    };
    match output.status.code() {
        Some(0) => Replayed::Made(String::from_utf8_lossy(&output.stdout).trim().to_string()),
        Some(1) => Replayed::Conflict,
        _ => Replayed::Failed(last_lines(&String::from_utf8_lossy(&output.stderr))),
    }
}

/// Whether this git updates refs from `git replay` unless told to print, which it says by listing
/// the option in its own usage. Git 2.44 to 2.51 only ever print, and do not know the option.
async fn prints_only_when_told(project: &Path) -> bool {
    let program = resolve("git").unwrap_or_else(|| PathBuf::from("git"));
    let Ok(output) =
        run_bounded(captured_command(program, &["replay", "-h"], project), GIT_DEADLINE).await
    else {
        return false;
    };
    let usage = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    usage.contains("--ref-action")
}

/// Which of `start..tip` does not apply onto `onto`, and in which files, in words.
async fn conflict_detail(project: &Path, start: &str, onto: &str, tip: &str) -> String {
    let range = format!("{start}..{tip}");
    let commits = git(project, &["rev-list", "--reverse", &range]).await.unwrap_or_default();
    for commit in commits.lines() {
        let base = format!("--merge-base={commit}^");
        let merged = run(
            project,
            &["merge-tree", "--write-tree", "--name-only", &base, onto, commit],
            GIT_DEADLINE,
        )
        .await;
        // On a conflict git exits 1 and prints the tree, then the files; `run` keeps only stderr
        if merged.is_ok() {
            continue;
        }
        let listed = merge_tree_files(project, &base, onto, commit).await;
        let subject = git(project, &["log", "-1", "--format=%s", commit]).await.unwrap_or_default();
        return match listed.as_slice() {
            [] => format!("\"{subject}\" conflicts with it"),
            files => format!(
                "\"{subject}\" conflicts with it in {}",
                files.iter().map(|file| format!("`{file}`")).collect::<Vec<_>>().join(", ")
            ),
        };
    }
    "git replay stopped on a conflict".into()
}

/// The files `git merge-tree` names as conflicted, read from its stdout, which a failed `run`
/// drops.
async fn merge_tree_files(project: &Path, base: &str, onto: &str, commit: &str) -> Vec<String> {
    let program = resolve("git").unwrap_or_else(|| PathBuf::from("git"));
    let args = ["merge-tree", "--write-tree", "--name-only", base, onto, commit];
    let Ok(output) = run_bounded(captured_command(program, &args, project), GIT_DEADLINE).await
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect()
}
