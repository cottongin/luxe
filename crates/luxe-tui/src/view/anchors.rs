//! Conversation anchors — semantic-scroll capture and restoration.
//!
//! Anchors follow a stable conversation item through wrapping and transcript changes. An anchor
//! also records the scroll offset it was captured at, so the render pass can tell a *reflow*
//! (restore the semantic position) apart from a *user scroll* (honor it) — see
//! [`super::render_conversation`].

use super::transcript::RenderedConversationLine;
use crate::app::ConversationAnchor;

pub(super) fn restore_offset(
    rendered_lines: &[RenderedConversationLine],
    anchor: &ConversationAnchor,
    first_screen_row: u16,
    viewport_height: usize,
) -> Option<usize> {
    let anchor_line = rendered_lines
        .iter()
        .position(|line| {
            *line.item_id == anchor.item_id
                && line.visual_line_within_item == anchor.visual_line_within_item
        })
        .or_else(|| {
            rendered_lines
                .iter()
                .enumerate()
                .filter(|(_, line)| *line.item_id == anchor.item_id)
                .min_by_key(|(_, line)| {
                    line.visual_line_within_item.abs_diff(anchor.visual_line_within_item)
                })
                .map(|(index, _)| index)
        })
        .or_else(|| {
            rendered_lines
                .iter()
                .enumerate()
                .filter(|(_, line)| line.visual_line_within_item == 0)
                .min_by_key(|(index, line)| (line.item_index.abs_diff(anchor.item_index), *index))
                .map(|(index, _)| index)
        })?;
    let row_within_viewport = usize::from(anchor.screen_row.saturating_sub(first_screen_row));
    let max_offset = rendered_lines.len().saturating_sub(viewport_height);
    Some(anchor_line.saturating_sub(row_within_viewport).min(max_offset))
}

pub(super) fn capture(
    rendered_lines: &[RenderedConversationLine],
    first_visible_line: usize,
    screen_row: u16,
) -> Option<ConversationAnchor> {
    rendered_lines.get(first_visible_line).map(|line| ConversationAnchor {
        item_id: (*line.item_id).clone(),
        visual_line_within_item: line.visual_line_within_item,
        item_index: line.item_index,
        screen_row,
        scroll_offset: first_visible_line,
    })
}
