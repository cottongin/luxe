use crate::app::App;
use crate::fixture::{seeded_app, FixtureScenario};
use crate::model_picker::{ModelEntry, ModelPickerState};
use crate::mouse::{MouseTarget, OverlayList, ScrollRegionId};
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use luxe_core::provider::{ModelInfo, ModelRef};
use ratatui::{backend::TestBackend, layout::Rect, Terminal};

/// An app with the model picker open over `count` models, drawn once so its regions are published.
fn picker_over(count: usize) -> App {
    let mut app = seeded_app("nord", FixtureScenario::ChatIdle).expect("a seeded app");
    let models = (0..count)
        .map(|index| ModelEntry {
            info: ModelInfo {
                r#ref: ModelRef::new("p", format!("m{index:02}")),
                ..Default::default()
            },
            label: format!("Model {index:02}"),
            configured: true,
            unverified: false,
        })
        .collect();
    app.overlays.model_picker = Some(ModelPickerState::open(models, None));
    draw(&mut app);
    app
}

fn draw(app: &mut App) {
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).expect("a terminal");
    terminal.draw(|frame| crate::view::render(frame, app)).expect("a frame");
}

/// Routes one mouse event at `(column, row)` the way the event loop does, and applies it.
fn mouse(app: &mut App, kind: MouseEventKind, (column, row): (u16, u16)) {
    let event = MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE };
    match crate::main_loop::route_mouse(app, &event) {
        crate::main_loop::RouteAction::Msg(message) => app.update(*message),
        crate::main_loop::RouteAction::None => {}
        other => panic!("a modal list should take the gesture itself: {other:?}"),
    }
}

/// The first published click target `pick` accepts, with its rect.
fn target(app: &App, pick: impl Fn(&MouseTarget) -> bool) -> Option<(Rect, MouseTarget)> {
    let hit = app.hit.borrow();
    let found =
        hit.clicks().find(|(_, target)| pick(target)).map(|(rect, target)| (rect, target.clone()));
    found
}

fn models_cursor(app: &App) -> usize {
    app.overlay_cursor(OverlayList::Models).expect("the picker is open").0
}

/// A cell on the picker's first row.
fn first_row(app: &App) -> (u16, u16) {
    let (rect, _) = target(app, |it| *it == MouseTarget::ModelRow(0)).expect("a first row");
    (rect.x + 2, rect.y)
}

/// The wheel over the picker walks its cursor a row a notch, and stops at the top rather than
/// wrapping to the bottom the way the arrows do.
///
/// The break this catches: no region published, so a notch fell through to the transcript behind
/// the modal and the list never moved.
#[test]
fn the_wheel_walks_the_model_picker_and_stops_at_its_ends() {
    let mut app = picker_over(40);
    let at = first_row(&app);
    mouse(&mut app, MouseEventKind::ScrollDown, at);
    mouse(&mut app, MouseEventKind::ScrollDown, at);
    assert_eq!(models_cursor(&app), 2, "two notches down, two rows down");
    for _ in 0..3 {
        mouse(&mut app, MouseEventKind::ScrollUp, at);
    }
    assert_eq!(models_cursor(&app), 0, "the top holds instead of wrapping to the bottom");
}

/// A long list draws a bar in a column of its own, and a press low on the bar takes the cursor to
/// the end of the list.
#[test]
fn the_model_pickers_bar_takes_a_column_and_steers_the_cursor() {
    let mut app = picker_over(40);
    let region = ScrollRegionId::OverlayList(OverlayList::Models);
    let (lane, _) =
        target(&app, |it| matches!(it, MouseTarget::Scrollbar { region: r, .. } if *r == region))
            .expect("a long list draws its bar");
    let (row, _) = target(&app, |it| *it == MouseTarget::ModelRow(0)).expect("a first row");
    assert!(row.right() <= lane.x, "rows stop short of the bar: {row:?} vs {lane:?}");

    let bottom = (lane.x, lane.bottom() - 1);
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), bottom);
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), bottom);
    assert_eq!(models_cursor(&app), 39, "the bottom of the bar is the end of the list");
}

/// A list that fits draws no bar, and the wheel over it still moves its cursor rather than the
/// transcript behind it.
#[test]
fn a_short_list_draws_no_bar_but_still_takes_the_wheel() {
    let mut app = picker_over(4);
    assert!(target(&app, |it| matches!(it, MouseTarget::Scrollbar { .. })).is_none());
    let at = first_row(&app);
    mouse(&mut app, MouseEventKind::ScrollDown, at);
    assert_eq!(models_cursor(&app), 1);
}

/// The wheel over the theme picker previews what it lands on, as arrowing there does.
#[test]
fn the_wheel_over_the_theme_picker_previews_the_theme() {
    let mut app = seeded_app("nord", FixtureScenario::ChatIdle).expect("a seeded app");
    let themes = vec!["nord".to_string(), "dracula".to_string(), "gruvbox".to_string()];
    app.overlays.theme_picker = Some(crate::theme_picker::ThemePickerState::open(themes, "nord"));
    draw(&mut app);
    let nord = app.theme_styles.accent;
    let (rect, _) = target(&app, |it| *it == MouseTarget::ThemeRow(0)).expect("a theme row");
    mouse(&mut app, MouseEventKind::ScrollDown, (rect.x + 2, rect.y));
    assert_eq!(app.overlay_cursor(OverlayList::Themes), Some((1, 3)));
    assert_ne!(app.theme_styles.accent, nord, "landing on a theme previews it");
}

/// Setup's theme step takes the wheel through the configuration surface, and repaints in the theme
/// it lands on the way the arrows do.
#[test]
fn the_wheel_over_setups_themes_previews_each_one() {
    let mut wheeled = seeded_app("nord", FixtureScenario::Setup).expect("a seeded app");
    draw(&mut wheeled);
    let before = wheeled.theme_styles.clone();
    let (rect, _) = target(&wheeled, |it| *it == MouseTarget::ConfigRow(0)).expect("a theme row");
    // Two rows, since the first one down is the theme already showing
    for _ in 0..2 {
        mouse(&mut wheeled, MouseEventKind::ScrollDown, (rect.x + 2, rect.y));
    }

    let mut arrowed = seeded_app("nord", FixtureScenario::Setup).expect("a seeded app");
    let down = crossterm::event::KeyEvent::new(crossterm::event::KeyCode::Down, KeyModifiers::NONE);
    for _ in 0..2 {
        match crate::main_loop::route_key(&mut arrowed, &down) {
            crate::main_loop::RouteAction::Configuration(outcome) => {
                arrowed.apply_configuration_outcome(outcome)
            }
            other => panic!("setup takes the arrow itself: {other:?}"),
        }
    }
    let cursor = |app: &App| app.overlay_cursor(OverlayList::Configuration);
    assert_eq!(cursor(&wheeled), cursor(&arrowed), "a notch moves as far as the arrow");
    assert_ne!(wheeled.theme_styles, before, "and the app repaints");
    assert_eq!(wheeled.theme_styles, arrowed.theme_styles, "in the same theme the arrow shows");
}

/// The login wizard's provider list takes the wheel too, and only while it is on that list.
#[test]
fn the_wheel_walks_the_login_providers() {
    let mut app = seeded_app("nord", FixtureScenario::ChatIdle).expect("a seeded app");
    crate::fixture::seed_login_wizard(&mut app, crate::fixture::LoginFixture::Providers);
    draw(&mut app);
    let (rect, _) =
        target(&app, |it| *it == MouseTarget::AuthProviderRow(0)).expect("a provider row");
    mouse(&mut app, MouseEventKind::ScrollDown, (rect.x + 2, rect.y));
    assert_eq!(app.overlay_cursor(OverlayList::Providers).map(|(cursor, _)| cursor), Some(1));

    let mut methods = seeded_app("nord", FixtureScenario::ChatIdle).expect("a seeded app");
    crate::fixture::seed_login_wizard(&mut methods, crate::fixture::LoginFixture::Methods);
    assert_eq!(methods.overlay_cursor(OverlayList::Providers), None, "a method list is not it");
}
