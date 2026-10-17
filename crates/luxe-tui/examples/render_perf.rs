//! What one frame costs, on every surface that draws one.
//!
//! Two numbers per surface: the wall clock, and the **allocation count**. The second is the one
//! that travels — it is identical on any machine, so a claim made here can be checked in review,
//! and a frame whose allocations scale with the transcript is a frame that re-derives it.
//!
//! Run: `cargo run -p luxe-tui --release --example render_perf`

use luxe_core::config::Config;
use luxe_core::keymap::KeyMap;
use luxe_tui::app::{App, ConvoBlock, Msg};
use luxe_tui::fixture::{seeded_app, FixtureScenario};
use luxe_tui::view::render;
use ratatui::{backend::TestBackend, Terminal};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

// === Allocation counting ===

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Passes everything through to the system allocator and counts what went by.
struct Counting;

// SAFETY: every method forwards to `System` with its caller's own arguments, so it upholds exactly
// the contract `System` does; the counting beside it never allocates and cannot panic.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(new_size.saturating_sub(layout.size()), Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn counters() -> (usize, usize) {
    (ALLOCATIONS.load(Ordering::Relaxed), ALLOCATED_BYTES.load(Ordering::Relaxed))
}

// === Fixtures ===

const WIDTH: u16 = 140;
const HEIGHT: u16 = 44;

fn prose(index: usize) -> String {
    format!(
        "Paragraph {index}: the streaming path re-lays out the whole transcript every frame, which \
         means markdown parsing, syntax highlighting and wrapping all happen again. This is a \
         realistic length for a model answer with `inline code`, **bold**, and _emphasis_ mixed \
         in, spanning several wrapped rows at a typical terminal width.\n\nA second paragraph so \
         the block is not trivially short, with a list:\n\n- first item\n- second item\n- third \
         item\n"
    )
}

fn code(index: usize) -> String {
    format!(
        "Here is a snippet:\n\n```rust\nfn thing_{index}(input: &str) -> usize {{\n    let mut \
         total = 0usize;\n    for (index, ch) in input.char_indices() {{\n        if \
         ch.is_alphanumeric() {{\n            total += index;\n        }}\n    }}\n    total\n}}\n\
         ```\n\nAnd some trailing prose after the fence.\n"
    )
}

fn chat_app(turns: usize, with_code: bool) -> App {
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
                "Reasoning {index}: consider the options, weigh them, then pick one. This is the \
                 sort of length a reasoning block reaches before the answer starts streaming."
            ),
            expanded: true,
        });
        if with_code && index % 3 == 0 {
            app.push_convo(ConvoBlock::Assistant(code(index)));
        } else {
            app.push_convo(ConvoBlock::Assistant(prose(index)));
        }
    }
    app
}

// === Measurement ===

struct Reading {
    mean_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    max_ms: f64,
    allocations: usize,
    kib: f64,
}

/// Draws `frames` steady-state frames and reports what one of them cost.
///
/// The first draw is thrown away: it warms the syntax set, the highlight memo and any layout cache,
/// and pricing a cold start as if it were a repaint is how a cache gets called useless.
fn measure(app: &mut App, width: u16, height: u16, frames: usize) -> Reading {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("a test terminal");
    for _ in 0..2 {
        terminal.draw(|frame| render(frame, app)).expect("a warm-up frame");
    }

    let (allocations_before, bytes_before) = counters();
    let mut samples = Vec::with_capacity(frames);
    for _ in 0..frames {
        let start = Instant::now();
        terminal.draw(|frame| render(frame, app)).expect("a frame");
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let (allocations_after, bytes_after) = counters();

    samples.sort_by(|left, right| left.partial_cmp(right).expect("no NaN timings"));
    Reading {
        mean_ms: samples.iter().sum::<f64>() / samples.len() as f64,
        p50_ms: samples[samples.len() / 2],
        p95_ms: samples[(samples.len() * 95 / 100).min(samples.len() - 1)],
        max_ms: samples[samples.len() - 1],
        allocations: (allocations_after - allocations_before) / frames,
        kib: (bytes_after - bytes_before) as f64 / frames as f64 / 1024.0,
    }
}

fn report(label: &str, reading: &Reading) {
    println!(
        "{label:<38} mean {:7.3} ms  p50 {:7.3}  p95 {:7.3}  max {:7.3}   allocs {:8}  {:8.0} KiB",
        reading.mean_ms,
        reading.p50_ms,
        reading.p95_ms,
        reading.max_ms,
        reading.allocations,
        reading.kib
    );
}

/// One Guided Review frame at `times` the large fixture.
///
/// The other surfaces are bounded by what they draw; a review is bounded by the change under
/// review, and the walkthrough draws one file's diff however large that is. So the question here
/// is not the absolute figure but the *shape*: anything that grows faster than the review does is
/// a scan where a lookup belongs.
fn review_at_scale(times: usize) {
    use luxe_core::guided_review::ReviewLoadRequestId;
    use luxe_tui::app::ViewMode;

    let review = luxe_tui::fixture::fixture_guided_review_scaled(times);
    let files = review.files.len();
    let hunks: usize = review.files.iter().map(|file| file.hunks.len()).sum();
    let Ok(mut app) = seeded_app("luxe", FixtureScenario::GuidedReviewReady) else {
        return;
    };
    app.current_view = ViewMode::GuidedReview;
    let request_id = ReviewLoadRequestId::new();
    app.active_guided_review_request = Some(request_id.clone());
    app.update(Msg::Engine(luxe_core::Event::GuidedReviewLoaded {
        request_id,
        review: Box::new(review),
    }));
    app.update(Msg::TerminalResized { width: 170, height: 48 });
    let reading = measure(&mut app, 170, 48, 40);
    report(&format!("{times}x large review ({files} files, {hunks} hunks)"), &reading);
}

fn scenario(label: &str, which: FixtureScenario, width: u16, height: u16) {
    let Ok(mut app) = seeded_app("luxe", which) else {
        println!("{label:<38} (fixture unavailable)");
        return;
    };
    app.update(Msg::TerminalResized { width, height });
    let reading = measure(&mut app, width, height, 120);
    report(label, &reading);
}

fn main() {
    println!("=== chat transcript, idle (the keystroke frame) ===");
    for turns in [1usize, 5, 20, 50, 150, 400] {
        let mut app = chat_app(turns, true);
        let reading = measure(&mut app, WIDTH, HEIGHT, if turns > 150 { 40 } else { 200 });
        report(&format!("{turns} turns ({} items)", turns * 3), &reading);
    }

    println!();
    println!("=== chat transcript, scrolled off the bottom ===");
    for turns in [50usize, 150] {
        let mut app = chat_app(turns, true);
        app.scroll_state.up_by(40);
        let reading = measure(&mut app, WIDTH, HEIGHT, 120);
        report(&format!("{turns} turns, scrolled"), &reading);
    }

    println!();
    println!("=== composing into the prompt ===");
    for characters in [0usize, 80, 400, 2000] {
        let mut app = chat_app(20, true);
        for _ in 0..characters {
            app.editor.insert_text("x");
        }
        let reading = measure(&mut app, WIDTH, HEIGHT, 120);
        report(&format!("{characters}-character draft"), &reading);
    }

    println!();
    println!("=== other surfaces ===");
    scenario("dashboard", FixtureScenario::Dashboard, WIDTH, HEIGHT);
    scenario("guided review (standard)", FixtureScenario::GuidedReviewReady, 170, 48);
    scenario("guided review (large corpus)", FixtureScenario::GuidedReviewLarge, 170, 48);
    scenario("guided review (split)", FixtureScenario::GuidedReviewSplit, 170, 48);
    scenario("guided review (no guide)", FixtureScenario::GuidedReviewGuideHidden, 170, 48);
    scenario("changes pane (wide)", FixtureScenario::ChangesWide, 170, 48);
    scenario("dock (side by side)", FixtureScenario::DockWide, 170, 48);
    scenario("dock (overlay)", FixtureScenario::DockOverlay, 120, 40);
    scenario("chat + tool cards", FixtureScenario::ChatTools, WIDTH, HEIGHT);
    scenario("chat + card bodies", FixtureScenario::ChatCardBodies, WIDTH, HEIGHT);
    scenario("chat streaming (mid-reveal)", FixtureScenario::ChatStreaming, WIDTH, HEIGHT);

    println!();
    println!("=== guided review, against the size of the review ===");
    for times in [1usize, 2, 4, 8, 16] {
        review_at_scale(times);
    }

    println!();
    println!("=== markdown + highlight isolation ===");
    let styles = {
        let registry = luxe_core::theme::ThemeRegistry::new();
        let resolved = registry.resolve("dark").expect("the dark theme");
        luxe_tui::theme::styles::ThemeStyles::from_resolved(&resolved, "dark")
    };
    for (label, text) in [("prose block", prose(1)), ("code block", code(1))] {
        let start = Instant::now();
        let (allocations, _) = counters();
        for _ in 0..1000 {
            std::hint::black_box(luxe_tui::markdown::render_markdown(&text, &styles));
        }
        let per_call = (counters().0 - allocations) / 1000;
        println!(
            "render_markdown({label:<11}) x1000: {:?}  ({per_call} allocs/call)",
            start.elapsed()
        );
    }
}
