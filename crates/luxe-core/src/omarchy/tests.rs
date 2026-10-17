use super::*;

const LAST_HORIZON: &str = r##"mode = "dark"

accent = "#b59790"
selection = "#584e51"
muted = "#584e51"

background = "#0c0b0c"
lighter_background = "#0c0b0c"
foreground = "#FAFCFB"
dark_foreground = "#584e51"

hyprland_active_border = "rgba(8a8588ee) rgba(e2dddcee)"
red = "#c38b7b"
yellow = "#6B5E73"
green = "#87a9b0"
cyan = "#a5a0b6"
blue = "#b59790"
magenta = "#c4d8e2"
"##;

fn resolved(theme: &Theme) -> HashMap<String, crate::theme::ColorValue> {
    theme.resolve_aliases()
}

fn rgb(theme: &Theme, token: &str) -> Rgb {
    match resolved(theme).get(token) {
        Some(crate::theme::ColorValue::Rgb(r, g, b)) => (*r, *g, *b),
        other => panic!("{token} is {other:?}"),
    }
}

/// The applied theme is read where Omarchy 4 keeps it, and where earlier releases did.
#[test]
fn the_desktop_theme_is_found_in_either_release() {
    let root = tempfile::tempdir().expect("temp dir");
    let (state, config) = (root.path().join("state"), root.path().join("config"));
    assert_eq!(DesktopTheme::current_in(&state, &config), None, "not Omarchy");

    let old = config.join("omarchy/current");
    std::fs::create_dir_all(&old).expect("mkdir");
    std::fs::write(old.join("theme.name"), "Nord\n").expect("write");
    let found = DesktopTheme::current_in(&state, &config).expect("3.x");
    assert_eq!(found.name, "nord");
    assert_eq!(found.dir, old.join("theme"));

    let new = state.join("omarchy/current");
    std::fs::create_dir_all(&new).expect("mkdir");
    std::fs::write(new.join("theme.name"), "last-horizon").expect("write");
    assert_eq!(DesktopTheme::current_in(&state, &config).expect("4").name, "last-horizon");
}

/// Keys come out the way Omarchy reads them: quoted or not, commented or not, gradients skipped.
#[test]
fn a_palette_parses_like_omarchy_reads_it() {
    let palette = Palette::parse(
        "# a comment\nbackground = \"#101010\" # trailing\nforeground=#EEEEEE\n\
             'accent' = '#ff0000'\nborder = \"rgba(ffffffee) rgba(000000ee) 45deg\"\n",
        false,
    );
    assert_eq!(palette.get("background"), Some((0x10, 0x10, 0x10)));
    assert_eq!(palette.get("foreground"), Some((0xee, 0xee, 0xee)));
    assert_eq!(palette.get("accent"), Some((0xff, 0, 0)));
    assert_eq!(palette.get("border"), None);
    assert!(!palette.light);
}

/// A theme from before the semantic names still resolves: short names and ANSI slots fall back.
#[test]
fn legacy_names_fall_back_like_omarchy() {
    let palette =
        Palette::parse("color0 = \"#000000\"\ncolor7 = \"#cccccc\"\ncolor1 = \"#aa0000\"\n", false);
    assert_eq!(palette.get("background"), Some((0, 0, 0)));
    assert_eq!(palette.get("foreground"), Some((0xcc, 0xcc, 0xcc)));
    assert_eq!(palette.get("red"), Some((0xaa, 0, 0)));
    let short = Palette::parse("bg = \"#111111\"\nfg = \"#dddddd\"\npurple = \"#aa00aa\"\n", false);
    assert_eq!(short.get("background"), Some((0x11, 0x11, 0x11)));
    assert_eq!(short.get("magenta"), Some((0xaa, 0, 0xaa)));
    assert_eq!(short.get("bright_magenta"), Some(mix((0xaa, 0, 0xaa), (255, 255, 255), 0.2)));
}

/// Light is read from `mode`, then a `light.mode` file, then how bright the background is.
#[test]
fn a_palette_knows_whether_it_is_light() {
    assert!(Palette::parse("mode = \"light\"\nbackground = \"#000000\"", false).light);
    assert!(!Palette::parse("mode = \"dark\"\nbackground = \"#ffffff\"", false).light);
    assert!(Palette::parse("theme_type = \"light\"", false).light);
    assert!(Palette::parse("background = \"#000000\"", true).light, "light.mode");
    assert!(Palette::parse("background = \"#faf4ed\"", false).light, "bright");
    assert!(!Palette::parse("background = \"#191724\"", false).light);
}

/// A derived theme is complete, keeps the palette's own colors where they read, and lifts the
/// ones that do not until they clear the floor.
#[test]
fn a_derived_theme_is_complete_and_readable() {
    let theme = Palette::parse(LAST_HORIZON, false).theme("omarchy");
    let tokens = resolved(&theme);
    for token in Theme::ALL_TOKENS {
        assert!(tokens.contains_key(*token), "{token} is missing");
    }
    assert_eq!(rgb(&theme, "background"), (0x0c, 0x0b, 0x0c));
    assert_eq!(rgb(&theme, "accent"), (0xb5, 0x97, 0x90), "readable already, so kept");
    let bg = rgb(&theme, "background");
    for token in
        ["text", "muted", "accent", "accentSecondary", "info", "success", "error", "warning"]
    {
        let ratio = contrast(rgb(&theme, token), bg);
        assert!(ratio >= TEXT_CONTRAST, "{token} is {ratio:.2}:1");
    }
    // `dark_foreground` there is a surface gray, far too dark for words, so it is lifted
    assert_ne!(rgb(&theme, "muted"), (0x58, 0x4e, 0x51));
    // A `lighter_background` equal to the background says nothing, so the surface is mixed
    assert_ne!(rgb(&theme, "userMsgBg"), bg);
}

/// A theme whose ANSI names are slots still gets verdicts that read as verdicts: its own color
/// of the right hue when it has one anywhere, a standard one when it does not.
#[test]
fn signals_are_chosen_by_hue_not_by_name() {
    // Every slot green or cyan, the way a green-on-black theme is drawn
    let terminal = Palette::parse(
        "background = \"#0b0c16\"\nforeground = \"#ddf7ff\"\nred = \"#50f872\"\n\
             yellow = \"#50f7d4\"\ngreen = \"#4fe88f\"\n",
        false,
    )
    .theme("omarchy");
    let (error_hue, _) = hue_saturation(rgb(&terminal, "error"));
    assert!(Signal::Error.fits(error_hue), "an error is red, whatever `red` says");
    assert_eq!(rgb(&terminal, "success"), (0x4f, 0xe8, 0x8f), "its own green, kept");
    assert_ne!(rgb(&terminal, "diffAddBg"), rgb(&terminal, "diffDelBg"));

    // Red and amber in each other's slots: each is found where the theme put it
    let swapped = Palette::parse(
        "background = \"#121212\"\nforeground = \"#bebebe\"\nred = \"#d35f5f\"\n\
             yellow = \"#b91c1c\"\ngreen = \"#ffc107\"\n",
        false,
    )
    .theme("omarchy");
    assert_eq!(rgb(&swapped, "error"), (0xd3, 0x5f, 0x5f));
    assert_eq!(rgb(&swapped, "warning"), (0xff, 0xc1, 0x07), "the amber in the green slot");
    let (success_hue, _) = hue_saturation(rgb(&swapped, "success"));
    assert!(Signal::Success.fits(success_hue), "no green anywhere, so the standard one");

    // Gray everywhere: standard verdicts, still readable
    let gray = Palette::parse(
        "background = \"#000000\"\nforeground = \"#ffffff\"\nred = \"#a4a4a4\"\n\
             green = \"#b6b6b6\"\n",
        false,
    )
    .theme("omarchy");
    let bg = rgb(&gray, "background");
    for token in ["error", "success", "warning"] {
        let (_, saturation) = hue_saturation(rgb(&gray, token));
        assert!(saturation > 0.2, "{token} has a hue");
        assert!(contrast(rgb(&gray, token), bg) >= TEXT_CONTRAST, "{token} reads");
    }
}

/// A light palette derives a light theme, not a dark one wearing its accent.
#[test]
fn a_light_palette_derives_a_light_theme() {
    let theme = Palette::parse(
        "mode = \"light\"\nbackground = \"#faf4ed\"\nforeground = \"#575279\"\n\
             accent = \"#d7827e\"\nred = \"#b4637a\"\ngreen = \"#286983\"\n",
        false,
    )
    .theme("omarchy");
    let bg = rgb(&theme, "background");
    assert!(luminance(bg) > 0.8);
    assert!(contrast(rgb(&theme, "accent"), bg) >= TEXT_CONTRAST, "lifted toward the ink");
    assert!(luminance(rgb(&theme, "diffAddBg")) > 0.5, "the wash stays light");
}

/// With no theme set, the session wears the desktop's; a theme that is set still wins, and off
/// Omarchy there is nothing to follow.
#[test]
fn an_unset_theme_follows_the_desktop() {
    use crate::config::Config;
    let root = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        root.path().join("colors.toml"),
        "background = \"#101418\"\nforeground = \"#d0d4d8\"\naccent = \"#7fb4ca\"",
    )
    .expect("write");
    let desktop = DesktopTheme { name: "custom".into(), dir: root.path().to_path_buf() };
    let registry = ThemeRegistry::new().with_desktop(Some(&desktop));
    assert_eq!(Config::default().resolved_theme_name(&registry), DESKTOP_THEME);
    let resolved = Config::default().resolved_theme(&registry);
    assert_eq!(resolved.get("background"), Some(&crate::theme::ColorValue::Rgb(16, 20, 24)));
    let chosen = Config { theme: Some("nord".into()), ..Default::default() };
    assert_eq!(chosen.resolved_theme_name(&registry), "nord");
    assert_eq!(Config::default().resolved_theme_name(&ThemeRegistry::new()), "luxe");
}

/// The theme's own `luxe.json` wins; then a built-in of the same name when both are dark; then
/// the palette. A light desktop theme never borrows a dark built-in's name.
#[test]
fn the_desktop_theme_resolves_most_specific_first() {
    let builtins = ThemeRegistry::new();
    let root = tempfile::tempdir().expect("temp dir");
    let dir = root.path().to_path_buf();
    let desktop = |name: &str| DesktopTheme { name: name.into(), dir: dir.clone() };

    // A dark theme Luxe ships: the built-in, under the desktop's name
    std::fs::write(dir.join("colors.toml"), "mode = \"dark\"\nbackground = \"#2e3440\"")
        .expect("write");
    let nord = desktop("nord").theme(&builtins).expect("nord");
    assert_eq!(nord.name, DESKTOP_THEME);
    assert_eq!(nord.resolve_aliases(), builtins.resolve("nord").expect("built in"));

    // Omarchy's rose-pine is Dawn: light, so the palette, not Luxe's dark rose-pine
    std::fs::write(
        dir.join("colors.toml"),
        "mode = \"light\"\nbackground = \"#faf4ed\"\nforeground = \"#575279\"",
    )
    .expect("write");
    let dawn = desktop("rose-pine").theme(&builtins).expect("rose-pine");
    assert_ne!(dawn.resolve_aliases(), builtins.resolve("rose-pine").expect("built in"));
    assert!(luminance(rgb(&dawn, "background")) > 0.8);

    // A retired alias is not a match: catppuccin-latte is not the dark Mocha it used to map to
    let latte = desktop("catppuccin-latte").theme(&builtins).expect("latte");
    assert!(luminance(rgb(&latte, "background")) > 0.8);

    // The theme's own luxe.json, laid over the default for anything it leaves out
    std::fs::write(dir.join("luxe.json"), r##"{"colors": {"accent": "#123456"}}"##).expect("write");
    let own = desktop("nord").theme(&builtins).expect("own");
    assert_eq!(rgb(&own, "accent"), (0x12, 0x34, 0x56));
    assert!(own.resolve_aliases().contains_key("diffAddBg"), "the rest from the default");

    // Nothing to go on at all
    let empty = tempfile::tempdir().expect("temp dir");
    let bare = DesktopTheme { name: "custom".into(), dir: empty.path().to_path_buf() };
    assert!(bare.theme(&builtins).is_none());
}
