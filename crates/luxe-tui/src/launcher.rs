//! Launcher — a key that hands the terminal to another program and takes it back.
//!
//! gh-dash, lazygit, anything that takes over a terminal. luxe gives the terminal up whole — the
//! alternate screen, raw mode, mouse, the keyboard protocol — and stops reading it, so every key is
//! the program's; the engine keeps running underneath, a turn included, and what it reports is
//! applied without drawing. When the program exits, luxe takes the terminal back and redraws it.
//! Text the program writes to the file `$LUXE_HANDOFF` names lands in the composer, which is how a
//! gh-dash command sends `#123 look at this` into luxe.

use crossterm::event::{EventStream, KeyEvent};
use luxe_core::config::Launcher;
use luxe_core::keymap::KeyChord;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

use crate::app::{App, ConvoBlock, Msg};
use crate::terminal::{self, Tui};

/// Longest a handoff is read, in bytes: a composer draft, not a file.
const HANDOFF_MAX_BYTES: u64 = 64 * 1024;

/// The launcher bound to `key`, when one is. A key a launcher's chord does not parse to is never
/// matched, and says so once in the log.
pub fn bound<'a>(launchers: &'a [Launcher], key: &KeyEvent) -> Option<&'a Launcher> {
    let pressed = crate::app::chord_of(key)?;
    launchers.iter().find(|launcher| match KeyChord::parse(&launcher.key) {
        Ok(chord) => chord == pressed,
        Err(error) => {
            if first_complaint(&launcher.key) {
                tracing::warn!(key = %launcher.key, %error, "a launcher's key does not parse");
            }
            false
        }
    })
}

/// Whether `key` has not been complained about yet this run, and now has been. Asked on every key
/// press, so without it a typo in one chord logged once per keystroke.
fn first_complaint(key: &str) -> bool {
    static COMPLAINED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let mut complained = COMPLAINED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let first = !complained.iter().any(|seen| seen == key);
    if first {
        complained.push(key.to_owned());
    }
    first
}

/// Hands the terminal to `launcher`'s program until it exits, then takes it back.
pub async fn run(
    term: &mut Tui,
    terminal_events: &mut Option<EventStream>,
    event_rx: &mut mpsc::Receiver<luxe_core::event::Event>,
    app: &mut App,
    launcher: &Launcher,
) -> std::io::Result<()> {
    let handoff = handoff_path();
    let program = luxe_core::program::resolve(&launcher.command)
        .unwrap_or_else(|| PathBuf::from(&launcher.command));
    let mut command = tokio::process::Command::new(program);
    command.args(&launcher.args).env("LUXE_HANDOFF", &handoff);
    match hand_over(term, terminal_events, event_rx, app, command).await? {
        // However it exited: a program quit with `q` often says so with a nonzero status
        Ok(_) => {
            if let Some(text) = read_handoff(&handoff) {
                place_handoff(&mut app.editor, &text);
            }
        }
        Err(error) => app.push_convo(ConvoBlock::System(format!(
            "[could not run {}: {error}]",
            launcher.command
        ))),
    }
    let _ = std::fs::remove_file(&handoff);
    Ok(())
}

/// Text to edit in the reader's own editor, and where it goes back to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    /// What the editor opens on
    pub text: String,
    /// Where the edited text lands
    pub target: EditTarget,
}

/// Where text edited in the reader's editor lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditTarget {
    /// The description of the pull request being opened
    PullRequestBody,
    /// The reply or comment open in a GitHub draft card
    GithubDraft,
}

/// Opens `edit`'s text in the reader's editor, and hands the result to the app once it closes.
///
/// A file in the temp directory, owner-only, named `.md` so the editor highlights markdown. An
/// editor that exits with an error, or leaves nothing behind, changes nothing.
pub async fn edit_text(
    term: &mut Tui,
    terminal_events: &mut Option<EventStream>,
    event_rx: &mut mpsc::Receiver<luxe_core::event::Event>,
    app: &mut App,
    edit: TextEdit,
) -> std::io::Result<()> {
    let (program, args) = match luxe_core::program::editor() {
        Ok(editor) => editor,
        Err(error) => {
            app.status.flash_toast(format!("Could not open an editor: {error}"));
            return Ok(());
        }
    };
    let path = edit_path();
    if let Err(error) = luxe_core::fs_atomic::write_private(&path, edit.text.as_bytes()) {
        app.status.flash_toast(format!("Could not open an editor: {error}"));
        return Ok(());
    }
    let resolved = luxe_core::program::resolve(&program).unwrap_or_else(|| PathBuf::from(&program));
    let mut command = tokio::process::Command::new(resolved);
    command.args(&args).arg(&path);
    let edited = match hand_over(term, terminal_events, event_rx, app, command).await? {
        // `:cq` is how vim says "throw this away", so only a clean exit keeps the edit
        Ok(status) if status.success() => std::fs::read_to_string(&path).ok(),
        Ok(status) => {
            app.status.flash_toast(format!("{program} exited with {status}; nothing changed"));
            None
        }
        Err(error) => {
            app.status.flash_toast(format!("Could not run {program}: {error}"));
            None
        }
    };
    let _ = std::fs::remove_file(&path);
    if let Some(text) = edited {
        app.update(Msg::TextEdited { target: edit.target, text: text.trim_end().to_owned() });
    }
    Ok(())
}

/// A file or folder to open in the reader's editor, and the line to open it at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenInEditor {
    /// Absolute, so the editor's own working directory never changes what it opens
    pub path: PathBuf,
    /// 1-based, where the editor can be told one
    pub line: Option<u32>,
}

/// Opens `target` in the reader's editor and takes the terminal back when it closes.
///
/// The same handover as a launcher: the engine keeps running underneath, a turn included. A
/// terminal editor takes the screen; a GUI editor usually returns at once, and that is fine too.
/// Whatever the editor changed is the reader's work on the tree, so the workspace is read again
/// once it is back.
pub async fn open_in_editor(
    term: &mut Tui,
    terminal_events: &mut Option<EventStream>,
    event_rx: &mut mpsc::Receiver<luxe_core::event::Event>,
    app: &mut App,
    target: OpenInEditor,
) -> std::io::Result<()> {
    let (program, args) = match luxe_core::program::editor() {
        Ok(editor) => editor,
        Err(error) => {
            app.status.flash_toast(format!("Could not open an editor: {error}"));
            return Ok(());
        }
    };
    let resolved = luxe_core::program::resolve(&program).unwrap_or_else(|| PathBuf::from(&program));
    let mut command = tokio::process::Command::new(resolved);
    command.args(&args).args(luxe_core::program::editor_file_args(
        &program,
        &target.path,
        target.line,
    ));
    match hand_over(term, terminal_events, event_rx, app, command).await? {
        Ok(status) if status.success() => {}
        Ok(status) => app.status.flash_toast(format!("{program} exited with {status}")),
        Err(error) => app.status.flash_toast(format!("Could not run {program}: {error}")),
    }
    app.send_quietly(luxe_core::engine::Action::RefreshWorkspace, "refresh the workspace");
    Ok(())
}

/// A folder to open in a terminal file manager, and the program that opens it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenInFileManager {
    /// The file manager, as a command names it
    pub program: String,
    /// Arguments that come before the folder
    pub args: Vec<String>,
    /// Absolute, like an editor's target
    pub folder: PathBuf,
}

/// Opens `target`'s folder in its file manager and takes the terminal back when it quits, then
/// reads the workspace and the tree's folders again, since whatever the reader moved or deleted
/// there is the tree's to show.
pub async fn open_in_file_manager(
    term: &mut Tui,
    terminal_events: &mut Option<EventStream>,
    event_rx: &mut mpsc::Receiver<luxe_core::event::Event>,
    app: &mut App,
    target: OpenInFileManager,
) -> std::io::Result<()> {
    let program = &target.program;
    let resolved = luxe_core::program::resolve(program).unwrap_or_else(|| PathBuf::from(program));
    let mut command = tokio::process::Command::new(resolved);
    command.args(&target.args).arg(&target.folder).current_dir(&target.folder);
    match hand_over(term, terminal_events, event_rx, app, command).await? {
        Ok(_) => {}
        Err(error) => app.status.flash_toast(format!("Could not run {program}: {error}")),
    }
    app.send_quietly(luxe_core::engine::Action::RefreshWorkspace, "refresh the workspace");
    app.files.refresh_all();
    crate::app::messages::files::sync(app);
    Ok(())
}

/// Gives the terminal to `command` until it exits, keeping up with the engine underneath without
/// drawing, then takes it back. The outer error is the terminal's; the inner result is how the
/// program went.
async fn hand_over(
    term: &mut Tui,
    terminal_events: &mut Option<EventStream>,
    event_rx: &mut mpsc::Receiver<luxe_core::event::Event>,
    app: &mut App,
    mut command: tokio::process::Command,
) -> std::io::Result<std::io::Result<std::process::ExitStatus>> {
    // The stream's reader thread sits in a blocking read of the terminal, holding crossterm's one
    // event reader: left there, it takes keys meant for the program and starves the queries that
    // taking the terminal back needs. Dropping the stream wakes it to exit — and it has to be
    // dropped *before* a new one exists, since making one takes that same reader, and waiting on
    // it waits for a keystroke that then goes missing.
    *terminal_events = None;
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    terminal::restore()?;
    let spawned = command
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .spawn();
    let outcome = match spawned {
        Ok(mut child) => loop {
            tokio::select! {
                status = child.wait() => break status,
                // Keep up with the engine, drawing nothing: the screen is the program's
                Some(event) = event_rx.recv() => app.update(Msg::Engine(event)),
            }
        },
        Err(error) => Err(error),
    };
    *term = terminal::init()?;
    let _ = terminal::set_mouse_capture(app.mouse_enabled);
    *terminal_events = Some(EventStream::new());
    Ok(outcome)
}

/// Puts what the program handed back at the composer's cursor, a space apart from a word already
/// there.
fn place_handoff(editor: &mut crate::editor::Editor, text: &str) {
    let before = editor.current_input()[..editor.cursor()].chars().next_back();
    if before.is_some_and(|character| !character.is_whitespace()) {
        editor.insert_text(" ");
    }
    editor.insert_text(text);
}

/// Where this launch's handoff goes: a fresh name in the temp directory, which the program may
/// write and nothing else reads.
fn handoff_path() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    std::env::temp_dir().join(format!("luxe-handoff-{}-{nanos}", std::process::id()))
}

/// Where an edit's text is written for the editor: a fresh name, like a handoff's.
fn edit_path() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    std::env::temp_dir().join(format!("luxe-edit-{}-{nanos}.md", std::process::id()))
}

/// What the program handed back, trimmed and bounded; `None` for nothing, or no file at all.
fn read_handoff(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path).ok()?.take(HANDOFF_MAX_BYTES).read_to_string(&mut text).ok()?;
    let text = text.trim_end();
    (!text.trim().is_empty()).then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crossterm::event::{KeyCode, KeyModifiers};

    /// A chord that does not parse is complained about once, not on every key press.
    #[test]
    fn a_bad_chord_is_complained_about_once() {
        let key = "ctrl+nonsense-launcher-key";
        assert!(first_complaint(key));
        assert!(!first_complaint(key), "once is enough");
        assert!(first_complaint("ctrl+another-bad-key"), "each bad chord gets its own");
    }

    fn launcher(key: &str, command: &str) -> Launcher {
        Launcher { key: key.into(), command: command.into(), args: Vec::new() }
    }

    #[test]
    fn a_launcher_is_found_by_its_chord_and_a_bad_chord_matches_nothing() {
        let launchers = [
            launcher("not a chord!", "broken"),
            launcher("alt+l", "lazygit"),
            launcher("ctrl+shift+d", "gh-dash"),
        ];
        let pressed = |code, modifiers| {
            bound(&launchers, &KeyEvent::new(code, modifiers)).map(|l| l.command.as_str())
        };
        assert_eq!(pressed(KeyCode::Char('l'), KeyModifiers::ALT), Some("lazygit"));
        assert_eq!(
            pressed(KeyCode::Char('d'), KeyModifiers::CONTROL | KeyModifiers::SHIFT),
            Some("gh-dash")
        );
        assert_eq!(
            pressed(KeyCode::Char('l'), KeyModifiers::NONE),
            None,
            "a plain letter is typing"
        );
        assert_eq!(pressed(KeyCode::Char('l'), KeyModifiers::CONTROL), None);
    }

    #[test]
    fn a_handoff_lands_a_space_apart_from_what_is_there() {
        let mut editor = crate::editor::Editor::new();
        place_handoff(&mut editor, "#123 look at this");
        assert_eq!(editor.current_input(), "#123 look at this");
        place_handoff(&mut editor, "and #124");
        assert_eq!(editor.current_input(), "#123 look at this and #124");
        editor.insert_text(" ");
        place_handoff(&mut editor, "then");
        assert_eq!(editor.current_input(), "#123 look at this and #124 then", "no second space");
    }

    /// What the program wrote comes back trimmed; nothing, whitespace, or no file comes back as
    /// nothing; and a file larger than a draft is cut rather than read whole.
    #[test]
    fn a_handoff_is_what_the_program_wrote_and_no_more() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("handoff");
        assert_eq!(read_handoff(&path), None, "no file");
        std::fs::write(&path, "#123 look at this\n\n").unwrap();
        assert_eq!(read_handoff(&path).as_deref(), Some("#123 look at this"));
        std::fs::write(&path, "  \n").unwrap();
        assert_eq!(read_handoff(&path), None);
        std::fs::write(&path, "x".repeat(200_000)).unwrap();
        assert_eq!(read_handoff(&path).map(|text| text.len()), Some(HANDOFF_MAX_BYTES as usize));
        let one = handoff_path();
        assert!(one.starts_with(std::env::temp_dir()));
        assert_ne!(one, handoff_path(), "each launch its own");
    }
}
