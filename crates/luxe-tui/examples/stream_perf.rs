//! Throwaway perf probe: cost of repainting while text streams in.
//!
//! Measures both halves of a frame: the layout/diff work (ratatui) and the bytes crossterm
//! actually writes to the terminal, for several reveal granularities.
//!
//! Run: `cargo run -p luxe-tui --release --example stream_perf`

use luxe_core::config::Config;
use luxe_core::keymap::KeyMap;
use luxe_tui::app::{App, ConvoBlock, Msg};
use luxe_tui::view::render;
use ratatui::{backend::CrosstermBackend, layout::Rect, Terminal, TerminalOptions, Viewport};
use std::cell::Cell;
use std::io::Write;
use std::rc::Rc;
use std::time::Instant;

/// Sinks bytes and counts them — stands in for the terminal so a frame's real escape-sequence
/// volume is measurable.
#[derive(Clone)]
struct Counter(Rc<Cell<usize>>);

impl Write for Counter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.set(self.0.get() + buf.len());
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

const W: u16 = 140;
const H: u16 = 44;

fn body() -> String {
    let mut s = String::new();
    for i in 0..12 {
        s.push_str(&format!(
            "Sentence {i} of a streamed answer, long enough to wrap across the prose canvas more \
             than once so the transcript keeps growing downward as it arrives. "
        ));
    }
    s
}

fn seed(turns: usize) -> App {
    let (tx, _rx) = tokio::sync::mpsc::channel(64);
    let mut app = App::new_fixture(tx, KeyMap::defaults(), Config::default());
    app.headless = true;
    app.update(Msg::TerminalResized { width: W, height: H });
    for i in 0..turns {
        app.push_convo(ConvoBlock::User {
            text: format!("question {i}"),
            attached: Vec::new(),
            chips: Vec::new(),
        });
        app.push_convo(ConvoBlock::Assistant(format!(
            "Answer {i}. Some prose with `code` and **bold** that wraps over a couple of rows in \
             the transcript, plus a second sentence for length."
        )));
    }
    app
}

/// Streams `text` into the app's tail block `chars_per_frame` at a time, drawing each step.
fn run(label: &str, turns: usize, chars_per_frame: usize) {
    let mut app = seed(turns);
    app.push_convo(ConvoBlock::Assistant(String::new()));
    let counter = Counter(Rc::new(Cell::new(0)));
    let backend = CrosstermBackend::new(counter.clone());
    let mut term = Terminal::with_options(
        backend,
        TerminalOptions { viewport: Viewport::Fixed(Rect::new(0, 0, W, H)) },
    )
    .unwrap();
    term.draw(|f| render(f, &mut app)).unwrap();
    counter.0.set(0);

    let text = body();
    let chars: Vec<char> = text.chars().collect();
    let mut frames = 0usize;
    let mut times = Vec::new();
    let mut bytes = Vec::new();
    for chunk in chars.chunks(chars_per_frame) {
        if let Some(ConvoBlock::Assistant(tail)) =
            app.convo.last_mut().map(luxe_tui::app::ConversationItem::block_mut)
        {
            tail.extend(chunk.iter());
        }
        let before = counter.0.get();
        let start = Instant::now();
        term.draw(|f| render(f, &mut app)).unwrap();
        times.push(start.elapsed().as_secs_f64() * 1000.0);
        bytes.push(counter.0.get() - before);
        frames += 1;
    }
    let total_bytes: usize = bytes.iter().sum();
    let mean_t = times.iter().sum::<f64>() / frames as f64;
    let mut sorted = times.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p95 = sorted[(frames * 95 / 100).min(frames - 1)];
    let mut bs = bytes.clone();
    bs.sort_unstable();
    println!(
        "{label:<46} frames {frames:4}  draw mean {mean_t:6.3} ms p95 {p95:6.3}  \
         bytes/frame p50 {:5} p95 {:6} max {:6}  total {:6} KiB",
        bs[frames / 2],
        bs[(frames * 95 / 100).min(frames - 1)],
        bs[frames - 1],
        total_bytes / 1024
    );
}

fn main() {
    println!("=== streaming {} chars into a transcript ({W}x{H}) ===", body().chars().count());
    for turns in [10usize, 60] {
        for cpf in [1usize, 2, 4, 8, 16, 80] {
            run(&format!("{turns} prior turns, {cpf} chars/frame"), turns, cpf);
        }
        println!();
    }
}
