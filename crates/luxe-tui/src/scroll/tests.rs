use super::*;

#[test]
fn test_new_follow_enabled() {
    let state = ScrollState::new(10, 100);
    assert!(state.follow);
    assert_eq!(state.offset, 90); // 100 - 10
    assert_eq!(state.viewport, 10);
    assert_eq!(state.total, 100);
}

#[test]
fn test_new_fewer_lines_than_viewport() {
    let state = ScrollState::new(100, 50);
    assert!(state.follow);
    assert_eq!(state.offset, 0);
}

#[test]
fn test_line_up_disables_follow() {
    let mut state = ScrollState::new(10, 100);
    assert!(state.follow);
    state.line_up();
    assert!(!state.follow);
    assert_eq!(state.offset, 89);
}

#[test]
fn test_line_down_re_enables_follow_at_bottom() {
    let mut state = ScrollState::new(10, 100);
    state.line_up();
    state.line_up();
    assert_eq!(state.offset, 88);
    // Scroll back down to bottom
    for _ in 0..88 {
        state.line_down();
    }
    assert!(state.follow);
    assert_eq!(state.offset, 90);
}

#[test]
fn test_page_up() {
    let mut state = ScrollState::new(10, 100);
    state.page_up();
    assert_eq!(state.offset, 80);
    assert!(!state.follow);
}

#[test]
fn test_page_down() {
    let mut state = ScrollState::new(10, 100);
    state.line_up();
    state.line_up();
    state.page_down();
    assert_eq!(state.offset, 90);
    assert!(state.follow);
}

#[test]
fn test_to_bottom() {
    let mut state = ScrollState::new(10, 100);
    state.line_up();
    state.to_bottom();
    assert_eq!(state.offset, 90);
    assert!(state.follow);
}

#[test]
fn test_on_new_content_following() {
    let mut state = ScrollState::new(10, 100);
    assert_eq!(state.offset, 90);
    state.on_new_content(110);
    assert_eq!(state.offset, 100); // 110 - 10
    assert!(state.follow);
}

#[test]
fn test_on_new_content_not_following() {
    let mut state = ScrollState::new(10, 100);
    state.line_up();
    let offset_before = state.offset;
    state.on_new_content(110);
    assert_eq!(state.offset, offset_before); // Stay at current offset
}

#[test]
fn test_window() {
    let state = ScrollState::new(10, 100);
    let window = state.window();
    assert_eq!(window.start, 90);
    assert_eq!(window.end, 100);
}

#[test]
fn test_window_clamped_at_end() {
    let mut state = ScrollState::new(10, 100);
    // Manually set offset past the end
    state.offset = 200;
    let window = state.window();
    assert_eq!(window.start, 200);
    assert_eq!(window.end, 100); // Clamped to total
}

#[test]
fn test_on_resize_clamps_offset() {
    let mut state = ScrollState::new(10, 100);
    state.offset = 200;
    state.on_resize(20);
    let max_offset = 100 - 20;
    assert_eq!(state.offset, max_offset);
}

#[test]
fn test_position_percent() {
    let state = ScrollState::new(10, 100);
    assert_eq!(state.position_percent(), 100); // At bottom
}

#[test]
fn test_position_percent_middle() {
    let mut state = ScrollState::new(10, 100);
    state.offset = 45; // Middle
    let percent = state.position_percent();
    assert!(percent > 0 && percent < 100);
}

/// A viewport that shrinks under a followed transcript (the composer growing a row, the
/// pending tray opening) must keep the view pinned to the bottom.
#[test]
fn shrinking_the_viewport_keeps_a_followed_transcript_at_the_bottom() {
    let mut state = ScrollState::new(10, 100);
    assert_eq!(state.offset, 90);

    state.on_resize(8);
    assert!(state.follow);
    assert_eq!(state.offset, 92, "still showing the last line");

    state.line_up();
    state.on_resize(6);
    assert!(!state.follow);
    assert_eq!(state.offset, 91, "a manual offset is left alone while it still fits");
}

/// A wheel notch moves several lines, and the ends still clamp cleanly.
#[test]
fn multi_line_steps_clamp_at_both_ends() {
    let mut state = ScrollState::new(10, 100);
    state.up_by(3);
    assert_eq!(state.offset, 87);
    assert!(!state.follow);

    state.down_by(3);
    assert_eq!(state.offset, 90);
    assert!(state.follow, "reaching the bottom resumes following");

    // Overshooting either end stops there rather than wrapping or panicking.
    state.up_by(1_000);
    assert_eq!(state.offset, 0);
    state.down_by(1_000);
    assert_eq!(state.offset, state.max_offset());
}

/// Dragging the thumb maps the pointer row onto the scroll range: top of the track is the top of
/// the transcript, bottom is the bottom (and resumes following).
#[test]
fn dragging_the_thumb_maps_the_track_onto_the_scroll_range() {
    let mut state = ScrollState::new(10, 210);
    let max = state.max_offset();

    state.drag_to(0, 20);
    assert_eq!(state.offset, 0);
    assert!(!state.follow);

    state.drag_to(9, 20);
    assert_eq!(state.offset, 95, "halfway down the track is halfway through the range");

    state.drag_to(19, 20);
    assert_eq!(state.offset, max);
    assert!(state.follow, "dragged to the end, so new output keeps arriving in view");

    // A row past the track, or a transcript that fits, is a no-op rather than a jump.
    state.drag_to(99, 20);
    assert_eq!(state.offset, max);
    let mut short = ScrollState::new(50, 10);
    short.drag_to(5, 20);
    assert_eq!(short.offset, 0);
}

#[test]
fn test_position_percent_at_top() {
    let mut state = ScrollState::new(10, 100);
    // Scroll all the way up
    for _ in 0..90 {
        state.line_up();
    }
    assert_eq!(state.offset, 0);
    assert_eq!(state.position_percent(), 0);
}
