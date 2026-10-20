use super::*;

#[test]
fn test_registry_creation() {
    let registry = ServerRegistry::new();
    assert!(registry.is_supported("rust"));
    assert!(registry.is_supported("typescript"));
    assert!(registry.is_supported("python"));
    assert!(registry.is_supported("go"));
}

#[test]
fn test_detect_language() {
    let registry = ServerRegistry::new();
    assert_eq!(registry.detect_language(Path::new("main.rs")), Some("rust".to_string()));
    assert_eq!(registry.detect_language(Path::new("app.ts")), Some("typescript".to_string()));
    assert_eq!(registry.detect_language(Path::new("script.py")), Some("python".to_string()));
    assert_eq!(registry.detect_language(Path::new("main.go")), Some("go".to_string()));
    assert_eq!(registry.detect_language(Path::new("readme.md")), None);
}

#[test]
fn test_register_custom() {
    let mut registry = ServerRegistry::default();
    registry.register(
        "custom".to_string(),
        ServerEntry {
            command: "custom-ls".to_string(),
            root_markers: vec![".customrc".to_string()],
            ..Default::default()
        },
    );
    assert!(registry.is_supported("custom"));
    assert_eq!(registry.get("custom").unwrap().command, "custom-ls");
}

#[test]
fn default_servers_bundle_parses() {
    let defaults: HashMap<String, ServerEntry> =
        serde_json::from_str(DEFAULT_SERVERS_JSON).expect("bundled defaults parse");
    for lang in ["rust", "typescript", "python", "go", "cpp", "bash", "svelte"] {
        let entry = defaults.get(lang).unwrap_or_else(|| panic!("missing {lang}"));
        assert!(!entry.command.is_empty(), "{lang} needs a command");
        assert!(!entry.extensions.is_empty(), "{lang} needs extensions");
        assert!(entry.install_hint.is_some(), "{lang} needs an install hint");
    }
}

#[test]
fn detects_expanded_builtins() {
    let r = ServerRegistry::new();
    assert_eq!(r.detect_language(Path::new("a.js")), Some("typescript".to_string()));
    assert_eq!(r.detect_language(Path::new("a.jsx")), Some("typescript".to_string()));
    assert_eq!(r.detect_language(Path::new("a.cpp")), Some("cpp".to_string()));
    assert_eq!(r.detect_language(Path::new("a.h")), Some("cpp".to_string()));
    assert_eq!(r.detect_language(Path::new("a.sh")), Some("bash".to_string()));
    assert!(r.install_hint("go").unwrap().contains("gopls"));
}

/// A component is not TypeScript, however much of it is TypeScript.
///
/// `.svelte` has to reach `svelteserver` rather than the TypeScript entry: a `tsserver` handed
/// a component parses the markup as expressions and reports errors on every line of it. The
/// two extension sets must therefore stay disjoint, which is also what keeps
/// [`ServerRegistry::detect_language`] — a scan for the first entry that claims the extension —
/// from depending on which entry it happens to reach first.
#[test]
fn a_svelte_component_routes_to_its_own_server() {
    let registry = ServerRegistry::new();
    assert_eq!(registry.detect_language(Path::new("src/App.svelte")), Some("svelte".to_string()));

    let typescript = registry.get("typescript").expect("typescript is registered");
    assert!(
        !typescript.extensions.iter().any(|ext| ext == "svelte"),
        "typescript must not also claim .svelte: {:?}",
        typescript.extensions
    );

    let svelte = registry.get("svelte").expect("svelte is registered");
    assert!(
        svelte.root_markers.iter().any(|marker| marker == "package.json"),
        "a component's root is its package, and not every project has a svelte.config: {:?}",
        svelte.root_markers
    );
    assert!(
        svelte.candidates().iter().any(|c| c.command == "svelte-language-server"),
        "the package installs both names, and which one lands has changed between releases"
    );
}

#[test]
fn config_servers_merge_and_route_by_extension() {
    let mut r = ServerRegistry::new();
    let json = r#"{"ruby":{"command":"ruby-lsp","extensions":["rb"],
            "root_markers":["Gemfile"],"install_hint":"gem install ruby-lsp"}}"#;
    let custom: HashMap<String, ServerEntry> = serde_json::from_str(json).unwrap();
    r.merge_custom(custom);
    assert_eq!(r.detect_language(Path::new("app.rb")), Some("ruby".to_string()));
    assert_eq!(r.install_hint("ruby"), Some("gem install ruby-lsp"));
}

/// The most natural thing a user writes must not delete the language.
///
/// `{"rust": {"command": "rust-analyzer-nightly"}}` means "same as yours, different binary".
/// Merged as a whole entry it meant "no extensions, no root markers", and `.rs` stopped being
/// detected as a language at all — the LSP tools went quiet with nothing to explain why.
#[test]
fn a_config_that_names_only_a_command_keeps_the_rest_of_the_language() {
    let mut registry = ServerRegistry::new();
    let custom: HashMap<String, ServerEntryPatch> =
        serde_json::from_str(r#"{"rust":{"command":"rust-analyzer-nightly"}}"#).unwrap();
    assert!(registry.merge_patches(custom).is_empty(), "the patch applies");

    let entry = registry.get("rust").expect("rust is still registered");
    assert_eq!(entry.command, "rust-analyzer-nightly", "the override takes effect");
    assert!(
        entry.extensions.iter().any(|e| e == "rs"),
        "extensions are inherited, not blanked: {:?}",
        entry.extensions
    );
    assert!(
        entry.root_markers.iter().any(|m| m == "Cargo.toml"),
        "root markers are inherited: {:?}",
        entry.root_markers
    );
    assert_eq!(
        registry.detect_language(Path::new("src/main.rs")).as_deref(),
        Some("rust"),
        "the language is still detected from its extension"
    );
}

/// A patch may still say "nothing", as long as it says so.
#[test]
fn an_explicitly_empty_field_overrides_rather_than_inherits() {
    let mut registry = ServerRegistry::new();
    let custom: HashMap<String, ServerEntryPatch> =
        serde_json::from_str(r#"{"rust":{"args":[],"root_markers":[]}}"#).unwrap();
    registry.merge_patches(custom);
    let entry = registry.get("rust").unwrap();
    assert!(entry.root_markers.is_empty(), "an explicit [] means []");
    assert_eq!(entry.command, "rust-analyzer", "unmentioned fields still inherit");
}

/// A brand-new language works, and a patch that cannot launch anything is reported.
#[test]
fn a_new_language_needs_a_command_and_says_so_when_it_lacks_one() {
    let mut registry = ServerRegistry::new();
    // Languages luxe does not ship, so there is nothing for a patch to inherit a command from.
    // Asserted rather than assumed: this test read `lua` as unshipped until the day it was.
    for unshipped in ["elixir", "cobol"] {
        assert!(!registry.is_supported(unshipped), "{unshipped} must not be a built-in here");
    }
    let custom: HashMap<String, ServerEntryPatch> = serde_json::from_str(
        r#"{"elixir":{"command":"elixir-ls","extensions":["ex","exs"]},
                "cobol":{"extensions":["cbl"]}}"#,
    )
    .unwrap();
    let rejected = registry.merge_patches(custom);

    assert_eq!(rejected, vec!["cobol".to_string()], "a new language with no command is named");
    assert!(registry.is_supported("elixir"), "a complete new language registers");
    assert_eq!(
        registry.detect_language(Path::new("app.exs")).as_deref(),
        Some("elixir"),
        "and routes by extension with no code change"
    );
    assert!(!registry.is_supported("cobol"), "the rejected one is not half-registered");
}
