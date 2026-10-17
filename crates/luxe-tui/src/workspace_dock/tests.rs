use super::*;

#[test]
fn configured_auto_only_shows_side_by_side_when_wide() {
    let dock = DockState::default();
    assert_eq!(dock.presentation(WorkspaceLayout::Compact), DockPresentation::Hidden);
    assert_eq!(dock.presentation(WorkspaceLayout::Standard), DockPresentation::Hidden);
    assert_eq!(dock.presentation(WorkspaceLayout::Wide), DockPresentation::SideBySide);
}

#[test]
fn configured_mode_maps_to_each_responsive_presentation() {
    let shown = DockState::new(DockMode::Shown);
    assert_eq!(shown.presentation(WorkspaceLayout::Compact), DockPresentation::Replacement);
    assert_eq!(shown.presentation(WorkspaceLayout::Standard), DockPresentation::Overlay);
    assert_eq!(shown.presentation(WorkspaceLayout::Wide), DockPresentation::SideBySide);

    let hidden = DockState::new(DockMode::Hidden);
    assert_eq!(hidden.presentation(WorkspaceLayout::Wide), DockPresentation::Hidden);
}

#[test]
fn runtime_override_wins_across_resizes_until_reset() {
    let mut dock = DockState::default();
    dock.hide();
    assert_eq!(dock.presentation(WorkspaceLayout::Wide), DockPresentation::Hidden);
    assert_eq!(dock.presentation(WorkspaceLayout::Compact), DockPresentation::Hidden);

    dock.show();
    assert_eq!(dock.presentation(WorkspaceLayout::Compact), DockPresentation::Replacement);
    assert_eq!(dock.presentation(WorkspaceLayout::Standard), DockPresentation::Overlay);
    assert_eq!(dock.presentation(WorkspaceLayout::Wide), DockPresentation::SideBySide);

    dock.reset_auto();
    assert_eq!(dock.presentation(WorkspaceLayout::Standard), DockPresentation::Hidden);
}

#[test]
fn toggle_persists_manual_choice_and_hiding_resets_scroll() {
    let mut dock = DockState::default();
    dock.toggle(WorkspaceLayout::Standard);
    assert_eq!(dock.runtime_override, DockRuntimeOverride::Shown);
    assert_eq!(dock.presentation(WorkspaceLayout::Standard), DockPresentation::Overlay);

    dock.scroll = 8;
    dock.toggle(WorkspaceLayout::Standard);
    assert_eq!(dock.runtime_override, DockRuntimeOverride::Hidden);
    assert_eq!(dock.presentation(WorkspaceLayout::Wide), DockPresentation::Hidden);
    assert_eq!(dock.scroll, 0);
}

#[test]
fn scroll_by_steps_and_clamps_both_ends() {
    let mut dock = DockState::default();
    dock.scroll_by(false, 10);
    assert_eq!(dock.scroll, SCROLL_STEP);
    dock.scroll_by(false, 4);
    assert_eq!(dock.scroll, 4, "clamped to max");
    dock.scroll_by(true, 10);
    assert_eq!(dock.scroll, 1);
    dock.scroll_by(true, 10);
    assert_eq!(dock.scroll, 0, "saturates at the top");
}

/// A list the reader has not touched is wherever the layout put it, and one they have is
/// wherever they put it — until they land back on the follow position, which hands it back.
#[test]
fn a_list_follows_the_layout_until_the_reader_takes_it_over() {
    let mut dock = DockState::default();
    let metrics = DockListMetrics { max: 12, follow: 6 };
    assert_eq!(dock.list_scroll(DockList::Plan, metrics), 6, "following");

    dock.scroll_list_by(DockList::Plan, false, metrics);
    assert_eq!(dock.list_scroll(DockList::Plan, metrics), 6 + SCROLL_STEP);
    assert_eq!(dock.taken(DockList::Plan), Some(6 + SCROLL_STEP), "taken over");

    dock.scroll_list_by(DockList::Plan, true, metrics);
    assert_eq!(dock.taken(DockList::Plan), None, "back on the follow position, so handed back");

    // The work in hand moving takes a followed list with it, and leaves a taken one alone.
    let moved = DockListMetrics { max: 12, follow: 9 };
    assert_eq!(dock.list_scroll(DockList::Plan, moved), 9);
    dock.scroll_list_by(DockList::Plan, true, moved);
    assert_eq!(dock.list_scroll(DockList::Plan, moved), 6);
    assert_eq!(dock.list_scroll(DockList::Plan, DockListMetrics { max: 12, follow: 0 }), 6);
}

/// Each list is its own, and a change that shrinks under the reader lands them at the new
/// bottom rather than at nothing.
#[test]
fn lists_scroll_independently_and_clamp_to_the_current_one() {
    let mut dock = DockState::default();
    let metrics = DockListMetrics { max: 20, follow: 0 };
    dock.scroll_list_by(DockList::Files, false, metrics);
    assert_eq!(dock.list_scroll(DockList::Files, metrics), SCROLL_STEP);
    assert_eq!(dock.list_scroll(DockList::Elsewhere, metrics), 0, "untouched");

    let shrunk = DockListMetrics { max: 1, follow: 0 };
    assert_eq!(dock.list_scroll(DockList::Files, shrunk), 1, "clamped to the new bottom");

    // Overshooting either end stops there rather than wrapping.
    for _ in 0..40 {
        dock.scroll_list_by(DockList::Files, false, metrics);
    }
    assert_eq!(dock.list_scroll(DockList::Files, metrics), 20);
    for _ in 0..40 {
        dock.scroll_list_by(DockList::Files, true, metrics);
    }
    assert_eq!(dock.list_scroll(DockList::Files, metrics), 0);
}

/// A dragged thumb maps the track onto the list, and dropping it at the follow position hands
/// the list back the same way scrolling there does.
#[test]
fn dragging_a_list_maps_the_track_onto_it() {
    let mut dock = DockState::default();
    let metrics = DockListMetrics { max: 40, follow: 0 };
    dock.drag_list(DockList::Elsewhere, 4, 9, metrics);
    assert_eq!(dock.list_scroll(DockList::Elsewhere, metrics), 20, "halfway");
    dock.drag_list(DockList::Elsewhere, 8, 9, metrics);
    assert_eq!(dock.list_scroll(DockList::Elsewhere, metrics), 40, "the end of the track");
    dock.drag_list(DockList::Elsewhere, 0, 9, metrics);
    assert_eq!(dock.taken(DockList::Elsewhere), None, "and back to the top hands it back");
}

#[test]
fn reset_auto_clears_scroll_without_affecting_visible_resize_state() {
    let mut dock = DockState::default();
    dock.show();
    dock.scroll = 12;
    assert_eq!(dock.presentation(WorkspaceLayout::Wide), DockPresentation::SideBySide);
    assert_eq!(dock.presentation(WorkspaceLayout::Standard), DockPresentation::Overlay);
    assert_eq!(dock.scroll, 12);

    dock.reset_auto();
    assert_eq!(dock.runtime_override, DockRuntimeOverride::None);
    assert_eq!(dock.scroll, 0);
}

/// Putting the panel away rewinds every list too, so reopening it is not a resumed read of a
/// change that has since moved on.
#[test]
fn hiding_the_panel_rewinds_its_lists() {
    let mut dock = DockState::default();
    let metrics = DockListMetrics { max: 20, follow: 0 };
    for list in [DockList::Plan, DockList::Files, DockList::Elsewhere] {
        dock.scroll_list_by(list, false, metrics);
        assert!(dock.taken(list).is_some());
    }
    dock.hide();
    for list in [DockList::Plan, DockList::Files, DockList::Elsewhere] {
        assert_eq!(dock.taken(list), None, "{list:?} kept a stale offset");
    }
}
