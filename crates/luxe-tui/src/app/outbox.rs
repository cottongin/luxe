//! The outbox — what the app parks for the event loop to run between frames.
//!
//! The app cannot hand the terminal to a program, leave the process, read the clipboard or run
//! anything off the frame itself; those are the loop's. So a surface parks the ask here, and the
//! loop takes it after the input that parked it.

use crate::launcher::{OpenInEditor, OpenInFileManager, TextEdit};
use crate::main_loop::{FixLaunch, TuiExit};
use std::path::PathBuf;

/// Asks parked for the loop.
#[derive(Default)]
pub struct Outbox {
    /// A launcher to hand the terminal to
    pub launch: Option<luxe_core::config::Launcher>,
    /// A file the reader chose to edit. The editor needs the terminal back, so the TUI exits and
    /// the CLI runs it, the same way the Configuration surface's own `EditFile` does.
    pub editor_file: Option<PathBuf>,
    /// The session's worktree, to remove once Luxe has left it: only the loop can end the session
    /// and only the CLI can remove the directory the process is standing in
    pub worktree_removal: Option<PathBuf>,
    /// A pull request to check out and fix in a new session, with the comments that go with it
    pub pull_request_fix: Option<FixLaunch>,
    /// An issue to start a new session on
    pub issue: Option<u64>,
    /// A session of yours to leave this one for, and what the reader carries into it
    pub resume: Option<crate::main_loop::ResumeLaunch>,
    /// Text to hand to the reader's editor and bring back
    pub text_edit: Option<TextEdit>,
    /// A file or folder to open in the reader's editor
    pub editor_open: Option<OpenInEditor>,
    /// A folder to hand to a terminal file manager
    pub file_manager: Option<OpenInFileManager>,
    /// A mention source to list, off the render thread
    pub mention_source: Option<luxe_core::config::MentionSource>,
    /// Whether to ask a local server what it is serving, for the model picker
    pub model_probe: bool,
    /// A read of the project's sessions, numbered for the session picker `/sessions` opened
    pub(crate) session_catalog: Option<u64>,
    /// Whether to read the clipboard's current payload
    pub clipboard_paste: bool,
    /// Text to put on the clipboard
    pub copy: Option<String>,
}

impl Outbox {
    /// The exit the parked asks end the TUI with, taking them all; when several are parked the
    /// last of an editor file, a worktree removal, an issue, a resume and a pull request fix wins.
    pub fn exit(&mut self) -> Option<TuiExit> {
        [
            self.editor_file.take().map(TuiExit::OpenInEditor),
            self.worktree_removal.take().map(TuiExit::RemoveWorktree),
            self.issue.take().map(TuiExit::TakeUpIssue),
            self.resume.take().map(TuiExit::ResumeSession),
            self.pull_request_fix.take().map(TuiExit::FixPullRequest),
        ]
        .into_iter()
        .flatten()
        .last()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With several exits parked, the last in the loop's order wins, and none is left behind to
    /// fire on a later pass.
    #[test]
    fn the_last_parked_exit_wins_and_all_are_taken() {
        let resume = crate::main_loop::ResumeLaunch {
            session: "s-other".into(),
            draft: String::new(),
            images: Vec::new(),
        };
        let mut outbox = Outbox {
            editor_file: Some(PathBuf::from("/work/notes.md")),
            issue: Some(7),
            resume: Some(resume.clone()),
            ..Outbox::default()
        };
        assert_eq!(outbox.exit(), Some(TuiExit::ResumeSession(resume.clone())));
        assert_eq!(outbox.exit(), None);
        assert!(outbox.editor_file.is_none() && outbox.issue.is_none());

        let fix = FixLaunch { number: 9, carried: Vec::new() };
        let mut everything = Outbox {
            editor_file: Some(PathBuf::from("/work/notes.md")),
            worktree_removal: Some(PathBuf::from("/work/.luxe/worktrees/x")),
            issue: Some(7),
            resume: Some(resume),
            pull_request_fix: Some(fix.clone()),
            ..Outbox::default()
        };
        assert_eq!(everything.exit(), Some(TuiExit::FixPullRequest(fix)));
        let mut removal_then_issue = Outbox {
            worktree_removal: Some(PathBuf::from("/work/.luxe/worktrees/x")),
            issue: Some(7),
            ..Outbox::default()
        };
        assert_eq!(removal_then_issue.exit(), Some(TuiExit::TakeUpIssue(7)));
    }
}
