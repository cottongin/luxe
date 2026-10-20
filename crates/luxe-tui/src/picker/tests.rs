use super::rows::{fit_label, picker_row};
use super::*;
use crate::glyphs::Icon;
use crate::mouse::{HitRegions, MouseTarget};
use crate::theme::styles::ThemeStyles;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{layout::Rect, text::Line};
use std::cell::RefCell;
use unicode_width::UnicodeWidthStr;

fn fitted(label: &str, room: usize, path: bool) -> String {
    fit_label(label, room, path).into_iter().map(|(character, _)| character).collect()
}

/// A path keeps its filename and as many leading folders as fit; only a filename wider than
/// the row is cut, at its end; anything else is cut at its end. Every kept character keeps its
/// index, so a match stays lit after the trim.
#[test]
fn a_long_path_loses_folders_before_its_filename() {
    let path = "crates/luxe-tui/src/mention_flyout.rs";
    assert_eq!(fitted(path, 40, true), path, "it fits");
    assert_eq!(fitted(path, 30, true), "crates/…/mention_flyout.rs");
    assert_eq!(fitted(path, 20, true), "…/mention_flyout.rs");
    assert_eq!(fitted(path, 10, true), "mention_f…");
    assert_eq!(fitted(path, 10, false), "crates/lu…", "not a path: cut at the end");
    let kept = fit_label(path, 20, true);
    let flyout = path.find("flyout").unwrap();
    assert!(kept.contains(&('f', Some(flyout))), "the filename keeps its indices: {kept:?}");
    assert_eq!(kept[0], ('\u{2026}', None), "the ellipsis stands for what it cut");
    let slash = path.rfind('/').unwrap();
    assert_eq!(kept[1], ('/', Some(slash)), "the slash kept is the filename's own");
}

/// Each cut happens exactly where the room runs out, not a cell early or late.
#[test]
fn a_label_is_cut_exactly_at_its_room() {
    assert_eq!(fitted("abcdefgh", 8, false), "abcdefgh", "exactly fits");
    assert_eq!(fitted("abcdefgh", 7, false), "abcdef\u{2026}");
    assert_eq!(fitted("abcdefgh", 0, false), "", "no room, nothing drawn");
    // `…/` and the filename, exactly
    assert_eq!(fitted("src/widgets/fit.rs", 8, true), "\u{2026}/fit.rs");
    // room for the filename and a `…` before it, but not its slash
    assert_eq!(fitted("src/widgets/fit.rs", 7, true), "\u{2026}fit.rs");
    assert_eq!(fitted("src/widgets/fit.rs", 6, true), "fit.r\u{2026}", "the filename itself cut");
    // a leading folder, exactly
    assert_eq!(fitted("src/widgets/fit.rs", 12, true), "src/\u{2026}/fit.rs");
    assert_eq!(fitted("src/widgets/fit.rs", 11, true), "\u{2026}/fit.rs");
}

/// A label that fills the row exactly is drawn whole; a detail cut short does not light the
/// `…` standing in for a matched character, and one that fits lights its last character.
#[test]
fn a_row_spends_exactly_its_room_and_never_lights_an_ellipsis() {
    let resolved = luxe_core::theme::ThemeRegistry::new().resolve("nord").expect("nord");
    let theme = ThemeStyles::from_resolved(&resolved, "nord");
    let text =
        |line: &Line| line.spans.iter().map(|span| span.content.as_ref()).collect::<String>();
    let room = 24;
    let width = room + crate::widgets::SELECTION_FRAME + 2;
    let label = "x".repeat(room);
    let whole =
        picker_row(PickerRow { label: label.clone(), ..Default::default() }, false, width, &theme);
    assert!(text(&whole).contains(&label) && !text(&whole).contains('\u{2026}'), "{whole:?}");

    let lit = theme.accent.add_modifier(ratatui::style::Modifier::BOLD);
    let row = |detail: &str, matched: Vec<usize>| PickerRow {
        label: "ab".into(),
        detail: Some(detail.into()),
        detail_matched: matched,
        ..Default::default()
    };
    // Room for `ab`, two spaces and five cells of detail: `wxyz…`
    let clipped =
        picker_row(row("wxyzvq", vec![4]), false, 9 + crate::widgets::SELECTION_FRAME + 2, &theme);
    let ellipsis = clipped.spans.iter().find(|span| span.content.contains('\u{2026}')).unwrap();
    assert_ne!(ellipsis.style, lit, "{clipped:?}");
    let fits = picker_row(row("wxyz", vec![3]), false, 60, &theme);
    let z =
        fits.spans.iter().find(|span| span.content == "z").expect("the last character, lit apart");
    assert_eq!(z.style, lit, "{fits:?}");
}

/// What the query matched is lit in the accent and bold — on the selection surface when the
/// row is chosen — a path's folder is quiet beside its filename, and a detail lights too.
#[test]
fn a_row_lights_what_matched_and_quiets_a_paths_folder() {
    let resolved = luxe_core::theme::ThemeRegistry::new().resolve("nord").expect("nord");
    let theme = ThemeStyles::from_resolved(&resolved, "nord");
    let lit = theme.accent.add_modifier(ratatui::style::Modifier::BOLD);
    let style_of = |line: &Line, text: &str| {
        line.spans.iter().find(|span| span.content == text).map(|span| span.style)
    };
    let row = || PickerRow {
        label: "src/main.rs".into(),
        label_matched: vec![4, 5, 6, 7],
        detail: Some("modified".into()),
        detail_matched: vec![0, 1],
        path: true,
        ..Default::default()
    };
    let line = picker_row(row(), false, 60, &theme);
    assert_eq!(style_of(&line, "src/"), Some(theme.muted), "{line:?}");
    assert_eq!(style_of(&line, "main"), Some(lit), "{line:?}");
    assert_eq!(style_of(&line, ".rs"), Some(theme.text), "{line:?}");
    assert_eq!(style_of(&line, "mo"), Some(lit), "the detail lights too: {line:?}");
    assert_eq!(style_of(&line, "dified"), Some(theme.muted), "{line:?}");

    let chosen = picker_row(row(), true, 60, &theme);
    assert_eq!(style_of(&chosen, "main").and_then(|s| s.bg), theme.selected_bg.bg);
    assert!(style_of(&chosen, "main")
        .unwrap()
        .add_modifier
        .contains(ratatui::style::Modifier::BOLD));
}

/// Moving the open tab moves the slab and nothing else: every label keeps its column.
#[test]
fn the_labels_hold_their_columns_as_the_open_tab_moves() {
    let resolved = luxe_core::theme::ThemeRegistry::new().resolve("nord").expect("nord");
    let theme = ThemeStyles::from_resolved(&resolved, "nord");
    let tabs = ["Files", "Skills", "GitHub", "MCP"];
    let columns = |open: usize| {
        let text = tab_line(&tabs, open, &theme).to_string();
        tabs.map(|tab| {
            let at = text.find(tab).expect("every label is drawn");
            UnicodeWidthStr::width(&text[..at])
        })
    };
    let first = columns(0);
    assert_eq!(first[0], 2, "the first label starts where the rows' cursor does");
    for open in 1..tabs.len() {
        assert_eq!(columns(open), first, "open = {open}");
    }
}

#[test]
fn nav_wraps_and_tracks_the_selected_item_index() {
    let mut nav = PickerNav::new(3); // filtered = [0,1,2], selected 0
    assert_eq!(nav.selected_item_index(), Some(0));
    nav.move_up(); // wrap to bottom
    assert_eq!(nav.selected(), 2);
    nav.move_down(); // wrap to top
    assert_eq!(nav.selected(), 0);
    nav.move_down();
    assert_eq!(nav.selected_item_index(), Some(1));
}

#[test]
fn apply_filter_remaps_indices_and_resets_cursor() {
    let mut nav = PickerNav::new(5);
    nav.move_down();
    nav.move_down(); // selected == 2
    nav.apply_filter("x", vec![4, 2]); // only items 4 and 2 survive
    assert_eq!(nav.query(), "x");
    assert_eq!(nav.filtered_indices(), &[4, 2]);
    assert_eq!(nav.selected(), 0, "cursor resets to the top after a re-filter");
    assert_eq!(nav.selected_item_index(), Some(4));
}

#[test]
fn select_clamps_and_empty_is_safe() {
    let mut nav = PickerNav::new(2);
    nav.select(99);
    assert_eq!(nav.selected(), 1);
    nav.apply_filter("none", vec![]);
    nav.move_up();
    nav.move_down();
    nav.select(3);
    assert_eq!(nav.selected(), 0);
    assert_eq!(nav.selected_item_index(), None);
}

/// The three modal pickers all route their search keys through this, so a divergence here is a
/// divergence in every one of them at once.
#[test]
fn editing_the_query_re_ranks_and_reports_the_key_consumed() {
    let items = ["anthropic/claude", "openai/gpt", "google/gemini"];
    let haystacks = || items.to_vec();
    let mut nav = PickerNav::new(items.len());

    assert!(nav.edit_query(&press(KeyCode::Char('g'), KeyModifiers::NONE), haystacks));
    assert_eq!(nav.query(), "g");
    assert!(nav.match_count() < items.len(), "a query narrows the set");

    assert!(nav.edit_query(&press(KeyCode::Backspace, KeyModifiers::NONE), haystacks));
    assert_eq!(nav.query(), "");
    assert_eq!(nav.match_count(), items.len(), "clearing restores every row");

    nav.edit_query(&press(KeyCode::Char('x'), KeyModifiers::NONE), haystacks);
    assert!(nav.edit_query(&press(KeyCode::Char('u'), KeyModifiers::CONTROL), haystacks));
    assert_eq!(nav.query(), "", "ctrl+u wipes the whole query");
}

/// Keys the nav does not own must fall through, or a picker could never bind its own.
#[test]
fn a_non_search_key_is_left_for_the_caller() {
    let mut nav = PickerNav::new(3);
    for (code, modifiers) in [
        (KeyCode::Enter, KeyModifiers::NONE),
        (KeyCode::Esc, KeyModifiers::NONE),
        (KeyCode::Up, KeyModifiers::NONE),
        (KeyCode::Char('n'), KeyModifiers::CONTROL),
    ] {
        assert!(
            !nav.edit_query(&press(code, modifiers), || vec!["a", "b", "c"]),
            "{code:?} + {modifiers:?} is not a query edit"
        );
    }
    assert_eq!(nav.query(), "");
}

/// A blank query keeps the owner's order, which is the one it deliberately sorted.
#[test]
fn a_blank_query_restores_the_original_order() {
    let mut nav = PickerNav::new(3);
    nav.set_query("b", &["alpha", "beta", "gamma"]);
    nav.set_query("   ", &["alpha", "beta", "gamma"]);
    assert_eq!(nav.filtered_indices(), &[0, 1, 2]);
}

/// The rule sits on the middle column, with the same room on either side of it once the gap
/// and the cell before the frame are counted, and `around` inverts `of`.
#[test]
fn a_split_puts_the_rule_in_the_middle() {
    for width in [60u16, 101, 118, 228] {
        let split = Split::of(width);
        let right = width - split.list - 1;
        assert!(split.list.abs_diff(right) <= 1, "{width}: {split:?}");
        assert_eq!(split.pane, right - 2, "{width}: the gap and the cell before the frame");
    }
    for half in [40u16, 52] {
        assert_eq!(Split::of(Split::around(half)).list, half);
    }
}

fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

#[test]
fn render_picker_draws_title_rows_selection_and_hit_regions() {
    use ratatui::{backend::TestBackend, layout::Rect, Terminal};
    let registry = luxe_core::theme::ThemeRegistry::new();
    let resolved = registry.resolve("nord").expect("nord resolves");
    let theme = ThemeStyles::from_resolved(&resolved, "nord");
    let selected_bg = theme.selected_bg.bg.expect("nord defines a selection background");
    let hit = RefCell::new(HitRegions::default());

    let labels = ["alpha", "beta", "gamma"];
    let mut terminal = Terminal::new(TestBackend::new(40, 8)).expect("terminal");
    terminal
        .draw(|f| {
            render_picker(
                f,
                Rect::new(0, 0, 40, 8),
                Edge::Above(8),
                &theme,
                &hit,
                PickerView {
                    glyph: Icon::SlashMenu.glyph(),
                    title: "Commands",
                    empty_message: "No matching commands",
                    item_count: labels.len(),
                    selected: 1,
                    row: |pos| PickerRow {
                        label: labels[pos].to_string(),
                        detail: Some(format!("detail {pos}")),
                        ..Default::default()
                    },
                    hit_target: crate::mouse::MouseTarget::SlashRow,
                    hint: None,
                    tabs: None,
                    divider: None,
                    detail: None,
                    rows: None,
                    list: crate::mouse::OverlayList::Slash,
                },
            );
        })
        .expect("render");

    let buffer = terminal.backend().buffer();
    let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
    assert!(text.contains("Commands"), "title present");
    assert!(text.contains(Icon::SlashMenu.glyph()), "title glyph present");
    assert!(text.contains("alpha") && text.contains("beta"), "rows present");
    assert!(text.contains('❯'), "selection cursor present");
    assert!(text.contains('╭'), "rounded panel border");

    // The flyout anchors above input_top=8: height 5 → y=3..7, content rows at y=4,5,6. Each
    // visible row publishes a SlashRow click target at its filtered position.
    let regions = hit.borrow();
    assert_eq!(regions.at(1, 4), Some(&crate::mouse::MouseTarget::SlashRow(0)));
    assert_eq!(regions.at(1, 5), Some(&crate::mouse::MouseTarget::SlashRow(1)));
    // The selected row (filtered pos 1) sits at y=5, cut into a slab: the first content cell is
    // the opening cap, which carries the selection color as its *foreground* over the canvas,
    // and the pad inside it is filled. The cursor and label sit where they do on every other
    // row, so the column does not move under the selection.
    assert_eq!(buffer[(1, 5)].fg, selected_bg, "the opening cap is drawn in the selection");
    assert_eq!(buffer[(2, 5)].bg, selected_bg);
    // border(1) + the row's leading frame + the cursor column(2).
    let label_x = 1 + crate::widgets::SELECTION_FRAME as u16 / 2 + 2;
    assert!(
        buffer[(label_x, 5)].modifier.contains(ratatui::style::Modifier::BOLD),
        "the focused row's label is bold"
    );
}

/// The `@` flyout lists file paths, which run as long as the tree is deep. A row that overruns
/// the panel loses its closing cap to the border, so the words clip and the shape survives.
#[test]
fn a_row_too_long_for_the_flyout_clips_rather_than_losing_its_cap() {
    use ratatui::{backend::TestBackend, layout::Rect, Terminal};
    crate::glyphs::set_tier(crate::glyphs::GlyphTier::Nerd);
    let registry = luxe_core::theme::ThemeRegistry::new();
    let resolved = registry.resolve("nord").expect("nord resolves");
    let theme = ThemeStyles::from_resolved(&resolved, "nord");
    let hit = RefCell::new(HitRegions::default());

    const WIDTH: u16 = 40;
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, 8)).expect("terminal");
    terminal
        .draw(|f| {
            render_picker(
                f,
                Rect::new(0, 0, WIDTH, 8),
                Edge::Above(8),
                &theme,
                &hit,
                PickerView {
                    glyph: Icon::FileMenu.glyph(),
                    title: "Files",
                    empty_message: "No matching files",
                    item_count: 1,
                    selected: 0,
                    row: |_| PickerRow {
                        label: "crates/luxe-tui/src/widgets/slab.rs".to_string(),
                        detail: Some("modified 2 minutes ago by somebody".to_string()),
                        ..Default::default()
                    },
                    hit_target: crate::mouse::MouseTarget::MentionRow,
                    hint: None,
                    tabs: None,
                    divider: None,
                    detail: None,
                    rows: None,
                    list: crate::mouse::OverlayList::Slash,
                },
            );
        })
        .expect("render");

    let buffer = terminal.backend().buffer();
    let line = |y: u16| -> String { (0..WIDTH).map(|x| buffer[(x, y)].symbol()).collect() };
    let row = (0..8u16).map(line).find(|row| row.contains('❯')).expect("the one row");
    assert!(
        row.contains(crate::widgets::Cap::Close.glyph()),
        "the closing cap is still on the row: {row:?}"
    );
    assert!(row.contains('\u{2026}'), "and the overrun is marked as clipped: {row:?}");
}

/// Tabs take a header of their own under the title — the open one cut into a slab, the rest
/// quiet, then the same muted rule the palette draws under its search field — and the list and
/// its click targets start below it.
#[test]
fn tabs_sit_above_the_list_and_move_it_down() {
    use ratatui::{backend::TestBackend, Terminal};
    let resolved = luxe_core::theme::ThemeRegistry::new().resolve("nord").expect("nord");
    let theme = ThemeStyles::from_resolved(&resolved, "nord");
    let line = tab_line(&["Files", "GitHub"], 1, &theme);
    assert_eq!(line.to_string(), "  Files  \u{2590} GitHub \u{258c}");
    assert_eq!(line.spans[0].style, theme.muted);
    let surface = theme.selected_bg.bg.expect("nord names a selection surface");
    let open = &line.spans[3];
    assert_eq!(open.content, "GitHub");
    assert_eq!(open.style.fg, theme.accent.fg, "the open tab keeps the accent");
    assert_eq!(open.style.bg, Some(surface), "on the surface a chosen row wears");
    assert!(open.style.add_modifier.contains(ratatui::style::Modifier::BOLD));

    let hit = RefCell::new(HitRegions::default());
    let rows = ["one", "two", "three"];
    let mut terminal = Terminal::new(TestBackend::new(40, 12)).expect("terminal");
    terminal
        .draw(|f| {
            render_picker(
                f,
                Rect::new(0, 0, 40, 12),
                Edge::Above(12),
                &theme,
                &hit,
                PickerView {
                    glyph: Icon::Mention.glyph(),
                    title: "Mention",
                    empty_message: "none",
                    item_count: rows.len(),
                    selected: 0,
                    row: |pos| PickerRow { label: rows[pos].into(), ..Default::default() },
                    hit_target: MouseTarget::MentionRow,
                    hint: None,
                    tabs: Some((&["Files", "GitHub"], 0)),
                    divider: None,
                    detail: None,
                    rows: None,
                    list: crate::mouse::OverlayList::Slash,
                },
            )
        })
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    let row = |y: u16| (0..40).map(|x| buffer[(x, y)].symbol()).collect::<String>();
    // Frame, tabs, the rule, three rows, frame: seven rows ending on the input
    let top = (0..12).find(|&y| row(y).contains("Mention")).expect("title");
    assert_eq!(top, 12 - 7);
    assert!(row(top + 1).starts_with("│\u{2590} Files \u{258c}  GitHub"), "{}", row(top + 1));
    let rule = row(top + 2);
    assert!(rule.trim_matches(|c| c == '│' || c == ' ').chars().all(|c| c == '─'), "{rule}");
    assert_eq!(
        buffer[(2, top + 2)].fg,
        theme.border_muted.fg.expect("nord defines a muted border"),
        "the rule is drawn in the same muted border as the palette's"
    );
    assert!(row(top + 3).contains("one") && row(top + 5).contains("three"));
    assert!(row(top + 6).contains('╰'), "the last row sits above the frame");
    assert_eq!(hit.borrow().at(5, top + 3), Some(&MouseTarget::MentionRow(0)));
    assert_eq!(hit.borrow().at(5, top + 5), Some(&MouseTarget::MentionRow(2)));
    for header in [top + 1, top + 2] {
        assert_eq!(hit.borrow().at(5, header), None, "the header is not a row");
    }

    // Squeezed to six rows, the header costs two of the list's: two show, and the selection
    // at the bottom is one of them
    let many: Vec<String> = (0..10).map(|n| format!("item {n}")).collect();
    let mut terminal = Terminal::new(TestBackend::new(40, 6)).expect("terminal");
    terminal
        .draw(|f| {
            render_picker(
                f,
                Rect::new(0, 0, 40, 6),
                Edge::Above(6),
                &theme,
                &RefCell::new(HitRegions::default()),
                PickerView {
                    glyph: Icon::Mention.glyph(),
                    title: "Mention",
                    empty_message: "none",
                    item_count: many.len(),
                    selected: 9,
                    row: |pos| PickerRow { label: many[pos].clone(), ..Default::default() },
                    hit_target: MouseTarget::MentionRow,
                    hint: None,
                    tabs: Some((&["Files", "GitHub"], 0)),
                    divider: None,
                    detail: None,
                    rows: None,
                    list: crate::mouse::OverlayList::Slash,
                },
            )
        })
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    let text: Vec<String> =
        (0..6).map(|y| (0..40).map(|x| buffer[(x, y)].symbol()).collect()).collect();
    assert!(text[4].contains("item 9"), "{text:#?}");
    assert!(text[3].contains("item 8") && !text.iter().any(|row| row.contains("item 7")));
}
