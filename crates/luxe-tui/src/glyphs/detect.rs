//! Glyph detection — what the terminal and its fonts can draw: braille, Nerd Font icons, kitty.

use super::spinner::SPINNER_BRAILLE;
use super::{GlyphTier, EVERY_ICON, RECENT_BRANDS};

/// Whether the terminal in front of the reader can draw the braille sweep.
///
/// Braille is not in the terminal's face and never needed to be — it is one cell, nothing is
/// aligned to it, and every desktop platform ships a font that covers the block in its fallback
/// chain. See [`braille_drawable`] for how the question is answered.
pub(super) fn braille_available() -> bool {
    braille_drawable(&|name| std::env::var(name).ok().filter(|value| !value.is_empty()), || {
        fontconfig_has_all(SPINNER_BRAILLE.iter().filter_map(|frame| frame.chars().next()))
    })
}

/// [`braille_available`], with the environment and the font probe passed in.
///
/// Asked of the terminal that draws, which is not always this machine:
///
/// - The kernel console (`TERM=linux`) cannot: its font is a few hundred glyphs and braille is not
///   among them, whatever is installed.
/// - macOS and Windows can: they ship Apple Symbols and Segoe UI Symbol, both covering
///   `U+2800`–`U+28FF` in the documented system fallback chain.
/// - Over SSH the terminal is at the other end, so this machine's fonts say nothing about it: a
///   headless server with no fonts at all may be shown on a laptop that has braille.
/// - kitty, Ghostty and WezTerm draw braille themselves, from the cell box, the way they draw box
///   lines, so no font is involved.
///
/// Anything else is a local terminal leaning on fontconfig, which is asked about the exact ten
/// frames — on a box carrying only Liberation and Noto that still resolves, to Noto Sans Symbols 2.
pub(super) fn braille_drawable(
    var: &dyn Fn(&str) -> Option<String>,
    fonts: impl FnOnce() -> bool,
) -> bool {
    let term = var("TERM").unwrap_or_default();
    if term == "linux" {
        return false;
    }
    if cfg!(any(target_os = "macos", target_os = "windows")) {
        return true;
    }
    if var("SSH_CONNECTION").is_some() || var("SSH_TTY").is_some() {
        return true;
    }
    let program = var("TERM_PROGRAM").unwrap_or_default().to_lowercase();
    let self_drawn = ["kitty", "ghostty", "wezterm"]
        .iter()
        .any(|terminal| term.contains(terminal) || program == *terminal);
    self_drawn || fonts()
}

/// Whether one installed font carries every character in `wanted`.
///
/// fontconfig ANDs a charset list, so the question costs a single process however many characters
/// it is asked about. One owner, because two callers ask it and the query is easy to get subtly
/// wrong — a list that is ORed would answer a different and much easier question.
fn fontconfig_has_all(wanted: impl IntoIterator<Item = char>) -> bool {
    let mut codepoints: Vec<u32> = wanted.into_iter().map(u32::from).collect();
    codepoints.sort_unstable();
    codepoints.dedup();
    if codepoints.is_empty() {
        return false;
    }
    let mut charset = String::from(":charset=");
    for (index, cp) in codepoints.iter().enumerate() {
        if index > 0 {
            charset.push(' ');
        }
        charset.push_str(&format!("{cp:x}"));
    }
    std::process::Command::new("fc-list")
        .args([charset.as_str(), "family"])
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|probe| probe.status.success() && names_a_real_font(&probe.stdout))
}

/// Whether `fc-list`'s family listing names a font a reader could have chosen.
///
/// macOS lists `.LastResort`, whose job is to draw a placeholder box for every codepoint, so it
/// answers yes to any charset question and the probe would never say no. Fonts the system hides
/// from its pickers carry a leading dot; none of them is a face a terminal is set to.
fn names_a_real_font(listing: &[u8]) -> bool {
    String::from_utf8_lossy(listing)
        .lines()
        .any(|family| !family.trim().is_empty() && !family.starts_with('.'))
}

/// Whether the Material icons can actually be drawn on this machine.
///
/// A terminal cannot be asked what its font contains, so this asks the two questions that do have
/// answers:
///
/// - **kitty ships its own symbol font** (`SymbolsNerdFontMono-Regular.ttf`), so its icons render
///   whatever the user configured. Measured rather than read: with `font_family=Liberation Mono`,
///   which contains none of these glyphs, kitty draws them anyway.
/// - **otherwise, ask fontconfig** whether one installed font provides **every** icon in the
///   vocabulary. On Linux that is a fair proxy for renderable, because terminals fall back through
///   fontconfig for a glyph their configured font lacks — ghostty draws the Material icons under
///   Liberation Mono for exactly that reason, and it bundles no font of its own.
///
/// Where fontconfig isn't the answer — macOS, Windows, a container without `fc-list` — this is false
/// and the fallback vocabulary is used. Conservative on purpose: a reader who *has* a patched font sees
/// plain shapes until they set `glyphs`, which is a smaller disappointment than a screen of boxes.
///
/// # Why the whole vocabulary and not a sample
///
/// One codepoint is no answer about ninety-odd: a partial font has most icons and misses a few,
/// which a one-glyph probe cannot see. Fifty-six of these live in the Material Design block that
/// Nerd Fonts **moved** in v3, so a v2 font passes a probe for, say, the cog and then draws boxes
/// for most of the dashboard.
///
/// fontconfig ANDs a charset list, so the exact question costs one process and about six
/// milliseconds. Requiring a *single* font to carry all of them is stricter than per-glyph
/// fallback strictly needs, and deliberately so: coverage scattered across three fonts renders at
/// three weights, which is the same disappointment arriving more slowly.
pub(super) fn nerd_icons_available() -> bool {
    if is_kitty() {
        return true;
    }
    // Derived from the table, so an icon added in a range nothing else uses extends the probe
    // rather than slipping past it. The recent brands are asked about on their own
    let probed = EVERY_ICON.iter().filter(|icon| !RECENT_BRANDS.contains(icon));
    fontconfig_has_all(probed.filter_map(|icon| icon.at(GlyphTier::Nerd).chars().next()))
}

/// Whether one installed font has every recent brand mark, which only Nerd Fonts 3.5 and later do.
///
/// fontconfig is the only way to ask, so without it (stock macOS, Windows) the answer is no and
/// each brand falls back to a mark every v3 font has. Not kitty's shortcut either: the symbol font
/// it bundles is as old as the kitty.
pub(super) fn brands_available() -> bool {
    fontconfig_has_all(
        RECENT_BRANDS.iter().filter_map(|icon| icon.at(GlyphTier::Nerd).chars().next()),
    )
}

/// Whether this is kitty — the one terminal that answers the question above by itself.
///
/// `TERM` is the only authoritative signal: the innermost pty sets it, so a `TERM` naming a terminal
/// names *this* terminal. `KITTY_WINDOW_ID` is inherited — a shell started from kitty carries it into
/// whatever it launches, so an alacritty opened from kitty would claim to be one. It is therefore
/// trusted only under a multiplexer's `TERM`, which describes the multiplexer and leaves nothing
/// better to go on.
fn is_kitty() -> bool {
    let term = std::env::var("TERM").unwrap_or_default();
    if term.contains("kitty") {
        return true;
    }
    let multiplexed = term.starts_with("tmux") || term.starts_with("screen");
    multiplexed && std::env::var_os("KITTY_WINDOW_ID").is_some()
}

#[cfg(test)]
mod probe_tests {
    use super::names_a_real_font;

    /// Copied from `fc-list ":charset=ec82 ec81 ec1e" family` on a Mac with Nerd Fonts older than 3.5.
    #[test]
    fn the_macos_placeholder_font_is_not_a_font_with_the_glyph() {
        assert!(!names_a_real_font(b".LastResort\n"));
    }

    #[test]
    fn a_hidden_font_beside_a_real_one_still_counts_the_real_one() {
        assert!(names_a_real_font(b".LastResort\nHack Nerd Font\n"));
    }

    #[test]
    fn an_empty_listing_names_no_font() {
        assert!(!names_a_real_font(b""));
        assert!(!names_a_real_font(b"\n"));
    }
}
