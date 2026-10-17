//! The spinner — its frames in each vocabulary, and how many ticks each frame holds.

use super::state::{braille, tier};
use super::GlyphTier;

/// The ten-frame braille sweep, when something on the machine can draw it.
///
/// Not in the terminal's font, and it does not have to be: this is the one glyph class that is
/// allowed to fall back, because it occupies one cell by `wcwidth` and nothing is aligned to it.
/// The survey that produced `glyphs/portable.txt` asked the wrong question of it — *does the
/// terminal face contain this* — and JetBrainsMono Nerd Font contains none of the 256 patterns
/// while rendering them perfectly well, out of Noto Sans Symbols 2.
///
/// Ten frames rather than four is the whole point: the sweep reads as motion instead of as four
/// positions. It is pi's set, which is the de-facto one.
pub(super) const SPINNER_BRAILLE: [&str; 10] = [
    "\u{280B}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283C}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280F}",
];

/// One quarter block orbiting the corners of its cell.
///
/// Block Elements, which terminals synthesize against the cell box rather than scaling a font
/// outline into it, so the animation lands the same whether the reader has a patched font or the
/// plainest monospace their distribution shipped — the wordmark's guarantee, for the same reason
/// and out of the same range.
///
/// **The mass has to stay constant**, which is what rules out the obvious eight-frame version
/// alternating quarters with halves. Rotation is read from a shape that moves; one that also grows
/// and shrinks reads as a flicker between unrelated glyphs, and was described by the first reader
/// to meet it as "different sized square and rectangle glyphs" — which is exactly what it was.
/// Four corners at one size is a coarser animation and a better spinner.
pub(super) const SPINNER_BLOCKS: [&str; 4] = ["\u{2598}", "\u{259D}", "\u{2597}", "\u{2596}"];

/// The same sweep for a terminal that is not speaking UTF-8.
pub(super) const SPINNER_ASCII: [&str; 4] = ["|", "/", "-", "\\"];

/// Render ticks each frame of the **block** sweep is held for.
///
/// The loop ticks at 30fps because that is what streamed text and the activity comet were drawn
/// for, and a spinner read straight off that counter turns near four times a second — faster than
/// an eye resolves as rotation. Four quarter blocks at that rate are not motion, they are a
/// flicker, so they step on their own clock: three ticks a frame puts one revolution just under
/// 400ms.
pub(super) const SPINNER_BLOCK_TICKS: usize = 3;

/// Render ticks each frame of the **braille** sweep is held for.
///
/// One: the sweep steps on every frame the loop draws, which is the smoothest a 30fps loop can be
/// and is what the braille did before it inherited the blocks' clock. Ten frames of one or two
/// dots is a shimmer at that rate rather than the flicker four quarter blocks would be — the whole
/// reason the two vocabularies do not share a number.
///
/// A held frame can only last a multiple of 33ms, so the options were 33, 67 and 100. It sat at
/// 100 for a release by accident; 67 was not tellable from it by eye, which leaves the one that is.
const SPINNER_BRAILLE_TICKS: usize = 1;

/// Render ticks each frame of the sweep the process will actually draw is held for.
///
/// For the callers that have to advance the spinner by a whole frame rather than by a tick.
pub(crate) fn spinner_ticks_per_frame() -> usize {
    match tier() {
        GlyphTier::Ascii => SPINNER_BLOCK_TICKS,
        _ if braille() => SPINNER_BRAILLE_TICKS,
        _ => SPINNER_BLOCK_TICKS,
    }
}

/// The turn spinner at render tick `frame`, wrapping.
///
/// One owner, because there were two: an eight-frame array in the status model and a ten-frame one
/// in the bar, spinning at different rates in the same window.
pub fn spinner(frame: usize) -> &'static str {
    // Each vocabulary keeps its own cadence: ten braille frames and four quarter blocks are not the
    // same animation, and one number for both slows whichever it was not chosen for.
    let step = frame / spinner_ticks_per_frame();
    match tier() {
        // `ascii` means assume nothing beyond ASCII, which a fallback font cannot rescue.
        GlyphTier::Ascii => SPINNER_ASCII[step % SPINNER_ASCII.len()],
        _ if braille() => SPINNER_BRAILLE[step % SPINNER_BRAILLE.len()],
        _ => SPINNER_BLOCKS[step % SPINNER_BLOCKS.len()],
    }
}
