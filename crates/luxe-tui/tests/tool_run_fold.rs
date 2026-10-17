//! A folded run must not move the transcript, and must not strobe while it works.
//!
//! Both defects came from deciding what a run was by looking at the cards already on screen. Three
//! lookups landed as three cards and the third folded the two above it, so the conversation dropped
//! four rows under whoever was reading it; and the folded line asked "is a call running right now",
//! which is false in the gap between two of them, so its band flashed green once per call. The run
//! is a fact about the message that asked for it, and both tests here read as one consequence.

use luxe_core::config::{Config, ConversationConfig, ToolRuns};
use luxe_core::keymap::KeyMap;
use luxe_core::message::ContentBlock;
use luxe_core::{ToolActivity, ToolActivityState};
use luxe_tui::app::{App, ConvoBlock, ToolItem};
use luxe_tui::view::render;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

const WIDTH: u16 = 100;
const HEIGHT: u16 = 30;
/// Where the transcript is watched from: a line of the reader's own, above the run.
const MARKER: &str = "MARKERUSER";

fn app_with(tool_runs: Option<ToolRuns>) -> App {
    let (tx, _rx) = tokio::sync::mpsc::channel::<luxe_core::engine::Action>(64);
    let config = Config {
        conversation: ConversationConfig { tool_runs, ..Default::default() },
        ..Default::default()
    };
    App::new(tx, KeyMap::defaults(), config)
}

fn draw(app: &mut App, backend: &TestBackend) -> Buffer {
    let mut terminal = Terminal::new(backend.clone()).unwrap();
    terminal.draw(|f| render(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

fn row_text(buffer: &Buffer, y: u16) -> String {
    (0..WIDTH).map(|x| buffer[(x, y)].symbol()).collect()
}

/// The screen row the reader's own line is on, which is the thing that must not move.
fn marker_row(buffer: &Buffer) -> Option<u16> {
    (0..HEIGHT).find(|y| row_text(buffer, *y).contains(MARKER))
}

/// The wash behind the folded run's line.
fn run_band(buffer: &Buffer) -> Option<ratatui::style::Color> {
    (0..HEIGHT).find(|y| row_text(buffer, *y).contains("read")).map(|y| buffer[(4, y)].bg)
}

/// One message's worth of `read` calls, built the way the runtime builds them.
fn reads(count: u32) -> Vec<ToolActivity> {
    let content = (1..=count)
        .map(|n| ContentBlock::ToolCall {
            id: format!("call-{n}"),
            name: "read".into(),
            arguments: serde_json::json!({ "path": format!("file-{n}.rs") }),
        })
        .collect::<Vec<_>>();
    let runs = luxe_core::lookup_runs(&content);
    content
        .iter()
        .map(|block| {
            let ContentBlock::ToolCall { id, name, arguments } = block else {
                unreachable!("built from tool calls");
            };
            let label = luxe_core::describe_call(name, arguments, "/project");
            let mut activity = ToolActivity::running("s", id, name, label, 1);
            activity.input_preview = Some(arguments.to_string());
            activity.run = runs.get(id).copied();
            activity
        })
        .collect()
}

fn push(app: &mut App, activity: ToolActivity) {
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));
}

/// Marks the last card the way the call ending does.
fn finish(app: &mut App) {
    let last = app.convo.len() - 1;
    if let ConvoBlock::Tool(tool) = app.convo[last].block_mut() {
        tool.activity.state = ToolActivityState::Succeeded;
        tool.activity.summary = Some("Completed".into());
        tool.activity.elapsed_ms = Some(40);
        tool.activity.output_preview = Some("some output".into());
    }
}

/// Nothing above a run may move while the run happens.
///
/// The transcript is anchored to the bottom, so every row a fold reclaims is a row everything above
/// it drops by. Measured before the fix: four rows, in one frame, on the third card.
#[test]
fn a_run_never_moves_the_transcript_above_it() {
    let backend = TestBackend::new(WIDTH, HEIGHT);
    let mut app = app_with(None);
    app.push_convo(ConvoBlock::User {
        text: format!("{MARKER} find the thing"),
        attached: Vec::new(),
        chips: Vec::new(),
    });
    app.push_convo(ConvoBlock::Assistant("Let me look at the code.".into()));

    let mut rows = Vec::new();
    for activity in reads(7) {
        push(&mut app, activity);
        rows.push(marker_row(&draw(&mut app, &backend)));
        finish(&mut app);
        rows.push(marker_row(&draw(&mut app, &backend)));
    }

    let settled = rows[0];
    assert!(settled.is_some(), "the reader's own line has to be on screen to be watched");
    assert!(
        rows.iter().all(|row| *row == settled),
        "the transcript moved under the reader while the run ran: {rows:?}"
    );
}

/// A run that is still going keeps one band the whole way down.
///
/// Dispatch is sequential, so between one call ending and the next starting nothing is running —
/// which is not the same as finished. Reading it as finished washed the line green and back on
/// every call boundary: several flashes a second on the line whose job is to be the calm one.
#[test]
fn a_working_run_keeps_one_band() {
    let backend = TestBackend::new(WIDTH, HEIGHT);
    let mut app = app_with(None);

    let calls = reads(7);
    let last = calls.len() - 1;
    let mut bands = Vec::new();
    for (index, activity) in calls.into_iter().enumerate() {
        push(&mut app, activity);
        bands.push(run_band(&draw(&mut app, &backend)));
        finish(&mut app);
        // The last call settling settles the run, which has earned a different band.
        if index < last {
            bands.push(run_band(&draw(&mut app, &backend)));
        }
    }

    let working = bands[0];
    assert!(working.is_some(), "the folded run has to be on screen");
    assert!(
        bands.iter().all(|band| *band == working),
        "the run's band flickered while it worked: {bands:?}"
    );
    let done = run_band(&draw(&mut app, &backend));
    assert_ne!(done, working, "and a finished run stops wearing the working band");
}

/// Asking for cards gets cards, however many the message asked for.
#[test]
fn tool_runs_cards_draws_every_lookup() {
    let backend = TestBackend::new(WIDTH, HEIGHT);
    let mut app = app_with(Some(ToolRuns::Cards));
    for activity in reads(4) {
        push(&mut app, activity);
        finish(&mut app);
    }

    let buffer = draw(&mut app, &backend);
    let text = (0..HEIGHT).map(|y| row_text(&buffer, y)).collect::<Vec<_>>().join("\n");
    assert!(text.contains("file-4.rs"), "every call keeps its own card: {text}");
    assert!(!text.contains("read calls"), "and none of them was folded away: {text}");
}
