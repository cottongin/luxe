//! reveal — pacing streamed text so it lands evenly instead of in the provider's lumps.
//!
//! A provider does not hand over text at the rate it produced it. Measured on the wire: Anthropic
//! batches server-side and delivers one ~75-character delta every ~470 ms, while a local model
//! delivers 4 characters every 16 ms like clockwork. Painting each delta the instant it lands turns
//! the first into two visible jumps a second, and quantizes the second into frame-sized steps.
//!
//! So the transcript keeps a **cursor**: items ahead of it are drawn whole, the item under it is
//! drawn up to the cursor, and items behind it are not drawn at all. The cursor advances on the
//! frame clock at the rate the model is actually producing — a jitter buffer, not a chase. Draining
//! at a speed proportional to the backlog is the obvious answer and the wrong one: against 80-char
//! lumps it sprints after each arrival and crawls before the next.
//!
//! Two properties the design has to keep, and does:
//!
//! - **Ordering.** One cursor over the ordered transcript means a tool card cannot appear above prose
//!   that has not finished arriving. Cards animate nothing; they wait for the text ahead of them and
//!   then land, all in the same frame.
//! - **Bounded lag.** An item that is no longer the tail of a live stream can no longer grow, so it
//!   drains to a fixed deadline rather than being paced. Every completed message is therefore a
//!   resync point, and lag cannot accumulate across a turn.
//!
//! The trail that fades in behind the cursor says "these characters are still arriving". A frontier
//! that stops moving makes that a lie — the newest characters sit on the background, unreadable,
//! for as long as the stream is quiet — so the trail *settles* to full contrast whenever the cursor
//! stalls, and re-forms when text starts moving again.

mod rate;

#[cfg(test)]
use rate::NOMINAL_RATE;
use rate::RATE_WINDOW;
use std::time::Instant;

/// Buffer to hold, in arrivals' worth of text.
///
/// Expressed in *lumps* rather than milliseconds so the lag scales itself to how lumpy the provider
/// is: ~680 ms against ~95-char arrivals, ~26 ms against a local model's 4-char ones. It costs
/// nothing at the start of a message — the first characters still appear within a frame — and the
/// tail is flushed on a deadline, so what it really buys is that the *middle* comes out evenly.
///
/// **Well above one lump, on purpose.** The backlog oscillates by half a lump either side of this
/// target, and arrival *sizes* vary far more than their spacing does (measured: 56–136 characters
/// every 410–540 ms), so a run of small arrivals drains the buffer further still. A target that lets
/// the trough reach zero means a reveal with nothing left to show — it *stops*, which is the exact
/// stutter it exists to remove. Simulated against that cadence: 0.9 lumps stalls on 0.5% of frames
/// and its fastest tenth of a second runs 2.9× its slowest; 1.3 lumps still finds the odd dry spell;
/// 1.6 never stalls and holds the swing to 1.45×, which reads as breathing rather than lurching.
const TARGET_LUMPS: f64 = 1.6;
/// Seconds over which a buffer error is corrected.
///
/// Long, because the routine "error" is just the sawtooth of a lumpy source, and correcting *that*
/// aggressively is what makes the speed visibly pulse. Drift that matters is caught by the snap below
/// and by the drain at the end of every message, so this only has to handle slow bias.
const CORRECTION: f64 = 5.0;
/// Speed clamp around the rate estimate. Hard-bounds how far the buffer correction can move the
/// speed, so it can never read as a lurch — while leaving enough authority to walk off real drift.
const SLEW: (f64, f64) = (0.85, 1.25);
/// Seconds to empty the buffer once no more text can arrive. Linear, so the last words land
/// promptly instead of trailing off asymptotically.
const DRAIN: f64 = 0.22;
/// A backlog this many lumps deep did not stream: a resume, a replay, a paste. Show it at once.
const SNAP_LUMPS: f64 = 6.0;
/// Characters that can only have been bulk-loaded, whatever the current arrival size looks like.
///
/// The lump-relative snap needs a floor: a block's opening delta is one character, and six times
/// *that* would call the next real arrival a bulk load and dump it on screen.
const MIN_BULK: usize = 1_200;
/// Smallest buffer target, chars — a provider that ships one character at a time still needs one.
const MIN_TARGET: f64 = 2.0;
/// Smallest step, chars per frame. Keeps a very slow stream moving rather than looking hung.
const MIN_STEP: f64 = 0.3;
/// Buffer to gather before revealing anything, as a fraction of the target.
///
/// A block's opening delta is one character. Showing it the instant it lands means showing a single
/// letter and then freezing for the quarter second until the next arrival — worse than showing
/// nothing for that quarter second and then flowing. So the first paint waits for enough text to pace
/// against, which for a local model's 4-character arrivals is satisfied immediately and for a
/// batching provider costs the fraction of a second it was going to be behind by anyway.
const PRIME_FRACTION: f64 = 0.6;
/// Longest the first paint waits for that buffer.
///
/// A ceiling, not a target: a slow or stopped model must not be able to hold text off the screen. One
/// nominal gap, so it only binds on a provider that takes longer than that to send a second delta —
/// a local model clears it in its first 16 ms, and the recorded batching one at 495 ms clears it by
/// having a real buffer to pace instead.
const PRIME_MAX: f64 = 0.5;
/// Seconds the frontier must sit still before the trail starts resolving.
///
/// The buffer is what normally covers the gap between two arrivals, so this only fires when the
/// buffer is empty *and* nothing is coming: a model pausing mid-answer, a block that has ended
/// while its stream stays open, a provider that stops without closing. Long enough that an ordinary
/// hiccup never reaches it, short enough that nobody gets to read a half-invisible word.
const SETTLE_DELAY: f64 = 0.12;
/// Seconds for a stalled trail to reach full contrast.
const SETTLE_RISE: f64 = 0.18;
/// Seconds for the fade to re-form once text arrives again.
///
/// Quicker than the rise, because the two are not symmetric: a character that just landed should
/// look like it just landed, while a character that stopped being new can take its time.
const SETTLE_FALL: f64 = 0.08;
/// Cap on a frame's timestep. Without it, a frame after the app sat idle would reveal everything.
const MAX_DT: f64 = 0.1;
/// Timestep assumed for the first frame of an item, before there are two timestamps to subtract.
const FIRST_DT: f64 = 0.012;
/// Cursor value meaning "not revealing anything": every item draws whole.
const DISARMED: usize = usize::MAX;

/// How much of a transcript item this frame draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Show {
    /// All of it: the item is behind the cursor, or animates nothing.
    Whole,
    /// The first `n` rendered characters, with the newest of them fading in.
    Partial(usize),
    /// None of it: the item is ahead of the cursor and its turn has not come.
    Hidden,
}

/// One item's shape this frame, as the reveal needs to see it.
///
/// `chars` is counted in *rendered* characters — after markdown and wrapping — because that is what
/// the cursor indexes and what the eye reads. The reveal never sees the source text.
#[derive(Debug, Clone, Copy)]
pub struct Item {
    /// Position in the transcript.
    pub index: usize,
    /// Items in the transcript this frame.
    pub items: usize,
    /// Rendered characters the item currently has.
    pub chars: usize,
    /// Whether the item animates at all. Reasoning and prose do; cards, messages and system lines
    /// are immediate, and a collapsed reasoning peek has nothing to reveal.
    pub animated: bool,
    /// Whether more text may still arrive for it — it is the tail of a live stream.
    pub growing: bool,
}

/// How the cursor is moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Mode {
    /// Gathering the first buffer, before anything of this item is drawn.
    #[default]
    Priming,
    /// Paced against the estimated arrival rate.
    Paced,
    /// Catching up to everything that has arrived, because something wants the user's attention.
    Catchup,
}

/// The reveal cursor and the estimates that pace it.
///
/// All scalars: cheap to copy out of the app for a frame and copy back, which is how the render path
/// gets a mutable pacer while it still holds borrows of everything else.
#[derive(Debug, Clone, Copy)]
pub struct Reveal {
    mode: Mode,
    /// Item the cursor is inside. Everything before it is drawn whole, everything after is hidden.
    cursor: usize,
    /// Rendered characters of that item shown so far — fractional, so a slow rate still moves.
    shown: f64,
    /// Rendered characters of that item that had arrived when it was last looked at.
    available: f64,
    /// Items in the transcript when it was last looked at.
    items: usize,
    /// The last few arrivals as `(characters, seconds since the previous arrival)`.
    ///
    /// A ring: [`Self::record_arrival`] writes at `window_next` and the estimates read all of it. The
    /// gap is zero for an arrival whose spacing is unknowable — the first of a message.
    window: [(f32, f32); RATE_WINDOW],
    /// Entries of `window` that hold an arrival.
    window_len: usize,
    /// Where the next arrival is written.
    window_next: usize,
    /// The rate actually being played, closing on [`Self::rate`] a fraction each frame.
    played: f64,
    /// When the item under the cursor was first looked at, for the priming ceiling.
    opened: Option<Instant>,
    /// When the last arrival was seen, for the gap between them.
    last_arrival: Option<Instant>,
    /// When the cursor last moved, for the frame's timestep.
    last_step: Option<Instant>,
    /// Rate that empties the buffer inside [`DRAIN`], fixed when the drain starts so it stays linear.
    drain: Option<f64>,
    /// How resolved the trail is: 0 fades the newest characters out of the background, 1 draws every
    /// revealed character at full contrast.
    settle: f64,
    /// When the frontier ran out of text to show, for [`SETTLE_DELAY`]. `None` while it is moving.
    starved_since: Option<Instant>,
    /// When the settle was last advanced, for its own timestep.
    last_settle: Option<Instant>,
}

impl Default for Reveal {
    fn default() -> Self {
        Self {
            mode: Mode::default(),
            cursor: DISARMED,
            shown: 0.0,
            available: 0.0,
            items: 0,
            window: [(0.0, 0.0); RATE_WINDOW],
            window_len: 0,
            window_next: 0,
            played: 0.0,
            opened: None,
            last_arrival: None,
            last_step: None,
            drain: None,
            settle: 0.0,
            starved_since: None,
            last_settle: None,
        }
    }
}

impl Reveal {
    /// Starts revealing a freshly opened streaming item at `index`.
    ///
    /// Called when a live delta opens a new reasoning or prose block — never on replay, where the
    /// transcript is history and has nothing to animate. The rate and lump estimates survive: they
    /// describe the model, not the block.
    pub fn begin(&mut self, index: usize) {
        self.mode = Mode::Priming;
        self.cursor = index;
        self.shown = 0.0;
        self.available = 0.0;
        self.drain = None;
        self.last_step = None;
        self.opened = None;
        self.settle = 0.0;
        self.starved_since = None;
        self.last_settle = None;
        // A new message follows a tool call or a round trip, so the gap since the previous arrival
        // measures that wait rather than the model's pace. Skip it.
        self.last_arrival = None;
    }

    /// Shows everything that has arrived, now.
    ///
    /// For anything that needs the user rather than an animation: a permission prompt, a question, an
    /// error, their own keystroke, an interrupt, or scrolling away from the bottom where the reveal
    /// isn't even visible. Pacing resumes at the next message.
    pub fn catch_up(&mut self) {
        self.mode = Mode::Catchup;
        self.drain = None;
        // Catching up is for a reader who needs the text *now*, so the trail resolves outright
        // rather than over its usual fifth of a second. Without this an interrupt or a keystroke
        // dumps the whole message and then leaves its last words sitting on the background.
        self.settle = 1.0;
    }

    /// Whether anything is still waiting to be shown — the loop's cue to keep the reveal clock armed.
    ///
    /// Armed-but-not-yet-drawn counts: the first frame after [`Self::begin`] is what tells the reveal
    /// how much text the item even has, and it would not otherwise ask for that frame.
    pub fn pending(&self) -> bool {
        if self.cursor == DISARMED {
            return false;
        }
        // An unresolved trail counts: the frontier can be finished while the last characters are
        // still coming up out of the background, and nothing else would ask for those frames.
        // Bounded by construction — the settle reaches 1 within [`SETTLE_DELAY`] + [`SETTLE_RISE`]
        // and the clock then goes quiet.
        self.last_step.is_none()
            || self.shown < self.available
            || self.cursor + 1 < self.items
            || !self.settled()
    }

    /// How much of `item` to draw this frame, advancing the cursor when it is done with it.
    ///
    /// Called once per item per frame, in transcript order. Several items can complete in one frame:
    /// a run of tool cards behind finished prose all land together.
    pub fn show(&mut self, now: Instant, item: Item) -> Show {
        self.items = item.items;
        if item.index < self.cursor {
            return Show::Whole;
        }
        if item.index > self.cursor {
            return Show::Hidden;
        }
        // An item that animates nothing never holds the cursor: the card lands the moment the prose
        // above it is done, and a collapsed reasoning peek can't be typed out.
        if !item.animated {
            self.pass();
            return Show::Whole;
        }
        let bulk = self.observe(now, item.chars);
        if self.priming(now, item.growing, bulk) {
            return Show::Partial(0);
        }
        self.step(now, !item.growing, bulk);
        self.drift_settle(now);
        // Fully shown *and* unable to grow: the cursor has no more business here.
        if self.shown >= self.available && !item.growing {
            self.pass();
            return Show::Whole;
        }
        Show::Partial(self.shown as usize)
    }

    /// How resolved the trail is this frame: 0 fades the newest characters, 1 draws them all solid.
    pub fn settle(&self) -> f32 {
        self.settle as f32
    }

    /// Whether the trail has finished resolving.
    ///
    /// Judged on the value the renderer is actually handed, not the `f64` behind it. The two
    /// disagree at the top of the ramp — an accumulation landing on `0.999…` is still short of one
    /// in `f64` and exactly one once narrowed — and reading the wrong one leaves the frame clock
    /// running at 83fps forever against a picture that has stopped changing.
    fn settled(&self) -> bool {
        self.settle() >= 1.0
    }

    /// Resolves the trail while the frontier is stalled, and re-forms it when text moves again.
    ///
    /// The fade is a claim that these characters are still arriving. When the cursor stops the claim
    /// is false and the tail is just unreadable text, so a stall past [`SETTLE_DELAY`] lifts it to
    /// full contrast and the next character to arrive puts the fade back.
    fn drift_settle(&mut self, now: Instant) {
        let dt = match self.last_settle.map(|at| now.duration_since(at).as_secs_f64()) {
            Some(elapsed) => elapsed.clamp(0.0, MAX_DT),
            None => FIRST_DT,
        };
        self.last_settle = Some(now);

        if self.shown < self.available {
            self.starved_since = None;
        } else {
            self.starved_since.get_or_insert(now);
        }

        // Catch-up is not a stall the animation should recover from — it is an instruction to stop
        // animating, so the trail stays resolved until the next message arms the reveal again.
        let stalled = self
            .starved_since
            .is_some_and(|since| now.duration_since(since).as_secs_f64() >= SETTLE_DELAY);
        let target = if self.mode == Mode::Catchup || stalled { 1.0 } else { 0.0 };

        let span = if target > self.settle { SETTLE_RISE } else { SETTLE_FALL };
        let step = dt / span;
        self.settle = if target > self.settle {
            (self.settle + step).min(target)
        } else {
            (self.settle - step).max(target)
        };
    }

    /// Whether the first buffer is still being gathered, so nothing of this item is drawn yet.
    ///
    /// Ends on whichever comes first: enough text to pace against, the [`PRIME_MAX`] ceiling, the
    /// message finishing, or a bulk load. Never applies once something has been shown.
    fn priming(&mut self, now: Instant, growing: bool, bulk: bool) -> bool {
        if self.mode != Mode::Priming {
            return false;
        }
        let opened = *self.opened.get_or_insert(now);
        // With no interval measured there is no lump to size a buffer against, and the thing being
        // avoided is painting a lone opening character — so an unknown target must not end priming
        // instantly. The clock is the only honest condition until the second arrival lands.
        let target = match self.lump() {
            0.0 => f64::INFINITY,
            lump => (lump * TARGET_LUMPS * PRIME_FRACTION).max(MIN_TARGET),
        };
        let enough = self.available >= target;
        let waited = now.duration_since(opened).as_secs_f64() >= PRIME_MAX;
        if enough || waited || !growing || bulk {
            self.mode = Mode::Paced;
            return false;
        }
        true
    }

    /// Moves the cursor to the next item, disarming once it runs out of transcript.
    ///
    /// Disarming matters more than it looks: with the cursor parked on an index that does not exist
    /// yet, *anything* appended there would be gated — and a resumed session, a replayed turn or a
    /// compaction summary appends whole assistant messages at once. Those are history, not a stream,
    /// and typing them out would be absurd. [`Self::begin`] is the only thing that arms the reveal,
    /// and it is only called when a live delta opens a block.
    fn pass(&mut self) {
        self.cursor = if self.cursor + 1 < self.items { self.cursor + 1 } else { DISARMED };
        self.shown = 0.0;
        self.available = 0.0;
        self.drain = None;
        self.last_step = None;
        self.opened = None;
        self.starved_since = None;
        self.last_settle = None;
        // The item just released was drawn whole; the next one starts its own fade from nothing.
        if self.mode != Mode::Catchup {
            self.settle = 0.0;
        }
        // Only a live delta arms the reveal, so an item the cursor merely walks past is not primed —
        // but the one it stops on next is a fresh message and should be.
        if self.mode == Mode::Paced {
            self.mode = Mode::Priming;
        }
    }

    /// Folds this frame's character count in, recognizing growth as an arrival.
    ///
    /// Returns whether that growth was a **bulk load** rather than a stream: a resumed session, a
    /// replayed turn, a pasted wall of text. Such an arrival is not evidence about the model's
    /// pace, and folding it into the estimates would raise the lump so far that the snap meant to
    /// catch it never fires, and a resumed session would type itself out at a crawl.
    fn observe(&mut self, now: Instant, chars: usize) -> bool {
        let chars = chars as f64;
        let mut bulk = false;
        if chars > self.available {
            let arrived = chars - self.available;
            bulk = arrived > (self.lump() * SNAP_LUMPS).max(MIN_BULK as f64);
            if !bulk {
                let gap = self.last_arrival.map(|at| now.duration_since(at).as_secs_f64());
                self.record_arrival(arrived, gap);
                self.last_arrival = Some(now);
            }
        }
        self.available = chars;
        // A collapse, a reflow, or a commit can shorten an item. Never animate backwards.
        self.shown = self.shown.min(chars);
        bulk
    }

    /// Advances the cursor by one frame's worth.
    fn step(&mut self, now: Instant, complete: bool, bulk: bool) {
        let dt = match self.last_step.map(|at| now.duration_since(at).as_secs_f64()) {
            Some(elapsed) => elapsed.clamp(0.0, MAX_DT),
            None => FIRST_DT,
        };
        self.last_step = Some(now);
        let backlog = self.available - self.shown;
        if backlog <= 0.0 {
            return;
        }
        // Nothing to pace against: show it and move on.
        if self.mode == Mode::Catchup || bulk {
            self.shown = self.available;
            return;
        }
        let step = if complete {
            // Fixed once, so the finish is linear and lands on the deadline.
            *self.drain.get_or_insert(backlog / DRAIN) * dt
        } else if backlog > (self.lump() * SNAP_LUMPS).max(MIN_BULK as f64) {
            // A backlog this deep did not arrive at the pace we think it did — catch up rather than
            // falling further behind.
            backlog
        } else {
            let rate = self.track_rate();
            let target = (self.lump() * TARGET_LUMPS).max(MIN_TARGET);
            let commanded =
                (rate + (backlog - target) / CORRECTION).clamp(rate * SLEW.0, rate * SLEW.1);
            commanded * dt
        };
        self.shown = (self.shown + step.max(MIN_STEP)).min(self.available);
    }

    /// The cursor's item and how many of its characters are shown — for tests and diagnostics.
    #[cfg(test)]
    fn at(&self) -> (usize, usize) {
        (self.cursor, self.shown as usize)
    }
}

#[cfg(test)]
mod tests;
