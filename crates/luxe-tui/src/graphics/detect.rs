//! Image tier detection — which fidelity the terminal can draw at, and what told us.

use luxe_core::config::ImageStyle;

/// How much image fidelity the terminal can be asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageTier {
    /// Kitty graphics protocol with Unicode placeholders.
    Pixels,
    /// Half-block cells.
    #[default]
    Blocks,
    /// A text line naming the image.
    Text,
}

/// A resolved tier and the reason it was chosen, which is the whole content of `/images`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Detection {
    pub tier: ImageTier,
    /// Why, in the words a person would use — never "detection failed".
    pub reason: &'static str,
}

impl ImageTier {
    /// Resolves the reader's preference against what the terminal can actually do.
    pub fn resolve(preference: ImageStyle) -> Self {
        Self::resolve_detail(preference).tier
    }

    /// [`Self::resolve`], keeping the reason so a status view can give one.
    pub fn resolve_detail(preference: ImageStyle) -> Detection {
        Self::resolve_detail_in(preference, &|name| std::env::var(name))
    }

    /// [`Self::resolve`], against an environment lookup rather than the process's own.
    #[cfg(any(test, feature = "fixture"))]
    pub(crate) fn resolve_in(
        preference: ImageStyle,
        env: &dyn Fn(&str) -> Result<String, std::env::VarError>,
    ) -> Self {
        Self::resolve_detail_in(preference, env).tier
    }

    fn resolve_detail_in(
        preference: ImageStyle,
        env: &dyn Fn(&str) -> Result<String, std::env::VarError>,
    ) -> Detection {
        match preference {
            ImageStyle::Pixels => {
                Detection { tier: Self::Pixels, reason: "set to `pixels` in config" }
            }
            ImageStyle::Blocks => {
                Detection { tier: Self::Blocks, reason: "set to `blocks` in config" }
            }
            ImageStyle::Off => Detection { tier: Self::Text, reason: "turned off in config" },
            ImageStyle::Auto | ImageStyle::Unknown => Self::detect(env),
        }
    }

    /// `auto`, against an environment lookup — a parameter so the branches are testable.
    ///
    /// Env only, and never a query the terminal has to answer: a probe that expects a reply has to
    /// read stdin, which the event loop owns, and one that times out has already cost the startup
    /// it was meant to protect.
    ///
    /// The answer is only ever pixels or words. A low-fidelity imitation is not a courtesy — a real
    /// screenshot is mostly text, and two scanlines per cell turn text into a smear that reads as a
    /// broken image rather than as a considered fallback. `blocks` exists for anyone who wants it
    /// (a multiplexer, mostly) and is never chosen on someone's behalf.
    fn detect(env: &dyn Fn(&str) -> Result<String, std::env::VarError>) -> Detection {
        let var = |name: &str| env(name).unwrap_or_default().to_ascii_lowercase();
        let term = var("TERM");
        let words = |reason| Detection { tier: Self::Text, reason };

        if term.is_empty() || term == "dumb" {
            return words("this is not an interactive terminal");
        }

        // A multiplexer forwards graphics it does not model and then repaints the wrong pane with
        // them.
        if !var("TMUX").is_empty() || term.starts_with("tmux") || term.starts_with("screen") {
            return words("a multiplexer cannot keep track of an image it does not model");
        }

        // Positive allowlist, not a guess. Speaking the kitty protocol is not the same as
        // supporting its Unicode placeholders: WezTerm speaks it and draws the placeholder
        // characters as tofu. `pixels` is the lever for anyone whose terminal has since learned to.
        let program = var("TERM_PROGRAM");
        let kitty = !var("KITTY_WINDOW_ID").is_empty() || term.contains("kitty");
        let ghostty = program == "ghostty"
            || term.contains("ghostty")
            || !var("GHOSTTY_RESOURCES_DIR").is_empty();
        if kitty {
            return Detection { tier: Self::Pixels, reason: "kitty draws images inline" };
        }
        if ghostty {
            return Detection { tier: Self::Pixels, reason: "Ghostty draws images inline" };
        }
        words("this terminal is not known to draw images inline")
    }

    /// Whether this tier draws an actual picture.
    pub fn draws_pixels(&self) -> bool {
        !matches!(self, Self::Text)
    }
}

#[cfg(test)]
mod tests;
