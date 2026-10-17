//! Changes footer — the pane's shortcuts, fitted to the width it has.

use crate::app::App;
use crate::changes::{ChangesView, PeekView};
use crate::mouse::{ChangesFooterAction, MouseTarget};
use ratatui::{
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

/// One clickable footer shortcut: its chord, then what it does.
struct Shortcut {
    chord: String,
    label: &'static str,
    action: ChangesFooterAction,
}

impl Shortcut {
    /// Cells this shortcut occupies, its separating space included.
    fn width(&self) -> usize {
        self.chord.chars().count() + 1 + self.label.chars().count()
    }
}

/// Cells between two shortcuts.
const SHORTCUT_GAP: usize = 3;

/// Which shortcut yields first when the footer will not fit.
///
/// Least actionable first, and `close` is never in the list: a surface that has taken the column
/// must always say how to give it back. Dropping whole shortcuts rather than letting the row
/// truncate is the point — `Alt+W no wr` is worse than no mention of wrapping at all.
const SHORTCUT_SACRIFICE: [ChangesFooterAction; 7] = [
    ChangesFooterAction::FilePrev,
    ChangesFooterAction::Focus,
    ChangesFooterAction::ToggleView,
    ChangesFooterAction::ToggleWordWrap,
    ChangesFooterAction::OpenInEditor,
    ChangesFooterAction::ToggleSplit,
    ChangesFooterAction::FileNext,
];

/// Drops shortcuts by priority until the row fits `width`.
fn fit_shortcuts(shortcuts: &mut Vec<Shortcut>, width: usize) {
    let rendered = |shortcuts: &Vec<Shortcut>| {
        shortcuts.iter().map(Shortcut::width).sum::<usize>()
            + SHORTCUT_GAP * shortcuts.len().saturating_sub(1)
    };
    for action in SHORTCUT_SACRIFICE {
        if rendered(shortcuts) <= width {
            return;
        }
        shortcuts.retain(|shortcut| shortcut.action != action);
    }
    // Whatever is left over: trim from the end, so `Esc close` is the last thing standing.
    while rendered(shortcuts) > width && shortcuts.len() > 1 {
        shortcuts.remove(shortcuts.len() - 2);
    }
}

/// The keys that do something here, and only those.
///
/// Chords are read from the live keymap rather than typed in, so a remapped binding cannot leave
/// the footer advertising a key that does nothing. Stepping disappears when there is nowhere to
/// step: a control that cannot act is worse than an absent one.
fn shortcuts(app: &App, view: &ChangesView, side_by_side: bool) -> Vec<Shortcut> {
    use luxe_core::keymap::UiAction;
    let chord = |action: UiAction, fallback: &str| {
        app.keymap
            .chords_for(action)
            .and_then(|chords| chords.first())
            .map_or_else(|| fallback.to_string(), ToString::to_string)
    };
    if view.focused {
        return focused_shortcuts(view);
    }
    let mut shortcuts = Vec::new();
    if view.changed() {
        shortcuts.push(Shortcut {
            chord: chord(UiAction::ChangesToggleView, "alt+m"),
            label: match view.view {
                PeekView::Diff => "whole file",
                PeekView::File => "changes",
            },
            action: ChangesFooterAction::ToggleView,
        });
    }
    if view.files.len() > 1 && view.changed() {
        shortcuts.push(Shortcut {
            chord: chord(UiAction::ChangesFileNext, "alt+n"),
            label: "next file",
            action: ChangesFooterAction::FileNext,
        });
        shortcuts.push(Shortcut {
            chord: chord(UiAction::ChangesFilePrev, "alt+p"),
            label: "previous",
            action: ChangesFooterAction::FilePrev,
        });
    }
    // Only where the pane could actually draw two columns: a control that cannot act is worse than
    // an absent one, because pressing it teaches the reader that the footer is not to be trusted.
    // Mid-switch `side_by_side` carries the last resolved layout, which is what keeps this from
    // blinking out for the frame the next diff spends loading.
    let diffing = view.view == PeekView::Diff;
    if diffing
        && (view.two_sided.get() || view.file().is_some_and(crate::guided_review::has_two_sides))
    {
        shortcuts.push(Shortcut {
            chord: chord(UiAction::ReviewToggleSplit, "/diff"),
            // Name the layout it switches *to*, and say what the layout is *of*: "side by side"
            // alone reads as a property of the pane rather than of the diff in it.
            label: if side_by_side { "unified diff" } else { "side-by-side diff" },
            action: ChangesFooterAction::ToggleSplit,
        });
    }
    shortcuts.push(Shortcut {
        chord: chord(UiAction::ChangesToggleWordWrap, "alt+w"),
        label: if view.word_wrap { "stop wrapping" } else { "wrap lines" },
        action: ChangesFooterAction::ToggleWordWrap,
    });
    // Only for a file the review could actually contain. A review is captured from the project's
    // own worktree, so offering it over a file that lives somewhere else advertises a destination
    // that does not exist.
    if !luxe_core::file_diff::is_external_path(&view.path) && view.changed() {
        shortcuts.push(Shortcut {
            chord: chord(UiAction::ReviewOpen, "/diff"),
            label: "review this file",
            action: ChangesFooterAction::OpenReview,
        });
    }
    shortcuts.push(Shortcut {
        chord: chord(UiAction::ChangesOpenInEditor, "/editor"),
        label: "open in editor",
        action: ChangesFooterAction::OpenInEditor,
    });
    shortcuts.push(Shortcut {
        chord: chord(UiAction::FocusFiles, "ctrl+t"),
        label: "navigate",
        action: ChangesFooterAction::Focus,
    });
    shortcuts.push(Shortcut {
        chord: "esc".to_string(),
        label: "close",
        action: ChangesFooterAction::Close,
    });
    shortcuts
}

/// The keys the pane answers to while the keyboard is in it, which are not the ones it shows
/// otherwise: a reader who just walked in needs the way around, and the way out.
fn focused_shortcuts(view: &ChangesView) -> Vec<Shortcut> {
    let keys: &[(&str, &'static str)] = match view.view {
        PeekView::File if view.visual => &[
            ("VISUAL", "j k grow"),
            ("o", "other end"),
            ("enter", "ask"),
            ("y", "copy"),
            ("esc", "cancel"),
        ],
        PeekView::File => &[
            ("j k", "line"),
            ("v", "select"),
            ("enter", "ask"),
            ("y", "copy"),
            ("o", "editor"),
            ("tab", "files"),
            ("esc", "back"),
        ],
        PeekView::Diff => &[
            ("j k", "scroll"),
            ("space", "page"),
            ("g G", "ends"),
            ("o", "editor"),
            ("tab", "files"),
            ("esc", "back"),
        ],
    };
    keys.iter()
        .map(|(chord, label)| Shortcut {
            chord: (*chord).to_string(),
            label,
            action: ChangesFooterAction::Hint,
        })
        .collect()
}

/// The footer, with a click region per shortcut so the mouse can do what the keys do.
pub(super) fn render_footer(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    view: &ChangesView,
    side_by_side: bool,
) {
    let styles = &app.theme_styles;
    let mut shortcuts = shortcuts(app, view, side_by_side);
    fit_shortcuts(&mut shortcuts, usize::from(area.width));
    let mut spans = Vec::new();
    let mut regions: Vec<(u16, u16, ChangesFooterAction)> = Vec::new();
    let mut column = 0usize;
    for (index, shortcut) in shortcuts.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(" · ", styles.border_muted));
            column += SHORTCUT_GAP;
        }
        let start = column;
        let hovered = app.pointer.hover == Some(MouseTarget::ChangesFooter(shortcut.action));
        let key_style =
            if hovered { styles.accent.add_modifier(Modifier::BOLD) } else { styles.accent };
        let label_style = if hovered { styles.text } else { styles.muted };
        spans.push(Span::styled(format!("{} ", shortcut.chord), key_style));
        spans.push(Span::styled(shortcut.label.to_string(), label_style));
        column += shortcut.chord.chars().count() + 1 + shortcut.label.chars().count();
        if let (Ok(x), Ok(span)) =
            (u16::try_from(start), u16::try_from(column.saturating_sub(start)))
        {
            regions.push((x, span, shortcut.action));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);

    let mut hit = app.hit.borrow_mut();
    for (x, span, action) in regions {
        let x = area.x.saturating_add(x);
        if x >= area.right() {
            continue;
        }
        let width = span.min(area.right().saturating_sub(x));
        hit.push(Rect::new(x, area.y, width, 1), MouseTarget::ChangesFooter(action));
    }
}
