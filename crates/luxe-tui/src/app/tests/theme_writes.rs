use super::*;

/// Picking a theme with a config that does not parse leaves the file alone and says the write
/// failed. The config may be half-way through an edit; replacing it with `{ "theme": … }` lost
/// everything else in it.
#[tokio::test]
async fn a_theme_pick_never_replaces_a_config_that_does_not_parse() {
    let home = tempfile::tempdir().expect("temp dir");
    let _config_home = crate::test_env::TempConfigHome::at(home.path());
    let path = luxe_core::config::Config::global_path();
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    let half_edited = "{ \"mouse\": false, \"theme\": ";
    std::fs::write(&path, half_edited).expect("seed");

    let (tx, _rx) = mpsc::channel(100);
    let mut app =
        App::new(tx, luxe_core::keymap::KeyMap::defaults(), luxe_core::config::Config::default());
    app.apply_theme_selection("gruvbox");

    assert_eq!(std::fs::read_to_string(&path).expect("read"), half_edited, "left alone");
    assert_eq!(app.config.theme.as_deref(), Some("gruvbox"), "the session still switches");
    let said = app.convo.iter().any(|item| {
        matches!(item.block(), ConvoBlock::System(text) if text.contains("Writing it to your config failed"))
    });
    assert!(said, "the failed write is said out loud");
}
