//! Reveal rate — the window of recent arrivals, and the rate the reveal plays back at.

use super::Reveal;

/// Arrivals the rate is averaged over.
///
/// A *window average* rather than a per-sample smoothing, because individual arrivals are nothing
/// like the average and no weighting fixes that. On the wire every content block opens with a
/// **1-character delta**, and the gaps that follow are mostly 300–600 ms but include runs tens of
/// milliseconds apart and multi-second pauses. An exponential average seeded on that opening delta
/// would spend the first seconds of every block convinced the model writes at 2 characters a
/// second; summing characters and time across a window dilutes it in one arrival instead.
///
/// Wide, because the average is what gets played back: the mean of a handful of samples from a
/// distribution that varies ±60% wanders enough to see. Thirty-two covers ~15 s of a batching
/// provider and ~500 ms of a local one — steady, still able to follow a model that genuinely changes
/// pace, and it carries across messages, so only the very first one of a session starts cold.
pub(super) const RATE_WINDOW: usize = 32;
/// How far the played rate closes on the measured one each frame, going up and coming down.
///
/// The window is robust but it still *steps* when an arrival enters or leaves it, and early on it
/// steps hard, half again over the true average for a single interval. Following at a few percent a
/// frame turns each step into a ramp of a few hundred milliseconds, which reads as the model
/// finding its pace rather than as the reveal lurching.
///
/// Asymmetric, because the two errors are not alike. Playing faster than the source empties the
/// buffer and *stops* — the one artifact worth avoiding — while playing slower just holds a little
/// more text back, which nobody can see. So a rising measurement is followed cautiously and a falling
/// one promptly.
const RATE_TRACKING: (f64, f64) = (0.015, 0.06);
/// Seconds of arrivals needed before the window is trusted as a rate.
const MIN_EVIDENCE: f64 = 0.12;
/// Rate assumed before any interval has been measured, chars/second.
///
/// Only ever governs a block whose second arrival has not landed within [`super::PRIME_MAX`],
/// since until then nothing is painted at all. Mid-range across measured providers.
pub(super) const NOMINAL_RATE: f64 = 150.0;
/// Sanity bounds on the rate estimate, chars/second.
const RATE_BOUNDS: (f64, f64) = (30.0, 4000.0);

impl Reveal {
    /// Records one arrival in the rate window.
    ///
    /// A `None` gap is the first arrival of a message, whose spacing is unknowable — measuring it from
    /// whatever came before would fold a tool call's runtime, or a round trip, into the model's pace.
    /// The characters still count: they arrived, and the window is about how much text moves per
    /// second, not about individual deltas.
    pub(super) fn record_arrival(&mut self, chars: f64, gap: Option<f64>) {
        self.window[self.window_next] = (chars as f32, gap.unwrap_or(0.0) as f32);
        self.window_next = (self.window_next + 1) % RATE_WINDOW;
        self.window_len = (self.window_len + 1).min(RATE_WINDOW);
    }

    /// Characters and seconds the window can account for, and how many arrivals that was.
    ///
    /// Only arrivals whose spacing is known count. An arrival with no gap — the first of a message —
    /// brings characters that belong to no interval, and counting them anyway inflates the rate by
    /// a whole arrival's worth: enough to drain the buffer and stall.
    fn measured(&self) -> (f64, f64, usize) {
        self.window[..self.window_len].iter().filter(|(_, gap)| *gap > 0.0).fold(
            (0.0, 0.0, 0),
            |(chars, seconds, count), (c, g)| {
                (chars + f64::from(*c), seconds + f64::from(*g), count + 1)
            },
        )
    }

    /// Characters a second the source is arriving at.
    ///
    /// Total characters over total time across the window, so one odd arrival moves it by a fraction
    /// rather than setting it. Falls back to a nominal rate until the window holds enough time to be
    /// worth dividing by.
    pub(super) fn rate(&self) -> f64 {
        let (chars, seconds, _) = self.measured();
        if seconds < MIN_EVIDENCE {
            return NOMINAL_RATE;
        }
        (chars / seconds).clamp(RATE_BOUNDS.0, RATE_BOUNDS.1)
    }

    /// Moves the played rate a fraction of the way to the measured one, and returns it.
    ///
    /// Seeded outright the first time, so a session does not have to ramp up from nothing.
    pub(super) fn track_rate(&mut self) -> f64 {
        let measured = self.rate();
        self.played = match self.played {
            // Seeded at the nominal rather than at whatever the first interval happened to read: one
            // interval is not a rate, and starting above the truth is what empties the buffer.
            0.0 => NOMINAL_RATE,
            played => {
                let tracking = if measured > played { RATE_TRACKING.0 } else { RATE_TRACKING.1 };
                played + tracking * (measured - played)
            }
        };
        self.played
    }

    /// Characters in a typical arrival — the unit the buffer is sized in.
    pub(super) fn lump(&self) -> f64 {
        let (chars, _, count) = self.measured();
        if count == 0 {
            return 0.0;
        }
        chars / count as f64
    }
}
