//! graphics — how an image becomes terminal cells.
//!
//! Three fidelities, chosen the way [`crate::glyphs`] chooses icon tiers and for the same reason: a
//! terminal that cannot do the beautiful thing should lose shine, not information.
//!
//! - [`ImageTier::Pixels`] — the real picture, via the kitty graphics protocol's Unicode
//!   placeholders (`kitty`).
//! - [`ImageTier::Blocks`] — half-block cells (`blocks`). Coarse, but it is ordinary colored
//!   text, so it works in every terminal, inside tmux, and over ssh.
//! - [`ImageTier::Text`] — a line naming the image and its size.
//!
//! [`ImageStore`] owns everything with a lifetime: the source bytes, the scaled raster or the
//! transmitted image, and the ids the terminal has to be told to forget. The render path asks it
//! how big an image wants to be ([`ImageStore::fit`]) while laying out, then hands it cells to
//! paint ([`ImageStore::paint_row`]) — one row at a time, so an image scrolled half out of the
//! viewport simply has fewer rows painted and the terminal crops it for free.
//!
//! Escape sequences that are *not* cell content — transmitting an image, telling the terminal to
//! release one — queue up in the store and are drained to the stream by the event loop
//! ([`ImageStore::drain_control`]). Keeping them out of the buffer is what leaves ratatui's diff
//! able to reason about every cell it owns.

mod blocks;
mod detect;
mod kitty;
mod raster;
mod store;

pub use detect::{Detection, ImageTier};
#[cfg(test)]
use raster::fit_within;
use std::sync::Arc;
pub use store::ImageStore;

/// Terminal cell geometry, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellSize {
    pub width: u16,
    pub height: u16,
}

impl Default for CellSize {
    fn default() -> Self {
        Self::ASSUMED
    }
}

impl CellSize {
    /// What a cell is assumed to be when the terminal will not say.
    ///
    /// Measured here at 10×21, 9×20, 6×14 and 10×22 across Ghostty, kitty, foot and Alacritty —
    /// all close to 1:2.1. Being wrong costs a little letterboxing, never distortion, because the
    /// protocol fits an image to its rectangle with the aspect ratio preserved.
    pub const ASSUMED: Self = Self { width: 10, height: 21 };

    /// Asks the terminal for its cell size, falling back to [`Self::ASSUMED`].
    ///
    /// `TIOCGWINSZ` answers this on every terminal measured, so no escape-sequence round trip is
    /// needed; a terminal that reports zeros gets the assumption rather than a division by zero.
    pub fn detect() -> Self {
        let Ok(size) = crossterm::terminal::window_size() else {
            return Self::ASSUMED;
        };
        Self::from_window(size.width, size.height, size.columns, size.rows)
    }

    /// Cell size from a window's pixel and cell dimensions.
    fn from_window(width: u16, height: u16, columns: u16, rows: u16) -> Self {
        if width == 0 || height == 0 || columns == 0 || rows == 0 {
            return Self::ASSUMED;
        }
        Self { width: (width / columns).max(1), height: (height / rows).max(1) }
    }
}

/// How much room an image takes in the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fit {
    pub cols: u16,
    pub rows: u16,
}

/// Identity for one image, stable across frames.
///
/// An opaque string because the two things that produce images — a tool result and a pasted
/// attachment — are identified in different terms, and neither of them belongs in this module.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImageKey(Arc<str>);

impl ImageKey {
    /// Names an image by whatever the caller identifies it with.
    pub fn new(id: impl AsRef<str>) -> Self {
        Self(Arc::from(id.as_ref()))
    }

    /// The image a tool call produced.
    pub fn tool_result(session_id: &str, call_id: &str, index: usize) -> Self {
        Self::new(format!("tool:{session_id}:{call_id}:{index}"))
    }
}

/// What an image is called and where it came from.
#[derive(Debug, Clone, Default)]
pub struct ImageOrigin {
    /// What produced it: a tool's name, or `pasted image`.
    pub label: String,
    /// The file it lives in, when it lives in one and that file is really an image.
    pub path: Option<std::path::PathBuf>,
}

impl ImageOrigin {
    /// An origin from a tool's name and the `name` its image block carried.
    ///
    /// The name is provenance a *model* chose, so it is checked before it becomes something a
    /// click can act on: resolved against the working directory, required to exist as a regular
    /// file, and required to carry an image extension. A front-end that offers to open a path it
    /// has not checked is offering to open whatever the model felt like naming.
    pub fn new(label: impl Into<String>, name: Option<&str>, cwd: &std::path::Path) -> Self {
        Self { label: label.into(), path: name.and_then(|name| openable_image(name, cwd)) }
    }
}

/// Extensions a click is willing to hand to the desktop's file opener.
const OPENABLE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// `name` as a path worth offering to open, or `None`.
fn openable_image(name: &str, cwd: &std::path::Path) -> Option<std::path::PathBuf> {
    let candidate = std::path::Path::new(name);
    let resolved =
        if candidate.is_absolute() { candidate.to_path_buf() } else { cwd.join(candidate) };
    let extension = resolved.extension()?.to_str()?.to_ascii_lowercase();
    if !OPENABLE_EXTENSIONS.contains(&extension.as_str()) {
        return None;
    }
    resolved.is_file().then_some(resolved)
}

#[cfg(test)]
mod tests;
