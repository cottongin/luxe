//! Context menu — what a right-click offers, beside the pointer.
//!
//! Everything a row can do, each item naming the key that does the same thing, so the menu teaches
//! the keyboard rather than standing in for it. It closes on a choice, on `Esc`, or on a click
//! anywhere else, and the arrows and `Enter` drive it like any other list.

use ratatui::{
    layout::Rect,
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::mouse::{MouseTarget, PeekSelectionAction};
use crate::theme::styles::ThemeStyles;

/// What choosing an item does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    /// Show a project file in the Changes pane
    Preview(String),
    /// Show a changed file's diff
    ShowDiff(String),
    /// Open a file or folder in the reader's editor
    OpenInEditor(String),
    /// Open the folder holding a file, or the folder itself, in a file manager
    OpenInFileManager(String),
    /// Put a mention of a file in the composer
    Mention(String),
    /// Open the Guided Review on the change, landing on this file
    Review(String),
    /// Put a path on the clipboard
    CopyPath(String),
    /// Open or close a folder
    ToggleFolder(String),
    /// Act on the lines picked in the File view
    Selection(PeekSelectionAction),
    /// Show every file, or only what changed
    ToggleScope,
    /// Show what `.gitignore` ignores too, or stop
    ToggleIgnored,
}

/// One row of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    /// What it does, in words
    pub label: String,
    /// The key that does the same, when there is one
    pub key: String,
    /// What choosing it does
    pub action: MenuAction,
}

impl MenuItem {
    fn new(label: &str, key: &str, action: MenuAction) -> Self {
        Self { label: label.to_string(), key: key.to_string(), action }
    }
}

/// An open menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextMenu {
    /// The cell the right-click landed on
    pub anchor: (u16, u16),
    /// What it offers, top to bottom
    pub items: Vec<MenuItem>,
    /// The highlighted row
    pub selected: usize,
}

impl ContextMenu {
    /// The menu for a file in the FILES tree.
    pub fn for_file(anchor: (u16, u16), path: &str, changed: bool) -> Self {
        let path = path.to_string();
        let mut items = vec![
            MenuItem::new("Preview", "enter", MenuAction::Preview(path.clone())),
            MenuItem::new("Open in editor", "o", MenuAction::OpenInEditor(path.clone())),
            MenuItem::new("Ask about this file", "@", MenuAction::Mention(path.clone())),
            MenuItem::new("Show in file manager", "O", MenuAction::OpenInFileManager(path.clone())),
        ];
        if changed {
            items.push(MenuItem::new("Show the diff", "alt+m", MenuAction::ShowDiff(path.clone())));
            items.push(MenuItem::new(
                "Review this change",
                "ctrl+g",
                MenuAction::Review(path.clone()),
            ));
        }
        items.push(MenuItem::new("Copy path", "y", MenuAction::CopyPath(path)));
        Self { anchor, items, selected: 0 }
    }

    /// The menu for a folder in the FILES tree.
    pub fn for_folder(anchor: (u16, u16), path: &str, open: bool) -> Self {
        let path = path.to_string();
        let toggle = if open { "Close" } else { "Open" };
        let items = vec![
            MenuItem::new(toggle, "enter", MenuAction::ToggleFolder(path.clone())),
            MenuItem::new("Open in editor", "o", MenuAction::OpenInEditor(path.clone())),
            MenuItem::new("Ask about this folder", "@", MenuAction::Mention(path.clone())),
            MenuItem::new("Open in file manager", "O", MenuAction::OpenInFileManager(path.clone())),
            MenuItem::new("Copy path", "y", MenuAction::CopyPath(path)),
        ];
        Self { anchor, items, selected: 0 }
    }

    /// The menu under the FILES scope switch: what the tree shows. Each item names what choosing
    /// it does, so the state is read from the words rather than a mark beside them.
    pub fn for_scope(anchor: (u16, u16), changed_only: bool, show_ignored: bool) -> Self {
        let scope = if changed_only { "Show all files" } else { "Show only changed files" };
        let ignored = if show_ignored { "Hide ignored files" } else { "Show ignored files" };
        let items = vec![
            MenuItem::new(scope, "a", MenuAction::ToggleScope),
            MenuItem::new(ignored, "i", MenuAction::ToggleIgnored),
        ];
        Self { anchor, items, selected: 0 }
    }

    /// The menu for lines picked in the File view.
    pub fn for_selection(anchor: (u16, u16), ask_key: &str, copy_key: &str) -> Self {
        let items = vec![
            MenuItem::new(
                "Ask about these lines",
                ask_key,
                MenuAction::Selection(PeekSelectionAction::Ask),
            ),
            MenuItem::new("Copy", copy_key, MenuAction::Selection(PeekSelectionAction::Copy)),
            MenuItem::new(
                "Open in editor",
                "alt+o",
                MenuAction::Selection(PeekSelectionAction::OpenInEditor),
            ),
            MenuItem::new(
                "Clear the selection",
                "",
                MenuAction::Selection(PeekSelectionAction::Clear),
            ),
        ];
        Self { anchor, items, selected: 0 }
    }

    /// Moves the highlight, wrapping at both ends.
    pub fn step(&mut self, down: bool) {
        let count = self.items.len().max(1);
        self.selected = match down {
            true => (self.selected + 1) % count,
            false => (self.selected + count - 1) % count,
        };
    }

    /// The highlighted item's action.
    pub fn chosen(&self) -> Option<MenuAction> {
        self.items.get(self.selected).map(|item| item.action.clone())
    }

    /// Where the menu sits in `screen`: below and right of the click, nudged back inside the
    /// screen where that would run off it.
    pub fn area(&self, screen: Rect) -> Rect {
        let inner = self
            .items
            .iter()
            .map(|item| item.label.width() + item.key.width() + 5)
            .max()
            .unwrap_or(10);
        let width = u16::try_from(inner + 2).unwrap_or(u16::MAX).min(screen.width);
        let height = u16::try_from(self.items.len() + 2).unwrap_or(u16::MAX).min(screen.height);
        let x = self.anchor.0.min(screen.right().saturating_sub(width));
        let below = self.anchor.1.saturating_add(1);
        let y = match below.saturating_add(height) > screen.bottom() {
            true => self.anchor.1.saturating_sub(height).max(screen.y),
            false => below,
        };
        Rect::new(x, y, width, height)
    }
}

/// Draws the menu over everything, and publishes a click region per item.
pub fn render(
    frame: &mut Frame,
    menu: &ContextMenu,
    hovered: Option<usize>,
    styles: &ThemeStyles,
    hit: &std::cell::RefCell<crate::mouse::HitRegions>,
) {
    let area = menu.area(frame.area());
    frame.render_widget(Clear, area);
    let inner = usize::from(area.width.saturating_sub(2));
    let canvas = styles.background.bg.unwrap_or(styles.slab.canvas);
    let rows: Vec<Line<'static>> = menu
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let gap = inner.saturating_sub(item.label.width() + item.key.width() + 4);
            crate::widgets::selection_row_on(
                vec![
                    Span::styled(format!(" {}", item.label), styles.text),
                    Span::raw(" ".repeat(gap)),
                    Span::styled(format!("{} ", item.key), styles.accent),
                ],
                inner,
                0,
                index == menu.selected || hovered == Some(index),
                canvas,
                styles,
            )
        })
        .collect();
    frame.render_widget(
        Paragraph::new(rows).block(
            crate::widgets::panel_block()
                .border_style(styles.border_accent)
                .style(styles.background),
        ),
        area,
    );
    let mut hit = hit.borrow_mut();
    for index in 0..menu.items.len() {
        let y = area.y + 1 + u16::try_from(index).unwrap_or(u16::MAX);
        if y + 1 < area.bottom() {
            hit.push(
                Rect::new(area.x + 1, y, area.width.saturating_sub(2), 1),
                MouseTarget::ContextMenuItem(index),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_file_offers_its_diff_and_its_review() {
        let plain = ContextMenu::for_file((0, 0), "a.rs", false);
        let changed = ContextMenu::for_file((0, 0), "a.rs", true);
        let has = |menu: &ContextMenu, action: &MenuAction| {
            menu.items.iter().any(|item| &item.action == action)
        };
        assert!(!has(&plain, &MenuAction::ShowDiff("a.rs".into())));
        assert!(has(&changed, &MenuAction::ShowDiff("a.rs".into())));
        assert!(has(&changed, &MenuAction::Review("a.rs".into())));
        assert!(has(&plain, &MenuAction::OpenInFileManager("a.rs".into())), "the handoff is there");
        assert_eq!(changed.chosen(), Some(MenuAction::Preview("a.rs".into())), "enter previews");
    }

    #[test]
    fn the_menu_stays_on_the_screen_and_the_highlight_wraps() {
        let screen = Rect::new(0, 0, 80, 24);
        let mut menu = ContextMenu::for_folder((78, 22), "src", true);
        let area = menu.area(screen);
        assert!(area.right() <= screen.right() && area.bottom() <= screen.bottom(), "{area:?}");
        assert!(area.y < 22, "opens upward when there is no room below");

        menu.step(false);
        assert_eq!(menu.selected, menu.items.len() - 1);
        menu.step(true);
        assert_eq!(menu.selected, 0);
    }
}
