//! Handing the terminal to a program: which one goes first, and the mouse capture after it.

use super::launch::{Launch, MouseCapture};
use crate::fixture::{seeded_app, FixtureScenario};
use std::path::PathBuf;

/// The loop writes mouse capture only when the app wants something the terminal was not last told,
/// and after a launch it no longer knows what that was, so the next pass writes the app's wish.
///
/// This pins the bookkeeping `run_launch` relies on; the call there needs a terminal, so no test
/// reaches it. The break this catches: a launch that bailed out before handing the terminal over
/// (no editor, an unwritable temp file) was recorded as having written the app's setting, so a
/// `/mouse` toggle parked in the same batch never reached the terminal.
#[test]
fn a_toggle_beside_a_launch_reaches_the_terminal_whether_or_not_it_handed_over() {
    let mut capture = MouseCapture::default();
    assert_eq!(capture.change(false), None, "off to start with, as the terminal is");
    assert_eq!(capture.change(true), Some(true), "turning it on writes");
    assert_eq!(capture.change(true), None, "and does not say it twice");

    // `/mouse` turned it off in the same batch as a launch that bailed out: the terminal is still on
    capture.forget();
    assert_eq!(capture.change(false), Some(false), "the toggle still reaches the terminal");

    // On again and written; then `/mouse` turned it off, a launch handed over and took the terminal
    // back with capture off, and `/mouse` turned it on before the next pass: the record still says
    // on, and only forgetting it gets on written
    assert_eq!(capture.change(true), Some(true));
    capture.forget();
    assert_eq!(capture.change(true), Some(true), "on is written though it was last recorded on");
    assert_eq!(capture.change(true), None, "once");
}

/// Everything parked for a launch runs one at a time, a program first, then a text edit, a file
/// manager and an editor, which is the order the loop has always checked them in.
#[test]
fn parked_launches_run_one_at_a_time_in_order() {
    let mut app = seeded_app("dark", FixtureScenario::ChatIdle).expect("fixture");
    app.outbox.editor_open =
        Some(crate::launcher::OpenInEditor { path: PathBuf::from("/work/notes.md"), line: None });
    app.outbox.file_manager = Some(crate::launcher::OpenInFileManager {
        program: "yazi".into(),
        args: Vec::new(),
        folder: PathBuf::from("/work"),
    });
    app.outbox.text_edit = Some(crate::launcher::TextEdit {
        text: "body".into(),
        target: crate::launcher::EditTarget::PullRequestBody,
    });
    app.outbox.launch = Some(luxe_core::config::Launcher {
        key: "alt+l".into(),
        command: "lazygit".into(),
        args: Vec::new(),
    });

    let order: Vec<&str> = std::iter::from_fn(|| Launch::take(&mut app))
        .map(|launch| match launch {
            Launch::Program(_) => "program",
            Launch::Edit(_) => "edit",
            Launch::FileManager(_) => "file manager",
            Launch::Editor(_) => "editor",
        })
        .collect();
    assert_eq!(order, ["program", "edit", "file manager", "editor"]);
}
