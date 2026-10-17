//! keys_card — the card that lists what a surface's keys do, grouped, and the chat's own groups.
//!
//! The palette answers "what can I run"; this answers "what keys are bound". Guided Review has
//! drawn one since it grew a footer too long to hold everything, and the chat gets the same card,
//! from the same renderer, so the two read as one thing. Rows come from the live keymap wherever
//! a key can be rebound, and a key nobody bound is left off rather than advertised.

use luxe_core::keymap::{KeyMap, UiAction};
use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::{Clear, Padding, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::glyphs::Icon;
use crate::theme::styles::ThemeStyles;

/// A titled group of the card's rows, each a key and what it does.
pub(crate) type HelpGroup = (&'static str, Vec<(String, String)>);

/// The chat card's title, shared by the rendered frame and the card-width computation.
pub(crate) const CHAT_TITLE: &str = "Help";

/// Where the rest is, on the chat card's footer: the guide, for what a card of keys cannot say.
pub(crate) const DOCS: &str = "luxe.dev/docs";

/// Cells between the card's two columns.
const HELP_GAP: u16 = 4;

/// Draws `groups` as a centered card over `area`: two columns where they fit, one where not.
///
/// Args: `(frame, area, title, groups, hint, theme)` — `hint` is the close key along the bottom.
pub(crate) fn render(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    groups: &[HelpGroup],
    hint: &str,
    note: Option<&str>,
    theme: &ThemeStyles,
) {
    let column = |groups: &[HelpGroup]| -> Vec<Line<'static>> {
        let keys_width = groups
            .iter()
            .flat_map(|(_, rows)| rows.iter().map(|(keys, _)| keys.width()))
            .max()
            .unwrap_or(0);
        let mut lines = Vec::new();
        for (index, (title, rows)) in groups.iter().enumerate() {
            if index > 0 {
                lines.push(Line::default());
            }
            lines.push(Line::from(Span::styled(
                *title,
                theme.muted.add_modifier(ratatui::style::Modifier::BOLD),
            )));
            for (keys, words) in rows {
                lines.push(Line::from(vec![
                    Span::styled(format!("{keys:<keys_width$}  "), theme.accent),
                    Span::styled(words.clone(), theme.text),
                ]));
            }
        }
        lines
    };
    let width_of = |lines: &[Line]| lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    // Two columns where they fit, so the card stays short enough to take in at once
    let split = groups.len().div_ceil(2);
    let (left, right) = (column(&groups[..split]), column(&groups[split..]));
    let two_wide = width_of(&left) + HELP_GAP + width_of(&right);
    let (columns, content_width, content_height) = if two_wide + 4 <= area.width {
        let height = left.len().max(right.len()) as u16;
        (vec![left, right], two_wide, height)
    } else {
        let single = column(groups);
        let (width, height) = (width_of(&single), single.len() as u16);
        (vec![single], width, height)
    };
    let title_width = crate::widgets::panel_title_width(Icon::Help.glyph(), title) as u16;
    // Borders, the left pad and a breathing column on the right
    let width = content_width.max(title_width).saturating_add(4).min(area.width);
    let height = content_height.saturating_add(2 + crate::widgets::FOOTER_ROWS).min(area.height);
    let popup = Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y.saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    );
    let block =
        crate::widgets::titled_panel(Icon::Help.glyph(), title, theme).padding(Padding::left(1));
    frame.render_widget(Clear, popup);
    let inner = crate::widgets::render_footed(frame, block, popup, hint, note, theme);
    let mut x = inner.x;
    for lines in columns {
        let width = width_of(&lines).min(inner.right().saturating_sub(x));
        frame.render_widget(Paragraph::new(lines), Rect { x, width, ..inner });
        x = x.saturating_add(width + HELP_GAP);
    }
}

/// The first chord bound to `action`, as the keymap writes it, or `None` when nothing is.
pub(crate) fn chord(keymap: &KeyMap, action: UiAction) -> Option<String> {
    keymap.chords_for(action).and_then(|chords| chords.first()).map(ToString::to_string)
}

/// Every key the chat answers to, grouped Compose, Read, Conversation and Workspace.
///
/// A rebindable key is named from `keymap` and dropped when unbound. The few the composer owns
/// outright — Enter, Alt+Enter, `/`, `@`, Esc — are written as they are, because they are not in
/// the keymap to be rebound.
pub(crate) fn chat_groups(keymap: &KeyMap) -> Vec<HelpGroup> {
    let bound =
        |action: UiAction, words: &str| chord(keymap, action).map(|keys| (keys, words.to_owned()));
    let fixed = |keys: &str, words: &str| Some((keys.to_owned(), words.to_owned()));
    let pair = |up: UiAction, down: UiAction, words: &str| match (
        chord(keymap, up),
        chord(keymap, down),
    ) {
        (Some(up), Some(down)) => Some((format!("{up}  {down}"), words.to_owned())),
        (Some(one), None) | (None, Some(one)) => Some((one, words.to_owned())),
        (None, None) => None,
    };
    let group = |title, rows: Vec<Option<(String, String)>>| -> HelpGroup {
        (title, rows.into_iter().flatten().collect())
    };
    vec![
        group(
            "Compose",
            vec![
                fixed("enter", "send, or steer the running turn"),
                bound(UiAction::NewLine, "new line"),
                fixed("alt+enter", "queue for a turn of its own"),
                bound(UiAction::DequeueMessages, "bring waiting messages back"),
                bound(UiAction::SelectAll, "select the whole draft"),
                bound(UiAction::PasteClipboard, "paste, images too"),
                fixed("/", "commands, at the start"),
                fixed("@", "reference a file, skill or issue"),
                fixed("esc", "clear the draft"),
            ],
        ),
        group(
            "Read",
            vec![
                pair(UiAction::ScrollUp, UiAction::ScrollDown, "scroll"),
                pair(UiAction::PageUp, UiAction::PageDown, "page"),
                pair(UiAction::SelectBlockUp, UiAction::SelectBlockDown, "select a message"),
                bound(UiAction::SearchConvo, "search the conversation"),
                bound(UiAction::ToggleToolOutput, "full tool output"),
                bound(UiAction::OpenImage, "view an image"),
            ],
        ),
        group(
            "Conversation",
            vec![
                bound(UiAction::CopyMessage, "copy the message"),
                bound(UiAction::CopySelection, "copy the selection"),
                bound(UiAction::Retry, "ask again"),
                bound(UiAction::Regenerate, "replace the last answer"),
                bound(UiAction::EditResend, "edit and resend"),
                bound(UiAction::Cancel, "stop the turn"),
            ],
        ),
        group(
            "Workspace",
            vec![
                bound(UiAction::OpenPalette, "commands"),
                bound(UiAction::ToggleDock, "sidebar"),
                bound(UiAction::ToggleStatusline, "statusline"),
                bound(UiAction::ToggleChanges, "see what changed"),
                bound(UiAction::ReviewOpen, "review changes"),
                bound(UiAction::Reload, "reload config"),
                bound(UiAction::Quit, "quit"),
            ],
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> ThemeStyles {
        let resolved = luxe_core::theme::ThemeRegistry::new().resolve("nord").expect("nord");
        ThemeStyles::from_resolved(&resolved, "nord")
    }

    fn drawn(groups: &[HelpGroup], width: u16, height: u16) -> Vec<String> {
        use ratatui::{backend::TestBackend, Terminal};
        let theme = theme();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| {
                render(
                    frame,
                    Rect::new(0, 0, width, height),
                    CHAT_TITLE,
                    groups,
                    " esc close ",
                    Some(DOCS),
                    &theme,
                )
            })
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..height).map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect()).collect()
    }

    fn row<'a>(groups: &'a [HelpGroup], words: &str) -> Option<&'a str> {
        groups
            .iter()
            .flat_map(|(_, rows)| rows.iter())
            .find(|(_, said)| said == words)
            .map(|(keys, _)| keys.as_str())
    }

    /// The card names what is bound, from the keymap: a rebound key shows its new chord, and one
    /// nobody bound is left off rather than advertised.
    /// The card is Help: its keys, and the guide's address beside the way out.
    #[test]
    fn help_names_itself_and_where_the_guide_is() {
        let rows = drawn(&chat_groups(&KeyMap::default()), 140, 40);
        let text = rows.join("\n");
        assert!(rows.iter().any(|row| row.contains("Help")), "{text}");
        let footer = rows.iter().find(|row| row.contains("esc close")).expect("the footer");
        assert!(footer.contains(DOCS), "{text}");
    }

    #[test]
    fn the_chat_card_names_what_is_bound_now() {
        let shipped = chat_groups(&KeyMap::defaults());
        assert_eq!(row(&shipped, "commands"), Some("ctrl+k"));
        assert_eq!(row(&shipped, "review changes"), Some("ctrl+g"));
        assert_eq!(row(&shipped, "scroll"), Some("ctrl+up  ctrl+down"));
        assert_eq!(row(&shipped, "send, or steer the running turn"), Some("enter"));

        let rebound = KeyMap::defaults().with_overrides(std::collections::HashMap::from([(
            "tui.app.open_palette".to_string(),
            vec!["ctrl+p".to_string()],
        )]));
        let groups = chat_groups(&rebound);
        assert_eq!(row(&groups, "commands"), Some("ctrl+p"));
        assert_eq!(row(&groups, "see what changed"), None, "ctrl+p moved away from it");
    }

    /// The keys the palette stopped listing, because they are keys and not commands, are on the
    /// card — otherwise the chat documents them nowhere.
    #[test]
    fn the_keys_cut_from_the_palette_are_on_the_card() {
        let groups = chat_groups(&KeyMap::defaults());
        for words in ["select a message", "page", "copy the selection", "paste, images too"] {
            assert!(row(&groups, words).is_some(), "{words} is documented nowhere");
        }
        assert_eq!(
            groups.iter().map(|(title, _)| *title).collect::<Vec<_>>(),
            ["Compose", "Read", "Conversation", "Workspace"]
        );
    }

    /// Two columns where they fit, one where they do not, and the close key along the bottom.
    #[test]
    fn the_card_takes_two_columns_where_they_fit() {
        let groups = chat_groups(&KeyMap::defaults());
        let row_of = |rows: &[String], word: &str| {
            rows.iter().position(|row| row.contains(word)).unwrap_or_else(|| panic!("{word}"))
        };
        let wide = drawn(&groups, 120, 30);
        assert_eq!(row_of(&wide, "Compose"), row_of(&wide, "Conversation"), "side by side");
        assert!(wide.iter().any(|row| row.contains("esc close")));
        let narrow = drawn(&groups, 50, 60);
        assert!(row_of(&narrow, "Conversation") > row_of(&narrow, "Compose"), "stacked");
    }
}
