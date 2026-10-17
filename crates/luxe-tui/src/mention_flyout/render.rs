//! The mention flyout's drawing — its tabs, rows and the status of a tab still fetching.

use super::{Listed, MentionFlyoutState, MentionTab};
use crate::glyphs::Icon;

/// Renders the picker directly above the input, via the shared [`crate::picker`] renderer.
///
/// Args: `(frame, area, edge, state, theme, hit, spinner frame)`.
pub fn render_mention_flyout(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    edge: crate::picker::Edge,
    state: &MentionFlyoutState,
    theme: &crate::theme::styles::ThemeStyles,
    hit: &std::cell::RefCell<crate::mouse::HitRegions>,
    frame: usize,
) {
    use crate::picker::{render_picker, PickerRow, PickerView};

    let filtered = state.nav.filtered_indices();
    let tab = &state.tabs[state.open];
    let fetching = format!("{} Fetching from {}…", crate::statusline::spinner(frame), tab.name());
    let fetching = match tab {
        MentionTab::GitHub => {
            format!("{} Fetching data from GitHub…", crate::statusline::spinner(frame))
        }
        _ => fetching,
    };
    let listed = state.listed[state.open].as_ref();
    let empty = match (tab, listed) {
        (MentionTab::Files, _) => "No matching files",
        (MentionTab::Skills, Some(Listed { entries, .. })) if entries.is_empty() => {
            "No skills yet: one is a folder holding a SKILL.md, in ~/.config/luxe/skills/ or the \
             project's .luxe/skills/"
        }
        (MentionTab::Skills, _) => "No matching skills",
        (MentionTab::Mcp, _) => "No matching servers",
        (_, None) => fetching.as_str(),
        (_, Some(Listed { error: Some(error), .. })) => error.as_str(),
        (MentionTab::GitHub, Some(_)) => "No matching pull requests or issues",
        (MentionTab::Source(_), Some(_)) => "Nothing matches",
    };
    let tabs: Vec<String> = state
        .tabs
        .iter()
        .map(|tab| match tab {
            MentionTab::Files => crate::glyphs::icon_label(Icon::FileMenu.glyph(), "Files"),
            MentionTab::Skills => crate::glyphs::icon_label(Icon::Skill.glyph(), "Skills"),
            MentionTab::GitHub => crate::glyphs::icon_label(Icon::GitHub.glyph(), "GitHub"),
            MentionTab::Mcp => crate::glyphs::icon_label(Icon::Mcp.glyph(), "MCP"),
            MentionTab::Source(source) => crate::glyphs::icon_label(
                crate::glyphs::configured_glyph(source.icon(), Icon::MentionSource),
                &source.name,
            ),
        })
        .collect();
    let tabs: Vec<&str> = tabs.iter().map(String::as_str).collect();
    render_picker(
        f,
        area,
        edge,
        theme,
        hit,
        PickerView {
            glyph: Icon::Mention.glyph(),
            title: "Mention",
            empty_message: empty,
            item_count: filtered.len(),
            selected: state.nav.selected(),
            row: |pos| {
                let candidate = &state.items[filtered[pos]];
                let status = candidate
                    .status
                    .filter(|_| state.show_status)
                    .map(|status| crate::view::status_mark(status, theme));
                PickerRow {
                    label: candidate.label.clone(),
                    detail: status.map(|(mark, _)| mark.to_string()).or(candidate.detail.clone()),
                    label_matched: candidate.label_matched.clone(),
                    detail_matched: candidate.detail_matched.clone(),
                    path: candidate.path,
                    detail_style: status.map(|(_, style)| style),
                }
            },
            hit_target: crate::mouse::MouseTarget::MentionRow,
            // Ctrl+R because the list is a snapshot of the disk, read as the picker opens and every
            // few seconds after; a file made a moment ago is one key away rather than a reopen
            hint: Some(match (filtered.is_empty(), tab) {
                (true, MentionTab::Files) => " tab switch · ctrl+r refresh · esc close ",
                (true, _) => " tab switch · esc close ",
                (false, MentionTab::Files) => " tab switch · enter insert · ctrl+r refresh ",
                (false, _) => " tab switch · enter insert ",
            }),
            tabs: Some((&tabs, state.open)),
            divider: None,
            detail: None,
            // The same height on every tab, so switching tabs never moves the frame
            rows: Some(crate::picker::TABBED_ROWS),
        },
    );
}
