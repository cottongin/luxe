//! wordmark_gif — the logotype as the terminal lights it, as a looping GIF for the README.
//!
//! Every frame comes from [`Wordmark::lines_at`], the same rows and the same two lights the
//! dashboard draws, so the recording cannot drift from the product. GitHub shows a README image on
//! its own page background, light or dark, so the background is transparent: the art is hard-edged
//! cells, which GIF's one-bit transparency draws exactly.
//!
//! ```sh
//! cargo run -p luxe-tui --example wordmark_gif -- dark  > web/src/assets/generated/wordmark-dark.gif
//! cargo run -p luxe-tui --example wordmark_gif -- light > web/src/assets/generated/wordmark-light.gif
//! ```

use luxe_tui::main_loop::ANIMATION_TICK;
use luxe_tui::widgets::{
    Wordmark, WordmarkInk, DRIFT_FRAME_TICKS, DRIFT_PERIOD_TICKS, SHINE_CYCLE_TICKS,
    SHINE_SWEEP_TICKS,
};
use ratatui::style::Color;
use std::borrow::Cow;
use std::io::Write;

/// Pixels per cell: the 10×22 grid `wordmark_svg` draws, twice over so the mark stays sharp on a
/// high-density screen when the README shows it at half size.
const CELL_W: usize = 20;
const CELL_H: usize = 44;

fn main() {
    let dark = match std::env::args().nth(1).as_deref() {
        Some("dark") => true,
        Some("light") => false,
        other => {
            eprintln!("usage: wordmark_gif dark|light  (got {other:?})");
            std::process::exit(2);
        }
    };
    let mut out = Vec::new();
    write_gif(&mut out, ink(dark)).expect("encoding into memory cannot fail");
    std::io::stdout().write_all(&out).expect("write the GIF to stdout");
}

/// The luxe theme's accent and canvas. `dark` sets how hard the shine pushes toward white: a light
/// page washes the letters out sooner, so it gets the gentler peak the terminal uses on light
/// themes. The canvas stays the dark one either way, so the drift deepens the brass on both.
fn ink(dark: bool) -> WordmarkInk {
    let theme = luxe_core::theme::ThemeRegistry::new().resolve("luxe").expect("a built-in theme");
    let rgb = |token: &str| match theme.get(token) {
        Some(luxe_core::theme::ColorValue::Rgb(red, green, blue)) => {
            Color::Rgb(*red, *green, *blue)
        }
        other => panic!("the luxe theme's {token} is a color, not {other:?}"),
    };
    WordmarkInk { accent: rgb("accent"), canvas: Some(rgb("background")), dark }
}

/// The ticks a frame is drawn at across one loop: every tick while the shine crosses, every drift
/// frame while it rests, as the terminal repaints.
///
/// One loop is one drift period, which has to close on itself for the GIF to loop without a seam.
/// The shine crosses once at its start and rests for the remainder.
fn frame_ticks() -> Vec<u64> {
    (0..DRIFT_PERIOD_TICKS)
        .filter(|&tick| tick < SHINE_SWEEP_TICKS || tick % DRIFT_FRAME_TICKS == 0)
        .collect()
}

/// Each frame's delay in hundredths of a second, rounded against the running clock so the loop
/// totals the drift period rather than gaining a rounding error per frame.
fn delays(ticks: &[u64]) -> Vec<u16> {
    let tick_ms = ANIMATION_TICK.as_millis() as f64;
    let at = |tick: u64| (tick as f64 * tick_ms / 10.0).round() as i64;
    let ends = ticks.iter().skip(1).copied().chain([DRIFT_PERIOD_TICKS]);
    ticks.iter().zip(ends).map(|(&start, end)| (at(end) - at(start)).max(1) as u16).collect()
}

fn write_gif(out: &mut Vec<u8>, ink: WordmarkInk) -> Result<(), gif::EncodingError> {
    let size = Wordmark::Full;
    let (width, height) = (usize::from(size.width()) * CELL_W, usize::from(size.height()) * CELL_H);
    let mut encoder = gif::Encoder::new(out, width as u16, height as u16, &[])?;
    encoder.set_repeat(gif::Repeat::Infinite)?;
    let ticks = frame_ticks();
    for (&tick, delay) in ticks.iter().zip(delays(&ticks)) {
        // The shine's own cycle is shorter than the loop; past it the shine rests
        let shine = (tick < SHINE_CYCLE_TICKS).then_some(tick);
        let (pixels, palette) = render(size, ink, tick, shine, width);
        encoder.write_frame(&frame((width, height), pixels, palette, delay))?;
    }
    Ok(())
}

/// One whole-canvas frame, index 0 transparent.
fn frame(
    (width, height): (usize, usize),
    pixels: Vec<u8>,
    palette: Vec<u8>,
    delay: u16,
) -> gif::Frame<'static> {
    gif::Frame {
        width: width as u16,
        height: height as u16,
        buffer: Cow::Owned(pixels),
        palette: Some(palette),
        transparent: Some(0),
        delay,
        // Cleared between frames, or a cell turned transparent would keep the last frame's ink
        dispose: gif::DisposalMethod::Background,
        ..gif::Frame::default()
    }
}

/// One frame as palette indices and the palette, index 0 transparent.
fn render(
    size: Wordmark,
    ink: WordmarkInk,
    drift: u64,
    shine: Option<u64>,
    width: usize,
) -> (Vec<u8>, Vec<u8>) {
    let mut palette = vec![0u8, 0, 0];
    let mut pixels = vec![0u8; width * usize::from(size.height()) * CELL_H];
    for (row, line) in size.lines_at(ink, drift, shine).iter().enumerate() {
        let cells = line
            .spans
            .iter()
            .flat_map(|span| span.content.chars().map(move |glyph| (glyph, span.style.fg)));
        for (column, (glyph, fg)) in cells.enumerate() {
            let Some(Color::Rgb(red, green, blue)) = fg else { continue };
            let index = palette_index(&mut palette, [red, green, blue]);
            ink_cell(&mut pixels, width, (column, row), glyph, index);
        }
    }
    (pixels, palette)
}

/// Paints the part of cell `(column, row)` that `glyph` inks: all of it, or the top or bottom half.
fn ink_cell(
    pixels: &mut [u8],
    width: usize,
    (column, row): (usize, usize),
    glyph: char,
    index: u8,
) {
    let (top, bottom) = match glyph {
        '█' => (0, CELL_H),
        '▀' => (0, CELL_H / 2),
        '▄' => (CELL_H / 2, CELL_H),
        _ => return,
    };
    for y in row * CELL_H + top..row * CELL_H + bottom {
        pixels[y * width + column * CELL_W..][..CELL_W].fill(index);
    }
}

/// The palette slot for `color`, adding it the first time it is seen.
fn palette_index(palette: &mut Vec<u8>, color: [u8; 3]) -> u8 {
    let existing = palette.chunks(3).position(|entry| entry == color);
    let index = existing.unwrap_or_else(|| {
        palette.extend_from_slice(&color);
        palette.len() / 3 - 1
    });
    u8::try_from(index).expect("a frame of one hue lit two ways stays far under 256 colors")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The loop is one drift period to the hundredth of a second, so it closes without a jump.
    #[test]
    fn the_frames_add_up_to_one_drift_period() {
        let total: u32 = delays(&frame_ticks()).iter().map(|&delay| u32::from(delay)).sum();
        let period_cs =
            (DRIFT_PERIOD_TICKS as f64 * ANIMATION_TICK.as_millis() as f64 / 10.0).round();
        assert_eq!(f64::from(total), period_cs);
    }

    /// The shine crosses frame by frame, and the rest repaints at the drift's slower cadence.
    #[test]
    fn the_shine_gets_every_tick_and_the_rest_every_drift_frame() {
        let ticks = frame_ticks();
        assert!((0..SHINE_SWEEP_TICKS).all(|tick| ticks.contains(&tick)));
        assert!(ticks
            .iter()
            .filter(|&&tick| tick >= SHINE_SWEEP_TICKS)
            .all(|tick| tick % DRIFT_FRAME_TICKS == 0));
    }

    /// The background is transparent and the letters are ink: a frame has both.
    #[test]
    fn a_frame_is_transparent_around_inked_letters() {
        let size = Wordmark::Full;
        let width = usize::from(size.width()) * CELL_W;
        let (pixels, palette) = render(size, ink(true), 0, None, width);
        assert!(pixels.contains(&0), "transparent background");
        assert!(pixels.iter().any(|&index| index != 0), "inked letters");
        assert!(palette.len() / 3 > 1);
    }
}
