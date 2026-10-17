use super::*;
use crate::commands::{CommandContext, CommandRegistry, Surface};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use luxe_core::keymap::KeyMap;

fn menu_with(context: &CommandContext, recent: &[&str]) -> Menu {
    let recent: Vec<String> = recent.iter().map(|id| id.to_string()).collect();
    CommandRegistry::all().arrange(context, &KeyMap::defaults(), &recent)
}

fn palette() -> PaletteState {
    PaletteState::open(menu_with(&CommandContext::default(), &[]))
}

fn theme() -> crate::theme::styles::ThemeStyles {
    let resolved = luxe_core::theme::ThemeRegistry::new().resolve("nord").expect("nord");
    crate::theme::styles::ThemeStyles::from_resolved(&resolved, "nord")
}

fn typed(state: &mut PaletteState, text: &str) {
    for character in text.chars() {
        state.on_key(&KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
}

fn names(state: &PaletteState) -> Vec<String> {
    (0..state.filtered_items().len())
        .filter_map(|index| state.entry_at(index))
        .map(|entry| entry.command.name.to_string())
        .collect()
}

/// Draws the palette into a `width` × `height` buffer and returns it with its rows as text.
fn drawn(state: &PaletteState, width: u16, height: u16) -> (ratatui::buffer::Buffer, Vec<String>) {
    use ratatui::{backend::TestBackend, layout::Rect, Terminal};
    crate::glyphs::set_tier(crate::glyphs::GlyphTier::Nerd);
    let theme = theme();
    let hit = std::cell::RefCell::new(crate::mouse::HitRegions::default());
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| render_palette(frame, Rect::new(0, 0, width, height), state, &theme, &hit))
        .expect("render");
    let buffer = terminal.backend().buffer().clone();
    let rows = (0..height).map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect()).collect();
    (buffer, rows)
}

#[test]
fn open_lists_the_whole_menu_on_its_first_row() {
    let menu = menu_with(&CommandContext::default(), &[]);
    let count = menu.entries.len();
    let state = PaletteState::open(menu);
    assert_eq!(state.filtered_items().len(), count);
    assert_eq!(state.selected(), 0);
    assert!(state.is_open());
}

/// The complaint, measured: typing a command's exact name used to return eight rows, seven of them
/// matched through scattered letters in long descriptions. Names do not carry sentences any more,
/// and the summary is not searched.
#[test]
fn typing_a_commands_name_finds_it_and_little_else() {
    let mut state = palette();
    typed(&mut state, "auth");
    let found = names(&state);
    assert_eq!(found.first().map(String::as_str), Some("Credentials"), "{found:?}");
    assert!(found.len() <= 2, "a name should not match half the list: {found:?}");

    let mut state = palette();
    typed(&mut state, "hunk");
    assert!(names(&state).is_empty(), "nothing is about hunks in a conversation");
}

/// Keywords find a command by a word that is not in its name.
#[test]
fn a_keyword_finds_a_command() {
    let mut state = palette();
    typed(&mut state, "exit");
    assert_eq!(names(&state).first().map(String::as_str), Some("Quit"));
}

/// A row found through its slash trigger lights the trigger, rather than drawing as though
/// nothing matched.
#[test]
fn a_match_in_the_trigger_is_lit() {
    let mut state = palette();
    typed(&mut state, "auth");
    let entry = state.selected_entry().expect("a match").clone();
    let matched = state.matched_at(state.selected());
    let name = entry.command.name.chars().count();
    assert!(
        matched.iter().any(|&position| position > name),
        "the /auth trigger holds the match: {matched:?}"
    );
    let theme = theme();
    let line = row(&entry, &matched, true, 60, &theme);
    let lit = theme.accent.add_modifier(ratatui::style::Modifier::BOLD);
    let lit_text: String = line
        .spans
        .iter()
        .filter(|span| {
            span.style.fg == lit.fg
                && span.style.add_modifier.contains(ratatui::style::Modifier::BOLD)
        })
        .map(|span| span.content.as_ref())
        .collect();
    assert!(lit_text.contains("auth"), "the lit characters spell the query: {lit_text:?}");
}

#[test]
fn typing_narrows_and_backspace_widens() {
    let mut state = palette();
    let all = state.filtered_items().len();
    typed(&mut state, "q");
    assert!(state.filtered_items().len() < all);
    state.on_key(&KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(state.filtered_items().len(), all);
}

#[test]
fn arrows_move_one_row_and_wrap() {
    let mut state = palette();
    let count = state.filtered_items().len();
    state.on_key(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    state.on_key(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(state.selected(), 2);
    state.on_key(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(state.selected(), 1);
    state.select(0);
    state.on_key(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(state.selected(), count - 1, "up from the top wraps");
    state.on_key(&KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
    assert_eq!(state.selected(), 0, "and down from the bottom");
}

#[test]
fn printable_j_and_k_filter_the_query() {
    let mut state = palette();
    typed(&mut state, "jk");
    assert_eq!(state.query(), "jk");
    assert_eq!(state.selected(), 0);
}

#[test]
fn enter_dispatches_the_selected_command_and_esc_closes() {
    let mut state = palette();
    let first = state.accept().expect("a command");
    assert_eq!(
        state.on_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        PaletteOutcome::Dispatch(Box::new(first))
    );
    assert_eq!(
        state.on_key(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        PaletteOutcome::Close
    );
    typed(&mut state, "zzzq");
    assert!(state.accept().is_none());
    assert_eq!(
        state.on_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        PaletteOutcome::Consumed,
        "nothing to run is not a dispatch"
    );
}

/// The rule under the recent lead shows only unfiltered, only with rows on both sides, and is
/// never a row the cursor can stand on.
#[test]
fn the_recent_rule_is_drawn_only_unfiltered_and_is_never_selected() {
    let mut state =
        PaletteState::open(menu_with(&CommandContext::default(), &["cmd.cost", "cmd.new"]));
    assert_eq!(state.divider(), Some(2));
    assert_eq!(names(&state)[..2], ["Session cost", "New session"]);
    typed(&mut state, "n");
    assert_eq!(state.divider(), None, "a query orders by match, and the rule would lie");

    let window = Window::new(10, Some(2), 2, 5);
    let slots: Vec<Slot> = window.slots().collect();
    assert_eq!(slots, [Slot::Row(0), Slot::Row(1), Slot::Rule, Slot::Row(2), Slot::Row(3)]);
    let scrolled: Vec<Slot> = Window::new(10, Some(2), 9, 5).slots().collect();
    assert_eq!(scrolled.last(), Some(&Slot::Row(9)), "the cursor stays on screen");
    assert!(!scrolled.contains(&Slot::Rule), "and the rule scrolls away with the lead");
    let plain: Vec<Slot> = Window::new(3, None, 0, 5).slots().collect();
    assert_eq!(plain, [Slot::Row(0), Slot::Row(1), Slot::Row(2)]);

    let none = PaletteState::open(menu_with(&CommandContext::default(), &[]));
    assert_eq!(none.divider(), None, "nothing used yet, nothing to divide");
}

/// With room, the pane beside the list says what the command under the cursor does, and follows
/// the cursor. Without it, the list takes the width and the summary gets a row of its own.
#[test]
fn the_detail_pane_follows_the_cursor_and_gives_way_when_narrow() {
    let mut state = palette();
    typed(&mut state, "diff");
    assert_eq!(state.selected_entry().map(|e| e.command.name.as_ref()), Some("Review changes"));
    let (_, wide) = drawn(&state, 140, 30);
    let text = wide.join("\n");
    assert!(text.contains("Opens a Guided Review"), "{text}");
    assert!(text.contains("Usage"), "{text}");
    assert!(text.contains("/diff commit <oid>"), "{text}");
    let rule_rows = wide.iter().filter(|row| row.contains('│')).count();
    assert!(rule_rows > 5, "a rule divides the panes");

    state.on_key(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let other = state.selected_entry().expect("a second match").command.summary.to_string();
    let (_, moved) = drawn(&state, 140, 30);
    let moved = moved.join("\n");
    assert!(!moved.contains("Opens a Guided Review"), "the pane follows the cursor");
    assert!(moved.contains(&other[..20]), "{moved}");

    let (_, narrow) = drawn(&state, 60, 24);
    let narrow = narrow.join("\n");
    assert!(!narrow.contains("Usage"), "no pane at sixty columns");
    assert!(narrow.contains(&other[..12]), "the summary keeps a row: {narrow}");
}

/// The key bound to a command rides at the right edge of its row.
#[test]
fn a_rows_key_sits_at_its_right_edge() {
    let mut state = palette();
    typed(&mut state, "context dock");
    let (_, rows) = drawn(&state, 140, 30);
    let row = rows.iter().find(|row| row.contains("Context Dock  /dock")).expect("the row");
    let dock = row.find("/dock").expect("trigger");
    let key = row.find("alt+d").expect("the bound key is shown");
    assert!(key > dock + 8, "pushed right, not tucked after the trigger: {row:?}");
}

/// The chosen command is cut from the same slab as every other picker and the dashboard menu,
/// and the label sits at the column it sits at on an unchosen row.
#[test]
fn the_chosen_command_is_cut_into_a_slab_without_moving_its_label() {
    let theme = theme();
    let selected_bg = theme.selected_bg.bg.expect("nord defines a selection background");
    let mut state = palette();
    let (first, _) = drawn(&state, 70, 20);
    let row = (0..20u16)
        .find(|&y| (0..70u16).any(|x| first[(x, y)].symbol() == "❯"))
        .expect("the selected row draws a cursor");
    let cursor_column = (0..70u16).find(|&x| first[(x, row)].symbol() == "❯").expect("found above");
    let inset = crate::widgets::SELECTION_FRAME as u16 / 2;
    assert_eq!(first[(cursor_column - inset, row)].fg, selected_bg, "the row opens on a cap");
    assert_eq!(first[(cursor_column - 1, row)].bg, selected_bg, "and the pad inside it is filled");
    assert_eq!(first[(cursor_column + 1, row)].bg, selected_bg, "and fills behind the label");

    state.on_key(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let (second, _) = drawn(&state, 70, 20);
    assert_ne!(second[(cursor_column, row)].symbol(), "❯", "the cursor moved off this row");
    let label_at = |buffer: &ratatui::buffer::Buffer| {
        (0..70u16)
            .find(|&x| buffer[(x, row)].symbol().chars().next().is_some_and(char::is_alphabetic))
    };
    assert_eq!(label_at(&first), label_at(&second), "the label must not shift");
}

/// Each drawn row is a click target for its filtered position, and the rule is not one.
#[test]
fn rows_are_click_targets_and_the_rule_is_not() {
    use ratatui::{backend::TestBackend, layout::Rect, Terminal};
    let state = PaletteState::open(menu_with(&CommandContext::default(), &["cmd.new"]));
    let theme = theme();
    let hit = std::cell::RefCell::new(crate::mouse::HitRegions::default());
    let mut terminal = Terminal::new(TestBackend::new(140, 30)).expect("terminal");
    terminal
        .draw(|frame| render_palette(frame, Rect::new(0, 0, 140, 30), &state, &theme, &hit))
        .expect("render");
    let buffer = terminal.backend().buffer().clone();
    let row_of = |needle: &str| {
        (0..30u16)
            .find(|&y| {
                (0..140u16).map(|x| buffer[(x, y)].symbol()).collect::<String>().contains(needle)
            })
            .unwrap_or_else(|| panic!("{needle}"))
    };
    let first = row_of("New session");
    let x = (0..140u16).find(|&x| buffer[(x, first)].symbol() == "N").expect("label");
    let regions = hit.borrow();
    assert_eq!(regions.at(x, first), Some(&crate::mouse::MouseTarget::PaletteRow(0)));
    assert_eq!(regions.at(x, first + 1), None, "the rule under the lead is not a row");
    assert_eq!(regions.at(x, first + 2), Some(&crate::mouse::MouseTarget::PaletteRow(1)));
}

/// In a review the palette lists the review's commands, and nothing that only means something in
/// the conversation.
#[test]
fn a_review_palette_is_the_reviews() {
    let review = CommandContext { surface: Surface::Review, ..CommandContext::default() };
    let mut state = PaletteState::open(menu_with(&review, &[]));
    typed(&mut state, "session");
    let found = names(&state);
    assert!(!found.iter().any(|name| name == "New session"), "{found:?}");
    let mut state = PaletteState::open(menu_with(&review, &[]));
    typed(&mut state, "finish");
    assert_eq!(names(&state).first().map(String::as_str), Some("Finish review"));
}

/// Ctrl+N and Ctrl+P move; the bare letters are text like any other, and a chord with some other
/// modifier edits nothing.
#[test]
fn only_the_control_chords_move_and_other_chords_do_not_type() {
    let mut state = palette();
    typed(&mut state, "np");
    assert_eq!(state.query(), "np", "bare n and p are text");
    let mut state = palette();
    state.on_key(&KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
    assert_eq!((state.selected(), state.query()), (1, ""));
    state.on_key(&KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
    assert_eq!((state.selected(), state.query()), (0, ""));
    state.on_key(&KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT));
    assert_eq!(state.query(), "", "alt+x is not a letter to search for");
    assert!(state.is_open());
    state.close();
    assert!(!state.is_open());
    assert_eq!(
        state.on_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        PaletteOutcome::Consumed,
        "a closed palette runs nothing"
    );
}

/// A rule needs something under it: a menu that is all recent draws none.
#[test]
fn a_menu_that_is_all_recent_draws_no_rule() {
    let menu = menu_with(&CommandContext::default(), &["cmd.cost", "cmd.new"]);
    let recent = menu.filtered(|command| ["cmd.cost", "cmd.new"].contains(&command.id.as_ref()));
    assert_eq!((recent.entries.len(), recent.recent), (2, 2));
    assert_eq!(PaletteState::open(recent).divider(), None);
}

/// One cursor, on the selected row; the modal sits inside the terminal with a margin, centered;
/// every row shows its whole name, trigger and key; and a terminal too small for it draws nothing.
#[test]
fn the_modal_fits_its_terminal_and_every_row_reads_whole() {
    let mut state = PaletteState::open(menu_with(&CommandContext::default(), &[]));
    state.on_key(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    // 88 is the band where a pane would fit and the rows beside it would not
    for (width, height) in [(60u16, 30u16), (88, 30), (100, 30), (140, 40), (220, 50)] {
        let (_, rows) = drawn(&state, width, height);
        let cursors: Vec<usize> =
            rows.iter().enumerate().filter(|(_, row)| row.contains('❯')).map(|(y, _)| y).collect();
        assert_eq!(cursors.len(), 1, "{width}: one cursor");
        let selected = state.selected_entry().expect("selected").command.name.to_string();
        assert!(rows[cursors[0]].contains(&selected), "{width}: on the selected row");

        let top = rows.iter().position(|row| row.contains('╭')).expect("frame");
        let left = rows[top].chars().position(|c| c == '╭').expect("corner");
        let right = rows[top].chars().position(|c| c == '╮').expect("corner");
        assert!(left >= 2 && right + 2 < usize::from(width), "{width}: a margin either side");
        let slack = usize::from(width) - 1 - right;
        assert!(left.abs_diff(slack) <= 1, "{width}: centered ({left} vs {slack})");

        // The list's column: between the frame and the rule before the pane, when there is one
        let lists: Vec<String> =
            rows.iter().map(|row| row.split('│').nth(1).unwrap_or_default().to_owned()).collect();
        for index in 0..state.filtered_items().len() {
            let entry = state.entry_at(index).expect("listed");
            let name = format!(" {}", entry.command.name);
            let Some(row) = lists.iter().find(|row| row.contains(&name)) else {
                continue; // scrolled out of the window
            };
            if let Some(slash) = entry.command.slash_name() {
                assert!(
                    row.contains(&format!("{}  {slash}", entry.command.name)),
                    "{width}: {row:?}"
                );
            }
            if let Some(chord) = entry.chord.as_deref() {
                assert!(row.contains(chord), "{width}: {chord} is clipped from {row:?}");
            }
        }
    }
    let (buffer, _) = drawn(&state, 27, 30);
    assert!(buffer.content.iter().all(|cell| cell.symbol() == " "), "too narrow draws nothing");
    let (buffer, _) = drawn(&state, 80, 7);
    assert!(buffer.content.iter().all(|cell| cell.symbol() == " "), "too short draws nothing");
}

/// The palette and the slash flyout divide the same way: the rule on the middle column of what is
/// inside the frame, at every width that has room for a pane. The flyout used to give its list
/// three quarters of a wide composer for rows forty columns long.
#[test]
fn the_palette_and_the_flyout_split_at_their_middle() {
    use ratatui::{backend::TestBackend, layout::Rect, Terminal};
    let theme = theme();
    // Where the rule and the frame's sides are on the row the cursor is on
    let columns = |buffer: &ratatui::buffer::Buffer, width: u16, height: u16| {
        let y = (0..height)
            .find(|&y| (0..width).any(|x| buffer[(x, y)].symbol() == "❯"))
            .expect("a selected row");
        let bars: Vec<u16> = (0..width).filter(|&x| buffer[(x, y)].symbol() == "│").collect();
        assert_eq!(bars.len(), 3, "frame, rule, frame: {bars:?}");
        (bars[0], bars[1], bars[2])
    };
    let centered = |(left, rule, right): (u16, u16, u16), what: &str| {
        let (inside_left, inside_right) = (rule - left - 1, right - rule - 1);
        assert!(inside_left.abs_diff(inside_right) <= 1, "{what}: {left} {rule} {right}");
    };
    for width in [110u16, 140, 230] {
        let (buffer, _) = drawn(&palette(), width, 36);
        centered(columns(&buffer, width, 36), &format!("palette at {width}"));

        let mut flyout =
            crate::slash_flyout::SlashFlyoutState::new(menu_with(&CommandContext::default(), &[]));
        flyout.set_query("");
        let hit = std::cell::RefCell::new(crate::mouse::HitRegions::default());
        let mut terminal = Terminal::new(TestBackend::new(width, 14)).expect("terminal");
        terminal
            .draw(|f| {
                crate::slash_flyout::render_slash_flyout(
                    f,
                    Rect::new(0, 0, width, 14),
                    crate::picker::Edge::Above(14),
                    &flyout,
                    &theme,
                    &hit,
                )
            })
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        let (left, rule, right) = columns(&buffer, width, 14);
        assert_eq!((left, right), (0, width - 1), "the flyout spans its composer");
        centered((left, rule, right), &format!("flyout at {width}"));
    }
}
