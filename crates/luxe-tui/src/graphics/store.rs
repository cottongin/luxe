//! The image store — every image's bytes and its prepared, live form, and their bounded lifetimes.

use super::detect::ImageTier;
use super::raster::{fit_within, probe_dimensions, scale_to_png};
use super::{blocks, kitty, CellSize, Fit, ImageKey, ImageOrigin};
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use std::collections::HashMap;
use std::sync::Arc;

/// Bytes one image may carry before it is refused.
const MAX_SOURCE_BYTES: usize = 24 * 1024 * 1024;

/// Pixels one image may decode to. A decoded image costs four bytes a pixel, and the only thing a
/// 40-megapixel source proves is that nothing checked.
const MAX_PIXELS: u64 = 40_000_000;

/// Longest edge either dimension may have, so a decompression bomb is refused on its header.
const MAX_EDGE: u32 = 16_384;

/// Images kept live — rasterized, or transmitted and held in the terminal's memory.
///
/// The transcript only ever paints what is on screen, so this is generous next to a viewport and
/// small next to a session: scrolling back through a hundred screenshots re-prepares rather than
/// holding all of them.
const LIVE_CAPACITY: usize = 16;

/// Source bytes held across all images before the oldest are dropped.
const MAX_TOTAL_SOURCE_BYTES: usize = 64 * 1024 * 1024;

/// One image's original bytes and what the header says about them.
struct Source {
    bytes: Arc<[u8]>,
    width: u32,
    height: u32,
    origin: ImageOrigin,
    inserted: u64,
}

/// What has been prepared for drawing, at the size it was prepared for.
struct Live {
    fit: Fit,
    seen: u64,
    kind: LiveKind,
}

enum LiveKind {
    /// Transmitted to the terminal under this id.
    Pixels(u32),
    /// Rasterized to half-block cells.
    Blocks(blocks::Raster),
}

/// Decoded images, what has been prepared from them, and what the terminal must be told.
pub struct ImageStore {
    tier: ImageTier,
    cell: CellSize,
    sources: HashMap<ImageKey, Source>,
    live: HashMap<ImageKey, Live>,
    /// Escapes owed to the terminal that are not cell content.
    control: String,
    clock: u64,
    next_id: u32,
    total_source_bytes: usize,
}

impl Default for ImageStore {
    fn default() -> Self {
        Self::new(ImageTier::Text, CellSize::ASSUMED)
    }
}

impl ImageStore {
    /// A store drawing at `tier` against `cell`-sized cells.
    pub fn new(tier: ImageTier, cell: CellSize) -> Self {
        Self {
            tier,
            cell,
            sources: HashMap::new(),
            live: HashMap::new(),
            control: String::new(),
            clock: 0,
            // Ids are global to the terminal, so a fixed base would collide with any other program
            // drawing images in the same window — and a collision silently replaces *their* image.
            next_id: seed_id(),
            total_source_bytes: 0,
        }
    }

    /// The fidelity this store is drawing at.
    pub fn tier(&self) -> ImageTier {
        self.tier
    }

    /// Switches fidelity, releasing anything prepared for the old one.
    pub fn set_tier(&mut self, tier: ImageTier) {
        if self.tier == tier {
            return;
        }
        self.tier = tier;
        self.release_prepared();
    }

    /// Re-reads the cell geometry, releasing rasters scaled against the old one.
    pub fn set_cell_size(&mut self, cell: CellSize) {
        if self.cell == cell {
            return;
        }
        self.cell = cell;
        self.release_prepared();
    }

    /// Takes in an image, returning whether it can be drawn.
    ///
    /// Refuses anything oversized on its header rather than on its pixels, so a decompression bomb
    /// costs a parse instead of a gigabyte. Re-inserting a known key is a no-op, which is what
    /// makes replaying a session's transcript on resume free.
    pub fn insert(&mut self, key: ImageKey, bytes: Vec<u8>, origin: ImageOrigin) -> bool {
        if self.sources.contains_key(&key) {
            return true;
        }
        if bytes.is_empty() || bytes.len() > MAX_SOURCE_BYTES {
            return false;
        }
        let Some((width, height)) = probe_dimensions(&bytes) else {
            return false;
        };
        if width == 0 || height == 0 || width > MAX_EDGE || height > MAX_EDGE {
            return false;
        }
        if u64::from(width) * u64::from(height) > MAX_PIXELS {
            return false;
        }
        self.clock += 1;
        self.total_source_bytes += bytes.len();
        self.sources.insert(
            key,
            Source { bytes: Arc::from(bytes), width, height, origin, inserted: self.clock },
        );
        self.trim_sources();
        true
    }

    /// Whether an image with this key can be drawn.
    pub fn contains(&self, key: &ImageKey) -> bool {
        self.sources.contains_key(key)
    }

    /// The image's pixel dimensions.
    pub fn source_size(&self, key: &ImageKey) -> Option<(u32, u32)> {
        self.sources.get(key).map(|source| (source.width, source.height))
    }

    /// What to call this image, and the file to open when the terminal cannot draw it.
    pub fn origin(&self, key: &ImageKey) -> Option<&ImageOrigin> {
        self.sources.get(key).map(|source| &source.origin)
    }

    /// How many cells the image wants, fitted into `max_cols`×`max_rows`.
    ///
    /// Width-first, because the transcript's width is the given and its height is not; the source's
    /// aspect ratio then decides the height, and a rectangle taller than the cap is scaled back
    /// down by width so the image shrinks rather than being cut off.
    pub fn fit(&self, key: &ImageKey, max_cols: u16, max_rows: u16) -> Option<Fit> {
        let source = self.sources.get(key)?;
        Some(fit_within((source.width, source.height), max_cols, max_rows, self.cell))
    }

    /// Paints one row of an image at `(x, y)`, preparing it if this is the first sight of it.
    ///
    /// Returns whether anything was drawn; `false` means the caller should fall back to text —
    /// which happens at [`ImageTier::Text`], for an unknown key, and when an image turns out not to
    /// decode after its header said it would.
    pub fn paint_row(
        &mut self,
        buf: &mut Buffer,
        (x, y): (u16, u16),
        key: &ImageKey,
        fit: Fit,
        image_row: u16,
        canvas: Color,
    ) -> bool {
        if !self.tier.draws_pixels() || fit.cols == 0 || fit.rows == 0 || image_row >= fit.rows {
            return false;
        }
        if !self.prepare(key, fit) {
            return false;
        }
        self.clock += 1;
        let clock = self.clock;
        let Some(live) = self.live.get_mut(key) else {
            return false;
        };
        live.seen = clock;

        match &live.kind {
            LiveKind::Pixels(id) => {
                let color = kitty::id_color(*id);
                for col in 0..fit.cols {
                    let Some(cell) = buf.cell_mut((x + col, y)) else {
                        break;
                    };
                    // The background shows through wherever the image is transparent, so it stays
                    // whatever the transcript painted the band.
                    cell.set_symbol(&kitty::placeholder(image_row, col))
                        .set_fg(color)
                        .set_bg(canvas);
                }
            }
            LiveKind::Blocks(raster) => {
                let under = blocks::canvas_rgb(canvas);
                for col in 0..fit.cols.min(raster.cols()) {
                    let Some(cell) = buf.cell_mut((x + col, y)) else {
                        break;
                    };
                    let (upper, lower) = raster.cell(col, image_row, under);
                    cell.set_symbol(blocks::Raster::symbol()).set_fg(upper).set_bg(lower);
                }
            }
        }
        true
    }

    /// Escapes the terminal is owed — transmissions and releases — cleared as they are taken.
    ///
    /// Written to the stream by the event loop rather than through the cell buffer, so ratatui's
    /// diff never has to reason about a symbol that is really a command.
    pub fn drain_control(&mut self) -> String {
        std::mem::take(&mut self.control)
    }

    /// Releases images past `LIVE_CAPACITY`, oldest sight first.
    ///
    /// Called once a frame, after painting, so what is on screen has just been touched and is
    /// never the thing evicted.
    pub fn sweep(&mut self) {
        while self.live.len() > LIVE_CAPACITY {
            let Some(stale) =
                self.live.iter().min_by_key(|(_, live)| live.seen).map(|(key, _)| key.clone())
            else {
                break;
            };
            self.forget(&stale);
        }
    }

    /// Releases every image this store put in the terminal. For shutdown.
    pub fn release_all(&mut self) {
        let keys: Vec<ImageKey> = self.live.keys().cloned().collect();
        for key in keys {
            self.forget(&key);
        }
    }

    /// Forgets everything, including the sources. For a session switch, whose images belong to a
    /// transcript that is no longer on screen.
    pub fn clear(&mut self) {
        self.release_all();
        self.sources.clear();
        self.total_source_bytes = 0;
    }

    /// Ensures `key` is prepared at `fit`, re-preparing when the size changed.
    fn prepare(&mut self, key: &ImageKey, fit: Fit) -> bool {
        if self.live.get(key).is_some_and(|live| live.fit == fit) {
            return true;
        }
        self.forget(key);

        let Some(source) = self.sources.get(key) else {
            return false;
        };
        let bytes = Arc::clone(&source.bytes);
        let Ok(decoded) = image::load_from_memory(&bytes) else {
            // The header parsed and the pixels did not. Drop the source so every later frame does
            // not pay the same failed decode.
            self.drop_source(key);
            return false;
        };

        let kind = match self.tier {
            ImageTier::Blocks => {
                LiveKind::Blocks(blocks::Raster::new(&decoded, fit.cols, fit.rows))
            }
            ImageTier::Pixels => {
                let Some(png) = scale_to_png(&decoded, fit, self.cell) else {
                    return false;
                };
                let id = self.allocate_id();
                self.control.push_str(&kitty::transmit(id, &png, fit.cols, fit.rows));
                LiveKind::Pixels(id)
            }
            ImageTier::Text => return false,
        };

        self.clock += 1;
        self.live.insert(key.clone(), Live { fit, seen: self.clock, kind });
        true
    }

    /// Drops what was prepared for `key`, telling the terminal to release it too.
    fn forget(&mut self, key: &ImageKey) {
        if let Some(live) = self.live.remove(key) {
            if let LiveKind::Pixels(id) = live.kind {
                self.control.push_str(&kitty::delete(id));
            }
        }
    }

    /// Drops everything prepared, keeping the sources so it can be prepared again.
    fn release_prepared(&mut self) {
        let keys: Vec<ImageKey> = self.live.keys().cloned().collect();
        for key in keys {
            self.forget(&key);
        }
    }

    fn drop_source(&mut self, key: &ImageKey) {
        if let Some(source) = self.sources.remove(key) {
            self.total_source_bytes = self.total_source_bytes.saturating_sub(source.bytes.len());
        }
    }

    /// Drops the oldest sources once they add up to more than the budget.
    fn trim_sources(&mut self) {
        while self.total_source_bytes > MAX_TOTAL_SOURCE_BYTES && self.sources.len() > 1 {
            let Some(oldest) = self
                .sources
                .iter()
                .min_by_key(|(_, source)| source.inserted)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.forget(&oldest);
            self.drop_source(&oldest);
        }
    }

    fn allocate_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = if id >= kitty::MAX_ID { 1 } else { id + 1 };
        id
    }
}

/// A starting image id unlikely to collide with another program drawing in the same terminal.
fn seed_id() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos =
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(1, |since| since.subsec_nanos());
    let mixed = nanos ^ (std::process::id().wrapping_mul(2_654_435_761));
    // Leave room to allocate a session's worth of ids without wrapping into the low numbers a
    // simpler program is likely to have picked.
    (mixed % (kitty::MAX_ID / 2)).max(1)
}

#[cfg(test)]
mod tests;
