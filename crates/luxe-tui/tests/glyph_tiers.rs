//! Glyph tiers — the promise that turning the icons down costs shine and nothing else.
//!
//! Two claims are checked against **rendered buffers**, not against the vocabulary table, because the
//! table is only half the story: an icon can be portable and still wreck the surface it sits in.
//!
//! 1. Below the nerd tier, nothing on screen needs a patched font. A private-use codepoint reaching a
//!    buffer is a leak — a glyph someone inlined instead of naming — and there are no exemptions.
//!    There used to be one, for the wordmark's corner triangles, on the grounds that they were
//!    chosen by a separate *measured* mechanism rather than by the font. Both the wordmark and the
//!    slab have since retreated to Block Elements, so the range nothing draws is now the range
//!    nothing may draw.
//! 2. Where an icon is *substituted*, the layout is identical. Icons are budgeted one cell each, so a
//!    two-cell substitute would shift everything after it on the row; comparing each row's non-icon
//!    skeleton catches that exactly, and catches an icon silently growing to two glyphs as well.
//!
//! Two surfaces deliberately lay out differently below the nerd tier, and each gets the assertion that
//! actually applies to it:
//!
//! - **Tool cards and tool preview lines omit their icon** rather than approximate it — the bold name
//!   and the card's color already say which tool ran and how it went. The rule is therefore stated
//!   generally rather than per surface: a row's *words* must be identical, and any shift must be
//!   uniform across the row and exactly the width an omitted icon frees. A two-cell glyph fails it,
//!   because its shift is neither zero nor uniform.
//! - **The dashboard at `ascii`** swaps the block-drawn logotype for the letter-spaced line, and a
//!   narrower mark takes the whole left column with it — the same cascade a small terminal already
//!   triggers. Icon width is still pinned there by the exhaustive table test in `glyphs.rs`.
//!
//! `nerd_wide` is the one tier those two claims do **not** cover, because it exists to break the
//! second on purpose: it is the shipped vocabulary drawn by a font cut whose icons carry two cells of
//! ink, so it hands each of them a second cell rather than pretending they fit one. Its own claim is
//! narrower and gets its own walk
//! ([`every_icon_gets_its_second_cell_at_the_wide_tier`]): the words are the nerd tier's words, and
//! every icon on screen has two cells of clearance after it instead of one.
//!
//! The tier is process-wide, so the tests that move it cannot run beside each other — hence
//! [`serial`] — and living in its own integration binary keeps it away from the unit tests, which
//! all assume the shipped look.

use luxe_tui::app::App;
use luxe_tui::fixture::{seeded_app, FixtureScenario};
use luxe_tui::glyphs::{set_tier, GlyphTier, EVERY_ICON, EVERY_MARK};
use luxe_tui::view::render;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

const TIERS: [GlyphTier; 3] = [GlyphTier::Nerd, GlyphTier::Unicode, GlyphTier::Ascii];

/// The surfaces that carry icons, named the way the gate names them.
const SCENARIOS: [(&str, FixtureScenario); 6] = [
    ("dashboard", FixtureScenario::Dashboard),
    ("tool cards", FixtureScenario::ChatTools),
    // A folded run puts an icon behind a spinner and in front of a meter, which is two more chances
    // to spend a cell the plainer tiers do not.
    ("folded run", FixtureScenario::ChatToolRun),
    // The rail in full, with a slab badge on every section heading.
    ("dock", FixtureScenario::DockWide),
    // The bar in full: the dock's own scenario hides it, because at this width the sidebar sits
    // beside the conversation and `auto` visibility stands down. This one carries every identity
    // module *and* its slab caps, which are the one decoration budgeted a cell at every tier.
    ("statusline", FixtureScenario::ChatIdle),
    ("session picker", FixtureScenario::SessionPicker),
];

/// Take the process-wide glyph tier, at the shipped default.
///
/// `cargo test` runs one binary's tests on threads — nextest gives each its own process, which is
/// why the gate never saw this — so without a lock the tier one test sets lands in another's
/// render, and the failure surfaces on whichever one was unlucky. Acquiring resets to `Nerd` as
/// well, so a test that panics part-way through a walk cannot hand the next one a tier it never
/// asked for.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static TIER: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let guard = TIER.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    set_tier(GlyphTier::Nerd);
    guard
}

fn draw(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| render(frame, app)).expect("render");
    terminal.backend().buffer().clone()
}

fn scenario_app(scenario: FixtureScenario) -> App {
    seeded_app("tokyo-night", scenario).unwrap_or_else(|error| panic!("fixture: {error}"))
}

/// A row reduced to its words and their columns: every non-alphanumeric cell becomes a placeholder.
///
/// Blind to *which* shape drew a cell — the point is not to compare glyphs but to prove nothing moved.
/// An icon that measured two cells would push the text after it one column right, and the words would
/// stop lining up.
fn skeleton(buffer: &Buffer, y: u16) -> String {
    (0..buffer.area.width)
        .map(|x| match buffer[(x, y)].symbol().chars().next() {
            Some(glyph) if glyph.is_alphanumeric() => glyph,
            // ASCII on purpose: `str::find` returns byte offsets, and a multi-byte placeholder would
            // make a two-column shift read as four.
            _ => '.',
        })
        .collect()
}

/// An omitted icon frees its own cell and the space after it.
const OMITTED_ICON_COLUMNS: usize = 2;

/// The words on `buffer`'s rows, in order, ignoring where they landed.
fn every_word(buffer: &Buffer) -> Vec<Vec<String>> {
    (0..buffer.area.height)
        .map(|y| words(&skeleton(buffer, y)).into_iter().map(|(_, word)| word).collect())
        .collect()
}

/// Every single-letter glyph the `ascii` tier can draw.
///
/// The tier's whole job is to stay inside ASCII, so a handful of its glyphs are letters — `v` for a
/// tick, `x` for a cross. To a tokenizer splitting on alphanumerics those are words, and a row that
/// swapped `✓` for `v` would read as having *gained* one. Derived from the vocabulary rather than
/// listed, so a new letter-shaped fallback cannot quietly start failing the comparison.
fn ascii_letter_glyphs() -> std::collections::HashSet<String> {
    EVERY_ICON
        .iter()
        .map(|icon| icon.at(GlyphTier::Ascii))
        .chain(EVERY_MARK.iter().map(|mark| mark.at(GlyphTier::Ascii)))
        .filter(|glyph| glyph.chars().count() == 1)
        .filter(|glyph| glyph.chars().all(char::is_alphanumeric))
        .map(str::to_string)
        .collect()
}

/// The words on a row, each with the column it starts at.
///
/// A letter-shaped `ascii` glyph is dropped from **both** sides of every comparison, so the filter
/// can hide a difference but never invent one.
fn words(row: &str) -> Vec<(usize, String)> {
    let letters = ascii_letter_glyphs();
    let mut found: Vec<(usize, String)> = Vec::new();
    let mut current: Option<(usize, String)> = None;
    for (column, glyph) in row.chars().enumerate() {
        if glyph.is_alphanumeric() {
            current.get_or_insert((column, String::new())).1.push(glyph);
        } else if let Some(word) = current.take() {
            found.push(word);
        }
    }
    found.extend(current);
    found.retain(|(_, word)| !letters.contains(word));
    found
}

fn private_use(symbol: &str) -> Option<u32> {
    symbol
        .chars()
        .map(u32::from)
        .find(|cp| (0xE000..=0xF8FF).contains(cp) || (0xF0000..=0xFFFFD).contains(cp))
}

#[test]
fn every_tier_draws_the_same_layout_and_only_nerd_needs_a_patched_font() {
    let _serial = serial();
    // Sized so every scenario gets its wide layout, which is the one carrying the most icons.
    let (width, height) = (170, 48);

    for (name, scenario) in SCENARIOS {
        let mut reference: Option<Vec<String>> = None;

        for tier in TIERS {
            set_tier(tier);
            let buffer = draw(&mut scenario_app(scenario), width, height);

            if tier != GlyphTier::Nerd {
                for y in 0..buffer.area.height {
                    for x in 0..buffer.area.width {
                        let symbol = buffer[(x, y)].symbol();
                        if let Some(cp) = private_use(symbol) {
                            panic!(
                                "{name} at {tier:?} draws U+{cp:04X} at ({x}, {y}) — a private-use \
                                 glyph reached the screen, so something inlined it instead of naming it \
                                 in `glyphs.rs`"
                            );
                        }
                    }
                }
            }

            let rows: Vec<String> = (0..buffer.area.height).map(|y| skeleton(&buffer, y)).collect();
            match &reference {
                None => reference = Some(rows),
                Some(nerd) => {
                    if name == "dashboard" && tier == GlyphTier::Ascii {
                        continue; // deliberate: a different wordmark, and a different column with it
                    }
                    assert_eq!(rows.len(), nerd.len(), "{name} at {tier:?} changed its row count");
                    for (y, (row, expected)) in rows.iter().zip(nerd).enumerate() {
                        let (here, there) = (words(row), words(expected));
                        assert_eq!(
                            here.iter().map(|(_, word)| word).collect::<Vec<_>>(),
                            there.iter().map(|(_, word)| word).collect::<Vec<_>>(),
                            "{name} at {tier:?} row {y}: the words changed, not just the icons"
                        );
                        let shifts: Vec<usize> = here
                            .iter()
                            .zip(&there)
                            .map(|((here, _), (there, _))| there.saturating_sub(*here))
                            .collect();
                        for shift in &shifts {
                            assert!(
                                *shift == 0 || *shift == OMITTED_ICON_COLUMNS,
                                "{name} at {tier:?} row {y}: text moved {shift} columns, which is \
                                 neither \"nothing changed\" nor \"an icon was omitted\""
                            );
                        }
                        assert!(
                            shifts.windows(2).all(|pair| pair[0] == pair[1]),
                            "{name} at {tier:?} row {y}: the row shifted unevenly ({shifts:?}), so \
                             something on it is not one cell wide"
                        );
                    }
                }
            }
        }
    }

    set_tier(GlyphTier::Nerd);
}

/// The wide tier's whole job: the same surface, with a cell of air added between every icon and
/// whatever it labels.
///
/// Asserted over the *drawn* buffer rather than over the gap constant, because the constant is only
/// half of it — a surface that spaces its own icons with a literal keeps drawing them one cell apart
/// however the shared gap moves, and this is what catches that. The words are checked too: room is
/// supposed to come out of the slack a row already carries, and a row that pushed a word off its own
/// end instead has lost information rather than gained air.
#[test]
fn every_icon_gets_its_second_cell_at_the_wide_tier() {
    let _serial = serial();
    let (width, height) = (170, 48);

    for (name, scenario) in SCENARIOS {
        set_tier(GlyphTier::Nerd);
        let narrow = draw(&mut scenario_app(scenario), width, height);
        set_tier(GlyphTier::NerdWide);
        let wide = draw(&mut scenario_app(scenario), width, height);

        assert_eq!(
            every_word(&wide),
            every_word(&narrow),
            "{name} at nerd_wide: the words changed, so the extra cell came out of the content"
        );

        for y in 0..wide.area.height {
            for x in 0..wide.area.width {
                if private_use(wide[(x, y)].symbol()).is_none() {
                    continue;
                }
                // The row's own right edge counts as clearance: an icon with nothing after it has
                // nothing to crowd.
                let clearance = (x + 1..wide.area.width)
                    .take(2)
                    .filter(|&next| wide[(next, y)].symbol() == " ")
                    .count();
                assert!(
                    clearance == 2 || x + 2 >= wide.area.width,
                    "{name} at nerd_wide: the icon at ({x}, {y}) has {clearance} cells of \
                     clearance, so something spaced it with a literal instead of `icon_gap`"
                );
            }
        }
    }

    set_tier(GlyphTier::Nerd);
}

/// The Powerline Extra range, at **every** tier including `nerd`.
///
/// The slab's caps were `U+E0BA`/`U+E0BC` until alacritty was measured drawing the second at 62%
/// fill, and the wordmark's chamfers were the other four before that. Nothing draws them now, and
/// the cheapest way to keep it that way is to say so: this is the assertion that turns "we stopped
/// using them" into "they cannot come back without deleting this test".
#[test]
fn no_surface_reaches_for_a_powerline_triangle_at_any_tier() {
    let _serial = serial();
    const TRIANGLES: std::ops::RangeInclusive<u32> = 0xE0B8..=0xE0BE;
    for scenario in [FixtureScenario::ChatIdle, FixtureScenario::DockWide] {
        for tier in TIERS {
            set_tier(tier);
            let buffer = draw(&mut scenario_app(scenario), 170, 48);
            for y in 0..buffer.area.height {
                for x in 0..buffer.area.width {
                    for glyph in buffer[(x, y)].symbol().chars() {
                        assert!(
                            !TRIANGLES.contains(&u32::from(glyph)),
                            "a Powerline triangle reached ({x}, {y}) at {tier:?} in {scenario:?}"
                        );
                    }
                }
            }
        }
    }
    set_tier(GlyphTier::Nerd);
}

/// The default has to be the look Luxe ships with, so a surface rendered without any resolution step
/// is unchanged from before the tiers existed.
#[test]
fn the_untouched_default_is_the_nerd_vocabulary() {
    let _serial = serial();
    let mut app = scenario_app(FixtureScenario::ChatTools);
    let buffer = draw(&mut app, 120, 40);
    let found = (0..buffer.area.height)
        .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
        .any(|(x, y)| private_use(buffer[(x, y)].symbol()).is_some());
    assert!(found, "the shipped look draws Material icons; none reached the buffer");
}
