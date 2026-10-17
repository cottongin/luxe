use super::*;
use crate::commands::{CommandContext, CommandRegistry};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use luxe_core::keymap::KeyMap;

fn flyout_with(recent: &[&str]) -> SlashFlyoutState {
    let recent: Vec<String> = recent.iter().map(|id| id.to_string()).collect();
    SlashFlyoutState::new(CommandRegistry::all().arrange(
        &CommandContext::default(),
        &KeyMap::defaults(),
        &recent,
    ))
}

fn flyout_on(query: &str) -> SlashFlyoutState {
    let mut flyout = flyout_with(&[]);
    flyout.set_query(query);
    flyout
}

#[test]
fn only_commands_with_a_trigger_are_listed() {
    let state = flyout_with(&[]);
    assert!(state.items.iter().all(|entry| entry.command.slash_name().is_some()));
    assert!(state.items.iter().any(|entry| entry.command.slash_name() == Some("/quit")));
}

/// Typing a command name surfaces the command whose trigger matches, whatever it is called:
/// "/di" is `/diff` even though the row reads "Review changes", and a keyword that also
/// matches (`/lsp`'s "diagnostics") comes after it rather than ahead.
#[test]
fn ranks_by_slash_trigger_not_name() {
    let state = flyout_on("di");
    assert_eq!(state.selected_command().and_then(Command::slash_name), Some("/diff"));
    let triggers: Vec<&str> = state
        .filtered_items()
        .iter()
        .filter_map(|&index| state.items[index].command.slash_name())
        .collect();
    let lsp = triggers.iter().position(|slash| *slash == "/lsp").expect("found by keyword");
    let by_trigger = triggers
        .iter()
        .filter(|slash| !crate::fuzzy::rank("di", &[slash.trim_start_matches('/')]).is_empty())
        .count();
    assert!(
        by_trigger > 0 && lsp >= by_trigger,
        "a keyword never outranks a trigger: {triggers:?}"
    );
    let exit = flyout_on("exit");
    assert_eq!(exit.selected_command().and_then(Command::slash_name), Some("/quit"));
}

/// The same order the palette opens in: the recently used lead, their rule under them, and
/// the rule goes the moment a query takes over the order.
#[test]
fn recently_used_commands_lead_until_you_type() {
    let mut state = flyout_with(&["cmd.cost", "tui.convo.copy_message", "cmd.new"]);
    assert_eq!(state.recent, 2, "Copy message has no trigger, so it is not a slash lead");
    let first: Vec<&str> =
        (0..2).map(|pos| state.items[state.filtered_items()[pos]].command.name.as_ref()).collect();
    assert_eq!(first, ["Session cost", "New session"]);
    assert_eq!(state.divider(), Some(2));
    state.set_query("n");
    assert_eq!(state.divider(), None);
}

#[test]
fn typing_narrows_and_text_keys_go_to_the_composer() {
    let mut state = flyout_with(&[]);
    let all = state.filtered_items().len();
    state.set_query("q");
    assert!(state.filtered_items().len() < all);
    for code in [KeyCode::Char('q'), KeyCode::Backspace] {
        assert_eq!(
            state.on_key(&KeyEvent::new(code, KeyModifiers::NONE)),
            SlashFlyoutOutcome::Edit
        );
    }
}

#[test]
fn enter_dispatches_and_esc_closes() {
    let mut state = flyout_with(&[]);
    assert!(matches!(
        state.on_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        SlashFlyoutOutcome::Dispatch(_)
    ));
    assert_eq!(
        state.on_key(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        SlashFlyoutOutcome::Close
    );
}

#[test]
fn arrows_move_one_row_in_each_direction() {
    let mut state = flyout_with(&[]);
    state.on_key(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let second = state.selected_command().map(|command| command.id.clone());
    state.on_key(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_ne!(second, state.selected_command().map(|command| command.id.clone()));
    assert_eq!(state.nav.selected(), 0);
}

/// Every argument-taking command has a useful bare form, so Enter runs it. Completing instead
/// made the obvious key look broken: `/diff` + Enter printed `"/diff "` and waited.
#[test]
fn enter_runs_the_selected_command_even_when_it_takes_arguments() {
    for query in ["diff", "dock", "statusline", "mouse", "permissions"] {
        let mut flyout = flyout_on(query);
        let selected = flyout.selected_command().cloned().expect("a match");
        assert!(!selected.usage.is_empty(), "{query} is the argument-taking case under test");
        let outcome = flyout.on_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            matches!(outcome, SlashFlyoutOutcome::Dispatch(command) if command.id == selected.id),
            "Enter on /{query} should run it"
        );
    }
}

/// Tab is the completion key, which is where typing an argument starts.
#[test]
fn tab_completes_into_the_composer() {
    let mut flyout = flyout_on("diff");
    let outcome = flyout.on_key(&KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert!(matches!(outcome, SlashFlyoutOutcome::Complete(command) if command.id == "cmd.diff"));
}

fn arguments_of(trigger: &str, typed: &str) -> Option<SlashFlyoutState> {
    let command = CommandRegistry::all()
        .iter()
        .find(|command| command.slash_name() == Some(trigger))
        .cloned()
        .expect("a command");
    SlashFlyoutState::arguments(command, typed)
}

fn forms(state: &SlashFlyoutState) -> Vec<String> {
    let arguments = state.arguments.as_ref().expect("argument mode");
    state.filtered_items().iter().map(|&at| arguments.forms()[at].0.to_string()).collect()
}

/// Past the trigger, the list is the command's forms, narrowed as its arguments are typed.
#[test]
fn past_the_trigger_the_list_is_the_commands_forms() {
    let mut state = arguments_of("/diff", "").expect("diff takes arguments");
    assert_eq!(forms(&state).len(), 6, "every form, the bare one too, before anything is typed");
    assert!(state.set_arguments("p"));
    assert_eq!(forms(&state), ["/diff pr <number or URL>"]);
    assert!(state.set_arguments("pr 1234"));
    assert_eq!(forms(&state), ["/diff pr <number or URL>"], "a value keeps its form");
    assert!(!state.set_arguments("bogus"), "nothing fits, so the caller puts it away");
    assert!(arguments_of("/quit", "").is_none(), "a command with no arguments has no forms");
    assert!(arguments_of("/compact", "keep the API notes").is_some(), "a sentence is a value");
}

/// Enter runs a form that is whole, fills one still waiting on its value, and Tab only fills.
#[test]
fn enter_runs_a_whole_form_and_fills_one_that_is_not() {
    let enter = || KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let tab = || KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);

    let mut turn = arguments_of("/diff", "tu").expect("fits");
    assert!(
        matches!(
            turn.on_key(&enter()),
            SlashFlyoutOutcome::Dispatch(command) if command.args.as_deref() == Some("turn")
        ),
        "a form with nothing to fill runs as written"
    );

    let mut waiting = arguments_of("/diff", "p").expect("fits");
    assert_eq!(waiting.on_key(&enter()), SlashFlyoutOutcome::Fill("/diff pr ".into()));
    assert_eq!(waiting.on_key(&tab()), SlashFlyoutOutcome::Fill("/diff pr ".into()));

    let mut whole = arguments_of("/diff", "pr 12").expect("fits");
    assert!(matches!(
        whole.on_key(&enter()),
        SlashFlyoutOutcome::Dispatch(command) if command.args.as_deref() == Some("pr 12")
    ));
    assert_eq!(whole.on_key(&tab()), SlashFlyoutOutcome::Consumed, "nothing left to fill");

    let mut bare = arguments_of("/diff", "").expect("fits");
    assert!(
        matches!(
            bare.on_key(&enter()),
            SlashFlyoutOutcome::Dispatch(command) if command.args.is_none()
        ),
        "on the bare form, Enter is what it always was"
    );

    let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
    assert_eq!(bare.on_key(&shift_enter), SlashFlyoutOutcome::Edit, "a newline stays a newline");
}

/// The forms list names the command, each form beside what it does.
#[test]
fn the_forms_are_drawn_with_what_each_does() {
    use ratatui::{backend::TestBackend, Terminal};
    let resolved = luxe_core::theme::ThemeRegistry::new().resolve("nord").expect("nord");
    let theme = crate::theme::styles::ThemeStyles::from_resolved(&resolved, "nord");
    let state = arguments_of("/diff", "").expect("fits");
    let mut terminal = Terminal::new(TestBackend::new(100, 20)).expect("terminal");
    let hit = std::cell::RefCell::new(crate::mouse::HitRegions::default());
    terminal
        .draw(|f| {
            render_slash_flyout(f, f.area(), crate::picker::Edge::Above(20), &state, &theme, &hit)
        })
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    let screen: String = (0..20u16)
        .map(|y| (0..100u16).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n")
        .collect();
    assert!(screen.contains("Review changes"), "{screen}");
    assert!(screen.contains("/diff pr <number or URL>"), "{screen}");
    assert!(screen.contains("rebuild the open review"), "{screen}");
    assert!(screen.contains("tab fill in"), "{screen}");
}

/// With room, the pane beside the list says what the command does and how to give it
/// arguments; on a narrow composer the list keeps the whole width.
#[test]
fn the_pane_says_what_and_how_where_there_is_room() {
    use ratatui::{backend::TestBackend, layout::Rect, Terminal};
    let resolved = luxe_core::theme::ThemeRegistry::new().resolve("nord").expect("nord");
    let theme = crate::theme::styles::ThemeStyles::from_resolved(&resolved, "nord");
    let draw = |state: &SlashFlyoutState, width: u16| {
        let hit = std::cell::RefCell::new(crate::mouse::HitRegions::default());
        let mut terminal = Terminal::new(TestBackend::new(width, 20)).expect("terminal");
        terminal
            .draw(|f| {
                render_slash_flyout(
                    f,
                    Rect::new(0, 0, width, 20),
                    crate::picker::Edge::Above(20),
                    state,
                    &theme,
                    &hit,
                )
            })
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..20)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let state = flyout_on("dock");
    let wide = draw(&state, 120);
    assert!(wide.contains("Shows or hides the sidebar."), "{wide}");
    assert!(wide.contains("/dock left"), "the argument forms: {wide}");
    let narrow = draw(&state, 70);
    assert!(!narrow.contains("/dock left"), "no pane on a narrow composer: {narrow}");
    assert!(narrow.contains("Context Dock"));
    // The keys under it, like every picker's, Tab named for what it does here
    assert!(wide.contains("enter run · tab add arguments"), "{wide}");
    assert!(draw(&flyout_on("zzzz"), 120).contains("esc close"), "nothing matched");
}

/// The letters of the trigger a query matched are kept beside the filter, for the row to
/// light; a command found by a keyword lights nothing, since that word is not on its row.
#[test]
fn the_matched_trigger_letters_are_kept_for_each_row() {
    let mut flyout = flyout_with(&[]);
    flyout.set_query("dif");
    let first = flyout.filtered_items()[0];
    assert_eq!(flyout.items[first].command.slash_name(), Some("/diff"));
    assert_eq!(flyout.matched_at(0), &[0, 1, 2]);
    flyout.set_query("");
    assert!(flyout.matched_at(0).is_empty(), "no query, nothing lit");
}

/// On screen, the row names its trigger and lights the letters typed, never its slash.
#[test]
fn a_row_lights_the_typed_letters_of_its_trigger() {
    use ratatui::{backend::TestBackend, Terminal};
    let resolved = luxe_core::theme::ThemeRegistry::new().resolve("nord").expect("nord");
    let theme = crate::theme::styles::ThemeStyles::from_resolved(&resolved, "nord");
    let mut flyout = flyout_with(&[]);
    flyout.set_query("dif");
    let mut terminal = Terminal::new(TestBackend::new(80, 16)).expect("terminal");
    let hit = std::cell::RefCell::new(crate::mouse::HitRegions::default());
    terminal
        .draw(|f| {
            render_slash_flyout(f, f.area(), crate::picker::Edge::Above(16), &flyout, &theme, &hit)
        })
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    let line_at = |y: u16| (0..80u16).map(|x| buffer[(x, y)].symbol()).collect::<String>();
    let row = (0..16u16).find(|&y| line_at(y).contains("/diff")).expect("the trigger is drawn");
    let line = line_at(row);
    let slash = line.chars().position(|c| c == '/').expect("a slash") as u16;
    let bold = |x: u16| buffer[(x, row)].modifier.contains(ratatui::style::Modifier::BOLD);
    assert!(!bold(slash), "the slash is not the trigger's");
    assert!((slash + 1..slash + 4).all(bold), "`dif` is lit: {line}");
    assert!(!bold(slash + 4), "the second `f` was not typed");
}
