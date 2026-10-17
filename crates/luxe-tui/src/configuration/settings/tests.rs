#![allow(clippy::unwrap_used)]

use super::*;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn state(global: Value, project: Option<Value>) -> SettingsState {
    SettingsState::open(
        ConfigLayers { global: Some(global), project },
        "luxe".into(),
        PathBuf::from("/home/dev/.config/luxe/config.json"),
    )
}

fn cursor_on(state: &mut SettingsState, wanted: Row) {
    let rows = state.rows();
    let at = rows.iter().position(|row| *row == wanted).expect("the row is on this page");
    state.cursor = at;
}

#[test]
fn a_toggle_writes_its_flip_and_a_flip_back_to_the_default_removes_the_key() {
    let mut settings = state(serde_json::json!({}), None).open_on(Page::Layout);
    let mouse = luxe_core::settings::find(&["mouse"]).unwrap();
    cursor_on(&mut settings, Row::Setting(mouse));
    assert_eq!(
        settings.on_key(&key(KeyCode::Enter)),
        ConfigOutcome::Write(SettingChange::Set { path: &["mouse"], value: false.into() })
    );
    settings.refresh(
        ConfigLayers { global: Some(serde_json::json!({ "mouse": false })), project: None },
        "luxe".into(),
    );
    assert_eq!(
        settings.on_key(&key(KeyCode::Char(' '))),
        ConfigOutcome::Write(SettingChange::Reset { path: &["mouse"] })
    );
}

#[test]
fn a_choice_steps_both_ways_from_the_value_in_force() {
    let mut settings =
        state(serde_json::json!({ "dock": { "mode": "shown" } }), None).open_on(Page::Layout);
    let dock = luxe_core::settings::find(&["dock", "mode"]).unwrap();
    cursor_on(&mut settings, Row::Setting(dock));
    assert_eq!(
        settings.on_key(&key(KeyCode::Right)),
        ConfigOutcome::Write(SettingChange::Set {
            path: &["dock", "mode"],
            value: "hidden".into()
        })
    );
    assert_eq!(
        settings.on_key(&key(KeyCode::Left)),
        ConfigOutcome::Write(SettingChange::Reset { path: &["dock", "mode"] }),
        "auto is the default, so stepping back to it removes the key"
    );
}

/// A project override would quietly win over anything written here, so the surface refuses
/// and says where the value lives instead of writing a change nobody would see.
#[test]
fn a_value_the_project_sets_is_explained_rather_than_changed() {
    let project = serde_json::json!({ "bell": true, "tools": { "disabled": ["debug"] } });
    let mut settings = state(serde_json::json!({}), Some(project)).open_on(Page::Agent);
    cursor_on(&mut settings, Row::Setting(luxe_core::settings::find(&["bell"]).unwrap()));
    assert_eq!(settings.on_key(&key(KeyCode::Enter)), ConfigOutcome::Consumed);
    assert!(settings.notice.as_ref().unwrap().text.contains(".luxe/config.json"));

    settings.select_page(3);
    let debug = luxe_core::tool_catalog::get("debug").unwrap();
    cursor_on(&mut settings, Row::Tool(debug));
    assert_eq!(settings.on_key(&key(KeyCode::Enter)), ConfigOutcome::Consumed);
    assert_eq!(settings.tool_state(debug), (Layer::Project, false));
}

#[test]
fn a_tool_toggles_and_reset_turns_one_the_reader_disabled_back_on() {
    let global = serde_json::json!({ "tools": { "disabled": ["screenshot"] } });
    let mut settings = state(global, None).open_on(Page::Tools);
    let screenshot = luxe_core::tool_catalog::get("screenshot").unwrap();
    cursor_on(&mut settings, Row::Tool(screenshot));
    assert_eq!(settings.tool_state(screenshot), (Layer::Global, false));
    assert_eq!(
        settings.on_key(&key(KeyCode::Char('r'))),
        ConfigOutcome::Write(SettingChange::Tool { name: "screenshot", enabled: true })
    );
    let grep = luxe_core::tool_catalog::get("grep").unwrap();
    cursor_on(&mut settings, Row::Tool(grep));
    assert_eq!(
        settings.on_key(&key(KeyCode::Enter)),
        ConfigOutcome::Write(SettingChange::Tool { name: "grep", enabled: false })
    );
}

/// The essentials are named on the page so their absence from the toggles is not a mystery,
/// and the cursor never rests on them or on a heading.
#[test]
fn the_tools_page_names_the_essentials_and_the_cursor_skips_what_is_not_a_control() {
    let mut settings = state(serde_json::json!({}), None).open_on(Page::Tools);
    assert_eq!(settings.rows().first(), Some(&Row::Essentials));
    for _ in 0..40 {
        assert!(settings.rows()[settings.cursor].selectable());
        settings.on_key(&key(KeyCode::Down));
    }
    let last = settings.rows().len() - 1;
    assert_eq!(settings.cursor, last, "the cursor stops on the last control");
}

#[test]
fn tab_moves_through_the_pages_and_wraps() {
    let mut settings = state(serde_json::json!({}), None);
    assert_eq!(settings.page(), Page::Appearance);
    settings.on_key(&key(KeyCode::Tab));
    assert_eq!(settings.page(), Page::Conversation);
    settings.on_key(&KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    settings.on_key(&KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(settings.page(), Page::Agent, "back from the first page is the last");
    settings.on_key(&key(KeyCode::Char('4')));
    assert_eq!(settings.page(), Page::Tools, "the pages are numbered");
}

#[test]
fn searching_finds_settings_and_tools_across_pages() {
    let mut settings = state(serde_json::json!({}), None);
    for code in [KeyCode::Char('/'), KeyCode::Char('w'), KeyCode::Char('r'), KeyCode::Char('a')] {
        settings.on_key(&key(code));
    }
    let rows = settings.rows();
    let wrap = luxe_core::settings::find(&["diff", "word_wrap"]).unwrap();
    assert!(rows.contains(&Row::Setting(wrap)), "a setting from another page");
    assert!(rows.iter().all(|row| row.selectable()), "a search is a flat list");
    settings.on_key(&key(KeyCode::Esc));
    assert_eq!(settings.page(), Page::Appearance, "esc puts the page back");
    assert!(settings.query.is_none());
}

#[test]
fn the_theme_opens_its_picker_and_e_opens_the_file() {
    let mut settings = state(serde_json::json!({}), None);
    assert_eq!(settings.on_key(&key(KeyCode::Enter)), ConfigOutcome::PickTheme);
    assert_eq!(
        settings.on_key(&key(KeyCode::Char('e'))),
        ConfigOutcome::EditFile(PathBuf::from("/home/dev/.config/luxe/config.json"))
    );
}

/// A theme the project sets is explained like any other project-set value, not opened in a picker
/// whose choice the project would quietly override; the reader's own theme still opens it.
#[test]
fn a_project_set_theme_is_explained_rather_than_picked() {
    let theme = Row::Setting(luxe_core::settings::find(&["theme"]).unwrap());
    let project = serde_json::json!({ "theme": "nord" });
    let mut settings = state(serde_json::json!({}), Some(project)).open_on(Page::Appearance);
    cursor_on(&mut settings, theme);
    assert_eq!(settings.on_key(&key(KeyCode::Enter)), ConfigOutcome::Consumed);
    assert!(settings.notice.as_ref().unwrap().text.contains(".luxe/config.json"));

    let mut own = state(serde_json::json!({ "theme": "nord" }), None).open_on(Page::Appearance);
    cursor_on(&mut own, theme);
    assert_eq!(own.on_key(&key(KeyCode::Enter)), ConfigOutcome::PickTheme);
}

/// Too small to draw, the panel says so, and its unseen controls change nothing until it fits.
#[test]
fn a_panel_too_small_to_draw_says_so_and_holds_its_keys() {
    use ratatui::{backend::TestBackend, Terminal};
    let mut settings = state(serde_json::json!({}), None).open_on(Page::Layout);
    let mouse = luxe_core::settings::find(&["mouse"]).unwrap();
    cursor_on(&mut settings, Row::Setting(mouse));
    let styles = crate::theme::styles::ThemeStyles::from_resolved(
        &luxe_core::theme::ThemeRegistry::new().resolve("nord").unwrap(),
        "nord",
    );
    let hit = std::cell::RefCell::new(crate::mouse::HitRegions::default());
    let draw = |settings: &SettingsState, height: u16| {
        let mut terminal = Terminal::new(TestBackend::new(80, height)).unwrap();
        terminal.draw(|f| settings.view(f, f.area(), &styles, &hit)).unwrap();
        terminal.backend().buffer().content.iter().map(|c| c.symbol()).collect::<String>()
    };
    let text = draw(&settings, 9);
    assert!(text.contains("Make the window bigger"), "{text}");
    assert_eq!(settings.on_key(&key(KeyCode::Enter)), ConfigOutcome::Consumed, "no write");

    draw(&settings, 20);
    assert!(matches!(settings.on_key(&key(KeyCode::Enter)), ConfigOutcome::Write(_)), "fits again");
}
