//! Dock usage — the USAGE block: how much of a subscription plan is spent, and when it resets.

use super::chrome::{section_heading_line, Section};
use super::telemetry::telemetry_row;
use crate::app::App;
use luxe_core::provider::PlanWindow;
use ratatui::text::Line;

/// Cells in the meter: ten reads in tenths, and leaves the percentage and countdown room beside it
/// on the default rail.
const BAR_CELLS: usize = 10;

/// The USAGE block, drawn only while the provider reports a plan window.
///
/// An API-key provider reports no allowance, so it gets no block at all rather than an empty one.
/// Each window gets its own row: a plan with a 5h and a weekly window is limited by whichever
/// fills first, and the reader can't steer by one they can't see.
pub(super) fn usage_telemetry(app: &App, column: usize) -> Vec<Line<'static>> {
    let Some(plan) = app.status.plan_usage.as_ref() else {
        return Vec::new();
    };
    let windows: Vec<&PlanWindow> =
        [&plan.primary, &plan.secondary].into_iter().flatten().collect();
    if windows.is_empty() {
        return Vec::new();
    }
    let styles = &app.theme_styles;
    let mut lines = vec![Line::default(), section_heading_line(Section::Usage, app)];
    for window in windows {
        let ink = styles.context_fill(window.used_percent.clamp(0.0, 100.0));
        lines.push(telemetry_row(&window.window_label(), window_value(window), ink, column, app));
    }
    lines
}

/// `▓▓▓░░░░░░░ 32% · 3d 4h`: the meter, the share, and the time left when the provider says.
fn window_value(window: &PlanWindow) -> String {
    let mut value = format!("{} {:.0}%", window.bar(BAR_CELLS), window.used_percent);
    if let Some(reset) = window.reset_label() {
        value.push_str(&format!(" · {reset}"));
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(used: f32, resets: Option<u64>) -> PlanWindow {
        PlanWindow { used_percent: used, window_minutes: 10_080, resets_in_secs: resets }
    }

    #[test]
    fn a_row_shows_meter_share_and_countdown() {
        assert_eq!(window_value(&window(30.0, Some(274_000))), "▓▓▓░░░░░░░ 30% · 3d 4h");
    }

    /// Copilot reports no reset countdown, and the row must not invent one.
    #[test]
    fn a_window_without_a_reset_time_omits_the_countdown() {
        assert_eq!(window_value(&window(30.0, None)), "▓▓▓░░░░░░░ 30%");
    }
}
