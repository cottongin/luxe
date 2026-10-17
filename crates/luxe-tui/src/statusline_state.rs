//! Statusline visibility state — UI-local show/hide/auto resolution.
//!
//! The bottom statusline and the full-height Context Dock are two forms of one workspace-context
//! surface. `Auto` treats the statusline as the compact fallback shown whenever the dock is not
//! sitting beside the conversation, so it disappears on wide terminals (where the sidebar carries
//! everything) and reappears on narrower ones. A runtime override (palette, slash, or key) wins
//! over config until it is reset.

use luxe_core::config::StatuslineVisibility;

use crate::workspace_dock::DockPresentation;

/// A temporary session-local statusline visibility choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatuslineOverride {
    /// Follow the persisted configuration.
    None,
    /// Keep the statusline shown across layout changes.
    Shown,
    /// Keep the statusline hidden across layout changes.
    Hidden,
}

/// Presentation-only statusline visibility state for the current session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatuslineState {
    /// Persisted user preference from layered UI config.
    pub configured: StatuslineVisibility,
    /// Temporary session-local visibility choice.
    pub runtime_override: StatuslineOverride,
}

impl StatuslineState {
    /// Creates a fresh statusline state from the resolved persisted preference.
    pub fn new(configured: StatuslineVisibility) -> Self {
        Self { configured, runtime_override: StatuslineOverride::None }
    }

    /// Resolves whether the statusline shows for the current dock presentation.
    ///
    /// In `Auto`, the statusline yields the bottom row to the sidebar only when the dock is
    /// side-by-side; every other presentation (hidden, overlay, replacement) keeps the compact
    /// bar so context is never lost.
    pub fn visible(&self, dock: DockPresentation) -> bool {
        match self.runtime_override {
            StatuslineOverride::Shown => true,
            StatuslineOverride::Hidden => false,
            StatuslineOverride::None => match self.configured {
                StatuslineVisibility::Shown => true,
                StatuslineVisibility::Hidden => false,
                StatuslineVisibility::Auto | StatuslineVisibility::Unknown => {
                    dock != DockPresentation::SideBySide
                }
            },
        }
    }

    /// Temporarily shows the statusline for the current session.
    pub fn show(&mut self) {
        self.runtime_override = StatuslineOverride::Shown;
    }

    /// Temporarily hides the statusline for the current session.
    pub fn hide(&mut self) {
        self.runtime_override = StatuslineOverride::Hidden;
    }

    /// Clears the temporary choice and returns to dock-aware automatic behavior.
    pub fn reset_auto(&mut self) {
        self.runtime_override = StatuslineOverride::None;
    }

    /// Flips visibility without making auto mode resize-sensitive again.
    pub fn toggle(&mut self, dock: DockPresentation) {
        if self.visible(dock) {
            self.hide();
        } else {
            self.show();
        }
    }
}

impl Default for StatuslineState {
    fn default() -> Self {
        Self::new(StatuslineVisibility::Auto)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_yields_only_to_the_side_by_side_sidebar() {
        let state = StatuslineState::default();
        assert!(!state.visible(DockPresentation::SideBySide));
        assert!(state.visible(DockPresentation::Hidden));
        assert!(state.visible(DockPresentation::Overlay));
        assert!(state.visible(DockPresentation::Replacement));
    }

    /// On a wide terminal under `auto`, the dock key is already the swap: hiding the sidebar
    /// brings the bar back and showing it puts the bar away, so the telemetry is on screen once
    /// either way. A separate "swap" action would only duplicate this.
    #[test]
    fn under_auto_the_dock_key_trades_the_sidebar_for_the_bar() {
        use crate::workspace_dock::DockState;
        use crate::workspace_layout::WorkspaceLayout;
        use luxe_core::config::DockMode;

        let wide = WorkspaceLayout::Wide;
        let mut dock = DockState::new(DockMode::Auto);
        let bar = StatuslineState::default();
        assert_eq!(dock.presentation(wide), DockPresentation::SideBySide);
        assert!(!bar.visible(dock.presentation(wide)));

        dock.toggle(wide);
        assert_eq!(dock.presentation(wide), DockPresentation::Hidden);
        assert!(bar.visible(dock.presentation(wide)), "the bar comes back with the sidebar gone");

        dock.toggle(wide);
        assert!(!bar.visible(dock.presentation(wide)), "and goes when the sidebar returns");
    }

    #[test]
    fn configured_modes_ignore_the_dock_presentation() {
        let shown = StatuslineState::new(StatuslineVisibility::Shown);
        assert!(shown.visible(DockPresentation::SideBySide));

        let hidden = StatuslineState::new(StatuslineVisibility::Hidden);
        assert!(!hidden.visible(DockPresentation::Hidden));
    }

    #[test]
    fn runtime_override_wins_and_toggles_from_the_resolved_state() {
        let mut state = StatuslineState::default();
        // Auto hides while side-by-side; toggling forces it shown across presentations.
        state.toggle(DockPresentation::SideBySide);
        assert_eq!(state.runtime_override, StatuslineOverride::Shown);
        assert!(state.visible(DockPresentation::SideBySide));

        state.toggle(DockPresentation::SideBySide);
        assert_eq!(state.runtime_override, StatuslineOverride::Hidden);
        assert!(!state.visible(DockPresentation::Hidden));

        state.reset_auto();
        assert_eq!(state.runtime_override, StatuslineOverride::None);
        assert!(state.visible(DockPresentation::Hidden));
    }
}
