//! Pacing — a provider's bursts are revealed evenly, recorded and synthetic.

use super::*;

/// A frame clock: `base` plus `n` frames of the reveal's ~83 fps.
pub(super) fn frames(base: Instant, n: u32) -> Instant {
    base + Duration::from_millis(12) * n
}

/// A streaming prose item at the tail of the transcript.
pub(super) fn growing(index: usize, items: usize, chars: usize) -> Item {
    Item { index, items, chars, animated: true, growing: true }
}

/// The same item once no more text can arrive.
pub(super) fn settled(index: usize, items: usize, chars: usize) -> Item {
    Item { index, items, chars, animated: false, growing: false }
}

/// Frames of warm-up excluded from the smoothness bounds.
///
/// The first arrival of a session carries no spacing to measure, so the cycle after it is paced on
/// a guess by construction. Two arrivals in, the estimate is real.
pub(super) const WARMUP_FRAMES: usize = 48;

/// A deterministic jittery source, modelled on what the wire actually does.
///
/// Jitter is the point: a perfectly uniform source hides what really makes a reveal look uneven,
/// which is the *estimate* moving from one arrival to the next. The two spreads are separate
/// because the measured ones are: arrival *size* varies far more than arrival *spacing*.
pub(super) struct Source {
    chars: f64,
    chars_spread: f64,
    gap: f64,
    gap_spread: f64,
    seed: u64,
}

impl Source {
    /// Measured Anthropic: 56–136 characters (±40%) every 410–540 ms (±15%).
    fn lumpy() -> Self {
        Self { chars: 96.0, chars_spread: 0.4, gap: 0.475, gap_spread: 0.15, seed: 0x5EED }
    }

    /// Measured local model: 4 characters every 16 ms, near clockwork (p99 gap 17.7 ms).
    fn local() -> Self {
        Self { chars: 4.0, chars_spread: 0.25, gap: 0.016, gap_spread: 0.15, seed: 0x5EED }
    }

    /// Spacing jitter far beyond anything measured — a provider stuttering under load.
    fn hostile() -> Self {
        Self { gap_spread: 0.45, ..Self::lumpy() }
    }

    fn next_random(&mut self) -> f64 {
        self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.seed >> 40) as f64) / 16_777_215.0
    }

    /// The next arrival: how many characters, and how long until the one after it.
    fn next(&mut self) -> (usize, Duration) {
        let roll = self.next_random();
        let chars = (self.chars * (1.0 - self.chars_spread + 2.0 * self.chars_spread * roll))
            .max(1.0) as usize;
        let roll = self.next_random();
        let gap = (self.gap * (1.0 - self.gap_spread + 2.0 * self.gap_spread * roll)).max(0.004);
        (chars, Duration::from_secs_f64(gap))
    }
}

/// What a simulated run looked like, per frame, while the source was still producing.
pub(super) struct Run {
    /// Characters revealed in each frame.
    steps: Vec<f64>,
}

impl Run {
    /// Frames that revealed nothing, once the estimate had something to go on.
    fn stalls(&self) -> usize {
        self.steps.iter().skip(WARMUP_FRAMES).filter(|step| **step <= 0.0).count()
    }

    /// Fraction of settled frames that revealed nothing.
    fn stall_fraction(&self) -> f64 {
        self.stalls() as f64 / self.steps.len().saturating_sub(WARMUP_FRAMES).max(1) as f64
    }

    /// Ratio of the fastest ~100 ms window to the slowest.
    ///
    /// The number that says whether a reveal *pulses*, and the one a coefficient of variation
    /// hides: a twice-a-second swing between 10 and 20 characters a window scores a respectable
    /// cv of 0.17 and looks like lurching, which is how the first tuning of this shipped.
    fn peak_to_trough(&self) -> f64 {
        let windows: Vec<f64> = self
            .steps
            .chunks(8)
            .skip(WARMUP_FRAMES / 8)
            .filter(|chunk| chunk.len() == 8)
            .map(|chunk| chunk.iter().sum())
            .collect();
        let min = windows.iter().copied().fold(f64::MAX, f64::min);
        let max = windows.iter().copied().fold(0.0, f64::max);
        max / min.max(0.01)
    }
}

/// Runs `total` characters from `source` through the pacer, one frame at a time.
pub(super) fn simulate(mut source: Source, total: usize) -> Run {
    let base = Instant::now();
    let mut reveal = Reveal::default();
    reveal.begin(0);
    let mut arrived = 0usize;
    let mut next = Duration::ZERO;
    let mut steps = Vec::new();
    for frame in 0..40_000u32 {
        let now = frames(base, frame);
        let elapsed = now.duration_since(base);
        while elapsed >= next && arrived < total {
            let (chars, gap) = source.next();
            arrived = (arrived + chars).min(total);
            next += gap;
        }
        let before = reveal.shown;
        reveal.show(now, growing(0, 1, arrived));
        if arrived < total {
            steps.push(reveal.shown - before);
        } else if reveal.shown >= arrived as f64 {
            break;
        }
    }
    Run { steps }
}

/// The reveal's whole reason to exist: a lumpy source comes out evenly.
///
/// Both halves matter. A stall — the buffer running dry between arrivals — *is* the stutter, and
/// the speed must not visibly pulse either, which is what the peak-to-trough bound holds.
///
/// A stress source rather than a recording: sizes and spacings are drawn independently and
/// uniformly, which clusters worse than the real thing. The tight bounds live on the recorded
/// response in [`a_recorded_response_is_revealed_evenly_from_the_start`].
#[test]
fn a_lumpy_provider_is_revealed_evenly() {
    let run = simulate(Source::lumpy(), 8_000);
    assert_eq!(run.stalls(), 0, "the buffer must not run dry between arrivals");
    let swing = run.peak_to_trough();
    assert!(swing <= 1.6, "the reveal pulses: fastest window is {swing:.2}x the slowest");
}

#[test]
fn a_fine_grained_provider_is_revealed_evenly_too() {
    let run = simulate(Source::local(), 8_000);
    assert_eq!(run.stalls(), 0, "a steady source certainly should not stall");
    let swing = run.peak_to_trough();
    assert!(swing <= 1.3, "a steady source should be steadier still, swing was {swing:.2}x");
}

/// Recorded reasoning deltas from a real Claude Sonnet response: `(ms into the block, chars)`.
///
/// Note the opening delta. Every content block starts with a single character, and an estimator
/// seeded on that one sample spent the first seconds of every block convinced the model wrote at
/// two characters a second — which is what "it lurched at first, then sped up" was.
pub(super) const TRACE_THINKING: [(u32, usize); 7] =
    [(0, 4), (495, 160), (966, 128), (1877, 118), (2092, 97), (2464, 97), (2980, 52)];
/// The answer deltas of that same response: 10,694 characters over 46 s.
///
/// 78% of the gaps fall in the 300–600 ms band, but there are runs 23 ms apart and one 2.15 s
/// pause — a real cadence, not a tidy one.
pub(super) const TRACE_TEXT: [(u32, usize); 100] = [
    (0, 1),
    (250, 140),
    (746, 134),
    (1241, 108),
    (1662, 120),
    (2105, 68),
    (2641, 98),
    (3101, 146),
    (3658, 100),
    (4249, 132),
    (4483, 124),
    (6633, 72),
    (6690, 92),
    (6753, 66),
    (7156, 149),
    (7324, 89),
    (7388, 103),
    (7750, 70),
    (8281, 129),
    (8701, 183),
    (9201, 63),
    (9899, 128),
    (10234, 123),
    (10954, 129),
    (11106, 80),
    (12103, 99),
    (12143, 95),
    (12554, 96),
    (13060, 113),
    (13486, 79),
    (13926, 104),
    (14424, 114),
    (14870, 78),
    (15359, 122),
    (15820, 80),
    (16278, 148),
    (16752, 178),
    (17414, 54),
    (17725, 109),
    (18300, 92),
    (18636, 115),
    (19120, 103),
    (19576, 90),
    (20139, 56),
    (20580, 102),
    (21082, 66),
    (21578, 103),
    (21985, 183),
    (22674, 118),
    (22971, 93),
    (23423, 160),
    (23978, 123),
    (24374, 130),
    (25129, 167),
    (25334, 142),
    (25833, 151),
    (26384, 112),
    (26846, 100),
    (27256, 105),
    (27850, 75),
    (28116, 69),
    (28629, 89),
    (29096, 87),
    (29554, 98),
    (29986, 61),
    (30455, 145),
    (30992, 108),
    (31395, 128),
    (31896, 127),
    (32350, 108),
    (33073, 141),
    (33386, 115),
    (33753, 104),
    (34218, 59),
    (34692, 94),
    (35162, 125),
    (35639, 79),
    (36162, 100),
    (36635, 125),
    (37162, 93),
    (37632, 127),
    (38233, 82),
    (38726, 95),
    (39153, 78),
    (39559, 111),
    (40051, 79),
    (40589, 84),
    (40930, 149),
    (41454, 121),
    (41958, 156),
    (42518, 137),
    (42866, 93),
    (43364, 124),
    (43916, 127),
    (44278, 145),
    (44758, 99),
    (45287, 99),
    (45718, 91),
    (46271, 132),
    (46294, 8),
];
/// Replays a recorded block through the pacer, one frame at a time, from `start_ms`.
///
/// Returns the per-frame reveal, and leaves the pacer holding the estimates it learned — so a
/// caller can play the reasoning block and then the answer, the way a turn really arrives.
pub(super) fn replay(
    reveal: &mut Reveal,
    base: Instant,
    start_ms: u32,
    trace: &[(u32, usize)],
    index: usize,
    items: usize,
) -> Vec<Frame> {
    let last = trace.last().expect("a recorded block has deltas").0;
    let mut arrived = 0usize;
    let mut next = 0usize;
    let mut steps = Vec::new();
    let mut frame = start_ms / 12;
    loop {
        let now = base + Duration::from_millis(12) * frame;
        let at_ms = frame * 12;
        while next < trace.len() && at_ms >= start_ms + trace[next].0 {
            arrived += trace[next].1;
            next += 1;
        }
        let growing = next < trace.len();
        let before = reveal.shown;
        // Starved means the source has nothing left to give, not that the pacer held back: with
        // an empty buffer there is nothing to reveal however smooth you would like to be.
        let starved = arrived as f64 - before <= 0.0;
        let shown =
            reveal.show(now, Item { index, items, chars: arrived, animated: true, growing });
        frame += 1;
        // The block is finished: the cursor has moved on and reset its counters, so there is no
        // meaningful step left to record.
        if shown == Show::Whole || at_ms > start_ms + last + 6_000 {
            break;
        }
        steps.push(Frame {
            revealed: reveal.shown - before,
            starved,
            pacing: growing,
            painting: reveal.shown > 0.0,
        });
    }
    steps
}

/// One frame of a replay.
pub(super) struct Frame {
    /// Characters the reveal put on screen.
    revealed: f64,
    /// Whether the buffer was empty before the frame — the source had not sent anything to show.
    starved: bool,
    /// Whether the source was still producing. Once it stops, the reveal is *draining* to a
    /// deadline rather than pacing, which is a deliberate flush and not part of the pacing.
    pacing: bool,
    /// Whether anything of the item was on screen yet. Before the first paint the reveal is
    /// deliberately holding text back to gather a buffer.
    painting: bool,
}

/// Ratio of the fastest ~100 ms window to the slowest, over windows the source kept fed.
///
/// Windows containing a starved frame are excluded: a provider that sends one character and then
/// nothing for a quarter second, or pauses for two seconds mid-answer, is not something a reveal
/// can smooth over — it can only avoid *adding* unevenness of its own, which is what this measures.
pub(super) fn fed_swing(frames: &[Frame]) -> f64 {
    let windows: Vec<f64> = frames
        .chunks(8)
        .filter(|chunk| {
            chunk.len() == 8
                && chunk.iter().all(|frame| !frame.starved && frame.pacing && frame.painting)
        })
        .map(|chunk| chunk.iter().map(|frame| frame.revealed).sum())
        .collect();
    let min = windows.iter().copied().fold(f64::MAX, f64::min);
    let max = windows.iter().copied().fold(0.0, f64::max);
    max / min.max(0.01)
}

/// The recorded response, replayed: reasoning, then the answer, exactly as they arrived.
///
/// This is the case that shipped badly. The bounds are on the *first three seconds* of the answer
/// as much as on the settled middle, because the complaint was that it lurched at first and then
/// smoothed out — an average over the whole response hides precisely that.
#[test]
fn a_recorded_response_is_revealed_evenly_from_the_start() {
    let base = Instant::now();
    let mut reveal = Reveal::default();
    reveal.begin(0);
    let thinking = replay(&mut reveal, base, 0, &TRACE_THINKING, 0, 1);
    reveal.begin(1);
    let answer = replay(&mut reveal, base, 4_000, &TRACE_TEXT, 1, 2);

    let early = &answer[..250.min(answer.len())];
    let settled = &answer[600..answer.len().saturating_sub(40)];
    let primed = answer.iter().take_while(|frame| !frame.painting).count();
    // The coldest start in a session: no rate measured, no lump known, and a four-character
    // opening delta. It is where "it stuttered at first and then smoothed out" lived.
    let cold_dry = thinking
        .iter()
        .skip_while(|frame| !frame.painting)
        .filter(|frame| frame.starved && frame.pacing)
        .count();
    let cold_primed = thinking.iter().take_while(|frame| !frame.painting).count();
    assert!(cold_primed > 0, "the opening delta of a session was painted on its own");
    assert_eq!(
        cold_dry, 0,
        "the first block of a session ran dry {cold_dry} times after its first paint"
    );
    let starved_after_first_paint =
        answer.iter().skip_while(|frame| !frame.painting).filter(|frame| frame.starved).count();
    println!(
        "reasoning {:.2}x | answer early {:.2}x | settled {:.2}x | primed {} frames | dry {}",
        fed_swing(&thinking),
        fed_swing(early),
        fed_swing(settled),
        primed,
        starved_after_first_paint
    );

    // Gather a buffer before painting anything, rather than showing the opening delta's single
    // character and freezing until the next arrival lands a quarter second later.
    let ceiling = (PRIME_MAX / 0.012) as usize + 2;
    assert!(primed <= ceiling, "the first paint waited {primed} frames (ceiling {ceiling})");
    assert!(primed > 0, "a one-character opening delta should not be painted on its own");

    assert!(fed_swing(early) <= 1.25, "the answer lurches as it starts: {:.2}x", fed_swing(early));
    assert!(
        fed_swing(settled) <= 1.5,
        "the answer pulses once settled: {:.2}x",
        fed_swing(settled)
    );
    // The coldest start there is: the first block of a session, seven arrivals long, so the
    // window never fills and the played rate is still ramping onto the measurement.
    assert!(fed_swing(&thinking) <= 1.7, "reasoning lurches: {:.2}x", fed_swing(&thinking));

    // Once painting starts, the only thing that empties the buffer is the model's own 2.15 s
    // pause, which no buffer this side of a second could cover.
    let pause_frames = ((2.150 - 0.7) / 0.012) as usize;
    assert!(
        starved_after_first_paint <= pause_frames,
        "{starved_after_first_paint} frames ran dry after the first paint (the model's own pause \
             accounts for about {pause_frames})"
    );
}

/// Spacing that jitters three times as much as anything measured still degrades gracefully:
/// the odd dry frame, not a stutter every cycle.
#[test]
fn a_provider_stuttering_under_load_degrades_gracefully() {
    let run = simulate(Source::hostile(), 8_000);
    let stalled = run.stall_fraction();
    assert!(stalled < 0.03, "{:.1}% of frames revealed nothing", stalled * 100.0);
}
