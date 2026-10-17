use super::*;

fn styles(name: &str) -> ThemeStyles {
    let registry = luxe_core::theme::ThemeRegistry::new();
    ThemeStyles::from_resolved(&registry.resolve(name).expect("a built-in"), name)
}

/// Every bundled syntax must highlight without erroring.
///
/// `fancy-regex` does not implement every Oniguruma construct, and a Sublime syntax that uses one
/// fails at *runtime* — meaning a language would silently stop highlighting on a syntect bump. All
/// 75 pass today; this is the tripwire for the day one doesn't.
#[test]
fn every_bundled_syntax_highlights_under_the_pure_rust_engine() {
    let syntaxes = get_syntax_set();
    let sample = "fn main() { let x = \"a\\\"b\"; /* c */ }\nSELECT * FROM t WHERE a='b';\n";
    let mut broken = Vec::new();
    for syntax in syntaxes.syntaxes() {
        let mut highlighter = syntect::easy::HighlightLines::new(syntax, role_theme());
        for line in syntect::util::LinesWithEndings::from(sample) {
            if let Err(error) = highlighter.highlight_line(line, syntaxes) {
                broken.push(format!("{}: {error}", syntax.name));
                break;
            }
        }
    }
    assert!(broken.is_empty(), "syntaxes the regex engine cannot handle: {broken:?}");
}

/// Every role names a token the theme resolves, so none falls back to body text by accident.
#[test]
fn every_role_names_a_token_the_theme_has() {
    let styles = styles("luxe");
    for role in Role::ALL {
        let (token, _) = role.token();
        assert!(token_style(&styles, token).is_some(), "{role:?} names {token}");
        assert!(luxe_core::theme::Theme::ALL_TOKENS.contains(&token), "{token} is a theme token");
    }
}

/// Every role survives the trip through syntect as itself.
#[test]
fn every_role_reads_back_from_its_marker() {
    for role in Role::ALL {
        assert_eq!(Role::from_marker(role.marker()), role);
    }
}

fn roles_of(code: &str, lang: &str) -> Vec<(Role, String)> {
    highlight_uncached(code, Some(lang)).into_iter().flatten().collect()
}

fn role_of(code: &str, lang: &str, piece: &str) -> Role {
    roles_of(code, lang)
        .into_iter()
        .find(|(_, text)| text.contains(piece))
        .map(|(role, _)| role)
        .unwrap_or_else(|| panic!("{piece:?} is not in {code:?}"))
}

/// Code is colored by what it is: the grammar's scopes land on the roles meant for them.
#[test]
fn code_lands_on_the_roles_meant_for_it() {
    let rust = "pub fn main() { let s = \"hello\\n\"; // hi\n    call(42, true) }";
    assert_eq!(role_of(rust, "rust", "fn"), Role::Keyword);
    assert_eq!(role_of(rust, "rust", "main"), Role::Function);
    assert_eq!(role_of(rust, "rust", "hello"), Role::String);
    assert_eq!(role_of(rust, "rust", "\\n"), Role::Escape);
    assert_eq!(role_of(rust, "rust", "hi"), Role::Comment);
    assert_eq!(role_of(rust, "rust", "42"), Role::Constant);
    assert_eq!(role_of(rust, "rust", "="), Role::Punctuation, "an operator is not a keyword");
    assert_eq!(role_of("struct Point;", "rust", "Point"), Role::Type);
}

/// Markdown in the preview wears the colors the rendered answer gives the same things, and a
/// list item's text stays body text with only its marker colored.
#[test]
fn markdown_lands_on_the_markdown_roles() {
    let md = "# Title\n\n> quoted\n\n1. item [link](https://x.y) and `code` and **bold**\n";
    assert_eq!(role_of(md, "md", "Title"), Role::Heading);
    assert_eq!(role_of(md, "md", "quoted"), Role::Quote);
    assert_eq!(role_of(md, "md", "1"), Role::ListMarker);
    assert_eq!(role_of(md, "md", "item"), Role::Text);
    assert_eq!(role_of(md, "md", "link"), Role::Link);
    assert_eq!(role_of(md, "md", "https"), Role::LinkUrl);
    assert_eq!(role_of(md, "md", "code"), Role::Code);
    assert_eq!(role_of(md, "md", "bold"), Role::Strong);
}

/// The colors are the active theme's: the same code in two themes wears each theme's own
/// palette, keyword in its violet and comment in its muted, and nothing is left over from a
/// fixed syntax theme.
#[test]
fn the_colors_are_the_active_themes() {
    let code = "fn main() {} // note";
    for name in ["luxe", "nord", "catppuccin"] {
        let styles = styles(name);
        let lines = highlight_code(code, Some("rust"), &styles);
        let fg = |piece: &str| {
            lines[0].spans.iter().find(|span| span.content.contains(piece)).unwrap().style.fg
        };
        assert_eq!(fg("fn"), styles.accent_secondary.fg, "{name}");
        assert_eq!(fg("note"), styles.muted.fg, "{name}");
        assert_eq!(fg("main"), styles.accent.fg, "{name}");
    }
    assert_ne!(
        highlight_code(code, Some("rust"), &styles("luxe")),
        highlight_code(code, Some("rust"), &styles("nord")),
        "one memo, two themes, two colorings"
    );
}

#[test]
fn test_unknown_lang_plain() {
    let lines = highlight_code("hello world", None, &styles("luxe"));
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].spans.len(), 1, "plain text is one body-text span");
}

#[test]
fn test_empty_code() {
    assert!(highlight_code("", Some("rust"), &styles("luxe")).is_empty());
}

#[test]
fn memoized_calls_agree_with_uncached_ones() {
    let code = "fn main() {\n    println!(\"hi\");\n}";
    let fresh = highlight_uncached(code, Some("rust"));
    let first = highlight_roles(code, Some("rust"));
    let second = highlight_roles(code, Some("rust"));
    assert_eq!(*first, fresh, "a memoized highlight must equal the real one");
    assert_eq!(*second, fresh, "and stay equal on the hit path");
}

#[test]
fn the_language_is_part_of_the_key() {
    let code = "let x = 1;";
    assert_ne!(highlight_roles(code, Some("rust")), highlight_roles(code, None));
}

fn plain(text: &str) -> Arc<Vec<RoleLine>> {
    Arc::new(vec![vec![(Role::Text, text.to_string())]])
}

#[test]
fn a_hash_collision_recomputes_instead_of_lying() {
    let mut cache = Cache::default();
    cache.insert(7, "first", Some("rust"), plain("first"));
    // Same key, different inputs: the verification has to reject it.
    assert!(cache.get(7, "second", Some("rust")).is_none());
    assert!(cache.get(7, "first", Some("python")).is_none());
    assert_eq!(cache.get(7, "first", Some("rust")), Some(plain("first")));
}

#[test]
fn eviction_keeps_the_entry_bound_and_drops_the_oldest() {
    let mut cache = Cache::default();
    for index in 0..MAX_ENTRIES + 8 {
        let code = format!("line {index}");
        cache.insert(index as u64, &code, None, plain(&code));
    }
    assert_eq!(cache.entries.len(), MAX_ENTRIES);
    assert!(cache.get(0, "line 0", None).is_none(), "the oldest entry should be gone");
    let newest = format!("line {}", MAX_ENTRIES + 7);
    assert!(cache.get((MAX_ENTRIES + 7) as u64, &newest, None).is_some());
}

#[test]
fn eviction_keeps_the_byte_bound_without_emptying_itself() {
    let mut cache = Cache::default();
    let big = "x".repeat(MAX_BYTES / 2 + 1);
    for key in 0..4 {
        cache.insert(key, &big, None, plain("big"));
    }
    assert!(cache.bytes <= MAX_BYTES, "byte bound held: {} bytes", cache.bytes);
    assert_eq!(cache.entries.len(), 1, "one oversized block still gets to be cached");
}

#[test]
fn a_hit_refreshes_recency_so_the_hot_entry_survives() {
    let mut cache = Cache::default();
    cache.insert(1, "hot", None, plain("hot"));
    for index in 2..MAX_ENTRIES as u64 + 2 {
        let code = format!("cold {index}");
        // Touch the hot entry between inserts, the way a re-render would.
        assert!(cache.get(1, "hot", None).is_some());
        cache.insert(index, &code, None, plain(&code));
    }
    assert!(cache.get(1, "hot", None).is_some(), "the re-read entry must survive");
}

/// A line the highlighter fails on comes back whole as plain text, not as nothing.
///
/// The break this catches: a failure was `unwrap_or_default()`, so the line had no spans, and the
/// empty result was cached as a successful highlight: the code was gone from the preview.
#[test]
fn a_highlight_error_keeps_the_original_line() {
    let failed: Result<Vec<(syntect::highlighting::Style, &str)>, &str> = Err("regex blew up");
    assert_eq!(line_roles(failed, "let x = 1;"), [(Role::Text, "let x = 1;".to_string())]);

    let keyword = syntect::highlighting::Style {
        foreground: Role::Keyword.marker(),
        ..syntect::highlighting::Style::default()
    };
    let plain = syntect::highlighting::Style::default();
    let fine: Result<_, &str> = Ok(vec![(keyword, "let"), (plain, " x")]);
    let roles = line_roles(fine, "let x");
    assert_eq!(roles, [(Role::Keyword, "let".to_string()), (Role::Text, " x".to_string())]);
}
