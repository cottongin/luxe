use super::*;

/// The terminal that draws is the one asked: SSH and the terminals that draw braille themselves
/// get it without a local font, and the kernel console never does.
#[test]
fn braille_is_asked_of_the_terminal_that_draws() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            pairs.iter().find(|(key, _)| *key == name).map(|(_, value)| (*value).to_owned())
        }
    };
    let no_fonts = || false;
    let desktop = cfg!(any(target_os = "macos", target_os = "windows"));

    assert!(!braille_drawable(&env(&[("TERM", "linux"), ("SSH_TTY", "/dev/pts/1")]), || true));
    assert!(braille_drawable(
        &env(&[("TERM", "xterm-256color"), ("SSH_TTY", "/dev/pts/1")]),
        no_fonts
    ));
    assert!(braille_drawable(&env(&[("SSH_CONNECTION", "10.0.0.2 5 10.0.0.1 22")]), no_fonts));
    assert!(braille_drawable(&env(&[("TERM", "xterm-kitty")]), no_fonts));
    assert!(braille_drawable(&env(&[("TERM", "xterm-ghostty")]), no_fonts));
    assert!(braille_drawable(&env(&[("TERM_PROGRAM", "WezTerm")]), no_fonts));
    assert_eq!(braille_drawable(&env(&[("TERM", "xterm-256color")]), no_fonts), desktop);
    assert!(braille_drawable(&env(&[("TERM", "xterm-256color")]), || true), "a font draws it");
}
use std::collections::HashMap;
use unicode_width::UnicodeWidthStr;

const TIERS: [GlyphTier; 4] =
    [GlyphTier::Nerd, GlyphTier::NerdWide, GlyphTier::Unicode, GlyphTier::Ascii];

/// The whole point of the slice: below the nerd tier, nothing on screen may need a patched font.
/// Asserted over the table rather than over a screenshot, because a table is exhaustive and an
/// eyeball is not.
#[test]
fn no_private_use_codepoint_escapes_the_fallback_tiers() {
    for tier in [GlyphTier::Unicode, GlyphTier::Ascii] {
        for icon in EVERY_ICON {
            for glyph in icon.at(tier).chars() {
                let cp = u32::from(glyph);
                assert!(
                    !(0xE000..=0xF8FF).contains(&cp) && !(0xF0000..=0xFFFFD).contains(&cp),
                    "{icon:?} at {tier:?} draws U+{cp:04X}, which needs a Nerd Font"
                );
            }
        }
    }
}

/// `ascii` has to survive a terminal that is not speaking UTF-8 at all.
#[test]
fn the_ascii_tier_is_actually_ascii() {
    for icon in EVERY_ICON {
        let glyph = icon.at(GlyphTier::Ascii);
        assert!(glyph.is_ascii(), "{icon:?} at ascii draws {glyph:?}");
    }
    for mark in EVERY_MARK {
        let glyph = mark.at(GlyphTier::Ascii);
        assert!(glyph.is_ascii(), "{mark:?} at ascii draws {glyph:?}");
    }
}

/// Characters a bare font has been *measured* to contain, plus the range terminals draw
/// themselves. Parsed from the survey's own output so the two cannot drift.
fn portable_characters() -> std::collections::HashSet<char> {
    include_str!("../glyphs/portable.txt")
        .lines()
        .filter_map(|line| line.strip_prefix("U+"))
        .filter_map(|line| line.split('\t').next())
        .filter_map(|hex| u32::from_str_radix(hex, 16).ok())
        .filter_map(char::from_u32)
        .collect()
}

/// The point of the fallback tiers, and the thing that was never checked: a substitute is only
/// a substitute if the font the reader fell back to actually has it.
///
/// `no_private_use_codepoint_escapes_the_fallback_tiers` proves these are not Nerd Font glyphs
/// and `every_rendering_is_one_cell_or_omitted` proves the Unicode width property says one
/// cell. Neither is a question about the reader's font, and for a long time nothing was:
/// Liberation Mono, the plainest monospace a distribution ships, was missing `◆ ▸ ◉ ✓ ✗ ❯`
/// and fourteen more — every one of them chosen *because* it was supposed to be safe.
///
/// `scripts/glyph-survey.py --check` is the other half: this asserts the vocabulary stays
/// inside the list, that asserts the list stays true of the nine faces it was measured from.
#[test]
fn the_fallback_tiers_draw_only_what_a_bare_font_has() {
    let portable = portable_characters();
    // U+2500-U+259F is synthesized by the terminal against the cell box, so the font is not in
    // the path and no survey applies.
    let synthesized = |c: char| ('\u{2500}'..='\u{259f}').contains(&c);
    let mut checked = 0;
    for tier in [GlyphTier::Unicode, GlyphTier::Ascii] {
        let renderings = EVERY_ICON
            .iter()
            .map(|icon| (format!("{icon:?}"), icon.at(tier)))
            .chain(EVERY_MARK.iter().map(|mark| (format!("{mark:?}"), mark.at(tier))));
        for (what, glyph) in renderings {
            for ch in glyph.chars() {
                checked += 1;
                assert!(
                    ch.is_ascii() || synthesized(ch) || portable.contains(&ch),
                    "{what} at {tier:?} draws {ch:?} (U+{:04X}), which is not in \
                         glyphs/portable.txt — add it there with a survey run that proves every \
                         face on the panel has it, or pick a character that is already listed",
                    u32::from(ch)
                );
            }
        }
    }
    assert!(checked > 100, "the survey should be covering the whole vocabulary, saw {checked}");
}

/// A mark is part of a row's grammar, so unlike an icon it may never stand down.
#[test]
fn no_mark_is_ever_omitted() {
    for mark in EVERY_MARK {
        for tier in TIERS {
            let glyph = mark.at(tier);
            assert!(!glyph.is_empty(), "{mark:?} has no rendering at {tier:?}");
            assert_eq!(glyph.chars().count(), 1, "{mark:?} at {tier:?} is not one glyph");
            assert_eq!(
                UnicodeWidthStr::width(glyph),
                1,
                "{mark:?} at {tier:?} is not one cell wide: {glyph:?}"
            );
        }
    }
}

/// A mark drops into `format!` where a literal used to sit, and has to render at the live tier
/// rather than the one that was current when the string was built.
/// A configured glyph is drawn where Nerd Font icons are, and the fallback's own rendering
/// stands in at the tiers where a private-use mark would be tofu, and when none is configured.
#[test]
fn a_configured_glyph_is_drawn_only_where_nerd_icons_are() {
    let jira = Some("\u{f0303}");
    set_tier(GlyphTier::Nerd);
    assert_eq!(configured_glyph(jira, Icon::MentionSource), "\u{f0303}");
    assert_eq!(configured_glyph(None, Icon::MentionSource), "\u{f04fc}");
    set_tier(GlyphTier::NerdWide);
    assert_eq!(configured_glyph(jira, Icon::MentionSource), "\u{f0303}");
    set_tier(GlyphTier::Unicode);
    assert_eq!(configured_glyph(jira, Icon::MentionSource), "▪");
    set_tier(GlyphTier::Ascii);
    assert_eq!(configured_glyph(jira, Icon::MentionSource), "-");
    set_tier(GlyphTier::Nerd);
}

#[test]
fn a_mark_formats_at_the_live_tier() {
    set_tier(GlyphTier::Unicode);
    assert_eq!(format!("{} open", Mark::FoldOpen), "▼ open");
    set_tier(GlyphTier::Ascii);
    assert_eq!(format!("{} open", Mark::FoldOpen), "v open");
    set_tier(GlyphTier::Nerd);
    assert_eq!(format!("{} open", Mark::FoldOpen), "▾ open");
    assert!(Mark::FoldOpen.leads("▼ open"), "leads accepts every tier's rendering");
    assert!(Mark::FoldOpen.leads("v open"));
}

/// Layout budgets one cell per icon everywhere. A two-cell substitute shifts every column after it,
/// which is how a "portable" fallback shears the row it was supposed to rescue. An icon may also
/// render as *nothing* below the nerd tier — that is the deliberate omission, and it takes its
/// separator with it through [`Icon::prefix`].
#[test]
fn every_rendering_is_one_cell_or_omitted() {
    for tier in TIERS {
        for icon in EVERY_ICON {
            let glyph = icon.at(tier);
            if glyph.is_empty() {
                assert!(
                    !matches!(tier, GlyphTier::Nerd | GlyphTier::NerdWide),
                    "{icon:?} has no shipped glyph"
                );
                continue;
            }
            assert_eq!(
                glyph.chars().count(),
                1,
                "{icon:?} at {tier:?} is not a single glyph: {glyph:?}"
            );
            assert_eq!(
                UnicodeWidthStr::width(glyph),
                1,
                "{icon:?} at {tier:?} is not one cell wide: {glyph:?}"
            );
        }
    }
}

/// An omitted icon must not leave a hole where it used to be.
#[test]
fn an_omitted_icon_takes_its_separator_with_it() {
    set_tier(GlyphTier::Unicode);
    assert_eq!(Icon::ToolBash.at(GlyphTier::Unicode), "", "tool cards drop the icon");
    assert_eq!(Icon::ToolBash.prefix(), "", "and the space that followed it");
    assert_eq!(Icon::Palette.prefix(), format!("{} ", Icon::Palette.at(GlyphTier::Unicode)));
    assert_eq!(icon_cells(Icon::ToolBash.glyph()), 0, "and the cells it was measured at");
    set_tier(GlyphTier::Nerd);
}

/// The wide tier is the shipped vocabulary with one more cell of clearance — same shapes, same
/// words, everything after an icon moved over by exactly one column. Anything else and it is a
/// second look rather than the same one on a different cut of the font.
#[test]
fn the_wide_tier_draws_the_nerd_shapes_with_a_cell_more_air() {
    for icon in EVERY_ICON {
        assert_eq!(
            icon.at(GlyphTier::NerdWide),
            icon.at(GlyphTier::Nerd),
            "{icon:?} is drawn differently"
        );
    }
    assert_eq!(GlyphTier::NerdWide.gap().len(), GlyphTier::Nerd.gap().len() + 1);

    set_tier(GlyphTier::NerdWide);
    assert_eq!(icon_label(Icon::Model.glyph(), "opus"), format!("{}  opus", Icon::Model.glyph()));
    assert_eq!(icon_cells(Icon::Model.glyph()), 3);
    // The omission rule outranks the gap: a row with no icon spends nothing, however much room
    // the tier would have given one.
    assert_eq!(icon_prefix(""), "");
    set_tier(GlyphTier::Nerd);
    assert_eq!(icon_label(Icon::Model.glyph(), "opus"), format!("{} opus", Icon::Model.glyph()));
    assert_eq!(icon_cells(Icon::Model.glyph()), 2);
}

/// The availability probe has to ask about the icons the product actually draws.
///
/// It asked about one — the cog — and fifty-six of the ninety-odd live in the Material Design
/// block Nerd Fonts moved in v3, so a v2 font answered "yes" and then drew boxes for most of
/// the dashboard. Asserted over the table rather than over the query string, because the point
/// is coverage of the vocabulary and not the spelling of an `fc-list` argument.
#[test]
fn the_availability_probe_covers_every_nerd_range_the_icons_use() {
    let probed: std::collections::HashSet<u32> = EVERY_ICON
        .iter()
        .filter_map(|icon| icon.at(GlyphTier::Nerd).chars().next())
        .map(u32::from)
        .collect();
    assert!(probed.len() > 80, "the vocabulary should be most of the table: {}", probed.len());

    // One codepoint from each block the table draws from, named so a future icon in a new
    // range is a visible addition here rather than a silent gap in the probe.
    let blocks: [(&str, std::ops::RangeInclusive<u32>); 4] = [
        ("Material Design (moved in Nerd Fonts v3)", 0xF0001..=0xF1AF0),
        ("Devicons", 0xE700..=0xE8EF),
        ("Font Awesome", 0xF000..=0xF2FF),
        ("Octicons", 0xF400..=0xF533),
    ];
    for (name, range) in blocks {
        assert!(
            probed.iter().any(|cp| range.contains(cp)),
            "nothing from {name} reaches the probe, so a font missing that block passes it"
        );
    }
}

/// `auto` may not choose the wide tier: it exists for a question the machine cannot answer, so
/// choosing it would be a guess presented as a measurement.
#[test]
fn only_an_explicit_preference_reaches_the_wide_tier() {
    assert_eq!(GlyphTier::resolve(GlyphStyle::NerdWide), GlyphTier::NerdWide);
    for preference in [GlyphStyle::Auto, GlyphStyle::Unknown] {
        assert_ne!(GlyphTier::resolve(preference), GlyphTier::NerdWide, "{preference:?}");
    }
}

/// Icons that have always drawn the same nerd glyph, and may keep doing so.
///
/// Most share a *metaphor* and never appear in the same surface: a document for "new session"
/// and the `@`-mention file menu, a console line for the shell and for the commands you type at
/// one, find-by-name for the session finder and `glob`, a tick for a review that was accepted
/// and an answer that was given.
///
/// `±` earns its place the other way round. The bar's changes module and an `edit` card are on
/// screen together and *should* look alike — both are a diff, at two scales, and a reader who
/// learns the mark on one has learned it on the other.
///
/// Declared rather than merged because the meanings stay distinct, and the fallbacks already
/// separate them: below the nerd tier a tool card drops its icon altogether while the bar keeps
/// its `±`.
const SHARED_NERD_SHAPES: &[[Icon; 2]] = &[
    [Icon::NewSession, Icon::FileMenu],
    [Icon::ToolBash, Icon::SlashMenu],
    [Icon::Find, Icon::ToolGlob],
    [Icon::Finish, Icon::Answer],
    [Icon::Changes, Icon::ToolEdit],
];

/// At the nerd tier a surface is identified by its silhouette, so two of them sharing one is a bug
/// unless it is declared above. The fallbacks deliberately group by family instead — there is no
/// honest way to give seventy shapes to a font guaranteed only a couple of dozen — so they are
/// exempt, and the label beside the icon tells them apart.
#[test]
fn the_nerd_tier_gives_every_icon_its_own_shape() {
    let declared = |a: Icon, b: Icon| {
        SHARED_NERD_SHAPES.iter().any(|pair| pair.contains(&a) && pair.contains(&b))
    };
    let mut seen: HashMap<&str, Icon> = HashMap::new();
    for icon in EVERY_ICON {
        if let Some(other) = seen.insert(icon.at(GlyphTier::Nerd), *icon) {
            assert!(declared(*icon, other), "{icon:?} and {other:?} share a nerd glyph");
        }
    }
}

/// Every tool the transcript can card. Inherited from the vocabulary this module absorbed, and
/// kept because it is the one place shape-identity is load-bearing: a reader picks a `bash` out of
/// a run of `read`s by silhouette, before reading a word.
const TOOL_ICONS: &[Icon] = &[
    Icon::ToolRead,
    Icon::ToolWrite,
    Icon::ToolEdit,
    Icon::ToolBash,
    Icon::ToolGrep,
    Icon::ToolGlob,
    Icon::ToolAstGrep,
    Icon::ToolGit,
    Icon::ToolTest,
    Icon::ToolTodo,
    Icon::ToolSkill,
    Icon::ToolWebFetch,
    Icon::ToolWebSearch,
    Icon::ToolScreenshot,
    Icon::ToolLoopControl,
    Icon::ToolDiagnostics,
    Icon::ToolNavigation,
    Icon::ToolSymbolSearch,
    Icon::ToolModuleReport,
    Icon::ToolReadSymbol,
    Icon::ToolImpact,
    Icon::ToolDebug,
    Icon::ToolMcp,
    Icon::ToolActivate,
];

/// Two tools sharing an icon defeats the point of having them: telling calls apart at a glance.
#[test]
fn no_two_tools_share_a_nerd_icon() {
    let mut seen: HashMap<&str, Icon> = HashMap::new();
    for icon in TOOL_ICONS {
        if let Some(other) = seen.insert(icon.at(GlyphTier::Nerd), *icon) {
            panic!("{icon:?} and {other:?} share an icon");
        }
    }
    assert_ne!(
        Icon::ToolGeneric.at(GlyphTier::Nerd),
        Icon::ToolRead.at(GlyphTier::Nerd),
        "the generic wrench has to be distinguishable from a real tool's icon"
    );
}

/// Tool cards carry an icon only where it is a real shape. Approximating twenty-six distinct tools
/// with a handful of geometric shapes would say less than the bold name already does, so below the
/// nerd tier there is nothing to tell apart — deliberately, and asserted so it stays deliberate.
#[test]
fn tool_cards_drop_their_icons_rather_than_approximate_them() {
    for icon in TOOL_ICONS.iter().chain(std::iter::once(&Icon::ToolGeneric)) {
        assert!(!icon.at(GlyphTier::Nerd).is_empty(), "{icon:?} has no shipped glyph");
        for tier in [GlyphTier::Unicode, GlyphTier::Ascii] {
            assert_eq!(icon.at(tier), "", "{icon:?} should be omitted at {tier:?}");
        }
    }
}

/// The spinner is the most-redrawn character in the product and the one the braille frames got
/// wrong: block elements are synthesized by the terminal, so the animation cannot depend on a
/// font having anything. The ascii tier has to leave the range altogether.
#[test]
fn the_spinner_is_drawn_from_the_range_terminals_synthesize() {
    for at in [GlyphTier::Nerd, GlyphTier::NerdWide, GlyphTier::Unicode] {
        set_tier(at);
        for frame in 0..SPINNER_BLOCKS.len() {
            let glyph = spinner(frame);
            let cp = u32::from(glyph.chars().next().expect("a frame is one glyph"));
            assert!(
                (0x2580..=0x259F).contains(&cp),
                "{at:?} frame {frame} draws U+{cp:04X}, outside Block Elements"
            );
            assert_eq!(UnicodeWidthStr::width(glyph), 1, "frame {frame} is not one cell");
        }
    }
    set_tier(GlyphTier::Ascii);
    for frame in 0..SPINNER_ASCII.len() {
        assert!(spinner(frame).is_ascii(), "the ascii tier has to survive a non-UTF-8 terminal");
    }
    set_tier(GlyphTier::Nerd);
}

/// A spinner that stops on the same glyph twice reads as a freeze, and the frame index is a
/// free-running counter, so it has to wrap rather than saturate.
#[test]
fn the_spinner_cycles_without_repeating_a_frame() {
    let ticks = spinner_ticks_per_frame();
    let cycle: Vec<&str> = (0..SPINNER_BLOCKS.len()).map(|step| spinner(step * ticks)).collect();
    assert_eq!(
        cycle.iter().collect::<std::collections::HashSet<_>>().len(),
        cycle.len(),
        "every frame of the sweep is a different glyph: {cycle:?}"
    );
    let revolution = SPINNER_BLOCKS.len() * ticks;
    assert_eq!(spinner(revolution), spinner(0), "the sweep wraps");
    assert_eq!(spinner(usize::MAX), SPINNER_BLOCKS[(usize::MAX / ticks) % SPINNER_BLOCKS.len()]);
}

/// The braille sweep is the preferred drawing where a font on the machine can draw it, and the
/// blocks are the floor. Both are one cell; neither may leak into the `ascii` tier, which means
/// "assume nothing beyond ASCII" and cannot be rescued by a fallback font.
#[test]
fn the_sweep_upgrades_to_braille_and_never_past_the_ascii_tier() {
    let frame = |step: usize| spinner(step * spinner_ticks_per_frame());

    set_braille(false);
    assert_eq!(frame(0), SPINNER_BLOCKS[0], "the floor asks nothing of any font");

    set_braille(true);
    assert_eq!(frame(0), SPINNER_BRAILLE[0]);
    for ticks in 0..SPINNER_BRAILLE.len() {
        let glyph = frame(ticks);
        let cp = u32::from(glyph.chars().next().expect("a frame is one glyph"));
        assert!((0x2800..=0x28FF).contains(&cp), "{glyph:?} is U+{cp:04X}, not braille");
        assert_eq!(UnicodeWidthStr::width(glyph), 1, "{glyph:?} is not one cell");
    }
    assert_eq!(frame(SPINNER_BRAILLE.len()), SPINNER_BRAILLE[0], "the sweep wraps");

    // The ascii tier outranks the probe: a terminal that is not speaking UTF-8 gets ASCII
    // however many braille fonts are installed.
    set_tier(GlyphTier::Ascii);
    for ticks in 0..SPINNER_ASCII.len() {
        assert!(frame(ticks).is_ascii(), "{:?} is not ascii", frame(ticks));
    }
    set_tier(GlyphTier::Nerd);
    set_braille(false);
}

/// The default has to be the drawing that needs nothing, or a test, a fixture and a generated
/// site asset stop agreeing across machines — and the one that disagrees is whichever box last
/// installed a symbols font.
#[test]
fn the_sweep_defaults_to_the_floor() {
    assert!(!braille(), "braille is opt-in, and only the startup probe opts in");
}

/// Rotation is read from a shape that moves, not one that changes. Every frame has to ink the
/// same fraction of the cell, or the sweep reads as a flicker between unrelated glyphs — which
/// is what the eight-frame version alternating quarters with halves actually did.
#[test]
fn every_frame_of_the_sweep_has_the_same_mass() {
    for frame in SPINNER_BLOCKS {
        let cp = u32::from(frame.chars().next().expect("a frame is one glyph"));
        assert!(
            (0x2596..=0x259F).contains(&cp),
            "{frame:?} is U+{cp:04X} — not a quadrant, so it does not weigh what its \
                 neighbors weigh"
        );
    }
}

/// The **block** sweep steps on its own clock rather than the render loop's, or a 30fps tick
/// spins four quarters about four times a second and the reader sees flicker instead of
/// motion. The braille is the case this does not apply to: ten frames of one or two dots read
/// as a shimmer at the same rate, which is why the two numbers are two numbers.
#[test]
fn the_block_sweep_is_slower_than_the_render_tick() {
    const { assert!(SPINNER_BLOCK_TICKS > 1, "the blocks would step at the render rate") };
    set_braille(false);
    let ticks = spinner_ticks_per_frame();
    for tick in 0..ticks {
        assert_eq!(spinner(tick), spinner(0), "tick {tick} should hold the first frame");
    }
    assert_ne!(spinner(ticks), spinner(0), "and then move on");
}

/// The two sweeps keep their own cadences, and the braille's is the faster of them.
///
/// One number served both for a release, which is how the ten-frame braille sweep came to be
/// held at the four-frame blocks' 100ms: slow enough to read as a stutter rather than a turn.
/// The set is drawn for 80ms a frame, and 30fps quantizes that to 67 or 100 — so this is the
/// nearer one, and the assertion is the relationship rather than the literals.
#[test]
fn each_sweep_keeps_the_cadence_it_was_drawn_for() {
    set_braille(false);
    let blocks = spinner_ticks_per_frame();
    set_braille(true);
    let braille = spinner_ticks_per_frame();
    set_braille(false);

    assert!(
        braille < blocks,
        "ten frames of one or two dots survive a rate four quarter blocks cannot: \
             {braille} vs {blocks} ticks"
    );
    // A revolution a reader waits on is not a spinner. Both stay well inside a second at 30fps.
    let revolution = |ticks: usize, frames: usize| ticks * frames * 1000 / 30;
    assert!(revolution(braille, SPINNER_BRAILLE.len()) <= 400, "the braille sweep drags");
    assert!(revolution(blocks, SPINNER_BLOCKS.len()) <= 500, "the block sweep drags");
}

/// The process tier is what the ~150 render sites read, so moving it has to move them.
#[test]
fn the_process_tier_selects_the_vocabulary() {
    assert_eq!(tier(), GlyphTier::Nerd, "the default is the look Luxe ships with");
    for at in TIERS {
        set_tier(at);
        assert_eq!(tier(), at, "{at:?} does not survive the round trip");
        assert_eq!(Icon::ToolBash.glyph(), Icon::ToolBash.at(at));
        assert_eq!(icon_gap(), at.gap());
    }
    set_tier(GlyphTier::Nerd);
    assert_eq!(Icon::ToolBash.glyph(), Icon::ToolBash.at(GlyphTier::Nerd));
}
