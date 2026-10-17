//! Attention cues: an optional terminal bell, and an optional desktop notification sent through the
//! terminal, for the moments a reader who looked away wants to hear about — a turn that finished or
//! failed, a permission prompt or question waiting on them, a review whose guide is written.
//!
//! Both are opt-in and best-effort: `config.bell` rings the terminal (`\x07`), `config.notifications`
//! asks the terminal to show a desktop notification for every moment or a chosen few, and headless
//! suppresses the pair. Which moment fired is the app's call ([`crate::app`]); this module only
//! knows how to deliver one.
//!
//! # Through the terminal
//!
//! A notification is an escape sequence the terminal turns into a native one: `OSC 9` (iTerm2's,
//! shown by Ghostty, iTerm2, kitty, WezTerm and Warp) or `OSC 777` (rxvt-unicode's, shown by foot).
//! That needs no D-Bus or platform library, works over ssh, and lands wherever the reader's terminal
//! already sends its own. The cost is that a terminal cannot be asked which it shows, so `auto`
//! goes by an allowlist read from the environment — the same approach [`crate::graphics`] takes for
//! images — and an unknown terminal gets nothing rather than a sequence it might print.
//! `notification_method` names the sequence for a terminal the list does not know.
//!
//! herdr is the one terminal `auto` deliberately skips. It emulates the terminal in each pane and
//! keeps an `OSC 9` for itself — as a progress report — while leaking the outer terminal's variables
//! into the pane, so the allowlist would pick a sequence that never leaves it. Inside herdr the
//! cue is the agent state [`crate::herdr`] reports, which herdr turns into its own notification.

use std::io::{IsTerminal, Write};

use luxe_core::config::NotificationMethod;

/// Longest notification text, in characters. A notification is a glance, and an error message can
/// be a page of JSON.
const MAX_TEXT_CHARS: usize = 160;

/// Emit a terminal bell character, when the reader has asked for one.
///
/// Off unless `config.bell` says otherwise, because a bell is not just a sound: terminals
/// turn it into a window-manager attention request — Ghostty's `attention`, an X11 urgency hint —
/// and a compositor that honors one raises and focuses the terminal. Ringing at the end of every
/// turn then pulls the pointer and the keyboard off whatever the reader walked away to do, which is
/// the opposite of what a "your turn finished" cue is for.
///
/// Skipped when stdout is not a terminal, which is the question actually being asked — a bell in a
/// redirected log is a stray byte. Not `TERM`, as a proxy: Windows does not set it, so the turn-end
/// bell would never ring there.
pub fn bell(enabled: bool) {
    let mut out = std::io::stdout();
    let ringing = rings(enabled, out.is_terminal());
    // Best-effort — a stream that will not take one byte is not worth an error path.
    let _ = ring(&mut out, ringing);
}

/// Whether a bell reaches anyone: asked for, and going somewhere that rings rather than scrolls.
fn rings(enabled: bool, to_a_terminal: bool) -> bool {
    enabled && to_a_terminal
}

/// Writes the bell, or writes nothing at all.
fn ring(out: &mut impl Write, ringing: bool) -> std::io::Result<()> {
    if !ringing {
        return Ok(());
    }
    out.write_all(b"\x07")?;
    out.flush()
}

/// Something that can show the reader a notification.
pub trait Notifier: Send + Sync {
    /// Show `body` under `title`.
    fn notify(&self, title: &str, body: &str);
}

/// A notifier that shows nothing: notifications off, or a terminal not known to show them.
pub struct NoopNotifier;

impl Notifier for NoopNotifier {
    fn notify(&self, _title: &str, _body: &str) {}
}

/// A notifier that records what it was asked to show, for tests that need to read it back after
/// handing the notifier to the app.
#[cfg(any(test, feature = "fixture"))]
#[derive(Default, Clone)]
pub struct MockNotifier {
    pub calls: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>>,
}

#[cfg(any(test, feature = "fixture"))]
impl MockNotifier {
    pub fn new() -> Self {
        Self::default()
    }
}

#[cfg(any(test, feature = "fixture"))]
impl Notifier for MockNotifier {
    fn notify(&self, title: &str, body: &str) {
        self.calls.lock().expect("mock notifier lock").push((title.to_string(), body.to_string()));
    }
}

/// The escape sequence a notification is written as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// `ESC ] 9 ; text BEL`: one field, so the title and body are joined
    Osc9,
    /// `ESC ] 777 ; notify ; title ; body BEL`
    Osc777,
}

/// Sends notifications as an escape sequence to the terminal on stdout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalNotifier {
    protocol: Protocol,
    /// Inside tmux, which drops an OSC it does not model unless it is wrapped for passthrough
    tmux: bool,
}

impl TerminalNotifier {
    /// Writes the notification to `out`, or nothing when that is not a terminal: the sequence in a
    /// redirected log is noise, like the bell.
    fn write_to(&self, out: &mut impl Write, to_a_terminal: bool, title: &str, body: &str) {
        if !to_a_terminal {
            return;
        }
        let sequence = sequence(self.protocol, self.tmux, title, body);
        // Best-effort, like the bell: a notification that cannot be written is not an error path
        let _ = out.write_all(sequence.as_bytes()).and_then(|()| out.flush());
    }
}

impl Notifier for TerminalNotifier {
    fn notify(&self, title: &str, body: &str) {
        let mut out = std::io::stdout();
        let to_a_terminal = out.is_terminal();
        self.write_to(&mut out, to_a_terminal, title, body);
    }
}

/// The notifier for this terminal: one that writes the sequence it shows, or nothing.
pub fn create_notifier(enabled: bool, method: NotificationMethod) -> Box<dyn Notifier> {
    match terminal_notifier(enabled, method, &|name: &str| std::env::var(name)) {
        Some(notifier) => {
            tracing::info!(?notifier, "desktop notifications go through the terminal");
            Box::new(notifier)
        }
        None if enabled && std::env::var("HERDR_ENV").is_ok_and(|value| value == "1") => {
            tracing::info!("inside herdr, notifications come from herdr's own agent-state toasts");
            Box::new(NoopNotifier)
        }
        None if enabled => {
            tracing::info!(
                "notifications are on, but this terminal is not known to show them; set \
                 `notification_method` to `osc9` or `osc777` if it does"
            );
            Box::new(NoopNotifier)
        }
        None => Box::new(NoopNotifier),
    }
}

/// The terminal notifier `create_notifier` builds, against an environment lookup.
///
/// Args: `(enabled, method, environment lookup)`.
fn terminal_notifier(
    enabled: bool,
    method: NotificationMethod,
    env: &dyn Fn(&str) -> Result<String, std::env::VarError>,
) -> Option<TerminalNotifier> {
    if !enabled {
        return None;
    }
    let protocol = protocol_for(method, env)?;
    let tmux = env("TMUX").is_ok_and(|value| !value.is_empty());
    Some(TerminalNotifier { protocol, tmux })
}

/// The sequence to send for `method` in this environment; `None` where `auto` knows of none.
///
/// Args: `(method, environment lookup)`; a parameter so the branches are testable.
fn protocol_for(
    method: NotificationMethod,
    env: &dyn Fn(&str) -> Result<String, std::env::VarError>,
) -> Option<Protocol> {
    match method {
        NotificationMethod::Osc9 => return Some(Protocol::Osc9),
        NotificationMethod::Osc777 => return Some(Protocol::Osc777),
        NotificationMethod::Auto | NotificationMethod::Unknown => {}
    }
    let var = |name: &str| env(name).unwrap_or_default().to_ascii_lowercase();
    let set = |name: &str| !var(name).is_empty();
    let term = var("TERM");
    let program = var("TERM_PROGRAM");
    // Checked first: herdr's panes inherit `GHOSTTY_RESOURCES_DIR` and friends from the terminal
    // outside it, which would otherwise pick a sequence herdr swallows
    if program == "herdr" || var("HERDR_ENV") == "1" {
        return None;
    }
    // The variables a terminal sets for itself outlive `TERM` and `TERM_PROGRAM` inside tmux, which
    // rewrites both, and `LC_TERMINAL` is the one ssh forwards by default
    let osc9 = program == "ghostty"
        || term.contains("ghostty")
        || set("GHOSTTY_RESOURCES_DIR")
        || set("KITTY_WINDOW_ID")
        || term.contains("kitty")
        || program == "iterm.app"
        || var("LC_TERMINAL") == "iterm2"
        || set("ITERM_SESSION_ID")
        || program == "wezterm"
        || set("WEZTERM_PANE")
        || program == "warpterminal";
    if osc9 {
        Some(Protocol::Osc9)
    } else if term.starts_with("foot") {
        Some(Protocol::Osc777)
    } else {
        None
    }
}

/// The bytes that ask the terminal for a notification, wrapped for tmux when inside it.
///
/// Args: `(protocol, inside tmux, title, body)`.
fn sequence(protocol: Protocol, tmux: bool, title: &str, body: &str) -> String {
    let osc = match protocol {
        Protocol::Osc9 => format!("\x1b]9;{}\x07", clean(&format!("{title}: {body}"))),
        // The title is a field of its own, so it cannot carry the separator
        Protocol::Osc777 => {
            format!("\x1b]777;notify;{};{}\x07", clean(title).replace(';', ","), clean(body))
        }
    };
    if tmux {
        // DCS passthrough: tmux hands the payload to the outer terminal, every ESC in it doubled
        format!("\x1bPtmux;{}\x1b\\", osc.replace('\x1b', "\x1b\x1b"))
    } else {
        osc
    }
}

/// `text` made safe to put inside an escape sequence, on one line, and short.
///
/// Every control character goes, because the text includes an error message from outside: a `BEL`
/// or an `ESC` in it would end this sequence early and start one of its own, and a terminal acts on
/// those (`OSC 52` writes the clipboard). Whitespace runs collapse to one space.
fn clean(text: &str) -> String {
    let words: Vec<String> = text
        .split(|character: char| character.is_whitespace() || character.is_control())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect();
    let line = words.join(" ");
    if line.chars().count() <= MAX_TEXT_CHARS {
        return line;
    }
    let mut cut: String = line.chars().take(MAX_TEXT_CHARS - 1).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests;
