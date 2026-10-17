//! Checks — what is failing on a commit, with the tail of each failing step's log.

use super::*;

#[async_trait]
impl Checks for GhCli {
    async fn failing_checks(
        &self,
        repository: &Repository,
        head_oid: &str,
        log_tail: usize,
    ) -> Result<Vec<FailingCheck>, GitHubError> {
        let commit = format!("repos/{}/{}/commits/{head_oid}", repository.owner, repository.name);
        let runs_endpoint = format!("{commit}/check-runs?per_page=100");
        let mut args = vec!["api", "--paginate"];
        push_hostname(&mut args, repository);
        args.extend([runs_endpoint.as_str(), "--jq", CHECK_RUNS_JQ]);
        let runs = self.gh(&std::env::temp_dir(), &args).await?;

        let statuses_endpoint = format!("{commit}/status");
        let mut args = vec!["api"];
        push_hostname(&mut args, repository);
        args.extend([statuses_endpoint.as_str(), "--jq", STATUSES_JQ]);
        let statuses = self.gh(&std::env::temp_dir(), &args).await?;

        let mut failing = parse_failing_checks(&runs, &statuses)?;
        let target = repo_arg(repository);
        let mut read = 0;
        for (check, job) in &mut failing {
            let Some(job) = job else { continue };
            if read == MAX_LOGS {
                check.log = CheckLog::NotRead;
                continue;
            }
            read += 1;
            let args = ["run", "view", "--job", job.as_str(), "--log-failed", "--repo", &target];
            check.log = match self.gh(&std::env::temp_dir(), &args).await {
                Ok(log) => CheckLog::Steps(parse_failed_log(&log, log_tail)),
                Err(error) => CheckLog::Unreadable(error.to_string()),
            };
        }
        Ok(failing.into_iter().map(|(check, _)| check).collect())
    }
}

/// The check-run fields a [`FailingCheck`] is read from, one JSON object per line.
pub(super) const CHECK_RUNS_JQ: &str =
    ".check_runs[] | {name, status, conclusion, html_url, details_url, \
     app: .app.slug, title: .output.title, summary: .output.summary}";

/// The commit-status fields a [`FailingCheck`] is read from, one JSON object per line.
pub(super) const STATUSES_JQ: &str = ".statuses[] | {context, state, description, target_url}";

/// How many failing Actions jobs have their logs read in one call. Each is its own download.
pub(super) const MAX_LOGS: usize = 4;

// === Failing checks ===

/// One line of the check-runs listing, as [`CHECK_RUNS_JQ`] shapes it.
#[derive(Deserialize)]
pub(super) struct CheckRunLine {
    name: String,
    status: String,
    conclusion: Option<String>,
    html_url: String,
    details_url: Option<String>,
    app: Option<String>,
    title: Option<String>,
    summary: Option<String>,
}

/// One line of the combined status, as [`STATUSES_JQ`] shapes it.
#[derive(Deserialize)]
pub(super) struct StatusLine {
    context: String,
    state: String,
    description: Option<String>,
    target_url: Option<String>,
}

/// The failing checks on a commit, each with the Actions job whose log would explain it.
///
/// Check runs first, in GitHub's order, then commit statuses. A check still running has not
/// failed, whatever else it says.
pub(super) fn parse_failing_checks(
    runs: &str,
    statuses: &str,
) -> Result<Vec<(FailingCheck, Option<String>)>, GitHubError> {
    let mut failing = Vec::new();
    for run in parse_lines::<CheckRunLine>(runs)? {
        let outcome = run.conclusion.unwrap_or_default();
        let failed = matches!(
            outcome.as_str(),
            "failure" | "timed_out" | "cancelled" | "action_required" | "startup_failure" | "stale"
        );
        if run.status != "completed" || !failed {
            continue;
        }
        let job = (run.app.as_deref() == Some("github-actions"))
            .then(|| run.details_url.as_deref().and_then(actions_job))
            .flatten();
        // Filtered one at a time, or an empty title would win and hide a real summary
        let written = |text: &Option<String>| text.clone().filter(|text| !text.trim().is_empty());
        let summary = written(&run.title).or_else(|| written(&run.summary));
        failing.push((
            FailingCheck {
                name: status_text(&run.name),
                outcome,
                url: run.html_url,
                summary: summary.map(|text| status_text(&text)),
                log: CheckLog::NotActions,
            },
            job,
        ));
    }
    for status in parse_lines::<StatusLine>(statuses)? {
        if !matches!(status.state.as_str(), "failure" | "error") {
            continue;
        }
        failing.push((
            FailingCheck {
                name: status_text(&status.context),
                outcome: status.state,
                url: status.target_url.unwrap_or_default(),
                summary: status
                    .description
                    .filter(|text| !text.trim().is_empty())
                    .map(|text| status_text(&text)),
                log: CheckLog::NotActions,
            },
            None,
        ));
    }
    Ok(failing)
}

/// The job id in an Actions job's address: `…/actions/runs/<run>/job/<job>`.
pub(super) fn actions_job(url: &str) -> Option<String> {
    let (_, job) = url.split_once("/actions/runs/")?.1.split_once("/job/")?;
    let job = job.split(['/', '?', '#']).next()?;
    (!job.is_empty() && job.bytes().all(|byte| byte.is_ascii_digit())).then(|| job.to_string())
}

/// The failing steps in `gh run view --log-failed` output, each cut to its last `tail` lines.
///
/// Every line is `job<TAB>step<TAB>timestamp text`. The timestamp and terminal color codes go,
/// since a reader of the tail needs neither; steps keep the order they first appear in.
pub(super) fn parse_failed_log(log: &str, tail: usize) -> Vec<FailedStep> {
    let mut steps: Vec<(String, Vec<String>)> = Vec::new();
    for line in log.lines() {
        let mut fields = line.splitn(3, '\t');
        let (Some(_job), Some(step), Some(text)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let text = strip_ansi(without_timestamp(text));
        match steps.last_mut() {
            Some((name, lines)) if name == step => lines.push(text),
            _ => steps.push((step.to_string(), vec![text])),
        }
    }
    steps
        .into_iter()
        .map(|(name, mut lines)| {
            let error_end = lines
                .iter()
                .rposition(|line| line.starts_with("##[error]"))
                .map_or(lines.len(), |last| last + 1);
            let after = lines.len() - error_end;
            lines.truncate(error_end);
            let mut blank = 0;
            while lines.last().is_some_and(|line| line.trim().is_empty()) {
                lines.pop();
                blank += 1;
            }
            let omitted = lines.len().saturating_sub(tail);
            FailedStep { name, tail: lines.split_off(omitted), omitted, after: after + blank }
        })
        .collect()
}

/// A log line without the `2026-09-23T00:21:48.1862910Z ` Actions puts in front of it.
///
/// A step's first line can carry a byte-order mark ahead of the timestamp, left over from the log
/// file it was cut from.
pub(super) fn without_timestamp(text: &str) -> &str {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    match text.split_once(' ') {
        Some((stamp, rest))
            if stamp.ends_with('Z') && stamp.as_bytes().first().is_some_and(u8::is_ascii_digit) =>
        {
            rest
        }
        _ => text,
    }
}

/// `text` without terminal color sequences (`ESC [ … letter`).
///
/// Actions logs spell the escape in caret notation, as the two characters `^[`, so both forms go.
pub(super) fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        let escape = match character {
            '\u{1b}' => true,
            '^' if chars.peek() == Some(&'[') => {
                chars.next();
                true
            }
            _ => false,
        };
        if escape && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(character);
    }
    out
}
