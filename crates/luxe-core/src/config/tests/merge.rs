//! Merging — how a project's config layers over the user's.

use super::*;

/// A list a project sets replaces the user's whole, and a list it leaves empty keeps the user's,
/// each list on its own.
#[test]
fn a_project_list_replaces_its_own_key_and_no_other() {
    let lists = |files: &[&str], exclude: &[&str], context: &[&str]| {
        let owned = |list: &[&str]| list.iter().map(|item| item.to_string()).collect::<Vec<_>>();
        let mut config = Config::default();
        config.plan.files = owned(files);
        config.review.exclude = owned(exclude);
        config.review.context_files = owned(context);
        config
    };
    let merged = crate::config::merge::Layered::layered(
        lists(&["PLAN.md"], &["global/**"], &["NOTES.md"]),
        lists(&[], &["project/**"], &[]),
    );
    assert_eq!(merged.plan.files, ["PLAN.md"]);
    assert_eq!(merged.review.exclude, ["project/**"]);
    assert_eq!(merged.review.context_files, ["NOTES.md"]);

    let merged = crate::config::merge::Layered::layered(
        lists(&["PLAN.md"], &["global/**"], &["NOTES.md"]),
        lists(&["docs/*.md"], &[], &["DESIGN.md"]),
    );
    assert_eq!(merged.plan.files, ["docs/*.md"]);
    assert_eq!(merged.review.exclude, ["global/**"]);
    assert_eq!(merged.review.context_files, ["DESIGN.md"]);
}

/// Project config overrides global when both exist.
#[test]
fn project_overrides_global() {
    let _xdg_config_home = XDG_CONFIG_HOME_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let config_dir = dir.path().join("luxe");
    let project_dir = dir.path().join(".luxe");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::create_dir_all(&project_dir).unwrap();

    std::fs::write(
        config_dir.join("config.json"),
        r#"{"theme":"nord","show_key_hints":false,"statusline":{"right":["context","model"],"thinking_colors":true},"dock":{"mode":"hidden"}}"#,
    )
    .unwrap();
    std::fs::write(
        project_dir.join("config.json"),
        r#"{"theme":"dracula","statusline":{"right":["thinking","model"],"thinking_colors":false},"dock":{"mode":"shown"}}"#,
    )
    .unwrap();

    // Point config discovery at the temp dir.
    std::env::set_var("XDG_CONFIG_HOME", dir.path());
    let config = Config::load(dir.path());
    assert_eq!(config.theme, Some("dracula".to_string()));
    assert_eq!(config.show_key_hints, Some(false)); // from global, not overridden
    assert_eq!(
        config.statusline.right,
        Some(vec![StatuslineModule::Thinking, StatuslineModule::Model])
    );
    assert_eq!(config.statusline.thinking_colors, Some(false));
    assert_eq!(config.dock.mode(), DockMode::Shown);

    std::env::remove_var("XDG_CONFIG_HOME");
}

/// colors overrides merge key-by-key, project winning per token without dropping global ones.
#[test]
fn color_overrides_merge_key_by_key() {
    let global = Some(HashMap::from([
        ("diffAddBg".to_string(), "#111111".to_string()),
        ("diffDelBg".to_string(), "#222222".to_string()),
    ]));
    let project = Some(HashMap::from([("diffAddBg".to_string(), "#333333".to_string())]));
    let merged = merge_color_overrides(global, project).expect("merged overrides");
    assert_eq!(merged.get("diffAddBg").map(String::as_str), Some("#333333"));
    assert_eq!(merged.get("diffDelBg").map(String::as_str), Some("#222222"));
}

#[test]
fn tools_deny_lists_merge_as_a_union() {
    assert_eq!(
        merge_disabled(vec!["a".into(), "b".into()], vec!["b".into(), "c".into()]),
        vec!["a".to_string(), "b".into(), "c".into()]
    );
}
