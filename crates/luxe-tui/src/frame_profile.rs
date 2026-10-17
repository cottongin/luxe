//! frame_profile — what each painted frame cost, when someone asks.
//!
//! `LUXE_FRAME_PROFILE=<path>` makes the render loop record a row per frame and write the lot as
//! CSV on the way out. Off by default and free when off: the switch is read once and every entry
//! point returns immediately.
//!
//! It exists because frame cost is the one thing about a TUI that cannot be reasoned about from
//! the source. A repaint that looks linear may be memoized, a memo that looks sound may be missing
//! its invalidation, and the difference is invisible until you count. Pair it with
//! `scripts/tui-latency.py`, which drives the real binary through a pty at a fixed key rate: this
//! says where a frame went, that says whether the loop kept up.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::Instant;

/// What the render loop knows about one painted frame.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Frame {
    /// What woke the loop: `input`, `event`, `tick`, or `motion`.
    pub(crate) wake: &'static str,
    /// Microseconds spent applying what woke it, before any drawing.
    pub(crate) update_us: u128,
    /// Microseconds spent drawing, including the write to the terminal.
    pub(crate) draw_us: u128,
    /// Terminal events folded into this frame beyond the one that woke it.
    pub(crate) coalesced: usize,
}

/// A frame, plus what the transcript reported while it was being drawn.
#[derive(Debug, Clone, Copy)]
struct Row {
    frame: Frame,
    /// Microseconds of the draw spent laying the transcript out.
    layout_us: u128,
    /// Conversation items and rendered rows the transcript held.
    items: usize,
    rows: usize,
}

thread_local! {
    static ROWS: RefCell<Vec<Row>> = const { RefCell::new(Vec::new()) };
    /// What the transcript's layout walk reported during the frame being drawn.
    static TRANSCRIPT: RefCell<(u128, usize, usize)> = const { RefCell::new((0, 0, 0)) };
}

static PATH: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();

fn path() -> Option<&'static std::path::Path> {
    PATH.get_or_init(|| std::env::var_os("LUXE_FRAME_PROFILE").map(Into::into)).as_deref()
}

/// Whether anything asked for a profile.
pub(crate) fn enabled() -> bool {
    path().is_some()
}

/// Records how long the transcript's layout walk took, and what it produced.
pub(crate) fn note_transcript(started: Instant, items: usize, rows: usize) {
    if enabled() {
        let elapsed = started.elapsed().as_micros();
        TRANSCRIPT.with(|slot| *slot.borrow_mut() = (elapsed, items, rows));
    }
}

/// Records one painted frame, together with whatever the transcript reported while drawing it.
pub(crate) fn note_frame(frame: Frame) {
    if !enabled() {
        return;
    }
    let (layout_us, items, rows) = TRANSCRIPT.with(|slot| *slot.borrow());
    ROWS.with(|recorded| recorded.borrow_mut().push(Row { frame, layout_us, items, rows }));
}

/// Writes everything recorded so far, if a profile was asked for.
pub(crate) fn write() {
    let Some(path) = path() else {
        return;
    };
    let recorded = ROWS.with(|rows| std::mem::take(&mut *rows.borrow_mut()));
    let mut out = String::from("wake,update_us,draw_us,layout_us,coalesced,items,rows\n");
    for row in &recorded {
        out.push_str(&format!(
            "{},{},{},{},{},{},{}\n",
            row.frame.wake,
            row.frame.update_us,
            row.frame.draw_us,
            row.layout_us,
            row.frame.coalesced,
            row.items,
            row.rows
        ));
    }
    if let Err(error) = std::fs::write(path, out) {
        tracing::warn!(%error, path = %path.display(), "frame profile could not be written");
    }
}
