//! Modes, drawn — the composer's title, and the color a mode lends the surfaces around it.
//!
//! The mode itself is the engine's ([`luxe_core::mode`]); this is only how the TUI shows it. One
//! rule decides which surfaces take the mode's color: the composer, whatever grows out of it, and
//! whatever the agent puts in front of you. That is the `/` and `@` menus, the question picker and
//! the permission prompt. Everything else keeps the accent, so the color always means "where your
//! words go, and what the agent may do with them".

use crate::glyphs::Icon;
use crate::theme::styles::ThemeStyles;
use ratatui::style::Style;
use std::borrow::Cow;

/// The icon beside a mode's name, here and on the phone app's mode button.
pub fn icon(mode: &str) -> Icon {
    match mode {
        luxe_core::mode::PLAN => Icon::ModePlan,
        _ => Icon::ModeBuild,
    }
}

/// The color a mode's composer border and title wear.
///
/// Plan wears its own; every other mode, a custom one included, wears the border accent every
/// other panel does, as its icon is build's, so a session that never plans looks the way luxe
/// always has.
pub(crate) fn accent(mode: &str, styles: &ThemeStyles) -> Style {
    match mode {
        luxe_core::mode::PLAN => styles.mode_plan,
        _ => styles.border_accent,
    }
}

/// The theme, with the border accent swapped for the mode's color.
///
/// Borrowed in every mode but plan, so the surfaces that take it cost nothing until somebody plans.
pub(crate) fn styles<'a>(mode: &str, styles: &'a ThemeStyles) -> Cow<'a, ThemeStyles> {
    if mode != luxe_core::mode::PLAN {
        return Cow::Borrowed(styles);
    }
    let mut tinted = styles.clone();
    tinted.border_accent = accent(mode, styles);
    Cow::Owned(tinted)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> ThemeStyles {
        let resolved =
            luxe_core::theme::ThemeRegistry::new().resolve("luxe").expect("the luxe theme");
        ThemeStyles::from_resolved(&resolved, "luxe")
    }

    /// A session that never plans must look exactly as it did before modes existed.
    #[test]
    fn build_borrows_the_theme_untouched() {
        let theme = theme();
        assert!(matches!(styles(luxe_core::mode::BUILD, &theme), Cow::Borrowed(_)));
        assert_eq!(accent(luxe_core::mode::BUILD, &theme), theme.border_accent);
    }

    /// A mode luxe does not know looks like one thing, not half build and half plan.
    ///
    /// The break this catches: the icon fell back to build and the color to plan, so a custom
    /// mode wore build's icon in plan's color.
    #[test]
    fn an_unknown_mode_falls_back_to_build_everywhere() {
        let theme = theme();
        assert!(matches!(icon("review"), Icon::ModeBuild));
        assert_eq!(accent("review", &theme), theme.border_accent);
        assert!(matches!(styles("review", &theme), Cow::Borrowed(_)));
    }

    #[test]
    fn plan_lends_its_color_to_the_border_and_nothing_else() {
        let theme = theme();
        let tinted = styles(luxe_core::mode::PLAN, &theme);
        assert_eq!(tinted.border_accent, theme.mode_plan);
        assert_ne!(tinted.border_accent, theme.border_accent, "plan has to look different");
        assert_eq!(tinted.accent, theme.accent, "keys and cursors keep the accent");
    }
}
