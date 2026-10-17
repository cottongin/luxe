//! Glyph state — the vocabulary and braille switch every glyph call reads, set once at startup.

use super::detect::braille_available;
use super::GlyphTier;
use luxe_core::config::GlyphStyle;
use std::sync::atomic::{AtomicU8, Ordering};

/// The process-wide tier, as a discriminant so it can be swapped atomically.
static TIER: AtomicU8 = AtomicU8::new(0);

/// Whether the spinner may use its braille sweep.
///
/// Off by default, which is what keeps a test, a fixture and a generated site asset drawing the
/// same thing on any machine: the block orbit asks nothing of any font. Only the startup path
/// turns it on, and only after measuring.
static BRAILLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Resolves the whole vocabulary from `preference` and installs it process-wide.
///
/// One call, because there are two questions and they are asked at the same two moments — startup
/// and `/reload`. Setting the tier and forgetting the sweep is the mistake this exists to make
/// impossible.
pub fn resolve_vocabulary(preference: GlyphStyle) {
    set_tier(GlyphTier::resolve(preference));
    set_braille(braille_available());
}

/// Whether the spinner is currently drawing its braille sweep.
pub fn braille() -> bool {
    BRAILLE.load(Ordering::Relaxed)
}

/// Chooses the spinner's vocabulary. Exposed for tests and for the startup probe.
pub fn set_braille(available: bool) {
    BRAILLE.store(available, Ordering::Relaxed);
}

/// Sets the vocabulary every [`Icon::glyph`](super::Icon::glyph) call renders with.
pub fn set_tier(tier: GlyphTier) {
    TIER.store(
        match tier {
            GlyphTier::Nerd => 0,
            GlyphTier::Unicode => 1,
            GlyphTier::Ascii => 2,
            GlyphTier::NerdWide => 3,
        },
        Ordering::Relaxed,
    );
}

/// The tier [`Icon::glyph`](super::Icon::glyph) is currently rendering with.
pub fn tier() -> GlyphTier {
    match TIER.load(Ordering::Relaxed) {
        1 => GlyphTier::Unicode,
        2 => GlyphTier::Ascii,
        3 => GlyphTier::NerdWide,
        _ => GlyphTier::Nerd,
    }
}
