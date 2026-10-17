//! Cursor — what the reveal cursor holds back, drains and fades.

use super::*;

#[test]
fn a_fresh_reveal_shows_everything() {
    let reveal = Reveal::default();
    let now = Instant::now();
    let mut reveal = reveal;
    assert_eq!(reveal.show(now, growing(0, 1, 100)), Show::Whole);
    assert!(!reveal.pending(), "nothing is being revealed, so nothing is pending");
}

#[test]
fn the_first_arrival_cannot_set_an_infinite_rate() {
    let base = Instant::now();
    let mut reveal = Reveal::default();
    reveal.begin(0);
    // 80 characters land in the first frame. Their gap is unknowable; a naive chars/gap would
    // be thousands of chars a second and reveal the lot at once.
    reveal.show(base, growing(0, 1, 80));
    assert_eq!(
        reveal.rate(),
        NOMINAL_RATE,
        "one arrival is not enough evidence to divide by, so the nominal rate stands"
    );
    let (_, shown) = reveal.at();
    assert!(shown < 10, "the first frame revealed {shown} of 80 characters");
}

/// The reveal's whole reason to exist: a lumpy source comes out evenly.
/// The buffer is bounded, and it does not drift.
///
/// The correction is deliberately slow, which is what keeps the speed steady — and the risk that
/// buys is a reveal that quietly falls further and further behind over a long message. It must
/// not: the lag in the second half of a message can be no worse than in the first.
#[test]
fn lag_stays_bounded_and_does_not_drift() {
    const LUMP: usize = 80;
    let base = Instant::now();
    let mut reveal = Reveal::default();
    reveal.begin(0);
    let mut arrived = 0usize;
    let (mut early, mut late) = (0.0f64, 0.0f64);
    // 40 arrivals of 80 chars, 39 frames apart: ~19 s of a single long message.
    for frame in 0..1_600u32 {
        let now = frames(base, frame);
        if frame % 39 == 0 && arrived < LUMP * 40 {
            arrived += LUMP;
        }
        reveal.show(now, growing(0, 1, arrived));
        let lag = arrived as f64 - reveal.shown;
        // Skip the warm-up, then compare the two halves.
        if (100..800).contains(&frame) {
            early = early.max(lag);
        } else if frame >= 800 {
            late = late.max(lag);
        }
    }
    let cap = LUMP as f64 * (TARGET_LUMPS + 0.9);
    assert!(early <= cap, "lag exceeded {cap:.0} chars early on: {early:.0}");
    assert!(late <= early + 16.0, "lag drifted: {early:.0} chars early, {late:.0} chars late");
}

#[test]
fn a_completed_item_drains_to_its_deadline_and_releases_the_cursor() {
    let base = Instant::now();
    let mut reveal = Reveal::default();
    reveal.begin(0);
    // A lumpy source, so there is a real buffer to drain: 80 chars every 39 frames (~470 ms).
    let mut arrived = 0usize;
    for frame in 0..120u32 {
        if frame % 39 == 0 {
            arrived += 80;
        }
        reveal.show(frames(base, frame), growing(0, 1, arrived));
    }
    // Close it right after an arrival, so the buffer is at its fullest and the deadline is the
    // thing under test rather than whatever the backlog happened to be.
    arrived += 80;
    let closed = Item { growing: false, ..growing(0, 2, arrived) };
    let backlog = arrived as f64 - reveal.shown;
    assert!(backlog > 40.0, "expected a full buffer to drain, had {backlog:.1} chars");

    let mut released = None;
    for frame in 120..400u32 {
        if reveal.show(frames(base, frame), closed) == Show::Whole {
            released = Some(frame - 120);
            break;
        }
    }
    let millis = released.expect("the cursor must move on once the item is drained") * 12;
    let deadline = (DRAIN * 1000.0) as u32;
    assert!(
        (deadline / 2..=deadline + 60).contains(&millis),
        "drain should land near the {deadline} ms deadline, took {millis} ms"
    );
}

#[test]
fn bulk_content_is_never_typed_out() {
    let base = Instant::now();
    let mut reveal = Reveal::default();
    reveal.begin(0);
    // Two arrivals to seed a lump estimate, then a compaction-sized dump.
    reveal.show(base, growing(0, 1, 40));
    reveal.show(frames(base, 30), growing(0, 1, 80));
    reveal.show(frames(base, 60), growing(0, 1, 20_000));
    assert_eq!(reveal.at().1, 20_000, "a bulk load appears at once");
    // ...and it left the estimates alone, so the *next* real stream is still paced.
    assert!(reveal.lump() < 200.0, "a bulk load is not evidence about the model's pace");
}

#[test]
fn an_item_that_animates_nothing_does_not_hold_the_cursor() {
    let now = Instant::now();
    let mut reveal = Reveal::default();
    reveal.begin(3);
    assert_eq!(reveal.show(now, settled(3, 5, 400)), Show::Whole);
    assert_eq!(reveal.at().0, 4, "the cursor moved straight past it");
}

/// History is not a stream: a resumed session, a replayed turn or a compaction summary appends
/// whole messages, and the reveal must not be sitting on the index they land at.
#[test]
fn running_out_of_transcript_disarms_the_cursor() {
    let base = Instant::now();
    let mut reveal = Reveal::default();
    reveal.begin(0);
    reveal.catch_up();
    // One item, revealed and closed: the cursor has nowhere left to go.
    assert_eq!(reveal.show(base, Item { growing: false, ..growing(0, 1, 200) }), Show::Whole);
    assert!(!reveal.pending(), "a disarmed reveal has nothing pending");

    // A whole assistant message lands at what used to be the next index.
    assert_eq!(reveal.show(frames(base, 1), growing(1, 2, 4_000)), Show::Whole);
    assert_eq!(reveal.at().1, 0, "it was not paced, it was just drawn");
}

/// The ordering property: a card cannot appear above prose that has not finished arriving.
#[test]
fn items_behind_the_cursor_stay_hidden_until_their_turn() {
    let base = Instant::now();
    let mut reveal = Reveal::default();
    reveal.begin(0);
    reveal.show(base, growing(0, 2, 400));
    assert_eq!(reveal.show(base, settled(1, 2, 3)), Show::Hidden, "the card waits");

    // Close the prose and let it drain; the card lands in the same frame the prose completes.
    let closed = Item { growing: false, ..growing(0, 2, 400) };
    let mut landed = false;
    for frame in 1..200u32 {
        let now = frames(base, frame);
        if reveal.show(now, closed) == Show::Whole {
            assert_eq!(reveal.show(now, settled(1, 2, 3)), Show::Whole);
            landed = true;
            break;
        }
    }
    assert!(landed, "the card must land once the prose above it is done");
}

#[test]
fn catch_up_shows_everything_that_arrived_and_cascades_through_the_items() {
    let base = Instant::now();
    let mut reveal = Reveal::default();
    reveal.begin(0);
    reveal.show(base, growing(0, 3, 500));
    assert!(reveal.at().1 < 500);

    reveal.catch_up();
    let now = frames(base, 1);
    // The prose catches up, and the two cards behind it land in the same frame.
    assert_eq!(reveal.show(now, Item { growing: false, ..growing(0, 3, 500) }), Show::Whole);
    assert_eq!(reveal.show(now, settled(1, 3, 4)), Show::Whole);
    assert_eq!(reveal.show(now, settled(2, 3, 4)), Show::Whole);
    assert!(!reveal.pending());
}

#[test]
fn pacing_resumes_on_the_next_message_after_a_catch_up() {
    let base = Instant::now();
    let mut reveal = Reveal::default();
    reveal.begin(0);
    reveal.catch_up();
    reveal.show(base, Item { growing: false, ..growing(0, 1, 200) });
    reveal.begin(1);
    reveal.show(frames(base, 1), growing(1, 2, 200));
    assert!(reveal.at().1 < 200, "the next message is paced again, not dumped");
}

#[test]
fn the_cursor_never_moves_backwards_when_an_item_shrinks() {
    let base = Instant::now();
    let mut reveal = Reveal::default();
    reveal.begin(0);
    for frame in 0..60u32 {
        reveal.show(frames(base, frame), growing(0, 1, 300));
    }
    let before = reveal.at().1;
    assert!(before > 0);
    // A reasoning block collapsed to a one-line peek: far fewer characters than were shown.
    reveal.show(frames(base, 61), growing(0, 1, 40));
    assert!(reveal.at().1 <= 40, "shown must clamp to what exists");
    assert!(reveal.shown >= 0.0);
}

/// The reported defect: a stream that goes quiet without closing left its last characters
/// sitting on the background, unreadable, for as long as the quiet lasted.
///
/// It happens wherever a block stops growing but stays open — a model pausing mid-answer, a
/// reasoning block waiting for the text block that follows it, a provider that stops sending
/// before it closes the stream.
#[test]
fn a_stalled_stream_resolves_its_trail() {
    let base = Instant::now();
    let mut reveal = Reveal::default();
    reveal.begin(0);
    // Two arrivals, then silence — with the item still flagged as growing throughout.
    reveal.show(base, growing(0, 1, 80));
    reveal.show(frames(base, 39), growing(0, 1, 160));
    let stalled = Item { growing: true, ..growing(0, 1, 160) };

    // Drain whatever is buffered, so the frontier is genuinely stalled rather than just slow.
    let mut frame = 40u32;
    while reveal.shown < reveal.available && frame < 400 {
        reveal.show(frames(base, frame), stalled);
        frame += 1;
    }
    assert!(frame < 400, "the buffer should drain long before this");
    assert_eq!(reveal.settle(), 0.0, "the trail is still faded the moment it catches up");
    assert!(reveal.pending(), "and the clock stays armed to finish resolving it");

    // Nothing more arrives. The trail must come up to full contrast, and promptly.
    let stalled_at = frame;
    let mut resolved = None;
    while resolved.is_none() && frame < stalled_at + 200 {
        reveal.show(frames(base, frame), stalled);
        if reveal.settle() >= 1.0 {
            resolved = Some(frame - stalled_at);
        }
        frame += 1;
    }
    let millis = resolved.expect("a stalled trail must resolve") * 12;
    let deadline = ((SETTLE_DELAY + SETTLE_RISE) * 1000.0) as u32;
    assert!(
        millis <= deadline + 40,
        "the tail stayed unreadable for {millis} ms (budget {deadline} ms)"
    );
    assert!(!reveal.pending(), "and then the clock goes quiet rather than spinning");
}

/// ...and the fade comes back the moment the model starts talking again.
#[test]
fn text_arriving_after_a_stall_puts_the_fade_back() {
    let base = Instant::now();
    let mut reveal = Reveal::default();
    reveal.begin(0);
    reveal.show(base, growing(0, 1, 80));
    reveal.show(frames(base, 39), growing(0, 1, 160));

    let stalled = Item { growing: true, ..growing(0, 1, 160) };
    let mut frame = 40u32;
    while reveal.settle() < 1.0 && frame < 400 {
        reveal.show(frames(base, frame), stalled);
        frame += 1;
    }
    assert_eq!(reveal.settle(), 1.0, "precondition: the trail resolved");

    // The stream picks up where it left off.
    for step in 0..12u32 {
        reveal.show(frames(base, frame + step), growing(0, 1, 160 + 80));
    }
    assert!(
        reveal.settle() < 1.0,
        "the newest characters should be fading again, not painted solid"
    );
}

/// A stall the user themselves caused is not something to animate out of: catching up means
/// "show me everything now", so the trail is legible in the same frame.
#[test]
fn catching_up_resolves_the_trail_immediately() {
    let base = Instant::now();
    let mut reveal = Reveal::default();
    reveal.begin(0);
    reveal.show(base, growing(0, 1, 500));

    reveal.catch_up();
    reveal.show(frames(base, 1), growing(0, 2, 500));
    assert_eq!(reveal.settle(), 1.0, "a caught-up transcript has nothing left fading");
}

/// A wall of text lands in one frame, which is a stalled frontier by any other name — so it
/// takes the same path out, and does not sit with its last words on the background.
#[test]
fn bulk_content_does_not_stay_half_faded() {
    let base = Instant::now();
    let mut reveal = Reveal::default();
    reveal.begin(0);
    reveal.show(base, growing(0, 1, 40));
    reveal.show(frames(base, 30), growing(0, 1, 80));
    reveal.show(frames(base, 60), growing(0, 1, 20_000));

    let dumped = growing(0, 1, 20_000);
    let budget = (((SETTLE_DELAY + SETTLE_RISE) / 0.012) as u32) + 4;
    for step in 1..=budget {
        reveal.show(frames(base, 60 + step), dumped);
    }
    assert_eq!(reveal.settle(), 1.0, "a pasted wall is not a stream tailing off");
}

#[test]
fn a_frame_after_a_long_pause_does_not_dump_the_buffer() {
    let base = Instant::now();
    let mut reveal = Reveal::default();
    reveal.begin(0);
    reveal.show(base, growing(0, 1, 60));
    // Five seconds later — the app was idle, or the terminal was suspended.
    reveal.show(base + Duration::from_secs(5), growing(0, 1, 400));
    assert!(reveal.at().1 < 100, "a huge timestep must not reveal everything");
}

/// The property the whole design turns on: a card waits for the text above it, and that wait
/// cannot accumulate across a turn.
///
/// Simulates a plausible turn — reason, speak, three cheap tool calls, speak again — driven the
/// way the render path drives it: walk the items in order every frame until one is hidden. Every
/// completed message drains to a deadline, and that is what makes it a resync point.
#[test]
fn a_card_waits_for_the_text_above_it_but_never_for_long() {
    /// 80 characters every 39 frames is the measured lumpy provider; 25 frames is its round trip.
    const LUMP: usize = 80;
    const GAP: u32 = 39;
    const TTFT: u32 = 25;
    /// Prose lengths and the cards between them, in the order the model produced them. `None` is
    /// a tool call.
    const SCRIPT: [Option<usize>; 8] =
        [Some(420), Some(240), None, None, None, Some(380), None, Some(260)];

    let base = Instant::now();
    let mut reveal = Reveal::default();
    // (total chars, chars arrived, is a card)
    let mut made: Vec<(usize, usize, bool)> = Vec::new();
    let mut made_at: Vec<u32> = Vec::new();
    let mut script = SCRIPT.into_iter();
    let mut next_event = 0u32;
    let mut growing = false;
    let mut released = 0usize;
    let mut card_waits: Vec<u32> = Vec::new();
    let mut produced_last_at = 0u32;
    let mut trailing = None;

    for frame in 0..3_000u32 {
        let now = base + Duration::from_millis(12) * frame;
        if frame >= next_event {
            match made.last_mut() {
                // A message still arriving gets its next lump.
                Some((total, arrived, false)) if *arrived < *total => {
                    *arrived = (*arrived + LUMP).min(*total);
                    growing = *arrived < *total;
                    next_event = frame + if growing { GAP } else { 0 };
                    produced_last_at = frame;
                }
                _ => match script.next() {
                    Some(Some(chars)) => {
                        made.push((chars, 0, false));
                        made_at.push(frame);
                        growing = true;
                        reveal.begin(made.len() - 1);
                        next_event = frame;
                        produced_last_at = frame;
                    }
                    Some(None) => {
                        made.push((0, 0, true));
                        made_at.push(frame);
                        growing = false;
                        // The next message follows the tool's run and a round trip.
                        next_event = frame + 2 + TTFT;
                        produced_last_at = frame;
                    }
                    None => next_event = u32::MAX,
                },
            }
        }

        let items = made.len();
        for (index, (total, arrived, is_card)) in made.iter().enumerate() {
            let item = Item {
                index,
                items,
                chars: *arrived,
                animated: !*is_card,
                growing: index + 1 == items && growing,
            };
            if reveal.show(now, item) == Show::Hidden {
                break;
            }
            // Fully drawn for the first time: record how long it waited to get there.
            if index == released && (*is_card || reveal.cursor > index) {
                if *is_card {
                    card_waits.push(frame - made_at[index]);
                }
                released += 1;
            }
            if *arrived < *total {
                break;
            }
        }
        if released == made.len() && next_event == u32::MAX {
            trailing = Some(frame.saturating_sub(produced_last_at) * 12);
            break;
        }
    }

    assert_eq!(released, made.len(), "every item is shown by the end of the turn");
    let trailing = trailing.expect("the turn finishes");
    assert!(trailing <= 320, "the transcript finished {trailing} ms after the model did");
    let worst = card_waits.iter().copied().max().unwrap_or(0) * 12;
    assert!(worst <= 320, "a card waited {worst} ms for the prose above it");
    // Anti-accumulation: the last card cannot have waited materially longer than the first.
    let (first, last) = (card_waits[0], card_waits[card_waits.len() - 1]);
    assert!(last <= first + GAP, "card waits grew across the turn: {first} then {last} frames");
}

#[test]
fn pending_tracks_whether_anything_is_left_to_show() {
    let base = Instant::now();
    let mut reveal = Reveal::default();
    reveal.begin(0);
    reveal.show(base, growing(0, 2, 400));
    assert!(reveal.pending(), "text is buffered and a card is hidden behind it");

    reveal.catch_up();
    let now = frames(base, 1);
    reveal.show(now, Item { growing: false, ..growing(0, 2, 400) });
    reveal.show(now, settled(1, 2, 4));
    assert!(!reveal.pending(), "everything is on screen");
}
