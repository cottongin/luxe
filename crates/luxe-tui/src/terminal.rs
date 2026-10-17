//! Terminal setup and teardown.
//!
//! Handles raw mode, alternate screen, and panic hooks to restore the terminal state,
//! and hands back a ratatui [`Tui`] the event loop draws into each frame.

use crossterm::{
    event::{
        DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, BeginSynchronizedUpdate,
        EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, Stdout, Write};
use std::panic;
use std::sync::atomic::{AtomicBool, Ordering};

static KEYBOARD_ENHANCEMENT_ENABLED: AtomicBool = AtomicBool::new(false);

/// The concrete terminal type the TUI renders into.
pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// Initialize the terminal for the TUI and return a ratatui `Terminal` to draw into.
pub fn init() -> io::Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Focus paces the background pull request refresh; a terminal that does not report it (or
    // tmux without `focus-events on`) leaves the interval to cover it
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste, EnableFocusChange)?;
    enable_keyboard_enhancement(&mut stdout)?;
    install_panic_hook();
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    terminal.clear()?;
    Ok(terminal)
}

/// Restores the terminal when dropped armed: an `Err` out of the event loop, which the panic hook
/// does not see, would otherwise leave the shell in raw mode on the alternate screen.
pub(crate) struct RestoreOnDrop {
    armed: bool,
    restore: fn() -> io::Result<()>,
}

impl RestoreOnDrop {
    /// A guard that restores unless [`Self::finish`]ed.
    pub(crate) fn armed() -> Self {
        Self { armed: true, restore }
    }

    /// Restores the terminal now, saying how it went, and leaves nothing for the drop to do.
    pub(crate) fn finish(mut self) -> io::Result<()> {
        self.armed = false;
        (self.restore)()
    }
}

impl Drop for RestoreOnDrop {
    fn drop(&mut self) {
        if self.armed {
            let _ = (self.restore)();
        }
    }
}

/// Restore the terminal to its original state.
///
/// Always releases mouse capture (harmless if it was never enabled) so a crash or exit never
/// leaves the outer terminal swallowing clicks.
pub fn restore() -> io::Result<()> {
    let mut stdout = io::stdout();
    let keyboard_result = disable_keyboard_enhancement(&mut stdout);
    // Ending a synchronized update we may not be inside is harmless, and a terminal left inside one
    // — by a panic mid-frame — stops presenting anything at all.
    let screen_result = execute!(
        stdout,
        EndSynchronizedUpdate,
        DisableMouseCapture,
        DisableBracketedPaste,
        DisableFocusChange,
        LeaveAlternateScreen
    );
    let raw_mode_result = disable_raw_mode();
    keyboard_result.and(screen_result).and(raw_mode_result)
}

/// Enables or disables terminal mouse capture at runtime (config default + the `/mouse` toggle).
///
/// Capturing routes wheel/click events to the app but suppresses the terminal's native
/// click-drag text selection; most terminals bypass capture while Shift is held.
pub fn set_mouse_capture(enable: bool) -> io::Result<()> {
    let mut stdout = io::stdout();
    if enable {
        execute!(stdout, EnableMouseCapture)
    } else {
        execute!(stdout, DisableMouseCapture)
    }
}

fn enable_keyboard_enhancement(stdout: &mut Stdout) -> io::Result<()> {
    if matches!(supports_keyboard_enhancement(), Ok(true)) {
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )?;
        KEYBOARD_ENHANCEMENT_ENABLED.store(true, Ordering::Relaxed);
    }
    Ok(())
}

fn disable_keyboard_enhancement(stdout: &mut Stdout) -> io::Result<()> {
    if KEYBOARD_ENHANCEMENT_ENABLED.swap(false, Ordering::Relaxed) {
        execute!(stdout, PopKeyboardEnhancementFlags)?;
    }
    Ok(())
}

/// Draws one frame inside a DEC 2026 synchronized update.
///
/// At a streaming frame rate the transcript repaints many times a second, and a terminal that
/// supports the sequence then presents each frame whole instead of risking a half-painted one. One
/// that doesn't ignores it. The update is always closed, including on a draw error.
pub fn draw_synchronized<F>(terminal: &mut Tui, render: F) -> io::Result<()>
where
    F: FnOnce(&mut ratatui::Frame),
{
    let mut stdout = io::stdout();
    let began = execute!(stdout, BeginSynchronizedUpdate).is_ok();
    let drawn = terminal.draw(render).map(|_| ());
    if began {
        let _ = execute!(stdout, EndSynchronizedUpdate);
    }
    drawn
}

/// Writes graphics-protocol commands straight to the stream, outside any frame.
///
/// Transmitting an image and telling the terminal to release one are not cell content, and the one
/// place they must never travel is the cell buffer: a symbol that is really a command is a cell
/// ratatui's diff cannot reason about, which is how images get left behind on screen. Between
/// frames there is nothing to interleave with. Measured on kitty and Ghostty, placeholder cells
/// printed before the image they name resolve as soon as it arrives, so this owes the frame no
/// ordering either.
pub fn write_graphics(sequence: &str) -> io::Result<()> {
    if sequence.is_empty() {
        return Ok(());
    }
    let mut stdout = io::stdout();
    stdout.write_all(sequence.as_bytes())?;
    stdout.flush()
}

/// Install a panic hook that restores the terminal before unwinding.
/// Once per process: taking the terminal back from a launcher runs [`init`] again, and each call
/// would otherwise wrap the hook one layer deeper.
fn install_panic_hook() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        let original_hook = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let _ = restore();
            original_hook(info);
        }));
    });
}

#[cfg(test)]
mod restore_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static RESTORES: AtomicUsize = AtomicUsize::new(0);

    fn counted() -> io::Result<()> {
        RESTORES.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// The guard restores once whichever way the loop ends: dropped by an error, or finished.
    #[test]
    fn the_guard_restores_once_however_the_loop_ends() {
        drop(RestoreOnDrop { armed: true, restore: counted });
        assert_eq!(RESTORES.load(Ordering::SeqCst), 1, "an error out of the loop restores");
        RestoreOnDrop { armed: true, restore: counted }.finish().unwrap();
        assert_eq!(RESTORES.load(Ordering::SeqCst), 2, "finishing restores, and only once");
    }
}
