//! Launches — the terminal handed to another program and taken back, and the mouse capture the
//! loop has to keep in step with it.

use crate::app::App;
use crate::launcher::{OpenInEditor, OpenInFileManager, TextEdit};
use crate::{terminal, view};
use crossterm::event::EventStream;
use tokio::sync::mpsc;

/// Something the app parked that needs the terminal to itself.
#[derive(Debug)]
pub(super) enum Launch {
    /// A configured launcher's program
    Program(luxe_core::config::Launcher),
    /// Text to edit in the reader's editor
    Edit(TextEdit),
    /// A folder in a terminal file manager
    FileManager(OpenInFileManager),
    /// A file in the reader's editor
    Editor(OpenInEditor),
}

impl Launch {
    /// Takes the next parked launch, a program first, then a text edit, a file manager and an
    /// editor; anything else stays parked for the next pass.
    pub(super) fn take(app: &mut App) -> Option<Launch> {
        if let Some(launcher) = app.outbox.launch.take() {
            return Some(Launch::Program(launcher));
        }
        if let Some(edit) = app.outbox.text_edit.take() {
            return Some(Launch::Edit(edit));
        }
        if let Some(target) = app.outbox.file_manager.take() {
            return Some(Launch::FileManager(target));
        }
        app.outbox.editor_open.take().map(Launch::Editor)
    }
}

/// Whether the terminal is reporting the mouse, as far as the loop last told it; `None` once a
/// launch has left that unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MouseCapture {
    on: Option<bool>,
}

impl Default for MouseCapture {
    /// Off, as a terminal starts.
    fn default() -> Self {
        Self { on: Some(false) }
    }
}

impl MouseCapture {
    /// What to write for the terminal to match `wanted`, recording it as written; `None` when it
    /// already does.
    pub(super) fn change(&mut self, wanted: bool) -> Option<bool> {
        (self.on != Some(wanted)).then(|| {
            self.on = Some(wanted);
            wanted
        })
    }

    /// Forgets what the terminal was told, after a launch.
    ///
    /// A launch that handed the terminal over set capture to the app's setting as it took it back,
    /// and one that bailed out first (no editor, an unwritable temp file) wrote nothing, and the
    /// loop cannot tell which happened. So the next pass writes the app's wish either way: at
    /// most one redundant escape, and a toggle parked beside the launch is never lost.
    pub(super) fn forget(&mut self) {
        self.on = None;
    }
}

/// Hands the terminal to `launch`'s program and takes it back, then repaints. The engine keeps
/// running underneath the whole time.
pub(super) async fn run_launch(
    term: &mut terminal::Tui,
    terminal_events: &mut Option<EventStream>,
    event_rx: &mut mpsc::Receiver<luxe_core::event::Event>,
    app: &mut App,
    launch: Launch,
    capture: &mut MouseCapture,
) -> std::io::Result<()> {
    match launch {
        Launch::Program(launcher) => {
            crate::launcher::run(term, terminal_events, event_rx, app, &launcher).await?;
        }
        Launch::Edit(edit) => {
            crate::launcher::edit_text(term, terminal_events, event_rx, app, edit).await?;
        }
        Launch::FileManager(target) => {
            crate::launcher::open_in_file_manager(term, terminal_events, event_rx, app, target)
                .await?;
        }
        Launch::Editor(target) => {
            crate::launcher::open_in_editor(term, terminal_events, event_rx, app, target).await?;
        }
    }
    capture.forget();
    view::draw(term, app)
}
