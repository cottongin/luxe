//! Dashboard keys — the actions a key bound in the keymap runs on the start screen.

use super::DashOutcome;
use luxe_core::dashboard::PickerKind;
use luxe_core::keymap::UiAction;

pub(super) fn outcome_for_ui_action(action: UiAction) -> Option<DashOutcome> {
    match action {
        UiAction::DashboardNewSession => Some(DashOutcome::NewSession),
        UiAction::DashboardResumeLatest => Some(DashOutcome::ResumeLatest),
        UiAction::DashboardFindSession => Some(DashOutcome::OpenPicker(PickerKind::FindSession)),
        UiAction::DashboardRecentSessions => {
            Some(DashOutcome::OpenPicker(PickerKind::RecentSessions))
        }
        UiAction::DashboardGrepSessions => Some(DashOutcome::OpenPicker(PickerKind::GrepSessions)),
        UiAction::DashboardReviewChanges => Some(DashOutcome::ReviewChanges),
        UiAction::DashboardOpenConfig => Some(DashOutcome::OpenConfig),
        UiAction::DashboardQuit => Some(DashOutcome::Quit),
        UiAction::CycleMode => Some(DashOutcome::CycleMode),
        _ => None,
    }
}
