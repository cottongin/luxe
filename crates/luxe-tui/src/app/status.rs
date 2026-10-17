//! Status state — what the status line and the dock report about the turn: model, usage, cost and
//! the clock.

/// Status line model.
#[derive(Debug)]
pub struct StatusModel {
    pub model: String,
    /// The provider serving it, as the dock's MODEL row labels it.
    pub provider: String,
    /// Percentage of the model's context window in use
    pub context_pct: f32,
    pub context_window: u32,
    pub task: Option<String>,
    pub thinking: luxe_core::provider::ThinkingLevel,
    /// The session's mode, as the engine last reported it (`build`, `plan`)
    pub mode: String,
    pub git_branch: Option<String>,
    pub git_dirty: bool,
    /// Project name, for the bar's left anchor.
    pub project: Option<String>,
    /// Compact working-directory path (e.g. `~/projects/luxe`) for the optional cwd module.
    pub cwd: Option<String>,
    pub active_tool: Option<String>,
    /// Current state (streaming, idle, compacting).
    pub state: AppState,
    /// Animation frame counter, advanced on every tick while streaming/compacting.
    ///
    /// Monotonic within a turn: every consumer takes it modulo its own frame count, and the
    /// activity bar's sweep needs a counter that only grows. Zeroed at the start of a turn so the
    /// sweep begins with the comet entering the rail rather than halfway through a pause.
    pub spinner_frame: usize,
    /// Whether something outside the turn states wants the frame to keep advancing
    pub animating: bool,
    pub turn_started: Option<std::time::Instant>,
    /// Time this turn has spent waiting on a person, and when the open wait began.
    ///
    /// A turn blocked on a permission prompt or the `question` tool is not a turn *working*, and
    /// the timer beside `Ctrl+Esc to interrupt` is read as how long the agent has been at it. Left
    /// running it reports the reader's own deliberation back to them and inflates every figure a
    /// slow decision touches.
    pub(crate) waited_on_user: std::time::Duration,
    pub(crate) waiting_since: Option<std::time::Instant>,
    /// When the tool call in flight was dispatched, for the live figure on its card.
    ///
    /// One, because the engine dispatches strictly one at a time. The projector only times a call
    /// once it has *finished*, so without this a card in flight reports nothing at all — which is
    /// the one moment a duration answers a question a reader actually has.
    pub tool_started: Option<std::time::Instant>,
    /// The provider couldn't answer and the request is going out again. Args: `(attempt, attempts)`
    pub retrying: Option<(u32, u32)>,
    /// Prompt tokens consumed in the current turn, not the session
    pub prompt_tokens: u64,
    /// Completion tokens consumed in the current turn, not the session
    pub completion_tokens: u64,
    /// Cached input tokens read this turn (billed ~10x cheaper); shown as a cache-hit indicator.
    pub cache_read_tokens: u64,
    /// Cost in USD of the current turn's provider calls (None while nothing priced has arrived).
    pub cost_usd: Option<f64>,
    /// Clock every timer here reads instead of the wall clock, when set.
    ///
    /// The reveal advances on elapsed time, which a test or a screenshot fixture cannot wait for and
    /// must not be at the mercy of. Setting this pins the frame to an exact instant, so a mid-reveal
    /// frame is reproducible; the turn timer and the toast read it too, so a recording's clock is
    /// the only one.
    pub frame_clock: Option<std::time::Instant>,
    /// A short-lived confirmation drawn above the prompt, and when it landed.
    ///
    /// For the things the screen cannot show by itself: a clipboard has no visible state, and a
    /// tray row that empties mid-stream is easy to miss.
    pub toast: Option<(String, std::time::Instant)>,
    /// The active subscription's remaining allowance, when the provider reports one.
    pub plan_usage: Option<luxe_core::provider::PlanUsage>,
    /// The session's spend, as the engine reports it.
    ///
    /// Not derived here. Usage events arrive **per provider call**, and a turn that uses tools
    /// makes many; a reducer that assumed otherwise reported a forty-seven-call turn as one call's
    /// worth. The engine owns the arithmetic now and this is the figure it hands over.
    pub session: luxe_core::usage::SessionUsage,
    /// What the review had cost when its latest spend began, shown in place of the live figure
    /// until that spend settles; `None` for a review that had cost nothing yet.
    ///
    /// A guide runs as concurrent batches and a summary draft can take several calls, so the live
    /// figure climbs in steps while the work is still under way — `$0.07` for a guide that came
    /// to `$0.22` — and a partial total in a row that names the review reads as its cost.
    pub review_held: Option<luxe_core::usage::UsageTotals>,
}

impl Default for StatusModel {
    fn default() -> Self {
        Self {
            model: String::new(),
            provider: String::new(),
            context_pct: 0.0,
            context_window: 0,
            task: None,
            thinking: luxe_core::provider::ThinkingLevel::Off,
            mode: luxe_core::mode::BUILD.into(),
            git_branch: None,
            git_dirty: false,
            project: None,
            cwd: None,
            active_tool: None,
            state: AppState::Idle,
            spinner_frame: 0,
            animating: false,
            turn_started: None,
            waited_on_user: std::time::Duration::ZERO,
            waiting_since: None,
            tool_started: None,
            retrying: None,
            prompt_tokens: 0,
            completion_tokens: 0,
            cache_read_tokens: 0,
            cost_usd: None,
            session: luxe_core::usage::SessionUsage::default(),
            review_held: None,
            frame_clock: None,
            toast: None,
            plan_usage: None,
        }
    }
}

impl StatusModel {
    /// Advance the animation frame.
    ///
    /// Streaming and compacting are the states with something running, plus whatever a surface has
    /// declared with [`StatusModel::animating`] — a review capture runs *after* a turn ends, so its
    /// spinner would otherwise be a still frame of a spinner.
    ///
    /// Args: `(waiting)` — whether a panel over the composer is waiting on something, which
    /// turns the same spinner.
    pub fn tick(&mut self, waiting: bool) {
        if matches!(self.state, AppState::Streaming | AppState::Compacting)
            || self.animating
            || waiting
        {
            self.spinner_frame = self.spinner_frame.wrapping_add(1);
        }
    }

    /// Mark the start of a new turn: elapsed time and per-turn token counts reset. The session
    /// figures are the engine's and are never touched here.
    pub fn on_turn_start(&mut self) {
        self.turn_started = Some(self.now());
        self.waited_on_user = std::time::Duration::ZERO;
        self.waiting_since = None;
        self.spinner_frame = 0;
        self.prompt_tokens = 0;
        self.completion_tokens = 0;
        self.cache_read_tokens = 0;
        self.cost_usd = None;
    }

    /// Fold one provider call's usage into the per-turn figures.
    ///
    /// Sums, because each event is one call: a turn that runs twenty tools makes twenty of them.
    pub fn on_call_usage(
        &mut self,
        prompt: u64,
        completion: u64,
        cache_read: u64,
        cost: Option<f64>,
    ) {
        self.prompt_tokens += prompt;
        self.completion_tokens += completion;
        self.cache_read_tokens += cache_read;
        if let Some(cost) = cost {
            self.cost_usd = Some(self.cost_usd.unwrap_or(0.0) + cost);
        }
    }

    /// Adopts the session totals the engine just published; they are never derived here.
    pub fn on_session_usage(&mut self, usage: luxe_core::usage::SessionUsage) {
        self.session = usage;
    }

    /// Notes what the review has cost so far, as the work that will add to it begins.
    pub fn hold_review_cost(&mut self) {
        self.review_held = self.session.source(luxe_core::usage::UsageSource::Review).copied();
    }

    /// Reset the running session total (a new or loaded session starts its own tally).
    pub fn reset_session_cost(&mut self) {
        self.session = luxe_core::usage::SessionUsage::default();
        self.review_held = None;
        self.cost_usd = None;
    }

    /// How much of the active subscription's allowance is spent, when the provider says.
    ///
    /// The number a plan user steers by: dollars are an equivalent, but "12% of weekly" is the
    /// actual constraint. Absent for API-key providers.
    pub fn plan_summary(&self) -> Option<String> {
        self.plan_usage.as_ref().and_then(luxe_core::provider::PlanUsage::summary)
    }

    /// The status-line form of the same thing, sized for a segment that competes for space.
    pub fn plan_compact(&self) -> Option<String> {
        self.plan_usage.as_ref().and_then(luxe_core::provider::PlanUsage::compact)
    }

    /// The session cost as it should appear, or `None` when there is nothing honest to show.
    ///
    /// A subscription-billed session reports its value with a `≈` rather than a charge: the tokens
    /// are covered by a plan, and showing `$4.20` next to them would claim money that never moved.
    pub fn session_cost_label(&self) -> Option<String> {
        self.session.total.cost_label().or_else(|| self.session.total.plan_label())
    }

    /// Note that a copy just happened, so the screen can say so — the clipboard cannot.
    pub fn flash_copied(&mut self) {
        self.flash_toast(crate::glyphs::icon_label(
            crate::glyphs::Icon::Copy.glyph(),
            "Copied to clipboard",
        ));
    }

    /// Show `message` above the prompt for a few seconds.
    pub fn flash_toast(&mut self, message: impl Into<String>) {
        self.toast = Some((message.into(), self.now()));
    }

    /// The confirmation still worth drawing, if it is recent enough to be one.
    pub fn toast(&self) -> Option<&str> {
        const FLASH_LIFETIME: std::time::Duration = std::time::Duration::from_secs(3);
        self.toast
            .as_ref()
            .filter(|(_, at)| self.now().saturating_duration_since(*at) < FLASH_LIFETIME)
            .map(|(message, _)| message.as_str())
    }

    /// The instant a frame is drawn against: the pinned clock, or the wall.
    pub fn now(&self) -> std::time::Instant {
        self.frame_clock.unwrap_or_else(std::time::Instant::now)
    }

    /// How long the turn under way has been running, if one is.
    ///
    /// Takes the instant the frame is being drawn against rather than reading the clock itself. A
    /// recording pins [`crate::app::App::frame_now`] to its own virtual clock, so a timer that
    /// consults the wall reports how fast the *recorder* rendered — which rounds to `0.0s` on
    /// every frame of every demo the site ships.
    pub fn elapsed(&self, now: std::time::Instant) -> Option<std::time::Duration> {
        let started = self.turn_started?;
        let open = self
            .waiting_since
            .map_or(std::time::Duration::ZERO, |since| now.saturating_duration_since(since));
        Some(
            now.saturating_duration_since(started)
                .saturating_sub(self.waited_on_user)
                .saturating_sub(open),
        )
    }

    /// Records whether the turn is blocked on a person right now, and banks the time if it was.
    ///
    /// Reconciled from whether a prompt is on screen rather than driven by an event of its own, so
    /// every way a prompt can close — answered, rejected, cancelled by an interrupt, dropped with
    /// the turn — stops the clock without a fourth call site to forget.
    /// Returns the wait it just banked, for the card that was blocked by it.
    pub fn waiting_on_user(
        &mut self,
        waiting: bool,
        now: std::time::Instant,
    ) -> Option<std::time::Duration> {
        match (waiting, self.waiting_since) {
            (true, None) => {
                self.waiting_since = Some(now);
                None
            }
            (false, Some(since)) => {
                let waited = now.saturating_duration_since(since);
                self.waited_on_user += waited;
                self.waiting_since = None;
                Some(waited)
            }
            _ => None,
        }
    }

    /// How long the call in flight has been running, if one is.
    pub fn tool_elapsed(&self, now: std::time::Instant) -> Option<std::time::Duration> {
        self.tool_started.map(|started| now.saturating_duration_since(started))
    }

    /// How long the turn has spent waiting on a person, including a wait still open at `now`.
    pub fn user_wait(&self, now: std::time::Instant) -> std::time::Duration {
        self.waited_on_user
            + self
                .waiting_since
                .map_or(std::time::Duration::ZERO, |since| now.saturating_duration_since(since))
    }

    /// Format elapsed time as a string (e.g. "31s", "1m 23s").
    ///
    /// Whole seconds: this counts up while a turn runs, and a tenth of a second is not a figure
    /// anyone reads — it is one they notice moving.
    pub fn format_elapsed(dur: std::time::Duration) -> String {
        let secs = dur.as_secs();
        if secs < 60 {
            format!("{secs}s")
        } else {
            format!("{}m {:02}s", secs / 60, secs % 60)
        }
    }

    /// Format token counts as a string (e.g. "↑1.2k ↓340").
    pub fn format_tokens(prompt: u64, completion: u64) -> String {
        let fmt = |n: u64| -> String {
            if n >= 1000 {
                format!("{:.1}k", n as f64 / 1000.0)
            } else {
                format!("{n}")
            }
        };
        format!("↑{} ↓{}", fmt(prompt), fmt(completion))
    }

    /// Format cost as a string, empty when there is nothing to show.
    ///
    /// Delegates to the one formatter so no surface can round a real spend to `$0.00`.
    pub fn format_cost(cost: Option<f64>) -> String {
        match cost {
            Some(c) if c > 0.0 => luxe_core::usage::format_usd(c),
            _ => String::new(),
        }
    }

    /// Renders the status line as a ratatui `Line`.
    pub fn render_line<'a>(
        &'a self,
        theme: &'a crate::theme::styles::ThemeStyles,
        now: std::time::Instant,
    ) -> ratatui::text::Line<'a> {
        use ratatui::text::{Line, Span};

        let spinner = if matches!(self.state, AppState::Streaming | AppState::Compacting) {
            crate::glyphs::spinner(self.spinner_frame)
        } else {
            "•"
        };
        let mut spans: Vec<Span> = vec![Span::styled(spinner, theme.accent), Span::raw(" ")];
        if !self.model.is_empty() {
            spans.push(Span::raw(&self.model));
            spans.push(Span::raw(" "));
        }
        let state = match self.state {
            AppState::Idle => "idle",
            AppState::Streaming => "streaming",
            AppState::Compacting => "compacting",
        };
        spans.push(Span::raw(state));
        if let Some(dur) = self.elapsed(now) {
            spans.push(Span::raw(" "));
            spans.push(Span::raw(Self::format_elapsed(dur)));
        }
        if self.prompt_tokens > 0 || self.completion_tokens > 0 {
            spans.push(Span::raw(" "));
            spans.push(Span::raw(Self::format_tokens(self.prompt_tokens, self.completion_tokens)));
        }
        if let Some(cost) = self.session_cost_label() {
            spans.push(Span::raw(" "));
            spans.push(Span::raw(cost));
        }
        Line::from(spans)
    }
}

/// Application state for the status line.
#[derive(Debug, Default)]
pub enum AppState {
    #[default]
    Idle,
    Streaming,
    Compacting,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The turn timer and the toast run on the pinned clock, as the frame does.
    ///
    /// The break this catches: both read the wall clock while `elapsed` took the frame's, so in a
    /// recording a toast never expired and a turn started at a different time than it was drawn.
    #[test]
    fn every_timer_reads_the_pinned_clock() {
        let base = std::time::Instant::now() + Duration::from_secs(3_600);
        let mut status = StatusModel { frame_clock: Some(base), ..StatusModel::default() };
        status.on_turn_start();
        status.flash_toast("copied");
        status.frame_clock = Some(base + Duration::from_secs(2));
        assert_eq!(status.elapsed(status.now()), Some(Duration::from_secs(2)));
        assert_eq!(status.toast(), Some("copied"));
        status.frame_clock = Some(base + Duration::from_secs(4));
        assert_eq!(status.toast(), None, "three seconds on the pinned clock, not the wall's");
    }
}
