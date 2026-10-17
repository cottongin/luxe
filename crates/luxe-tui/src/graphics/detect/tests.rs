use super::*;
use std::collections::HashMap;

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Result<String, std::env::VarError> {
    let map: HashMap<String, String> =
        pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect();
    move |name: &str| map.get(name).cloned().ok_or(std::env::VarError::NotPresent)
}

/// The allowlist is the whole safety of the pixel tier. WezTerm speaks the kitty protocol and
/// renders its placeholder characters as tofu, so "speaks kitty graphics" is not the question
/// being asked here.
#[test]
fn only_terminals_measured_to_draw_placeholders_get_pixels() {
    let pixels = [
        vec![("TERM", "xterm-kitty"), ("LANG", "en_US.UTF-8")],
        vec![("KITTY_WINDOW_ID", "1"), ("TERM", "xterm-256color"), ("LANG", "en_US.UTF-8")],
        vec![("TERM", "xterm-ghostty"), ("LANG", "en_US.UTF-8")],
        vec![("TERM_PROGRAM", "ghostty"), ("TERM", "xterm-256color")],
        vec![("GHOSTTY_RESOURCES_DIR", "/usr/share/ghostty"), ("TERM", "xterm-256color")],
    ];
    for env in pixels {
        let detected = ImageTier::detect(&env_of(&env));
        assert_eq!(detected.tier, ImageTier::Pixels, "{env:?}");
        assert!(!detected.reason.is_empty(), "and it can say why");
    }
}

/// Anything not on the allowlist gets words, never an imitation. A real screenshot is mostly
/// text, and text at two scanlines a cell is a smear that reads as a broken image — which is a
/// worse thing to ship than a sentence saying what happened.
#[test]
fn every_other_terminal_gets_words_rather_than_an_imitation() {
    for env in [
        vec![("TERM", "xterm-256color"), ("WEZTERM_PANE", "0")],
        vec![("TERM", "alacritty")],
        vec![("TERM", "foot")],
        vec![("TERM_PROGRAM", "iterm.app"), ("TERM", "xterm-256color")],
        vec![("TERM", "dumb")],
        vec![],
    ] {
        assert_eq!(ImageTier::detect(&env_of(&env)).tier, ImageTier::Text, "{env:?}");
    }
}

/// A multiplexer forwards graphics it does not model and then repaints the wrong pane with
/// them — whatever terminal is underneath.
#[test]
fn a_multiplexer_gets_words_whatever_is_underneath() {
    for env in [
        vec![("TMUX", "/tmp/tmux-1000/default,1,0"), ("TERM", "xterm-kitty")],
        vec![("TERM", "tmux-256color"), ("KITTY_WINDOW_ID", "1")],
        vec![("TERM", "screen.xterm-256color")],
    ] {
        let detected = ImageTier::detect(&env_of(&env));
        assert_eq!(detected.tier, ImageTier::Text, "{env:?}");
        assert!(detected.reason.contains("multiplexer"), "{}", detected.reason);
    }
}

/// An explicit preference is an instruction, not a hint — including the two nothing is ever
/// resolved to on its own: the low-fidelity preview, and off.
#[test]
fn an_explicit_preference_overrides_detection() {
    assert_eq!(ImageTier::resolve(ImageStyle::Pixels), ImageTier::Pixels);
    assert_eq!(ImageTier::resolve(ImageStyle::Blocks), ImageTier::Blocks);
    assert_eq!(ImageTier::resolve(ImageStyle::Off), ImageTier::Text);
    for preference in [ImageStyle::Pixels, ImageStyle::Blocks, ImageStyle::Off] {
        assert!(ImageTier::resolve_detail(preference).reason.contains("config"));
    }
    assert!(!ImageTier::Text.draws_pixels());
    assert!(ImageTier::Blocks.draws_pixels() && ImageTier::Pixels.draws_pixels());
}
