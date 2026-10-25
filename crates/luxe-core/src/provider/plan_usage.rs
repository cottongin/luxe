//! Plan usage — how much of a subscription plan is left, as the provider reports it.

use super::*;

/// A [`PlanWindow`] length that reads as a calendar month: thirty days, in minutes.
pub const MONTH_MINUTES: u64 = 43_200;

/// One rate-limit window on a subscription, as the provider reports it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlanWindow {
    /// Share of the allowance consumed, 0–100.
    pub used_percent: f32,
    /// Length of the window in minutes (`10080` is a week).
    pub window_minutes: u64,
    /// Seconds until the window resets, when the provider says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_in_secs: Option<u64>,
}

impl PlanWindow {
    /// A human name for the window's length: the unit people actually think in.
    pub fn window_label(&self) -> String {
        match self.window_minutes {
            0 => "window".to_string(),
            // a plan renewing on a date each month, which GitHub Copilot's does
            MONTH_MINUTES => "monthly".to_string(),
            minutes if minutes % 10_080 == 0 => match minutes / 10_080 {
                1 => "weekly".to_string(),
                weeks => format!("{weeks}-week"),
            },
            minutes if minutes % 1_440 == 0 => match minutes / 1_440 {
                1 => "daily".to_string(),
                days => format!("{days}-day"),
            },
            minutes if minutes % 60 == 0 => format!("{}h", minutes / 60),
            minutes => format!("{minutes}m"),
        }
    }

    /// "resets in 3d 4h" — coarse on purpose; nobody plans around the minute.
    pub fn reset_label(&self) -> Option<String> {
        let secs = self.resets_in_secs?;
        let label = match secs {
            0 => "now".to_string(),
            s if s < 3_600 => format!("{}m", s.div_ceil(60)),
            s if s < 86_400 => format!("{}h", s / 3_600),
            s => format!("{}d {}h", s / 86_400, (s % 86_400) / 3_600),
        };
        Some(label)
    }

    /// A fixed-width meter such as `▓▓▓░░░░░░░`, filled in proportion to the share used.
    ///
    /// Rounds to the nearest cell so a nearly-full window never reads as empty, and clamps so a
    /// provider reporting over 100% can't draw past the edge.
    pub fn bar(&self, cells: usize) -> String {
        let share = (self.used_percent / 100.0).clamp(0.0, 1.0);
        let filled = ((share * cells as f32).round() as usize).min(cells);
        format!("{}{}", "▓".repeat(filled), "░".repeat(cells - filled))
    }
}

/// A subscription's remaining headroom, as reported alongside a response.
///
/// Tokens and dollars say what a turn *was*; this says what the plan has left, which is the number a
/// subscriber actually steers by. Every field is optional because it is scraped from whatever the
/// provider chose to tell us — a provider that says nothing produces `None`, not a zero.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlanUsage {
    /// Plan name the provider reports (`plus`, `pro`, `business`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// The main allowance window (for a ChatGPT plan, the weekly one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<PlanWindow>,
    /// A shorter secondary window, when the plan has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary: Option<PlanWindow>,
    /// Pay-as-you-go credits that take over once the plan allowance is spent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits_balance: Option<f64>,
}

impl PlanUsage {
    /// Whether the provider told us anything at all.
    pub fn is_empty(&self) -> bool {
        self.plan.is_none()
            && self.primary.is_none()
            && self.secondary.is_none()
            && self.credits_balance.is_none()
    }

    /// The window worth putting on a status line: whichever is closest to its limit.
    pub fn tightest(&self) -> Option<&PlanWindow> {
        match (&self.primary, &self.secondary) {
            (Some(primary), Some(secondary)) => {
                Some(match secondary.used_percent > primary.used_percent {
                    true => secondary,
                    false => primary,
                })
            }
            (Some(window), None) | (None, Some(window)) => Some(window),
            (None, None) => None,
        }
    }

    /// The status-line form: `12% weekly`.
    ///
    /// The statusline drops whole segments when it runs out of room, so the plan meter has to earn
    /// its width. The percentage and the window are the whole message; the plan name and the reset
    /// time live in `/cost`, where there is space to read them.
    pub fn compact(&self) -> Option<String> {
        let window = self.tightest()?;
        Some(format!("{:.0}% {}", window.used_percent, window.window_label()))
    }

    /// A one-line summary: `plus · 12% of weekly (resets in 3d 4h)`.
    pub fn summary(&self) -> Option<String> {
        let window = self.tightest()?;
        let mut line = String::new();
        if let Some(plan) = &self.plan {
            line.push_str(plan);
            line.push_str(" · ");
        }
        line.push_str(&format!("{:.0}% of {}", window.used_percent, window.window_label()));
        if let Some(reset) = window.reset_label() {
            line.push_str(&format!(" (resets in {reset})"));
        }
        Some(line)
    }
}

#[cfg(test)]
mod plan_tests {
    use super::*;

    fn window(used: f32, minutes: u64, resets: Option<u64>) -> PlanWindow {
        PlanWindow { used_percent: used, window_minutes: minutes, resets_in_secs: resets }
    }

    #[test]
    fn windows_are_labeled_in_the_unit_people_think_in() {
        assert_eq!(window(0.0, 10_080, None).window_label(), "weekly");
        assert_eq!(window(0.0, 1_440, None).window_label(), "daily");
        assert_eq!(window(0.0, 300, None).window_label(), "5h");
        assert_eq!(window(0.0, 45, None).window_label(), "45m");
        assert_eq!(window(0.0, 20_160, None).window_label(), "2-week");
    }

    #[test]
    fn resets_read_coarsely_because_nobody_plans_by_the_minute() {
        assert_eq!(window(0.0, 60, Some(0)).reset_label().as_deref(), Some("now"));
        assert_eq!(window(0.0, 60, Some(90)).reset_label().as_deref(), Some("2m"));
        assert_eq!(window(0.0, 60, Some(7_200)).reset_label().as_deref(), Some("2h"));
        assert_eq!(window(0.0, 60, Some(603_553)).reset_label().as_deref(), Some("6d 23h"));
        assert!(window(0.0, 60, None).reset_label().is_none());
    }

    #[test]
    fn a_bar_fills_in_proportion_and_never_overflows() {
        assert_eq!(window(0.0, 60, None).bar(10), "░░░░░░░░░░");
        assert_eq!(window(50.0, 60, None).bar(10), "▓▓▓▓▓░░░░░");
        assert_eq!(window(100.0, 60, None).bar(10), "▓▓▓▓▓▓▓▓▓▓");
        // over-reported and negative values clamp instead of panicking on the subtraction
        assert_eq!(window(250.0, 60, None).bar(4), "▓▓▓▓");
        assert_eq!(window(-5.0, 60, None).bar(4), "░░░░");
        // a window that is nearly spent must not round down to an empty-looking bar
        assert_eq!(window(96.0, 60, None).bar(10), "▓▓▓▓▓▓▓▓▓▓");
    }

    /// The status line has room for one window, and it should be whichever is about to bite.
    #[test]
    fn the_tightest_window_wins_the_status_line() {
        let plan = PlanUsage {
            plan: Some("pro".into()),
            primary: Some(window(20.0, 10_080, None)),
            secondary: Some(window(83.0, 300, Some(1_800))),
            credits_balance: None,
        };
        assert_eq!(plan.compact().as_deref(), Some("83% 5h"));
        assert_eq!(plan.summary().as_deref(), Some("pro · 83% of 5h (resets in 30m)"));
    }

    #[test]
    fn a_provider_that_says_nothing_reports_nothing() {
        let empty = PlanUsage::default();
        assert!(empty.is_empty());
        assert!(empty.summary().is_none());
        assert!(empty.compact().is_none());
        // A plan name alone isn't a meter, so there's no percentage to show.
        let named = PlanUsage { plan: Some("plus".into()), ..PlanUsage::default() };
        assert!(!named.is_empty());
        assert!(named.compact().is_none());
    }
}
