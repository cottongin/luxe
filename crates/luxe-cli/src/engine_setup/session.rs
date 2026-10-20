//! The session a run attaches to — which one, and what its log records about the run.

use std::path::PathBuf;

use luxe_core::session::{SessionClaim, SessionError, SessionId, SessionStore};

use super::AgentSettings;

/// How [`super::build_engine`] chooses the session to run.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) enum ResumeMode {
    /// Start a fresh session (the default).
    #[default]
    New,
    /// Resume the most recently modified session for this project.
    Latest,
    /// Resume a specific session by id.
    Session(String),
    /// Resume the newest session that worked on this pull request.
    PullRequest(u64),
}

/// The existing session a resume mode picks, or `None` when it starts a new one or finds none.
///
/// One answer for the two places that need it: the CLI, which has to know before the engine
/// starts whether that session works in a worktree, and `build_engine`, which opens it.
pub(crate) fn resumed_session(store: &impl SessionStore, mode: &ResumeMode) -> Option<SessionId> {
    match mode {
        ResumeMode::New => None,
        ResumeMode::Latest => store.most_recent_session(),
        ResumeMode::PullRequest(number) => {
            let recent: Vec<SessionId> =
                store.recent_sessions(usize::MAX).into_iter().map(|(id, _)| id).collect();
            luxe_core::session::session_for_pull_request(store, &recent, *number)
        }
        ResumeMode::Session(id) => {
            let sid = SessionId(id.clone());
            store.list_sessions().contains(&sid).then_some(sid)
        }
    }
}

/// Appends `kind` to the session's log, unless the log already says it.
///
/// Appended at the current leaf and never made the leaf, like a usage record. `what` names the
/// record in the warning: a failed write costs the next resume a convenience, not this run.
fn record_once(
    store: &mut luxe_core::session::JsonlSessionStore,
    session: &SessionId,
    already: impl FnOnce(&[luxe_core::session::Entry]) -> bool,
    kind: luxe_core::session::EntryKind,
    what: &str,
) {
    let Ok(entries) = store.load(session) else {
        return;
    };
    if already(&entries) {
        return;
    }
    if let Err(error) = store.append(session, luxe_core::session::latest_leaf(&entries), kind) {
        tracing::warn!(%error, record = what, "could not record it in the session's log");
    }
}

/// Records in the session's own log that it works in `binding`, unless it already says so.
fn record_worktree(
    store: &mut luxe_core::session::JsonlSessionStore,
    session: &SessionId,
    binding: &WorktreeBinding,
    state_root: &std::path::Path,
) {
    let path = binding.path.strip_prefix(state_root).unwrap_or(&binding.path);
    let kind = luxe_core::session::EntryKind::Worktree {
        branch: binding.branch.clone(),
        path: luxe_core::paths::display(path),
    };
    let already = |entries: &[luxe_core::session::Entry]| {
        luxe_core::session::session_worktree(entries).as_deref() == Some(binding.branch.as_str())
    };
    record_once(store, session, already, kind, "worktree");
}

/// The issue a session takes up as its brief.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IssueBinding {
    /// Number within its repository
    pub(crate) number: u64,
    /// Its web address
    pub(crate) url: String,
    /// What the session's first message carries
    pub(crate) brief: String,
}

/// Records in the session's log the issue it took up, unless it already says so.
fn record_issue(
    store: &mut luxe_core::session::JsonlSessionStore,
    session: &SessionId,
    issue: &IssueBinding,
) {
    let kind =
        luxe_core::session::EntryKind::IssueLink { number: issue.number, url: issue.url.clone() };
    let already = |entries: &[luxe_core::session::Entry]| {
        luxe_core::session::session_issue(entries) == Some(issue.number)
    };
    record_once(store, session, already, kind, "issue");
}

/// Records in the session's log that it works on pull request `number`, unless it already says so,
/// so `--from-pr` finds it again.
fn record_pull_request(
    store: &mut luxe_core::session::JsonlSessionStore,
    session: &SessionId,
    number: u64,
    url: &str,
) {
    let kind = luxe_core::session::EntryKind::PullRequestLink { number, url: url.to_string() };
    let already = |entries: &[luxe_core::session::Entry]| {
        luxe_core::session::linked_pull_requests(entries).contains(&number)
    };
    record_once(store, session, already, kind, "pull request");
}

/// The worktree a session works in: its branch and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeBinding {
    pub(crate) branch: String,
    pub(crate) path: PathBuf,
}

/// The session to run: resumed or fresh, with the worktree, issue and pull request it is tied to
/// written into its log.
///
/// The engine's `current_leaf` lazily walks to the tail on the first turn, so an existing id
/// resumes.
pub(super) fn attach_session(
    settings: &AgentSettings,
    store: &mut luxe_core::session::JsonlSessionStore,
) -> anyhow::Result<(SessionId, Option<SessionClaim>)> {
    let session_id = match (&settings.resume, resumed_session(store, &settings.resume)) {
        (ResumeMode::New, _) => store.create(&settings.state_root.to_string_lossy())?,
        (_, Some(id)) => {
            tracing::info!(session = %id, "resuming session");
            id
        }
        (ResumeMode::Latest, None) => {
            tracing::info!("no prior session to resume; starting fresh");
            store.create(&settings.state_root.to_string_lossy())?
        }
        (ResumeMode::PullRequest(number), None) => anyhow::bail!(
            "no session in this project worked on pull request #{number}; `luxe sessions list` \
             shows the ones that exist"
        ),
        (ResumeMode::Session(id), None) => {
            anyhow::bail!("session '{id}' not found for this project")
        }
    };
    // Before the records below, which are writes too
    let claim = claim_session(store, &session_id)?;
    // A session in a worktree says so in its own log, which is how resuming finds the worktree
    if let Some(binding) = &settings.worktree {
        record_worktree(store, &session_id, binding, &settings.state_root);
    }
    if let Some(issue) = &settings.issue {
        record_issue(store, &session_id, issue);
    }
    if let Some((number, url)) = &settings.pull_request {
        record_pull_request(store, &session_id, *number, url);
    }
    Ok((session_id, claim))
}

/// Claim `session_id` for this process, or say which luxe has it open and how to get there.
fn claim_session(
    store: &luxe_core::session::JsonlSessionStore,
    session_id: &SessionId,
) -> anyhow::Result<Option<SessionClaim>> {
    match store.claim(session_id) {
        Ok(claim) => Ok(claim),
        Err(SessionError::OpenElsewhere(id)) => anyhow::bail!(open_elsewhere(&id)),
        Err(error) => Err(error.into()),
    }
}

/// Why `session` can't be opened here right now, when another luxe has it open.
///
/// Asked before leaving the session that's running, so the reader stays where they are rather
/// than landing outside the TUI. This process's own claims don't count against it.
pub(crate) fn open_elsewhere_now(state_root: &std::path::Path, session: &str) -> Option<String> {
    let store = super::open_session_store(state_root);
    let id = SessionId(session.to_string());
    match store.claim(&id) {
        Err(SessionError::OpenElsewhere(id)) => Some(open_elsewhere(&id)),
        // Any other failure meets the session's start, which reports it in full
        Ok(_) | Err(_) => None,
    }
}

/// Where the session is open, when the luxe that has it says so, and what to do about it.
fn open_elsewhere(id: &SessionId) -> String {
    let runtime = luxe_core::user_dirs::luxe_runtime_dir();
    let holder = luxe_core::local_endpoint::live_sessions(&runtime)
        .into_iter()
        .find(|live| live.session_id == id.0);
    let place = match holder {
        Some(live) => format!(" (process {}, in {})", live.pid, live.cwd),
        None => String::new(),
    };
    // `luxe join` is not offered: it speaks the protocol, so a person running it sees JSON
    format!(
        "session {id} is already open in another luxe{place}. Carry on there, or close it \
         there first"
    )
}

#[cfg(test)]
mod tests {
    use super::super::open_session_store;
    use super::*;

    /// A session records its worktree once, relative to the project, and each way of resuming
    /// finds the session it names — which is how the CLI knows to re-enter the worktree first.
    #[test]
    fn a_session_records_its_worktree_once_and_resuming_finds_it() {
        use luxe_core::session::{EntryKind, SessionStore};
        let dir = tempfile::tempdir().unwrap();
        let mut store = open_session_store(dir.path());
        let id = store.create("/work").unwrap();
        store
            .append(&id, None, EntryKind::Message(luxe_core::message::Message::user("hi")))
            .unwrap();
        let binding = WorktreeBinding {
            branch: "topic".into(),
            path: dir.path().join(".luxe/worktrees/topic"),
        };

        record_worktree(&mut store, &id, &binding, dir.path());
        record_worktree(&mut store, &id, &binding, dir.path());

        let entries = store.load(&id).unwrap();
        let recorded: Vec<_> = entries
            .iter()
            .filter_map(|entry| match &entry.kind {
                EntryKind::Worktree { branch, path } => Some((branch.clone(), path.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(recorded, [("topic".to_string(), ".luxe/worktrees/topic".to_string())]);
        assert_eq!(luxe_core::session::session_worktree(&entries).as_deref(), Some("topic"));

        let issue = IssueBinding {
            number: 7,
            url: "https://github.com/acme/app/issues/7".into(),
            brief: String::new(),
        };
        record_issue(&mut store, &id, &issue);
        record_issue(&mut store, &id, &issue);
        let entries = store.load(&id).unwrap();
        assert_eq!(luxe_core::session::session_issue(&entries), Some(7));
        assert_eq!(
            entries.iter().filter(|e| matches!(e.kind, EntryKind::IssueLink { .. })).count(),
            1,
            "once"
        );

        record_pull_request(&mut store, &id, 142, "https://github.com/acme/app/pull/142");
        record_pull_request(&mut store, &id, 142, "https://github.com/acme/app/pull/142");
        let entries = store.load(&id).unwrap();
        assert_eq!(luxe_core::session::linked_pull_requests(&entries), [142], "once");
        assert_eq!(resumed_session(&store, &ResumeMode::PullRequest(142)), Some(id.clone()));

        assert_eq!(resumed_session(&store, &ResumeMode::New), None);
        assert_eq!(resumed_session(&store, &ResumeMode::Latest), Some(id.clone()));
        assert_eq!(resumed_session(&store, &ResumeMode::Session(id.0.clone())), Some(id.clone()));
        assert_eq!(resumed_session(&store, &ResumeMode::Session("nope".into())), None);
        assert_eq!(resumed_session(&store, &ResumeMode::PullRequest(9)), None);
    }

    /// A session another process holds is refused up front, with the way to reach it, and the
    /// check made before leaving a running session says the same.
    ///
    /// The break this catches: attaching without a claim, so a second `luxe --resume` opens a
    /// second writer on the log, or a resume from inside the TUI that lands the reader outside it.
    #[test]
    fn a_session_open_in_another_process_is_refused_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open_session_store(dir.path());
        let id = store.create("/work").unwrap();
        let lock = store.session_path(&id).unwrap().with_extension("lock");
        let other_process = std::fs::File::create(&lock).unwrap();
        other_process.try_lock().unwrap();
        let settings = {
            let _env = super::super::test_env::ENV_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let _home = super::super::test_env::isolated_env();
            let mut settings = AgentSettings::resolve(dir.path().to_path_buf(), Default::default());
            settings.resume = ResumeMode::Session(id.0.clone());
            settings
        };

        let refused = attach_session(&settings, &mut store).expect_err("held elsewhere");
        let expected = format!("session {id} is already open in another luxe");
        assert!(refused.to_string().contains(&expected), "{refused}");
        assert!(open_elsewhere_now(dir.path(), &id.0).is_some());
        drop(other_process);
        assert!(open_elsewhere_now(dir.path(), &id.0).is_none());
    }
}
