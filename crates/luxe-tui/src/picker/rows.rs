//! Picker rows — one row fitted to its width, with the matched characters lit.

use super::view::PickerRow;
use crate::theme::styles::ThemeStyles;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// One list row: the cursor, the label, and the detail in whatever room the label leaves.
///
/// What the query matched is drawn in the accent and bold, the way the palette lights its rows, so
/// a result says why it is there. A path's folder is quiet beside its filename, which is the part
/// being looked for, and a path too long for the row loses folders before it loses the filename.
pub(super) fn picker_row(
    row: PickerRow,
    selected: bool,
    row_width: usize,
    theme: &ThemeStyles,
) -> Line<'static> {
    use ratatui::style::Modifier;
    // What the words may occupy: the row, less its frame and the cursor column. A file path is
    // exactly the kind of string that runs long, and an overrun row loses its closing cap to the
    // border rather than wrapping.
    let room = row_width.saturating_sub(crate::widgets::SELECTION_FRAME + 2);
    let on = |style: ratatui::style::Style| match selected {
        true => style.patch(theme.selected_bg),
        false => style,
    };
    let lit = on(theme.accent.add_modifier(Modifier::BOLD));
    let quiet = on(theme.muted);
    let label_style = theme.label_on(selected);
    let folder_end = match row.path {
        true => row.label.rfind('/').map_or(0, |slash| row.label[..=slash].chars().count()),
        false => 0,
    };

    let mut spans =
        vec![Span::styled(crate::glyphs::Mark::Cursor.gutter(selected), theme.accent_on(selected))];
    let mut used = 0;
    for (character, at) in fit_label(&row.label, room, row.path) {
        let style = match at {
            None => quiet,
            Some(at) if row.label_matched.contains(&at) => lit,
            Some(at) if at < folder_end => quiet,
            Some(_) => label_style,
        };
        used += UnicodeWidthStr::width(character.encode_utf8(&mut [0; 4]) as &str);
        push_styled(&mut spans, character, style);
    }
    if let Some(detail) = row.detail {
        let detail_style = on(row.detail_style.unwrap_or(theme.muted));
        let fitted =
            crate::widgets::truncate_end(&format!("  {detail}"), room.saturating_sub(used));
        let clipped = fitted.chars().count() < detail.chars().count() + 2;
        let last = fitted.chars().count().saturating_sub(1);
        for (index, character) in fitted.chars().enumerate() {
            // The two leading spaces are not the detail's, and a closing `…` stands for what it cut
            let at = index.checked_sub(2).filter(|_| !(clipped && index == last));
            let style = match at {
                Some(at) if row.detail_matched.contains(&at) => lit,
                _ => detail_style,
            };
            push_styled(&mut spans, character, style);
        }
    }
    crate::widgets::selection_row(spans, row_width, selected, theme)
}

/// Adds `character` to the last span when it has the same style, else starts a span.
fn push_styled(spans: &mut Vec<Span<'static>>, character: char, style: ratatui::style::Style) {
    match spans.last_mut() {
        Some(last) if last.style == style => last.content.to_mut().push(character),
        _ => spans.push(Span::styled(character.to_string(), style)),
    }
}

/// `text`'s characters, each with its index in the label it came from, counting from `from`.
fn indexed(from: usize, text: &str) -> impl Iterator<Item = (char, Option<usize>)> + '_ {
    text.chars().enumerate().map(move |(offset, character)| (character, Some(from + offset)))
}

/// `label` fitted to `room` cells, each character with its index in `label` — `None` for a `…`
/// standing in for what was cut — so what the query matched stays lit after trimming.
///
/// A path keeps its filename whole and as many leading folders as fit, `crates/…/picker.rs`, then
/// just `…/picker.rs`, then `…picker.rs`; only a filename as wide as the row is cut, at its end.
/// Anything else is cut at its end.
pub(super) fn fit_label(label: &str, room: usize, path: bool) -> Vec<(char, Option<usize>)> {
    let width = |text: &str| UnicodeWidthStr::width(text);
    if width(label) <= room {
        return indexed(0, label).collect();
    }
    let cut_end = |from: usize, text: &str| {
        let mut fitted = Vec::new();
        let mut used = 0;
        for (character, at) in indexed(from, text) {
            let cell = UnicodeWidthStr::width(character.encode_utf8(&mut [0; 4]) as &str);
            if used + cell + 1 > room {
                break;
            }
            used += cell;
            fitted.push((character, at));
        }
        if room > 0 {
            fitted.push(('\u{2026}', None));
        }
        fitted
    };
    let Some(slash) = label.rfind('/').filter(|_| path) else {
        return cut_end(0, label);
    };
    let filename = &label[slash + 1..];
    let filename_from = label[..=slash].chars().count();
    // `…/` costs two cells; with room for only the `…`, it still goes before the filename, since
    // one after a whole filename would say the filename was cut
    if width(filename) + 2 > room {
        if width(filename) < room {
            return std::iter::once(('\u{2026}', None))
                .chain(indexed(filename_from, filename))
                .collect();
        }
        return cut_end(filename_from, filename);
    }
    let mut kept = 0;
    for (end, _) in label[..slash].match_indices('/') {
        if width(&label[..=end]) + 2 + width(filename) > room {
            break;
        }
        kept = end + 1;
    }
    let mut fitted: Vec<(char, Option<usize>)> = indexed(0, &label[..kept]).collect();
    fitted.push(('\u{2026}', None));
    fitted.push(('/', Some(filename_from - 1)));
    fitted.extend(indexed(filename_from, filename));
    fitted
}
