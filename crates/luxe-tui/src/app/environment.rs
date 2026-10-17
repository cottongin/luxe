//! App environment — what an [`App`](super::App) reads from the machine it starts on.
//!
//! The installed themes, the terminal's graphics, its cell size and how it shows notifications are
//! facts about the machine, not about the app. Handing them in keeps construction one function of
//! its inputs: the real app detects them, and a fixture or a test pins them, so a user theme named
//! `nord` or a kitty terminal never changes a captured frame.

use crate::graphics::{CellSize, ImageTier};
use crate::notify::Notifier;
use luxe_core::config::Config;
use luxe_core::theme::ThemeRegistry;

/// The machine-dependent inputs to [`App`](super::App) construction.
pub(crate) struct AppEnvironment {
    /// Every theme a name can resolve to
    pub(crate) themes: Themes,
    /// What the terminal can draw an image with
    pub(crate) image_tier: ImageTier,
    /// How big a cell is, in pixels
    pub(crate) cell_size: CellSize,
    /// How a notification reaches the reader
    pub(crate) notifier: Box<dyn Notifier>,
}

/// Every theme a name can resolve to, read once, and how to read them again.
pub(crate) struct Themes {
    registry: ThemeRegistry,
    read: fn() -> ThemeRegistry,
}

impl Themes {
    pub(crate) fn read(read: fn() -> ThemeRegistry) -> Self {
        Self { registry: read(), read }
    }

    /// Reads the themes again, for the moments they may have changed under a running app.
    ///
    /// Opening `/theme` and `/reload` (which a settings or setup write goes through) are where a
    /// reader looks for a theme file they just edited, and a desktop re-theme changes the palette
    /// the desktop's theme is built from. Everywhere else the themes already read stand.
    pub(crate) fn reread(&mut self) {
        self.registry = (self.read)();
    }
}

impl std::ops::Deref for Themes {
    type Target = ThemeRegistry;

    fn deref(&self) -> &ThemeRegistry {
        &self.registry
    }
}

impl AppEnvironment {
    /// The machine as it is: installed themes, and the terminal's own answers.
    pub(crate) fn detect(config: &Config) -> Self {
        Self {
            themes: Themes::read(ThemeRegistry::installed),
            image_tier: ImageTier::resolve(config.images.unwrap_or_default()),
            cell_size: CellSize::detect(),
            notifier: crate::notify::create_notifier(
                config.any_notifications(),
                config.notification_method(),
            ),
        }
    }

    /// A machine that is the same everywhere: built-in themes only, a terminal that says nothing,
    /// the assumed cell and no notifications.
    #[cfg(any(test, feature = "fixture"))]
    pub(crate) fn fixed(config: &Config) -> Self {
        Self {
            themes: Themes::read(ThemeRegistry::new),
            image_tier: ImageTier::resolve_in(config.images.unwrap_or_default(), &|_| {
                Err(std::env::VarError::NotPresent)
            }),
            cell_size: CellSize::ASSUMED,
            notifier: Box::new(crate::notify::NoopNotifier),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::{EnvGuard, TempConfigHome};
    use luxe_core::config::ImageStyle;

    /// The real app finds the themes a reader installed, and a fixture app does not.
    ///
    /// The break this catches: detection and the fixed environment were both built from the
    /// built-ins, so a theme in `~/.config/luxe/themes/` named in the config fell back to the
    /// default with a startup notice.
    #[test]
    fn the_real_app_finds_an_installed_theme_and_a_fixture_app_does_not() {
        let home = tempfile::tempdir().expect("temp dir");
        let _config = TempConfigHome::at(home.path());
        let themes = home.path().join("luxe").join("themes");
        std::fs::create_dir_all(&themes).expect("themes dir");
        std::fs::write(themes.join("harbor.json"), r##"{"name":"harbor","vars":{},"colors":{}}"##)
            .expect("theme file");
        let config = Config { theme: Some("harbor".into()), ..Config::default() };
        let (tx, _rx) = tokio::sync::mpsc::channel(8);

        let real =
            crate::app::App::new(tx.clone(), luxe_core::keymap::KeyMap::defaults(), config.clone());
        let fixture =
            crate::app::App::new_fixture(tx, luxe_core::keymap::KeyMap::defaults(), config);

        assert!(real.startup_notices.is_empty(), "{:?}", real.startup_notices);
        assert!(
            fixture.startup_notices.iter().any(|notice| notice.contains("harbor")),
            "the fixture sees the built-ins only: {:?}",
            fixture.startup_notices
        );
    }

    /// A fixture app's themes are the built-ins whatever the machine has installed, its theme
    /// picker included.
    ///
    /// The break this catches: the picker read the machine's themes itself, so a fixture app
    /// showed a reader's own themes.
    #[test]
    fn a_fixture_app_offers_the_themes_it_started_with() {
        let home = tempfile::tempdir().expect("temp dir");
        let _config = TempConfigHome::at(home.path());
        let themes = home.path().join("luxe").join("themes");
        std::fs::create_dir_all(&themes).expect("themes dir");
        std::fs::write(themes.join("harbor.json"), r##"{"name":"harbor","vars":{},"colors":{}}"##)
            .expect("theme file");
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let mut fixture = crate::app::App::new_fixture(
            tx,
            luxe_core::keymap::KeyMap::defaults(),
            Config::default(),
        );

        fixture.open_theme_picker();
        let offered = fixture.overlays.theme_picker.as_ref().expect("the picker").themes().to_vec();
        assert!(!offered.iter().any(|name| name == "harbor"), "{offered:?}");
        assert_eq!(offered, ThemeRegistry::new().names());
    }

    /// A fixture app is the same on every machine: a kitty terminal changes what the real app
    /// would draw and nothing a fixture does.
    ///
    /// The break this catches: the fixture constructor read the terminal's environment, so a
    /// captured frame with an inline image depended on where it was recorded.
    #[test]
    fn a_fixed_environment_ignores_the_machine() {
        let _env = EnvGuard::set(&[
            ("TERM", Some("xterm-kitty")),
            ("KITTY_WINDOW_ID", Some("1")),
            ("TMUX", None),
        ]);
        assert_eq!(ImageTier::resolve(ImageStyle::Auto), ImageTier::Pixels, "the machine says so");

        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        let app = crate::app::App::new_fixture(
            tx,
            luxe_core::keymap::KeyMap::defaults(),
            Config::default(),
        );

        assert_eq!(app.images.tier(), ImageTier::Text);
    }
}
