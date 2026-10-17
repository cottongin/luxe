//! What one frame is allowed to allocate.
//!
//! Frame cost is the one property of a TUI that cannot be read off the source. A repaint that
//! looks linear may be memoized, and a memo that looks sound may have quietly stopped engaging —
//! and nothing fails until someone holds a key down on a long session and the screen stops moving.
//! That is exactly how the transcript came to re-parse, re-wrap and re-allocate its whole history
//! on every keystroke: three hundred thousand allocations a frame at four hundred turns, and no
//! test in the suite could tell.
//!
//! So the budget is **allocation count**, not wall time. It is identical on every machine and in
//! every CI runner, it does not flake, and it is the number that actually describes the mistake:
//! work proportional to the session rather than to the screen.
//!
//! A budget that fails is not automatically wrong. Read the number first — if a frame went from
//! 200 to 210 allocations because a surface gained a row, raise the ceiling and say so. If it went
//! to 200,000, something stopped being memoized.
//!
//! Allocation count cannot see everything, though, and the last test here is why. Guided Review
//! resolved a chapter's items by scanning every file's every hunk, once per chapter, on every
//! frame: quadratic in the size of the review, 23 ms a frame on a 784-file change — and it
//! allocated *nothing extra* doing it, so the counter above was flat across a sixteen-fold
//! increase in work. A scan is caught by the shape of the curve instead. Comparing one size
//! against another is safe where an absolute millisecond budget is not: the ratio divides the
//! machine out, so a slow runner moves both measurements together and cancels.

use luxe_core::config::Config;
use luxe_core::keymap::KeyMap;
use luxe_tui::app::{App, ConvoBlock, Msg};
use luxe_tui::fixture::{seeded_app, FixtureScenario};
use luxe_tui::view::render;
use ratatui::{backend::TestBackend, Terminal};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

// === Counting allocator ===

thread_local! {
    /// Allocations on *this* thread, so tests sharing a process cannot pollute each other.
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

/// Passes everything through to the system allocator and counts what went by.
struct Counting;

// SAFETY: every method forwards to `System` with its caller's own arguments, so it upholds exactly
// the contract `System` does; the counting beside it never allocates and cannot panic.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // `try_with`, because a thread being torn down no longer has the slot — and a panic
        // raised inside the allocator is not survivable.
        let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn counted<T>(body: impl FnOnce() -> T) -> (T, usize) {
    let before = ALLOCATIONS.with(Cell::get);
    let value = body();
    (value, ALLOCATIONS.with(Cell::get) - before)
}

// === Measurement ===

const WIDTH: u16 = 140;
const HEIGHT: u16 = 44;

/// Allocations in one steady-state frame — the cost of a keystroke that changed nothing.
///
/// The first frames are thrown away on purpose: they warm the syntax set, the highlight memo and
/// the transcript layout, and pricing a cold start as if it were a repaint is how a cache gets
/// declared useless.
fn steady_frame(app: &mut App, width: u16, height: u16) -> usize {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
    for _ in 0..3 {
        terminal.draw(|frame| render(frame, app)).expect("a warm-up frame");
    }
    // Two frames, halved: a single one can catch a Vec growing into its final capacity.
    let ((), allocations) = counted(|| {
        for _ in 0..2 {
            terminal.draw(|frame| render(frame, app)).expect("a frame");
        }
    });
    allocations / 2
}

fn chat_app(turns: usize) -> App {
    let (tx, _rx) = tokio::sync::mpsc::channel(64);
    let mut app = App::new_fixture(tx, KeyMap::defaults(), Config::default());
    app.headless = true;
    app.update(Msg::TerminalResized { width: WIDTH, height: HEIGHT });
    for index in 0..turns {
        app.push_convo(ConvoBlock::User {
            text: format!("question {index}"),
            attached: Vec::new(),
            chips: Vec::new(),
        });
        app.push_convo(ConvoBlock::Thinking {
            text: format!(
                "Reasoning {index}: consider the options, weigh them, then pick one, at about the \
                 length a reasoning block reaches before the answer starts."
            ),
            expanded: true,
        });
        app.push_convo(ConvoBlock::Assistant(format!(
            "Answer {index}: prose with `inline code`, **bold** and _emphasis_, long enough to \
             wrap across several rows of the conversation canvas.\n\n- first\n- second\n\n```rust\n\
             fn thing_{index}() -> usize {{ {index} }}\n```\n\nAnd a trailing paragraph."
        )));
    }
    app
}

// === The budgets ===

/// The one that matters: a frame's cost must not grow with the length of the session.
///
/// Typing changes nothing about the transcript, so a keystroke on a four-hundred-turn session must
/// cost what a keystroke on a twenty-turn session costs. The margin is generous — the walk that
/// checks whether each item moved is genuinely proportional to the item count — but it is nowhere
/// near enough to hide a relayout, which runs to hundreds of allocations *per item*.
#[test]
fn a_frame_does_not_scale_with_the_length_of_the_session() {
    let short = steady_frame(&mut chat_app(20), WIDTH, HEIGHT);
    let long = steady_frame(&mut chat_app(400), WIDTH, HEIGHT);

    assert!(
        long <= short + 100,
        "a 400-turn frame allocated {long} against a 20-turn frame's {short}; the transcript is \
         being laid out again instead of reused"
    );
}

/// And the absolute figure stays small, so "it did not get worse" cannot drift upward forever.
#[test]
fn a_steady_chat_frame_stays_within_its_budget() {
    const BUDGET: usize = 400;
    let allocations = steady_frame(&mut chat_app(150), WIDTH, HEIGHT);
    assert!(
        allocations <= BUDGET,
        "a steady chat frame allocated {allocations}, over its budget of {BUDGET}"
    );
}

/// Composing a long message must not cost per character.
///
/// The composer wraps the draft to answer three questions — how tall it is, what it draws, and
/// where the caret sits — and it once did that three times over, allocating a string per character
/// each time. A thousand-character draft is an ordinary message, and every keystroke redraws it.
#[test]
fn a_long_draft_does_not_cost_per_character() {
    const BUDGET: usize = 800;
    let mut app = chat_app(20);
    for _ in 0..2000 {
        app.editor.insert_text("x");
    }
    let allocations = steady_frame(&mut app, WIDTH, HEIGHT);
    assert!(
        allocations <= BUDGET,
        "a frame with a 2000-character draft allocated {allocations}, over its budget of {BUDGET}"
    );
}

/// Every other surface, held to a ceiling of its own.
///
/// These are bounded by what they show rather than by the session, so the numbers are small and
/// the point of the test is that they stay small — a sidebar that starts measuring its own wrapped
/// height per row is quadratic before anyone notices it slowed down.
#[test]
fn every_surface_stays_within_its_budget() {
    let budgets = [
        // The start screen draws the session's own composer and statusline under the wordmark now,
        // which is two of the chat frame's three parts; it was 400 as a menu
        (FixtureScenario::Dashboard, WIDTH, HEIGHT, 600usize),
        (FixtureScenario::GuidedReviewReady, 170, 48, 1_000),
        // Was 3,000, measured while the authorship guess folded every hunk here as "not written by
        // the agent" (the fixture's provenance names none of its files). With that gone the frame
        // draws the reading list it always should have; scaling is held by the linear test above.
        (FixtureScenario::GuidedReviewLarge, 170, 48, 4_300),
        (FixtureScenario::GuidedReviewGuideHidden, 170, 48, 800),
        // Higher than the other diff surfaces because the preview now honors `diff.layout`
        // like they do, so a wide one draws two columns where it used to draw one list.
        (FixtureScenario::ChangesWide, 170, 48, 2_400),
        (FixtureScenario::DockWide, 170, 48, 1_500),
        (FixtureScenario::DockOverlay, 120, 40, 1_400),
        (FixtureScenario::ChatTools, WIDTH, HEIGHT, 600),
        (FixtureScenario::ChatCardBodies, WIDTH, HEIGHT, 600),
    ];
    let mut over = Vec::new();
    let mut measured = Vec::new();
    for (scenario, width, height, budget) in budgets {
        let mut app = seeded_app("luxe", scenario).expect("the fixture");
        app.update(Msg::TerminalResized { width, height });
        let allocations = steady_frame(&mut app, width, height);
        measured.push(format!("{scenario:?}: {allocations}/{budget}"));
        if allocations > budget {
            over.push(format!("{scenario:?}: {allocations} allocations, budget {budget}"));
        }
    }
    eprintln!("frame budgets: {}", measured.join(" \u{b7} "));
    assert!(over.is_empty(), "surfaces over their frame budget: {}", over.join("; "));
}

/// Streaming redraws the tail. It must not redraw everything above it as well.
///
/// The reveal runs the loop at eighty frames a second while text arrives, so this is the frame
/// rate at which a transcript-wide relayout hurts most.
#[test]
fn streaming_a_reply_costs_the_tail_and_not_the_history() {
    let mut app = chat_app(150);
    app.push_convo(ConvoBlock::Assistant(String::new()));
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("a test terminal");
    for _ in 0..3 {
        terminal.draw(|frame| render(frame, &mut app)).expect("a warm-up frame");
    }

    let mut worst = 0usize;
    for chunk in ["Streaming ", "arrives in ", "pieces and ", "the tail ", "re-wraps."] {
        if let Some(ConvoBlock::Assistant(tail)) =
            app.convo.last_mut().map(luxe_tui::app::ConversationItem::block_mut)
        {
            tail.push_str(chunk);
        }
        let ((), allocations) = counted(|| {
            terminal.draw(|frame| render(frame, &mut app)).map(|_| ()).expect("a frame")
        });
        worst = worst.max(allocations);
    }

    const BUDGET: usize = 600;
    assert!(
        worst <= BUDGET,
        "the worst streaming frame allocated {worst}, over its budget of {BUDGET} — the history \
         above the tail is being laid out again"
    );
}

// === Scaling ===

/// The wall-clock cost of one Guided Review frame at `times` the large fixture.
///
/// The minimum of several runs, not the mean: noise only ever adds time, so the floor is the
/// stablest thing a loaded machine will report.
fn review_frame_ms(times: usize) -> f64 {
    use luxe_core::guided_review::ReviewLoadRequestId;
    use luxe_tui::app::ViewMode;

    let review = luxe_tui::fixture::fixture_guided_review_scaled(times);
    let mut app = seeded_app("luxe", FixtureScenario::GuidedReviewReady).expect("the fixture");
    app.current_view = ViewMode::GuidedReview;
    let request_id = ReviewLoadRequestId::new();
    app.active_guided_review_request = Some(request_id.clone());
    app.update(Msg::Engine(luxe_core::Event::GuidedReviewLoaded {
        request_id,
        review: Box::new(review),
    }));
    app.update(Msg::TerminalResized { width: 170, height: 48 });

    let mut terminal = Terminal::new(TestBackend::new(170, 48)).expect("a test terminal");
    for _ in 0..3 {
        terminal.draw(|frame| render(frame, &mut app)).expect("a warm-up frame");
    }
    (0..7)
        .map(|_| {
            let started = std::time::Instant::now();
            terminal.draw(|frame| render(frame, &mut app)).expect("a frame");
            started.elapsed().as_secs_f64() * 1000.0
        })
        .fold(f64::INFINITY, f64::min)
}

/// A Guided Review frame must not cost more than linearly in the size of the review.
///
/// The walkthrough shows one file's diff and a bounded rail however large the change is, so the
/// only thing that may grow with the review is the bookkeeping — and that has to be lookups rather
/// than scans. Eight times the review measured 3.5x the frame when this was written and 35x before
/// the ids were indexed, so the ceiling sits an order of magnitude clear of the first and a third
/// of the way to the second.
///
/// A ratio rather than a millisecond budget on purpose: it divides the machine out, so a loaded CI
/// runner moves both measurements together instead of failing the test.
#[test]
fn a_guided_review_frame_does_not_scale_worse_than_linearly() {
    let small = review_frame_ms(2);
    let large = review_frame_ms(16);
    let ratio = large / small;

    assert!(
        ratio < 12.0,
        "eight times the review cost {ratio:.1}x the frame ({small:.3} ms -> {large:.3} ms); \
         something in the review path is scanning where it should be looking up"
    );
}
