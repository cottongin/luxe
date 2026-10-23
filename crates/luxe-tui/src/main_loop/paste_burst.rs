//! Paste burst — rebuilding a paste from the key presses a console delivers in its place.
//!
//! The Windows console has no bracketed paste: a paste arrives as one key press per character, and
//! every line break as an Enter, which the composer would submit. A run of text keys that lands
//! all at once is a paste, so it is folded back into one [`Event::Paste`] and meets the same code
//! a bracketed paste does, chip and all.

use super::pacing::{next_event, queued_event};
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use std::time::{Duration, Instant};

/// Text keys that must arrive together before they read as a paste instead of fast typing.
///
/// A person cannot get this many keys into one batch, but a loop stalled on a slow frame can queue
/// a few, so the bar sits above that.
const MIN_BURST_KEYS: usize = 5;

/// How long a paste may go quiet before it is taken as finished.
///
/// The console hands a large paste over in chunks, and a tail that arrived alone would run its
/// last Enter as a submit.
const BURST_GAP: Duration = Duration::from_millis(30);

/// The longest one burst holds the loop, so a huge paste cannot starve the engine's events.
const BURST_CEILING: Duration = Duration::from_secs(2);

/// Whether this platform delivers a paste as keystrokes.
pub(super) const fn pastes_as_keystrokes() -> bool {
    cfg!(windows)
}

/// What one terminal event adds to a run of pasted text.
#[derive(Debug, PartialEq, Eq)]
enum Piece {
    Text(char),
    /// A key release: part of the run, but no text
    Quiet,
}

/// The text a key press stands for, or `None` when the event could not be part of a paste.
fn piece(event: &Event) -> Option<Piece> {
    let Event::Key(key) = event else { return None };
    if !key.modifiers.difference(KeyModifiers::SHIFT).is_empty() {
        return None;
    }
    let character = match key.code {
        KeyCode::Char(character) => character,
        KeyCode::Enter => '\n',
        KeyCode::Tab => '\t',
        _ => return None,
    };
    Some(if key.kind == KeyEventKind::Release { Piece::Quiet } else { Piece::Text(character) })
}

/// Consecutive text key events, kept both raw and as the text they spell.
#[derive(Default)]
struct Run {
    events: Vec<Event>,
    text: String,
    keys: usize,
}

impl Run {
    fn push(&mut self, event: Event, piece: Piece) {
        if let Piece::Text(character) = piece {
            self.text.push(character);
            self.keys += 1;
        }
        self.events.push(event);
    }

    fn looks_pasted(&self) -> bool {
        self.keys >= MIN_BURST_KEYS
    }
}

/// Applies `first`, folding it and the text keys behind it into one paste when they are one.
///
/// Answers like `apply_terminal_event`: whether everything applied was idle pointer motion.
pub(super) async fn apply_folding_pastes(
    app: &mut crate::app::App,
    events: &mut Option<EventStream>,
    first: Event,
    exit: &mut super::TuiExit,
) -> bool {
    use super::keys::apply_terminal_event;
    let Some(opening) = piece(&first).filter(|_| pastes_as_keystrokes()) else {
        return apply_terminal_event(app, first, exit);
    };
    let (run, stopper) = gather(events, first, opening).await;
    let mut idle = apply_run(app, run, exit);
    if let Some(event) = stopper {
        idle &= apply_terminal_event(app, event, exit);
    }
    idle
}

/// Applies a run as one paste when it looks like one, and as the keys it was otherwise.
fn apply_run(app: &mut crate::app::App, run: Run, exit: &mut super::TuiExit) -> bool {
    use super::keys::apply_terminal_event;
    if run.looks_pasted() {
        return apply_terminal_event(app, Event::Paste(run.text), exit);
    }
    let mut idle = true;
    for event in run.events {
        idle &= apply_terminal_event(app, event, exit);
    }
    idle
}

/// Reads text keys until something else arrives or the run ends, returning that something.
async fn gather(
    events: &mut Option<EventStream>,
    first: Event,
    opening: Piece,
) -> (Run, Option<Event>) {
    let mut run = Run::default();
    run.push(first, opening);
    let started = Instant::now();
    loop {
        let waiting = run.looks_pasted() && started.elapsed() < BURST_CEILING;
        let Some(event) = next_in_run(events, waiting).await else { return (run, None) };
        match piece(&event) {
            Some(piece) => run.push(event, piece),
            None => return (run, Some(event)),
        }
    }
}

/// The next event: one already queued, else one that lands within the gap when `waiting`.
async fn next_in_run(events: &mut Option<EventStream>, waiting: bool) -> Option<Event> {
    if let Some(event) = queued_event(events.as_mut()).await {
        return Some(event);
    }
    match waiting {
        true => next_within_gap(events).await,
        false => None,
    }
}

/// The next event if one lands within [`BURST_GAP`].
async fn next_within_gap(events: &mut Option<EventStream>) -> Option<Event> {
    match tokio::time::timeout(BURST_GAP, next_event(events)).await {
        Ok(Some(Ok(event))) => Some(event),
        Ok(Some(Err(error))) => {
            tracing::warn!(%error, "terminal read failed while gathering a paste");
            None
        }
        Ok(None) | Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    fn run_of(events: Vec<Event>) -> Run {
        let mut run = Run::default();
        for event in events {
            let piece = piece(&event).expect("every event in these runs is a text key");
            run.push(event, piece);
        }
        run
    }

    fn typed(text: &str) -> Vec<Event> {
        text.chars()
            .map(|character| match character {
                '\n' => key(KeyCode::Enter, KeyModifiers::NONE),
                other => key(KeyCode::Char(other), KeyModifiers::NONE),
            })
            .collect()
    }

    #[test]
    fn enter_becomes_a_newline_and_tab_a_tab() {
        let run = run_of(typed("a\nb\tc"));
        assert_eq!(run.text, "a\nb\tc");
    }

    #[test]
    fn a_run_below_the_bar_is_typing() {
        assert!(!run_of(typed("ab\n")).looks_pasted());
    }

    #[test]
    fn a_run_at_the_bar_is_a_paste() {
        assert!(run_of(typed("ab\ncd")).looks_pasted());
    }

    #[test]
    fn releases_spell_nothing_and_do_not_count() {
        let release = Event::Key(KeyEvent::new_with_kind(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
            KeyEventKind::Release,
        ));
        let mut events = typed("abcd");
        events.extend((0..10).map(|_| release.clone()));
        let run = run_of(events);
        assert_eq!(run.text, "abcd");
        assert!(!run.looks_pasted());
    }

    #[test]
    fn a_shifted_capital_is_text() {
        assert_eq!(piece(&key(KeyCode::Char('A'), KeyModifiers::SHIFT)), Some(Piece::Text('A')));
    }

    #[test]
    fn a_chord_is_never_part_of_a_paste() {
        assert_eq!(piece(&key(KeyCode::Char('c'), KeyModifiers::CONTROL)), None);
        assert_eq!(piece(&key(KeyCode::Enter, KeyModifiers::ALT)), None);
    }

    #[test]
    fn navigation_and_pointer_events_end_a_run() {
        assert_eq!(piece(&key(KeyCode::Up, KeyModifiers::NONE)), None);
        assert_eq!(piece(&key(KeyCode::Esc, KeyModifiers::NONE)), None);
        assert_eq!(piece(&Event::FocusGained), None);
    }
}
