//! Failures — what git's and `gh`'s own complaints say once a model has to act on them.

use std::path::Path;

/// Turn git's plumbing failure into something a model can act on.
///
/// Outside a repository, `git status` answers with "fatal: not a git repository … Stopping at
/// filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set)" and `git diff` prints its entire
/// usage screen — twenty lines of flags in place of a fact. Neither tells a reader the one thing
/// that matters: this directory has no version control, so stop asking it about history.
///
/// **Both streams, always.** Git puts a *fatal* on stderr and a *refusal* on stdout: a commit with
/// nothing staged exits 1 having said "no changes added to commit" on stdout and nothing at all on
/// stderr. Reading stderr alone turned the most common commit failure there is into "Git command
/// failed (no output).", which names no cause and suggests no fix — so the model abandoned the
/// tool and ran the same commit through `bash`, where git's own words were waiting.
pub(super) fn explain_failure(stderr: &str, stdout: &str, cwd: &Path) -> String {
    let lowered = format!("{stderr}\n{stdout}").to_ascii_lowercase();
    if lowered.contains("not a git repository") {
        return format!(
            "Not a git repository: `{}` is not version controlled, so there is no status, diff, \
             history, or branch to read. Use `read`/`grep` for the files themselves.",
            luxe_core::paths::display(cwd)
        );
    }
    // Reached because run_git closes the editor off deliberately. Git's own reply names `-m` and
    // `-F`, which are not the arguments this tool takes.
    if lowered.contains("problem with the editor") {
        return "Git wanted an editor to write this commit message and there is nobody to type \
                into one. Pass `message`, or add `--no-edit` to reuse the existing message."
            .to_string();
    }
    // Git's own advice here names `git add` and `git commit -a`, which are command lines rather
    // than this tool's arguments. The three refusals are distinct: what to do about each depends
    // on whether anything changed at all, and whether git can see it.
    if lowered.contains("no changes added to commit") {
        return "Nothing is staged, so there is nothing to commit — the changes are in the working \
                tree only. Stage them first with `{\"action\": \"add\", \"args\": [\".\"]}`, or \
                commit every tracked change at once by adding `-a` to `args`."
            .to_string();
    }
    if lowered.contains("nothing added to commit but untracked files present") {
        return "Every change here is in a file git has never tracked, so a commit would be empty. \
                Add the new paths first with `{\"action\": \"add\", \"args\": [\".\"]}` — `-a` does \
                not pick up untracked files."
            .to_string();
    }
    if lowered.contains("nothing to commit") {
        return "The working tree is clean, so this change is already committed. Check \
                `{\"action\": \"log\"}` before making another one."
            .to_string();
    }
    let reported = [stderr.trim(), stdout.trim()]
        .into_iter()
        .filter(|stream| !stream.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if reported.is_empty() {
        return "Git command failed (no output).".to_string();
    }
    // Anything else keeps git's own words, bounded — a usage dump is not an explanation.
    crate::output_limit::cap_lines(
        &format!("Git command failed: {reported}"),
        GIT_ERROR_MAX_LINES,
        "lines",
        "Run the command through `bash` if you need git's full output.",
    )
}

/// Max lines of git's own error text passed through.
pub(super) const GIT_ERROR_MAX_LINES: usize = 12;

/// Max lines of `gh`'s error text passed through.
pub(super) const GH_ERROR_MAX_LINES: usize = 12;

/// Rewrite a `gh` failure that has a known cause into the sentence that fixes it.
///
/// `gh` reports these accurately but in its own vocabulary, and the useful half is often below the
/// part a bounded result keeps. Which failure it was is decided by
/// [`luxe_core::github::classify_failure`], the one reading of `gh`'s wording in the tree; this only
/// picks the words. The rest is passed through untouched — guessing at an error nobody anticipated
/// is how a tool starts lying about what went wrong.
pub(super) fn explain_gh_failure(reported: &str) -> Option<&'static str> {
    use luxe_core::github::GitHubError;
    match luxe_core::github::classify_failure(reported, "") {
        GitHubError::NotAuthenticated { .. } => return Some(
            "the GitHub CLI is installed but not signed in — run `gh auth login` in a terminal \
                 (it needs a browser or a device code, so the agent cannot do it for you)",
        ),
        GitHubError::NotGitHub => {
            return Some(
                "this repository has no remote on a GitHub host, so there are no pull requests \
                 for `gh` to work with — `git remote -v` shows where it points",
            )
        }
        _ => {}
    }
    let lower = reported.to_lowercase();
    if lower.contains("no pull requests match") || lower.contains("no open pull requests") {
        return Some("no pull request matches — nothing to report, rather than a failure");
    }
    None
}
