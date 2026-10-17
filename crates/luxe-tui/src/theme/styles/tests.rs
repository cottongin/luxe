/// A theme's mode comes from the canvas it paints, not from what it is called.
///
/// This used to be `name.contains("light")`, which is right only for themes named after their
/// own mode. Three things read it — the wordmark's highlight peak, `diff_emphasis`, and the
/// gauge/stream shade ramps — so a theme that guessed wrong was wrong three times over.
#[test]
fn a_themes_mode_is_read_from_its_canvas_not_its_name() {
    use luxe_core::theme::ColorValue;

    let with_background = |value: ColorValue| HashMap::from([("background".to_string(), value)]);

    // A dark canvas under a name that says nothing about it.
    let porcelain = with_background(ColorValue::Rgb(0x12, 0x10, 0x0e));
    assert!(ThemeStyles::from_resolved(&porcelain, "porcelain").dark_mode);

    // ...and a light canvas under a name that would previously have read as dark.
    let luxe = with_background(ColorValue::Rgb(0xf6, 0xf4, 0xf1));
    assert!(!ThemeStyles::from_resolved(&luxe, "luxe").dark_mode);

    // An indexed canvas is still measurable: 16 is the cube's black corner, 231 its white one.
    assert!(ThemeStyles::from_resolved(&with_background(ColorValue::Ansi(16)), "x").dark_mode);
    assert!(!ThemeStyles::from_resolved(&with_background(ColorValue::Ansi(231)), "x").dark_mode);
    // 255 is the top of the grayscale ramp, which the cube arm would get wrong.
    assert!(!ThemeStyles::from_resolved(&with_background(ColorValue::Ansi(255)), "x").dark_mode);

    // ANSI 0-15 and the terminal default belong to the terminal, so the name gets the say.
    let named = |value: ColorValue, name: &str| {
        ThemeStyles::from_resolved(&with_background(value), name).dark_mode
    };
    assert!(!named(ColorValue::Ansi(7), "solar-light"));
    assert!(named(ColorValue::Ansi(7), "solar"));
    assert!(!named(ColorValue::Default, "paper-light"));
    assert!(named(ColorValue::Default, "paper"));
}

/// `userMsgBg` stands in when a theme names no `background`, mirroring `bg_or` — otherwise the
/// mode could disagree with the color actually painted behind the app.
#[test]
fn the_mode_falls_back_to_the_same_token_the_canvas_does() {
    use luxe_core::theme::ColorValue;

    let resolved = HashMap::from([("userMsgBg".to_string(), ColorValue::Rgb(0xf6, 0xf4, 0xf1))]);
    assert!(!ThemeStyles::from_resolved(&resolved, "unnamed").dark_mode);
}

/// Every shipped theme agrees with itself: the mode it reports matches the canvas it paints.
#[test]
fn every_builtin_theme_reports_the_mode_it_paints() {
    let registry = luxe_core::theme::ThemeRegistry::new();
    for name in registry.names() {
        let Some(resolved) = registry.resolve(&name) else { continue };
        let styles = ThemeStyles::from_resolved(&resolved, &name);
        let Some(RatatuiColor::Rgb(red, green, blue)) = styles.background.bg else { continue };
        assert_eq!(
            styles.dark_mode,
            is_dark_rgb(red, green, blue),
            "{name} reports dark_mode={} for canvas #{red:02x}{green:02x}{blue:02x}",
            styles.dark_mode
        );
    }
}

/// Nearly every bundled theme maps `selectedBg` to the same color as `dim`, so secondary
/// text on a highlighted row must not use `dim` or it disappears entirely.
#[test]
fn secondary_text_stays_visible_on_a_selected_row() {
    let registry = luxe_core::theme::ThemeRegistry::new();
    for name in registry.names() {
        let Some(resolved) = registry.resolve(&name) else { continue };
        let styles = ThemeStyles::from_resolved(&resolved, &name);
        let selected = styles.secondary_on(true);
        assert_ne!(
            selected.fg, styles.selected_bg.bg,
            "{name}: selected-row secondary text is the same color as the selection background"
        );
        assert_eq!(styles.secondary_on(false), styles.dim, "{name}: unselected rows keep dim");
    }
}

/// The focused row must be legible from weight alone, so a terminal that drops the
/// selection background (or a theme that barely tints it) still shows where the cursor is.
#[test]
fn the_focused_row_is_bold_as_well_as_highlighted() {
    let registry = luxe_core::theme::ThemeRegistry::new();
    for name in registry.names() {
        let Some(resolved) = registry.resolve(&name) else { continue };
        let styles = ThemeStyles::from_resolved(&resolved, &name);

        let focused = styles.label_on(true);
        assert!(focused.add_modifier.contains(Modifier::BOLD), "{name}: focused row is bold");
        assert_eq!(focused.bg, styles.selected_bg.bg, "{name}: focused row is highlighted");
        assert_eq!(styles.label_on(false), styles.text, "{name}: other rows are plain text");
        assert_eq!(styles.accent_on(false), styles.accent, "{name}: cursor stays accent");
        assert_eq!(
            styles.accent_on(true).bg,
            styles.selected_bg.bg,
            "{name}: the cursor shares the row highlight"
        );
    }
}

use super::*;
use luxe_core::theme::ThemeRegistry;

/// from_resolved: built-in dark theme produces non-default Styles.
#[test]
fn from_resolved_dark_theme_produces_styles() {
    let registry = ThemeRegistry::new();
    let resolved = registry.resolve("catppuccin").unwrap();
    let styles = ThemeStyles::from_resolved(&resolved, "catppuccin");

    assert_ne!(styles.accent, Style::default());
    assert_ne!(styles.info, Style::default());
    assert_ne!(styles.text, Style::default());
}

/// A dark built-in, a cool one, and a light palette, which no built-in is any more but a
/// reader's own theme file can be.
fn sample_styles() -> Vec<(&'static str, ThemeStyles)> {
    let registry = ThemeRegistry::new();
    let built_in = |name: &'static str| {
        (name, ThemeStyles::from_resolved(&registry.resolve(name).unwrap(), name))
    };
    vec![
        built_in("catppuccin"),
        built_in("nord"),
        ("light-fixture", crate::fixture::light_theme_styles()),
    ]
}

/// from_resolved: a light theme produces non-default Styles.
#[test]
fn from_resolved_light_theme_produces_styles() {
    let styles = crate::fixture::light_theme_styles();

    assert_ne!(styles.accent, Style::default());
    assert_ne!(styles.info, Style::default());
    assert_ne!(styles.text, Style::default());
}

#[test]
fn builtins_define_distinct_thinking_level_styles() {
    for (name, styles) in sample_styles() {
        assert_ne!(styles.thinking_off, Style::default(), "{name}");
        assert_eq!(styles.thinking_minimal, styles.text, "{name}");
        assert_eq!(styles.thinking_low, styles.warning, "{name}");
        assert_eq!(styles.thinking_medium, styles.success, "{name}");
        assert_eq!(styles.thinking_high, styles.accent, "{name}");
        assert_eq!(styles.thinking_xhigh, styles.accent_secondary, "{name}");
        assert_ne!(styles.thinking_ultra, styles.thinking_xhigh, "{name}");
    }
}

#[test]
fn builtins_give_diff_lines_distinct_backgrounds() {
    for (name, styles) in sample_styles() {
        assert!(styles.diff_add_bg.bg.is_some(), "{name} add bg");
        assert!(styles.diff_del_bg.bg.is_some(), "{name} del bg");
        assert_ne!(styles.diff_add_bg.bg, styles.diff_del_bg.bg, "{name} add != del");
        // The diff tints are their own thing, not the generic selection background.
        assert_ne!(styles.diff_add_bg.bg, styles.selected_bg.bg, "{name} add != selection");
    }
}

/// The changed span of a replaced line has to out-read the rest of the row it sits on: a deeper
/// wash of the row's own hue, plus a mark that survives a terminal with no backgrounds.
#[test]
fn the_changed_span_of_a_replaced_line_is_washed_deeper_and_marked() {
    let registry = ThemeRegistry::new();
    for name in registry.names() {
        let Some(resolved) = registry.resolve(&name) else { continue };
        let styles = ThemeStyles::from_resolved(&resolved, &name);

        // Distance from a hue: how much of it a shade carries.
        let toward = |shade: Option<RatatuiColor>, hue: (u8, u8, u8)| match rgb_color(shade) {
            Some((r, g, b)) => {
                let delta = |a: u8, b: u8| (f32::from(a) - f32::from(b)).powi(2);
                delta(r, hue.0) + delta(g, hue.1) + delta(b, hue.2)
            }
            None => panic!("{name}: an RGB theme must derive an RGB emphasis wash"),
        };
        for (row, emphasis, hue) in [
            (styles.diff_add_bg, styles.diff_add_emphasis, "success"),
            (styles.diff_del_bg, styles.diff_del_emphasis, "error"),
        ] {
            let hue = rgb_token(&resolved, hue).expect("a bundled theme names its hues");
            assert_ne!(emphasis.bg, row.bg, "{name}: the emphasis wash must differ from the row");
            assert!(
                toward(emphasis.bg, hue) < toward(row.bg, hue),
                "{name}: the emphasis wash must lean further into the line's own hue"
            );
        }
        assert_ne!(
            styles.diff_add_emphasis.bg, styles.diff_del_emphasis.bg,
            "{name}: added and removed emphasis read differently"
        );

        // Bold either way, and never struck: the removed text is what the reader is comparing
        let removed = styles.diff_emphasis(styles.error, false);
        let added = styles.diff_emphasis(styles.success, true);
        assert!(removed.add_modifier.contains(Modifier::BOLD), "{name}: removed bold");
        assert!(!removed.add_modifier.contains(Modifier::CROSSED_OUT), "{name}: never struck");
        assert!(added.add_modifier.contains(Modifier::BOLD), "{name}: added bold");
        assert_eq!(removed.bg, styles.diff_del_emphasis.bg, "{name}: in its own wash");
        assert_eq!(removed.fg, styles.error.fg, "{name}: the line keeps its own foreground");
        assert_eq!(added.bg, styles.diff_add_emphasis.bg, "{name}: and wears the deeper wash");
    }
}

/// A theme that names an ANSI color cannot be blended, so the changed span keeps the row tint
/// and leans on bold alone rather than rendering an invisible "deeper" wash.
#[test]
fn an_unblendable_theme_marks_the_changed_span_without_a_deeper_wash() {
    let resolved = HashMap::from([
        ("success".to_string(), ColorValue::Ansi(2)),
        ("error".to_string(), ColorValue::Ansi(1)),
        ("diffAddBg".to_string(), ColorValue::Ansi(22)),
        ("diffDelBg".to_string(), ColorValue::Ansi(52)),
    ]);
    let styles = ThemeStyles::from_resolved(&resolved, "ansi");

    assert_eq!(styles.diff_add_emphasis.bg, styles.diff_add_bg.bg);
    assert_eq!(styles.diff_del_emphasis.bg, styles.diff_del_bg.bg);
    assert!(styles.diff_emphasis(styles.error, false).add_modifier.contains(Modifier::BOLD));
}

/// An explicit token wins over the derived blend, so a theme can dictate the wash outright.
#[test]
fn an_explicit_emphasis_token_overrides_the_blend() {
    let mut resolved = ThemeRegistry::new().resolve("catppuccin").expect("catppuccin");
    resolved.insert("diffAddEmphasisBg".to_string(), ColorValue::Rgb(1, 2, 3));
    let styles = ThemeStyles::from_resolved(&resolved, "dark");

    assert_eq!(styles.diff_add_emphasis.bg, Some(RatatuiColor::Rgb(1, 2, 3)));
}

#[test]
fn builtins_tint_tool_calls_by_outcome() {
    for (name, styles) in sample_styles() {
        assert!(styles.tool_call_success_bg.bg.is_some(), "{name} success bg");
        assert!(styles.tool_call_error_bg.bg.is_some(), "{name} error bg");
        // Success and failure read differently, and neither equals the neutral tool band.
        assert_ne!(
            styles.tool_call_success_bg.bg, styles.tool_call_error_bg.bg,
            "{name} success != error"
        );
        assert_ne!(
            styles.tool_call_success_bg.bg, styles.tool_call_bg.bg,
            "{name} success != neutral"
        );
        assert_ne!(styles.tool_call_error_bg.bg, styles.tool_call_bg.bg, "{name} error != neutral");
    }
}

/// WCAG relative luminance, for the one assertion that has to be about what an eye can resolve.
fn relative_luminance(color: RatatuiColor) -> f32 {
    let (red, green, blue) = rgb_color(Some(color)).expect("an RGB theme");
    let channel = |value: u8| {
        let value = f32::from(value) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(red) + 0.7152 * channel(green) + 0.0722 * channel(blue)
}

/// Contrast ratio between two colors, lighter over darker.
fn contrast(a: RatatuiColor, b: RatatuiColor) -> f32 {
    let (first, second) = (relative_luminance(a), relative_luminance(b));
    let (lighter, darker) = if first > second { (first, second) } else { (second, first) };
    (lighter + 0.05) / (darker + 0.05)
}

/// A depth ramp is only a hierarchy if its steps are *visibly* different, and for a long time
/// this one was not: five stops whose four receded steps measured 1.14–1.33:1 apart in every
/// shipped theme. The code computed five levels, assigned them by role and tested that they
/// were distinct `Color` values — which they were, and a reader could see two.
///
/// So the assertion is about contrast rather than inequality. 1.5:1 is well under the 3:1 WCAG
/// wants between non-text elements, and deliberately so: these are neighboring shades of one
/// hue with a hard edge between them, not a control against its background. It is the floor at
/// which the step stops being a rounding error.
#[test]
fn every_theme_tells_plan_mode_from_build_by_color() {
    let registry = ThemeRegistry::new();
    for name in registry.names() {
        let Some(resolved) = registry.resolve(&name) else { continue };
        let styles = ThemeStyles::from_resolved(&resolved, &name);
        let (Some(plan), Some(build)) =
            (rgb_color(styles.mode_plan.fg), rgb_color(styles.border_accent.fg))
        else {
            continue;
        };
        assert!(
            super::color::distinct_colors(plan, build),
            "{name}: plan {plan:?} and build {build:?} read as one color"
        );
    }
}

/// A theme that names a plan color gets exactly that one.
#[test]
fn a_themes_own_plan_color_wins() {
    use luxe_core::theme::ColorValue;
    let resolved = HashMap::from([
        ("borderAccent".to_string(), ColorValue::Rgb(200, 160, 60)),
        ("accentSecondary".to_string(), ColorValue::Rgb(190, 150, 210)),
        ("modePlan".to_string(), ColorValue::Rgb(1, 2, 3)),
    ]);
    let styles = ThemeStyles::from_resolved(&resolved, "custom");
    assert_eq!(styles.mode_plan.fg, Some(RatatuiColor::Rgb(1, 2, 3)));
}

/// The break this catches: a fixed `info` plan hue, which in a theme whose accent is blue too
/// draws a plan-mode border nobody can tell from the build one.
/// The two measures each cover what the other misses.
#[test]
fn colors_are_told_apart_by_hue_or_by_distance() {
    use super::color::distinct_colors;
    assert!(distinct_colors((135, 143, 95), (187, 123, 73)), "olive and orange, close in RGB");
    assert!(!distinct_colors((88, 166, 255), (90, 170, 250)), "two blues");
    assert!(distinct_colors((40, 40, 40), (200, 200, 200)), "dark and light grays");
    assert!(!distinct_colors((120, 120, 120), (130, 128, 125)), "two grays");
}

#[test]
fn a_plan_color_too_close_to_the_accent_is_passed_over() {
    use luxe_core::theme::ColorValue;
    let resolved = HashMap::from([
        ("borderAccent".to_string(), ColorValue::Rgb(88, 166, 255)),
        ("accentSecondary".to_string(), ColorValue::Rgb(90, 170, 250)),
        ("thinkingUltra".to_string(), ColorValue::Rgb(80, 160, 240)),
        ("info".to_string(), ColorValue::Rgb(210, 120, 220)),
    ]);
    let styles = ThemeStyles::from_resolved(&resolved, "custom");
    assert_eq!(styles.mode_plan.fg, Some(RatatuiColor::Rgb(210, 120, 220)));
}

#[test]
fn every_theme_separates_its_depth_stops_enough_to_see() {
    const FLOOR: f32 = 1.5;
    let registry = ThemeRegistry::new();
    for name in registry.names() {
        let Some(resolved) = registry.resolve(&name) else { continue };
        let styles = ThemeStyles::from_resolved(&resolved, &name);
        let ramp = styles.slab.ramp();
        // An ANSI or indexed theme names colors the terminal owns and collapses to two levels
        // by design, so there is nothing to measure.
        if ramp.iter().any(|color| rgb_color(Some(*color)).is_none()) {
            continue;
        }
        for pair in ramp.windows(2) {
            let step = contrast(pair[0], pair[1]);
            assert!(
                step >= FLOOR,
                "{name}: {:?} and {:?} are {step:.2}:1 apart, which reads as one surface",
                pair[0],
                pair[1]
            );
        }
        // And the far end still has to be a slab rather than the canvas it is receding into,
        // or the deepest module loses its shape instead of its emphasis.
        let canvas = styles.slab.canvas;
        if rgb_color(Some(canvas)).is_some() {
            let deepest = contrast(*ramp.last().expect("a ramp"), canvas);
            assert!(deepest >= 1.2, "{name}: the deepest slab is {deepest:.2}:1 off the canvas");
        }
    }
}

/// The comet's polish is the ramp: every cell a step dimmer than the one ahead of it, the
/// leading cell brighter than the bare accent, and the last still clear of the rail.
#[test]
fn builtins_derive_a_monotonic_activity_ramp() {
    let registry = ThemeRegistry::new();
    for name in registry.names() {
        let Some(resolved) = registry.resolve(&name) else { continue };
        let styles = ThemeStyles::from_resolved(&resolved, &name);
        let rail = rgb_token(&resolved, "borderMuted").expect("a bundled theme names its rail");
        let accent = rgb_token(&resolved, "accent").expect("a bundled theme names its accent");
        // How far a shade is from the rail: the ramp's ordering, whichever way the hues run.
        let reach = |color: Option<RatatuiColor>| match color {
            Some(RatatuiColor::Rgb(r, g, b)) => {
                let delta = |a: u8, b: u8| (f32::from(a) - f32::from(b)).powi(2);
                delta(r, rail.0) + delta(g, rail.1) + delta(b, rail.2)
            }
            other => panic!("{name}: an RGB theme must derive RGB shades, got {other:?}"),
        };

        let shades: Vec<f32> = styles.activity_trail.iter().map(|style| reach(style.fg)).collect();
        for pair in shades.windows(2) {
            assert!(pair[0] >= pair[1], "{name}: the ramp never brightens tailward: {shades:?}");
        }
        let middle = shades[ACTIVITY_TRAIL_LEN / 2];
        assert!(
            shades[0] > middle && middle > 0.0,
            "{name}: head, middle and tail are separated: {shades:?}"
        );
        assert!(
            shades[0] > reach(Some(RatatuiColor::Rgb(accent.0, accent.1, accent.2))),
            "{name}: the leading cell is lifted past the bare accent"
        );
        // The point of the fade: the last cell *is* the rail, so it vanishes outright.
        assert_eq!(
            shades[ACTIVITY_TRAIL_LEN - 1],
            0.0,
            "{name}: the tail must land on the rail exactly"
        );
    }
}

/// A gauge's shading points at the frontier; it must not dissolve the bar's start.
///
/// Its length is the datum, so the dimmest shade has to stay obviously the value's color rather
/// than fading into the rail the way the comet's tail deliberately does.
#[test]
fn the_gauge_ramp_keeps_its_start_visible() {
    let registry = ThemeRegistry::new();
    for name in registry.names() {
        let Some(resolved) = registry.resolve(&name) else { continue };
        let styles = ThemeStyles::from_resolved(&resolved, &name);
        let shades = styles.gauge_shades(styles.text, styles.tool_bg);
        let rail = rgb_color(styles.border_muted.fg).expect("a bundled theme names its rail");
        let value = rgb_color(styles.text.fg).expect("and its text");
        let reach = |color: (u8, u8, u8)| {
            let delta = |a: u8, b: u8| (f32::from(a) - f32::from(b)).powi(2);
            delta(color.0, rail.0) + delta(color.1, rail.1) + delta(color.2, rail.2)
        };

        let start = rgb_color(shades.fill.last().and_then(|shade| shade.fg))
            .expect("an RGB theme derives RGB shades");
        let frontier = rgb_color(shades.fill[0].fg).expect("an RGB theme derives RGB shades");
        assert!(
            reach(start) > reach(value) * 0.2,
            "{name}: the bar's start must stay clear of the rail: {start:?}"
        );
        assert!(reach(frontier) > reach(start), "{name}: and the frontier must lead it");
        // The rail's own ramp is free to disappear — that is what it is for — but into the band
        // it sits on, which is not the same thing as away from the rail's color.
        let band = rgb_color(styles.tool_bg.bg).expect("a bundled theme names its raised band");
        let into_band = |color: (u8, u8, u8)| {
            let delta = |a: u8, b: u8| (f32::from(a) - f32::from(b)).powi(2);
            delta(color.0, band.0) + delta(color.1, band.1) + delta(color.2, band.2)
        };
        let far = rgb_color(shades.rail.last().and_then(|shade| shade.fg)).expect("rail shades");
        assert!(
            into_band(far) < into_band(rail),
            "{name}: the rail must fade into the band it sits on: {far:?} vs {rail:?}"
        );
    }
}

/// A theme that names an ANSI color cannot be blended, so the comet degrades to a head and a
/// dimmed tail instead of rendering a ramp of one flat color.
#[test]
fn an_unblendable_accent_still_gets_a_head_and_a_tail() {
    let resolved = HashMap::from([("accent".to_string(), ColorValue::Ansi(4))]);
    let styles = ThemeStyles::from_resolved(&resolved, "ansi");
    let trail = &styles.activity_trail;

    assert_eq!(trail[0], styles.accent, "the head is the accent itself");
    assert!(trail[1].add_modifier.contains(Modifier::DIM), "and the tail is dimmed");
    assert!(trail.iter().all(|shade| shade.fg == styles.accent.fg), "the hue is untouched");
}

#[test]
fn builtins_give_paste_indicators_a_distinct_background() {
    for (name, styles) in sample_styles() {
        assert_eq!(styles.paste_indicator.fg, styles.text.fg, "{name}");
        assert_eq!(styles.chip.fg, styles.accent.fg, "{name}");
        assert_eq!(styles.chip.bg, styles.paste_indicator.bg, "{name}");
        assert!(styles.paste_indicator.bg.is_some(), "{name}");
        assert_ne!(styles.paste_indicator.bg, styles.user_msg_bg.bg, "{name}");
    }
}

/// from_resolved: missing tokens fall back to default Style.
#[test]
fn from_resolved_missing_tokens_fallback() {
    let mut resolved = HashMap::new();
    resolved.insert("accent".to_string(), ColorValue::Rgb(255, 0, 0));

    let styles = ThemeStyles::from_resolved(&resolved, "dark");
    assert_ne!(styles.accent, Style::default());
    assert_eq!(styles.accent_secondary, styles.accent);
    assert_eq!(styles.info, Style::default());
    assert_eq!(styles.text, Style::default());
}

/// color_value_to_ratatui: Rgb maps correctly.
#[test]
fn color_value_rgb_maps() {
    let rgb = ColorValue::Rgb(100, 200, 50);
    let expected = RatatuiColor::Rgb(100, 200, 50);
    assert_eq!(color_value_to_ratatui(&rgb), Some(expected));
}

/// color_value_to_ratatui: Ansi 0-7 maps to named colors.
/// The cursor's band and a pick's are both washes of the canvas, never the same one, and
/// neither is the plain highlight a hovered row wears.
#[test]
fn the_cursor_band_and_the_pick_band_are_told_apart() {
    for (name, styles) in sample_styles() {
        let (cursor, pick) = (styles.cursor_line_bg().bg, styles.pick_bg().bg);
        assert!(cursor.is_some() && pick.is_some(), "{name}");
        assert_ne!(cursor, pick, "{name}");
        assert_ne!(pick, styles.selected_bg.bg, "{name}");
        assert_ne!(cursor, styles.background.bg, "{name}: the cursor band shows at all");
    }
}

#[test]
fn color_value_ansi_named() {
    let ansi = ColorValue::Ansi(0);
    assert_eq!(color_value_to_ratatui(&ansi), Some(RatatuiColor::Black));

    let ansi = ColorValue::Ansi(1);
    assert_eq!(color_value_to_ratatui(&ansi), Some(RatatuiColor::Red));

    let ansi = ColorValue::Ansi(7);
    assert_eq!(color_value_to_ratatui(&ansi), Some(RatatuiColor::White));
}

/// color_value_to_ratatui: Ansi 16-255 maps to Indexed.
#[test]
fn color_value_ansi_indexed() {
    let ansi = ColorValue::Ansi(16);
    assert_eq!(color_value_to_ratatui(&ansi), Some(RatatuiColor::Indexed(16)));

    let ansi = ColorValue::Ansi(255);
    assert_eq!(color_value_to_ratatui(&ansi), Some(RatatuiColor::Indexed(255)));
}

/// color_value_to_ratatui: Default returns None.
#[test]
fn color_value_default_returns_none() {
    let default = ColorValue::Default;
    assert_eq!(color_value_to_ratatui(&default), None);
}
