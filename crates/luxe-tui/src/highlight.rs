//! highlight — syntax highlighting via syntect, in the active theme's colors.
//!
//! syntect parses; it does not choose the colors. The theme it highlights against is a table of
//! [`Role`]s, each a placeholder color, and `paint` swaps every role for the theme's own style
//! when a line is drawn. So a code block, a tool card, a diff and the preview all wear the palette
//! everything else wears, a custom theme or Omarchy's included, and switching themes recolors code
//! at once: the memo holds roles, not colors, so it never goes stale.
//!
//! [`highlight_code`] is memoized: the transcript re-lays out every block on every frame, so an
//! answer with a few fenced blocks, or a handful of `write` cards, would otherwise spend more of
//! each frame here than in the rest of the render path put together.
//!
//! syntect runs on **`fancy-regex`**, not its default Oniguruma binding. Oniguruma is roughly 3.5×
//! faster with identical output, but it is C, and a C toolchain in the dependency graph is a tax on
//! every `cargo install`, every cross-build, and every CI runner. The memo is what makes that trade
//! cheap: the cost is paid once per distinct line, not per frame, and highlighting is per line
//! rather than per file.

use crate::theme::styles::ThemeStyles;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock};

static SYNTAX_SET: OnceLock<syntect::parsing::SyntaxSet> = OnceLock::new();
static ROLE_THEME: OnceLock<syntect::highlighting::Theme> = OnceLock::new();

fn get_syntax_set() -> &'static syntect::parsing::SyntaxSet {
    SYNTAX_SET.get_or_init(syntect::parsing::SyntaxSet::load_defaults_newlines)
}

/// What a stretch of code is, as far as its color goes.
///
/// Few on purpose: a theme has about seven hues, and a role only earns its place by landing on a
/// different one, or by meaning something the markdown renderer already draws its own way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Role {
    Text,
    Punctuation,
    Comment,
    Keyword,
    String,
    Escape,
    Constant,
    Function,
    Type,
    Tag,
    Heading,
    Link,
    LinkUrl,
    Quote,
    Code,
    Strong,
    Emphasis,
    ListMarker,
    Inserted,
    Deleted,
    Invalid,
}

impl Role {
    const ALL: [Role; 21] = [
        Role::Text,
        Role::Punctuation,
        Role::Comment,
        Role::Keyword,
        Role::String,
        Role::Escape,
        Role::Constant,
        Role::Function,
        Role::Type,
        Role::Tag,
        Role::Heading,
        Role::Link,
        Role::LinkUrl,
        Role::Quote,
        Role::Code,
        Role::Strong,
        Role::Emphasis,
        Role::ListMarker,
        Role::Inserted,
        Role::Deleted,
        Role::Invalid,
    ];

    /// The theme token this role is drawn in, and how.
    ///
    /// The tokens spend the palette on meaning: the accent goes on what you scan a file for, the
    /// functions; types take the cool hue so they separate from calls; keywords take
    /// `accentSecondary`, the violet most syntax themes give them; punctuation recedes into
    /// `muted`. `dim` is never used, since it is a rule color, not a text color. Markdown takes the
    /// `md*` tokens, so the preview of a `.md` file wears the colors the rendered answer does.
    pub fn token(self) -> (&'static str, Modifier) {
        match self {
            Role::Text | Role::Strong | Role::Emphasis => (
                "text",
                match self {
                    Role::Strong => Modifier::BOLD,
                    Role::Emphasis => Modifier::ITALIC,
                    _ => Modifier::empty(),
                },
            ),
            Role::Punctuation => ("muted", Modifier::empty()),
            Role::Comment => ("muted", Modifier::ITALIC),
            Role::Keyword => ("accentSecondary", Modifier::empty()),
            Role::String | Role::Inserted => ("success", Modifier::empty()),
            Role::Escape | Role::Type => ("info", Modifier::empty()),
            Role::Constant => ("warning", Modifier::empty()),
            Role::Function | Role::ListMarker => ("accent", Modifier::empty()),
            // the palette's berry, which a resolved theme carries only as `thinkingUltra`
            Role::Tag => ("thinkingUltra", Modifier::empty()),
            Role::Heading => ("mdHeading", Modifier::BOLD),
            Role::Link => ("mdLink", Modifier::empty()),
            Role::LinkUrl => ("mdLinkUrl", Modifier::empty()),
            Role::Quote => ("mdQuote", Modifier::ITALIC),
            Role::Code => ("mdCode", Modifier::empty()),
            Role::Deleted | Role::Invalid => ("error", Modifier::empty()),
        }
    }

    /// The theme's style for this role: only the foreground and the modifier, so a diff's or a
    /// card's background shows through.
    pub fn style(self, styles: &ThemeStyles) -> Style {
        let (token, modifier) = self.token();
        let source = token_style(styles, token).unwrap_or(styles.text);
        Style { fg: source.fg, ..Style::default() }.add_modifier(modifier)
    }

    /// The placeholder color that stands for this role inside syntect.
    fn marker(self) -> syntect::highlighting::Color {
        syntect::highlighting::Color { r: self as u8, g: 0x1c, b: 0x5e, a: 0xff }
    }

    fn from_marker(color: syntect::highlighting::Color) -> Role {
        Role::ALL.get(usize::from(color.r)).copied().unwrap_or(Role::Text)
    }
}

/// The resolved style a theme token names, for the tokens [`Role::token`] uses.
fn token_style(styles: &ThemeStyles, token: &str) -> Option<Style> {
    Some(match token {
        "text" => styles.text,
        "muted" => styles.muted,
        "accent" => styles.accent,
        "accentSecondary" => styles.accent_secondary,
        "info" => styles.info,
        "success" => styles.success,
        "warning" => styles.warning,
        "error" => styles.error,
        "thinkingUltra" => styles.thinking_ultra,
        "mdHeading" => styles.md_heading,
        "mdLink" => styles.md_link,
        "mdLinkUrl" => styles.md_link_url,
        "mdQuote" => styles.md_quote,
        "mdCode" => styles.md_code,
        _ => return None,
    })
}

/// Scope selectors and the role each takes, in TextMate's vocabulary: the one table both the TUI
/// and the website's code blocks (`examples/shiki_theme.rs`) are colored from.
///
/// The most specific selector that matches wins, in syntect and in TextMate alike, so
/// `keyword.operator` beats `keyword` and an `=` recedes with the punctuation rather than wearing
/// the keyword color. Broad entries come first anyway, since TextMate breaks a tie by taking the
/// later one. `storage.type` is a keyword: the bundled grammars put `fn`, `let`, `def` and `class`
/// under it, and a primitive type in the keyword color is the smaller price.
pub const SCOPES: &[(&str, Role)] = &[
    ("variable, variable.parameter, variable.other", Role::Text),
    ("punctuation, meta.brace, keyword.operator, meta.separator", Role::Punctuation),
    ("comment, punctuation.definition.comment", Role::Comment),
    ("string, punctuation.definition.string, meta.embedded.string", Role::String),
    ("constant.character.escape, constant.other.placeholder, string.regexp", Role::Escape),
    ("constant.numeric, constant.language, constant.other, keyword.other.unit", Role::Constant),
    ("keyword, storage, storage.type, storage.modifier, keyword.control", Role::Keyword),
    (
        "entity.name.function, support.function, meta.function-call, variable.function, \
         support.macro",
        Role::Function,
    ),
    (
        "entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, \
         entity.name.trait, entity.name.impl, entity.name.namespace, \
         entity.other.inherited-class, support.type, support.class",
        Role::Type,
    ),
    (
        "entity.name.tag, entity.other.attribute-name, meta.annotation, variable.annotation",
        Role::Tag,
    ),
    // keys take the cool hue rather than the berry: in a JSON file keys are half the tokens, and a
    // warm accent on every one of them reads as alarm
    ("support.type.property-name, meta.object-literal.key, meta.mapping.key", Role::Type),
    ("markup.heading", Role::Heading),
    ("meta.link.inline.description, string.other.link", Role::Link),
    ("markup.underline.link", Role::LinkUrl),
    ("markup.quote", Role::Quote),
    ("markup.raw", Role::Code),
    ("markup.bold", Role::Strong),
    ("markup.italic", Role::Emphasis),
    (
        "markup.list.numbered.bullet, markup.list.unnumbered.bullet, \
         punctuation.definition.list_item",
        Role::ListMarker,
    ),
    ("markup.inserted", Role::Inserted),
    ("markup.deleted", Role::Deleted),
    ("invalid, invalid.illegal, message.error", Role::Invalid),
];

/// The syntect theme whose every color is a [`Role`] marker.
fn role_theme() -> &'static syntect::highlighting::Theme {
    use syntect::highlighting::{ScopeSelectors, StyleModifier, Theme, ThemeItem, ThemeSettings};
    ROLE_THEME.get_or_init(|| Theme {
        settings: ThemeSettings {
            foreground: Some(Role::Text.marker()),
            ..ThemeSettings::default()
        },
        scopes: SCOPES
            .iter()
            .map(|(selectors, role)| ThemeItem {
                scope: ScopeSelectors::from_str(selectors).expect("a well-formed scope selector"),
                style: StyleModifier {
                    foreground: Some(role.marker()),
                    ..StyleModifier::default()
                },
            })
            .collect(),
        ..Theme::default()
    })
}

/// One highlighted line, as roles: what the memo keeps, since it holds for every theme.
pub type RoleLine = Vec<(Role, String)>;

/// Highlights `code` as `lang`, in `styles`' colors.
///
/// Unknown or absent languages fall back to plain text. Memoized; see the module note.
pub fn highlight_code(code: &str, lang: Option<&str>, styles: &ThemeStyles) -> Vec<Line<'static>> {
    paint(&highlight_roles(code, lang), styles)
}

/// Highlights `code` as `lang` into roles, for a surface that colors roles itself: the phone app's
/// HTML names each role's theme token as a class. Memoized, like [`highlight_code`].
pub fn highlight_roles(code: &str, lang: Option<&str>) -> Arc<Vec<RoleLine>> {
    let key = fingerprint(code, lang);
    if let Some(hit) = cache().get(key, code, lang) {
        return hit;
    }
    // Deliberately outside the lock: syntect is the slow part, nothing else needs to wait for it,
    // and a panic in it cannot poison the cache it is not holding.
    let lines = Arc::new(highlight_uncached(code, lang));
    cache().insert(key, code, lang, Arc::clone(&lines));
    lines
}

/// Turns roles into the theme's styles.
fn paint(lines: &[RoleLine], styles: &ThemeStyles) -> Vec<Line<'static>> {
    lines
        .iter()
        .map(|line| {
            Line::from(
                line.iter()
                    .map(|(role, text)| Span::styled(text.clone(), role.style(styles)))
                    .collect::<Vec<_>>(),
            )
        })
        .collect()
}

fn highlight_uncached(code: &str, lang: Option<&str>) -> Vec<RoleLine> {
    let syntax_set = get_syntax_set();
    let syntax = lang
        .and_then(|l| syntax_set.find_syntax_by_token(l))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());
    let mut h = syntect::easy::HighlightLines::new(syntax, role_theme());
    let mut failed = false;
    code.lines()
        .map(|line| {
            let highlighted = h.highlight_line(line, syntax_set);
            if let (Err(error), false) = (&highlighted, failed) {
                // once per block: the result is cached, so a repaint never asks again
                tracing::debug!(%error, lang, "highlighting failed; showing plain text");
                failed = true;
            }
            line_roles(highlighted, line)
        })
        .collect()
}

/// A highlighted line as roles, neighbors in one role joined so a line is as few spans as it has
/// colors. A line the highlighter failed on is kept whole as plain text: color is best effort, the
/// source is not, so a failure must not come back as no spans at all.
fn line_roles<E>(
    highlighted: Result<Vec<(syntect::highlighting::Style, &str)>, E>,
    line: &str,
) -> RoleLine {
    let Ok(highlighted) = highlighted else { return vec![(Role::Text, line.to_string())] };
    let mut roles: RoleLine = Vec::new();
    for (style, text) in highlighted {
        let role = Role::from_marker(style.foreground);
        match roles.last_mut() {
            Some((last, joined)) if *last == role => joined.push_str(text),
            _ => roles.push((role, text.to_string())),
        }
    }
    roles
}

// === Memoization ===

/// Entries the cache holds before evicting the least recently used.
///
/// Generous next to what one screen needs, because the working set is every block in the
/// transcript — the render path lays out all of them, not just the visible ones.
const MAX_ENTRIES: usize = 256;
/// Source bytes the cache holds, as a second bound: a few very large blocks should not be able to
/// pin megabytes of highlighted spans just because they are few.
const MAX_BYTES: usize = 2 << 20;

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(Cache::default()));

/// A poisoned highlight cache is no reason to take the UI down: every entry is a pure function of
/// its key, so the worst a recovered lock holds is a memo that is still correct.
fn cache() -> MutexGuard<'static, Cache> {
    CACHE.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Highlighted blocks kept between frames.
///
/// Keyed by a hash of the inputs and *verified* against the stored ones on the way out, so a hash
/// collision costs a re-highlight rather than the wrong colors on screen.
#[derive(Default)]
struct Cache {
    entries: HashMap<u64, Entry>,
    /// Source bytes currently held, kept incrementally so eviction never walks the map twice.
    bytes: usize,
    /// Logical clock: every lookup and insert stamps its entry, and eviction takes the lowest.
    clock: u64,
}

/// One memoized highlight, with the inputs that produced it.
struct Entry {
    code: Box<str>,
    lang: Option<Box<str>>,
    lines: Arc<Vec<RoleLine>>,
    used: u64,
}

impl Entry {
    /// Whether this entry really is the answer for these inputs (a hash match is not proof).
    fn matches(&self, code: &str, lang: Option<&str>) -> bool {
        &*self.code == code && self.lang.as_deref() == lang
    }
}

impl Cache {
    fn get(&mut self, key: u64, code: &str, lang: Option<&str>) -> Option<Arc<Vec<RoleLine>>> {
        self.clock += 1;
        let clock = self.clock;
        let entry = self.entries.get_mut(&key)?;
        if !entry.matches(code, lang) {
            return None;
        }
        entry.used = clock;
        Some(Arc::clone(&entry.lines))
    }

    fn insert(&mut self, key: u64, code: &str, lang: Option<&str>, lines: Arc<Vec<RoleLine>>) {
        self.clock += 1;
        let entry =
            Entry { code: code.into(), lang: lang.map(Into::into), lines, used: self.clock };
        self.bytes += entry.code.len();
        if let Some(replaced) = self.entries.insert(key, entry) {
            self.bytes = self.bytes.saturating_sub(replaced.code.len());
        }
        self.evict();
    }

    /// Drops least-recently-used entries until both bounds hold.
    fn evict(&mut self) {
        while self.entries.len() > MAX_ENTRIES || (self.bytes > MAX_BYTES && self.entries.len() > 1)
        {
            let Some(oldest) =
                self.entries.iter().min_by_key(|(_, entry)| entry.used).map(|(key, _)| *key)
            else {
                return;
            };
            if let Some(dropped) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(dropped.code.len());
            }
        }
    }
}

/// Hash of everything that changes the highlighted output.
fn fingerprint(code: &str, lang: Option<&str>) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    code.hash(&mut hasher);
    lang.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests;
