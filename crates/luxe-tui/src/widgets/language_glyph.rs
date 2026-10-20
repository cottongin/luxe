//! language_glyph — which mark a fenced code block wears.
//!
//! A code block's opening rule already names its language in words, so the mark beside it is pure
//! shine: a reader scrolling an answer full of fences picks the Rust one out by silhouette before
//! reading anything. That makes this the same job [`super::tool_glyph`] does for tool cards, and it
//! is built the same way — the shapes live in [`crate::glyphs`] with a rendering per fidelity tier,
//! and this module only decides which language maps to which.
//!
//! **Below the nerd tier every mark is nothing at all**, by declaration in `glyphs.rs`. Twenty-seven
//! languages cannot be given twenty-seven honest shapes in a font guaranteed only a couple of dozen,
//! and one `▪` standing for all of them says strictly less than the word already beside it. So the
//! reader without a patched font loses decoration and keeps every fact — which is the glyph rule for
//! the whole product, applied here.
//!
//! # Which languages are here
//!
//! The ones luxe is first-class in, which is not a taste judgment: every language with a default
//! entry in `luxe-lsp`'s server registry or `luxe-dap`'s adapter registry earns a mark, and
//! `every_first_class_language_has_a_mark` fails if one is added to either registry without one.
//! On top of those sit the languages that show up in *answers* rather than in a project —
//! `json`, `yaml`, `toml`, `markdown`, `html`, `css`, `sql`, `diff` — and a handful of common
//! others. Anything unrecognized keeps the generic [`GENERIC_LANGUAGE`] mark, because a fence luxe
//! has never heard of is still a fence.
//!
//! Spellings are folded rather than multiplied: `rs` is `rust`, `sh`/`zsh`/`shell` are `bash`,
//! `c++`/`cc`/`hpp` are `cpp`, `yml` is `yaml`. The label the rule draws is whatever the author
//! wrote; only the mark is normalized.

use crate::glyphs::Icon;

/// The mark a fence wears when its language has none of its own — also an unlabeled fence's.
pub const GENERIC_LANGUAGE: Icon = Icon::Code;

/// The mark for `language`, falling back to [`GENERIC_LANGUAGE`] for anything unrecognized.
///
/// `language` arrives already trimmed and lowercased by the markdown layer, and is matched rather
/// than parsed: a fence info string is written by hand and there is no vocabulary to validate it
/// against.
pub fn language_icon(language: &str) -> Icon {
    match language {
        // === First-class: an LSP server, a debug adapter, or both ===
        "rust" | "rs" => Icon::LangRust,
        "python" | "py" | "python3" => Icon::LangPython,
        "typescript" | "ts" | "tsx" => Icon::LangTypeScript,
        "javascript" | "js" | "jsx" | "mjs" | "cjs" | "node" => Icon::LangJavaScript,
        "go" | "golang" => Icon::LangGo,
        "c" | "h" => Icon::LangC,
        "cpp" | "c++" | "cc" | "cxx" | "hpp" | "hxx" => Icon::LangCpp,
        "bash" | "sh" | "shell" | "zsh" | "console" | "shellsession" => Icon::LangShell,
        "ruby" | "rb" => Icon::LangRuby,
        "lua" => Icon::LangLua,
        "svelte" => Icon::LangSvelte,
        "json" | "jsonc" | "json5" => Icon::LangJson,
        "yaml" | "yml" => Icon::LangYaml,

        // === The rest of what an answer actually contains ===
        "toml" => Icon::LangToml,
        "html" | "htm" | "xhtml" => Icon::LangHtml,
        "css" | "scss" | "sass" | "less" => Icon::LangCss,
        "sql" | "postgres" | "postgresql" | "mysql" | "sqlite" => Icon::LangSql,
        "markdown" | "md" | "mdx" => Icon::LangMarkdown,
        "java" => Icon::LangJava,
        "php" => Icon::LangPhp,
        "swift" => Icon::LangSwift,
        "kotlin" | "kt" | "kts" => Icon::LangKotlin,
        "haskell" | "hs" => Icon::LangHaskell,
        "zig" => Icon::LangZig,
        "nix" => Icon::LangNix,
        "docker" | "dockerfile" => Icon::LangDocker,
        "diff" | "patch" => Icon::LangDiff,
        _ => GENERIC_LANGUAGE,
    }
}

/// The color a language's mark wears beside a file name: the hue its own logo is known by, taken
/// from the theme's palette rather than from the logo, so every theme keeps its own look.
///
/// The palette has a red, an orange-yellow, a green, a blue, a magenta and the accent pair, which
/// is enough to tell the common languages apart at a glance, as file managers such as yazi do.
/// Several share a hue; the mark's shape and the name beside it still tell them apart. A mark
/// with no known hue stays muted, so color only ever adds.
pub fn language_hue(
    icon: Icon,
    styles: &crate::theme::styles::ThemeStyles,
) -> ratatui::style::Style {
    match language_hue_role(icon) {
        LanguageHue::Warning => styles.warning,
        LanguageHue::Info => styles.info,
        LanguageHue::Success => styles.success,
        LanguageHue::Error => styles.error,
        LanguageHue::ThinkingUltra => styles.thinking_ultra,
        LanguageHue::Text => styles.text,
        LanguageHue::Muted => styles.muted,
    }
}

/// The theme role a language's mark is colored by, for a surface that colors by role rather than
/// by a terminal's style: the phone app, whose stylesheet has a variable for each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageHue {
    /// The theme's warning hue
    Warning,
    /// The theme's info hue
    Info,
    /// The theme's success hue
    Success,
    /// The theme's error hue
    Error,
    /// The deepest thinking level's hue
    ThinkingUltra,
    /// Body text
    Text,
    /// Muted text, for a mark with no known hue
    Muted,
}

/// The role [`language_hue`] colors `icon` by.
pub fn language_hue_role(icon: Icon) -> LanguageHue {
    match icon {
        Icon::LangRust | Icon::LangSwift | Icon::LangZig => LanguageHue::Warning,
        Icon::LangPython | Icon::LangJavaScript | Icon::LangJson => LanguageHue::Warning,
        Icon::LangTypeScript
        | Icon::LangGo
        | Icon::LangC
        | Icon::LangLua
        | Icon::LangCss
        | Icon::LangNix
        | Icon::LangDocker => LanguageHue::Info,
        Icon::LangShell => LanguageHue::Success,
        Icon::LangRuby | Icon::LangJava | Icon::LangHtml | Icon::LangSvelte | Icon::LangToml => {
            LanguageHue::Error
        }
        Icon::LangCpp | Icon::LangPhp | Icon::LangKotlin | Icon::LangHaskell => {
            LanguageHue::ThinkingUltra
        }
        Icon::LangMarkdown => LanguageHue::Text,
        _ => LanguageHue::Muted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyphs::GlyphTier;

    /// The languages luxe ships a language server or a debug adapter for. Read off
    /// `luxe-lsp/src/default-lsp-servers.json` and `luxe-dap/src/default-debug-adapters.json`
    /// rather than invented, so this list is a claim about the product and not about this file.
    const FIRST_CLASS: &[&str] = &[
        // LSP registry keys.
        "rust",
        "typescript",
        "python",
        "go",
        "cpp",
        "bash",
        "json",
        "yaml",
        "ruby",
        "lua",
        "svelte",
        // Debug adapters: debugpy (py), delve (go), and CodeLLDB/lldb-dap/gdb for the natives.
        "c",
    ];

    /// A language luxe can run a server or a debugger for is one it claims to know, so a fence in
    /// it must not fall back to the generic mark. Add an entry to either registry without one here
    /// and this is what says so.
    #[test]
    fn every_first_class_language_has_a_mark() {
        for language in FIRST_CLASS {
            assert_ne!(
                language_icon(language),
                GENERIC_LANGUAGE,
                "{language} is first-class but wears the generic mark"
            );
        }
    }

    /// Spelling variants are the same language, so they cannot look like different ones.
    #[test]
    fn aliases_share_their_language_mark() {
        for (alias, primary) in [
            ("rs", "rust"),
            ("py", "python"),
            ("ts", "typescript"),
            ("tsx", "typescript"),
            ("js", "javascript"),
            ("jsx", "javascript"),
            ("golang", "go"),
            ("c++", "cpp"),
            ("hpp", "cpp"),
            ("sh", "bash"),
            ("zsh", "bash"),
            ("yml", "yaml"),
            ("md", "markdown"),
            ("kt", "kotlin"),
            ("rb", "ruby"),
            ("dockerfile", "docker"),
        ] {
            assert_eq!(
                language_icon(alias),
                language_icon(primary),
                "{alias} and {primary} are the same language"
            );
        }
    }

    /// A language nobody mapped still gets a fence, and an unlabeled fence still gets a rule.
    #[test]
    fn an_unknown_language_falls_back_to_the_generic_mark() {
        assert_eq!(language_icon("brainfuck"), GENERIC_LANGUAGE);
        assert_eq!(language_icon(""), GENERIC_LANGUAGE);
    }

    /// The mark is decoration and the label beside it is the fact, so below the nerd tier every
    /// language draws nothing rather than a shape that would stand for two dozen of them.
    #[test]
    fn no_language_draws_a_mark_below_the_nerd_tier() {
        for language in FIRST_CLASS.iter().chain(&["toml", "html", "diff", "brainfuck"]) {
            let icon = language_icon(language);
            assert!(!icon.at(GlyphTier::Nerd).is_empty(), "{language} has a nerd mark");
            for tier in [GlyphTier::Unicode, GlyphTier::Ascii] {
                assert!(
                    icon.at(tier).is_empty(),
                    "{language} draws {:?} at {tier:?}, where the word is the whole label",
                    icon.at(tier)
                );
            }
        }
    }

    /// Every mark is a distinct silhouette, which is the only reason drawing one is worth cells.
    /// The shared-shape escape hatch in `glyphs.rs` covers the rest of the product; a language
    /// borrowing another language's mark is never right.
    #[test]
    fn no_two_languages_share_a_mark() {
        use std::collections::HashMap;
        let mut seen: HashMap<&str, Icon> = HashMap::new();
        for language in FIRST_CLASS.iter().chain(&[
            "toml",
            "html",
            "css",
            "sql",
            "markdown",
            "java",
            "php",
            "swift",
            "kotlin",
            "haskell",
            "zig",
            "nix",
            "docker",
            "diff",
            "javascript",
        ]) {
            let icon = language_icon(language);
            if let Some(other) = seen.insert(icon.at(GlyphTier::Nerd), icon) {
                assert_eq!(icon, other, "two languages share one mark: {icon:?} and {other:?}");
            }
        }
    }
}
