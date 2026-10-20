//! Reveal — streamed text paced onto the screen, and what never waits.

use super::*;

/// One provider lump, the size Anthropic actually delivers (~75 characters at a time).
pub(super) const LUMP: &str =
    "alpha the provider hands you text in lumps and the render loop paints omega";
/// Characters of `text` a frame is showing, ignoring everything between them.
///
/// Letters only: wrapping breaks the text across rows, and the chrome those rows end with — a
/// scrollbar lane, a panel border — would otherwise interrupt the match.
pub(super) fn revealed_count(frame: &str, text: &str) -> usize {
    let flat: String = frame.chars().filter(char::is_ascii_alphanumeric).collect();
    let target: Vec<char> = text.chars().filter(char::is_ascii_alphanumeric).collect();
    (0..=target.len())
        .rev()
        .find(|n| flat.contains(&target[..*n].iter().collect::<String>()))
        .unwrap_or(0)
}

/// Runs the reveal forward the way the event loop does: one frame every 12 ms.
///
/// `at` is the frame the clock has reached and is advanced in place, so successive calls continue
/// rather than replaying from the start. Jumping the clock instead of stepping it would prove nothing
/// — a frame's timestep is capped precisely so that a suspended terminal cannot dump the whole buffer
/// on its next frame.
pub(super) fn advance_frames(
    app: &mut App,
    backend: &TestBackend,
    base: std::time::Instant,
    at: &mut u32,
    frames: u32,
) -> String {
    let mut painted = String::new();
    for _ in 0..frames {
        *at += 1;
        app.status.frame_clock = Some(base + std::time::Duration::from_millis(12) * *at);
        painted = buffer_to_string(&render_app(app, backend));
    }
    painted
}

/// Streamed prose is paced onto the screen instead of landing in the provider's lumps.
#[test]
fn streamed_prose_is_revealed_as_it_is_paced() {
    let backend = test_backend(60, 20);
    let mut app = create_app();
    let base = std::time::Instant::now();
    app.status.frame_clock = Some(base);
    let total = LUMP.chars().filter(|c| !c.is_whitespace()).count();

    app.update(Msg::Engine(luxe_core::event::Event::TokenDelta { block: 0, delta: LUMP.into() }));
    let mut at = 0;
    // Nothing yet: the reveal gathers a buffer before its first paint, rather than showing the
    // opening characters and freezing until the next arrival lands.
    let first = buffer_to_string(&render_app(&mut app, &backend));
    assert_eq!(revealed_count(&first, LUMP), 0, "the first paint waits for a buffer: {first}");

    // Past the priming ceiling it starts painting — and paces, rather than dropping the lot.
    let started = advance_frames(&mut app, &backend, base, &mut at, 45);
    let seen = revealed_count(&started, LUMP);
    assert!(seen > 0, "painting has started: {started}");
    assert!(seen < total, "but it stays behind the text that arrived: {seen} of {total}");

    // A few frames on, more of it has resolved.
    let midway = advance_frames(&mut app, &backend, base, &mut at, 10);
    let more = revealed_count(&midway, LUMP);
    assert!(more > seen, "the reveal advances with the clock: {more} vs {seen}");

    // The turn ends, so nothing more can arrive: the buffer drains to its deadline.
    app.update(Msg::Engine(luxe_core::event::Event::TurnEnded {
        reason: luxe_core::event::StopReason::EndTurn,
    }));
    let done = advance_frames(&mut app, &backend, base, &mut at, 60);
    assert_eq!(revealed_count(&done, LUMP), total, "a finished message is all there: {done}");
    assert!(!app.reveal.pending(), "and the reveal has nothing left to show");
}

/// The ordering rule, end to end: a card cannot appear above prose that is still arriving.
#[test]
fn a_tool_card_waits_for_the_prose_above_it() {
    let backend = test_backend(60, 20);
    let mut app = create_app();
    let base = std::time::Instant::now();
    app.status.frame_clock = Some(base);

    app.update(Msg::Engine(luxe_core::event::Event::TokenDelta { block: 0, delta: LUMP.into() }));
    let mut activity = ToolActivity::running("session-a", "call-read", "read", "src/lib.rs", 1);
    activity.state = ToolActivityState::Succeeded;
    app.push_convo(ConvoBlock::Tool(Box::new(ToolItem {
        activity,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    })));

    let held = buffer_to_string(&render_app(&mut app, &backend));
    assert!(!held.contains("src/lib.rs"), "the card waits its turn: {held}");

    // The card being there at all means the message is complete, so the prose drains and both land.
    let mut at = 0;
    let landed = advance_frames(&mut app, &backend, base, &mut at, 90);
    assert!(landed.contains("src/lib.rs"), "and lands once the prose is done: {landed}");
    assert_eq!(
        revealed_count(&landed, LUMP),
        LUMP.chars().filter(|c| !c.is_whitespace()).count(),
        "with the prose complete above it: {landed}"
    );
}

/// Reasoning shown as a one-line peek has nothing to type out, and must never gate what follows.
#[test]
fn collapsed_reasoning_is_never_paced() {
    let config = Config {
        conversation: luxe_core::config::ConversationConfig {
            thinking: Some(luxe_core::config::ThinkingDisplay::Collapsed),
            ..Default::default()
        },
        ..Default::default()
    };
    let backend = test_backend(60, 20);
    let mut app = create_app_with_config(config);
    app.status.frame_clock = Some(std::time::Instant::now());
    app.update(Msg::Engine(luxe_core::event::Event::ThinkingDelta {
        block: 0,
        delta: "weighing the options before answering".into(),
    }));
    let frame = buffer_to_string(&render_app(&mut app, &backend));
    assert!(frame.contains("weighing the options"), "the peek is drawn at once: {frame}");
}

/// `stream_reveal: off` is a real off: the same instant paint as before the reveal existed.
#[test]
fn stream_reveal_off_paints_every_arrival_immediately() {
    let config = Config {
        conversation: luxe_core::config::ConversationConfig {
            stream_reveal: Some(luxe_core::config::StreamReveal::Off),
            ..Default::default()
        },
        ..Default::default()
    };
    let backend = test_backend(60, 20);
    let mut app = create_app_with_config(config);
    app.status.frame_clock = Some(std::time::Instant::now());
    app.update(Msg::Engine(luxe_core::event::Event::TokenDelta { block: 0, delta: LUMP.into() }));
    let frame = buffer_to_string(&render_app(&mut app, &backend));
    assert!(frame.contains("alpha") && frame.contains("omega"), "all of it, at once: {frame}");
}

/// Anything that needs the user stops animating around them.
#[test]
fn a_permission_prompt_shows_everything_that_arrived() {
    let backend = test_backend(60, 24);
    let mut app = create_app();
    app.status.frame_clock = Some(std::time::Instant::now());
    app.update(Msg::Engine(luxe_core::event::Event::TokenDelta { block: 0, delta: LUMP.into() }));
    let total = LUMP.chars().filter(char::is_ascii_alphanumeric).count();
    let held = buffer_to_string(&render_app(&mut app, &backend));
    assert!(revealed_count(&held, LUMP) < total, "mid-reveal to begin with: {held}");

    app.update(Msg::Engine(luxe_core::event::Event::PermissionRequested {
        id: luxe_core::permission::PermissionRequestId::new(),
        session_id: "session-a".into(),
        tool: "bash".into(),
        permission: "bash".into(),
        summary: "Run: rm -rf /etc".into(),
        always_patterns: Vec::new(),
    }));
    let asked = buffer_to_string(&render_app(&mut app, &backend));
    assert!(asked.contains("Permission required"), "the prompt is up: {asked}");
    // Nothing is held back behind it either — dismissing it has to reveal a whole message rather
    // than the half sentence the reveal happened to be on.
    assert!(!app.reveal.pending(), "a question is not asked over text that is still arriving");

    app.update(Msg::Engine(luxe_core::event::Event::TurnEnded {
        reason: luxe_core::event::StopReason::EndTurn,
    }));
    let answered = buffer_to_string(&render_app(&mut app, &backend));
    assert_eq!(revealed_count(&answered, LUMP), total, "all of it is there: {answered}");
}

/// A prompt grows up from the composer over a `Clear`, so it used to paint out whatever the
/// transcript had last drawn there — which is the output it is asking about. The conversation
/// stands down by the rows the panel takes instead, and both are readable at once.
#[test]
fn a_prompt_above_the_composer_does_not_cover_the_output_that_prompted_it() {
    let backend = test_backend(60, 30);
    let ask = |app: &mut App, event| {
        app.update(Msg::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        app.update(Msg::Engine(event));
        buffer_to_string(&render_app(app, &backend))
    };

    let mut app = create_app();
    // Longer than the viewport, so the tail sits at the bottom whichever way the transcript grows.
    for index in 0..40 {
        app.push_convo(ConvoBlock::Assistant(format!("reasoning line {index}")));
    }
    // The composer is the last panel on screen, so its top border is the last one drawn.
    let composer_row =
        |frame: &str| frame.lines().collect::<Vec<_>>().iter().rposition(|row| row.contains('╭'));
    let idle = buffer_to_string(&render_app(&mut app, &backend));
    assert!(idle.contains("reasoning line 39"), "the tail is on screen to begin with: {idle}");
    let composer = composer_row(&idle).expect("a composer");

    let asked = ask(
        &mut app,
        luxe_core::event::Event::QuestionAsked {
            id: luxe_core::question::QuestionRequestId::new(),
            session_id: "session-a".into(),
            prompt: "Which runtime should the worker use?".into(),
            options: vec!["tokio".into(), "async-std".into()],
            multi: false,
        },
    );
    assert!(asked.contains("Which runtime"), "the question is up: {asked}");
    assert!(asked.contains("reasoning line 39"), "and so is what prompted it: {asked}");

    let approved = ask(
        &mut app,
        luxe_core::event::Event::PermissionRequested {
            id: luxe_core::permission::PermissionRequestId::new(),
            session_id: "session-a".into(),
            tool: "bash".into(),
            permission: "bash".into(),
            summary: "Run: rm -rf build".into(),
            always_patterns: Vec::new(),
        },
    );
    assert!(approved.contains("Permission required"), "the ask is up: {approved}");
    assert!(approved.contains("reasoning line 39"), "and so is what prompted it: {approved}");

    // The rows come out of the conversation, never out of the prompt the reader types into.
    for frame in [&asked, &approved] {
        assert_eq!(composer_row(frame), Some(composer), "the composer moved: {frame}");
    }

    // A terminal with no rows to spare keeps the prompt whole and the composer intact — the
    // conversation can only stand down from rows it has.
    let cramped = buffer_to_string(&render_app(&mut app, &test_backend(60, 12)));
    assert!(cramped.contains("Allow once"), "the prompt is not squeezed: {cramped}");
    let rows: Vec<&str> = cramped.lines().collect();
    let top = composer_row(&cramped).expect("a composer");
    assert!(rows[top + 2].contains('╰'), "the composer kept its rows: {cramped}");
}

/// Scrolled away from the bottom there is no animation to see, so nothing is held back — and coming
/// back to the bottom shows the whole message rather than a stale prefix of it.
#[test]
fn scrolling_away_from_the_bottom_stops_holding_text_back() {
    let backend = test_backend(60, 10);
    let mut app = create_app();
    // Enough transcript to have somewhere to scroll.
    for index in 0..8 {
        app.push_convo(ConvoBlock::User {
            text: format!("earlier {index}"),
            attached: Vec::new(),
            chips: Vec::new(),
        });
    }
    let base = std::time::Instant::now();
    app.status.frame_clock = Some(base);
    app.update(Msg::Engine(luxe_core::event::Event::TokenDelta { block: 0, delta: LUMP.into() }));
    let _ = render_app(&mut app, &backend);

    app.scroll_state.up_by(3);
    assert!(!app.scroll_state.follow, "scrolling up leaves the bottom");
    let _ = render_app(&mut app, &backend);
    assert!(!app.reveal.pending(), "nothing is held back while the tail is off screen");

    app.scroll_state.to_bottom();
    let back = buffer_to_string(&render_app(&mut app, &backend));
    assert_eq!(
        revealed_count(&back, LUMP),
        LUMP.chars().filter(|c| !c.is_whitespace()).count(),
        "coming back to the bottom shows the whole message: {back}"
    );
}

/// Where `needle` starts in the rendered buffer, as `(x, y)`.
pub(super) fn find_in_buffer(buffer: &Buffer, needle: &str) -> Option<(u16, u16)> {
    (0..buffer.area.height).find_map(|y| {
        let row: String = (0..buffer.area.width).map(|x| buffer[(x, y)].symbol()).collect();
        row.find(needle).map(|byte| (row[..byte].chars().count() as u16, y))
    })
}

/// The bug: a stream that goes quiet without closing left its last word painted on the canvas.
///
/// The trail that fades in behind the reveal cursor says "these characters are still arriving".
/// When the frontier catches up and nothing follows — a model pausing mid-answer, a block waiting
/// on the one after it, a provider that stops before it closes — the claim stops being true and the
/// newest characters are simply text the reader cannot see. Measured here the way a reader meets
/// it: the foreground the terminal is actually told to paint.
#[test]
fn a_stalled_stream_resolves_its_last_characters() {
    let backend = test_backend(110, 20);
    let mut app = create_app();
    let base = std::time::Instant::now();
    app.status.frame_clock = Some(base);
    let total = LUMP.chars().filter(|c| !c.is_whitespace()).count();
    app.update(Msg::Engine(luxe_core::event::Event::TokenDelta { block: 0, delta: LUMP.into() }));

    // Pace it out to the end of the lump. Nothing closes the stream, so the item stays growing.
    let mut at = 0u32;
    for _ in 0..400 {
        let painted = advance_frames(&mut app, &backend, base, &mut at, 1);
        if revealed_count(&painted, LUMP) == total {
            break;
        }
    }

    let buffer = render_app(&mut app, &backend);
    let (head_x, head_y) = find_in_buffer(&buffer, "alpha").expect("the settled start is drawn");
    let (tail_x, tail_y) = find_in_buffer(&buffer, "omega").expect("the newest word is drawn");
    let tail = (tail_x + 4, tail_y);
    let settled_fg = buffer[(head_x, head_y)].fg;

    assert_ne!(
        buffer[tail].fg, settled_fg,
        "precondition: the character that just landed is still fading in"
    );

    // Nothing more arrives. The tail has to become readable, and on its own.
    let budget = 40;
    advance_frames(&mut app, &backend, base, &mut at, budget);
    let buffer = render_app(&mut app, &backend);

    assert_eq!(
        buffer[tail].fg, settled_fg,
        "the last character of a stalled stream is still painted on the background"
    );
    assert!(
        !app.reveal.pending(),
        "and the reveal clock stops rather than spinning on a still frame"
    );
}

/// A still screen must be still. The image viewer draws over a transcript that also draws the same
/// image, and asking the store for one image at two sizes made every frame release and re-transmit
/// it — which shows up as the picture flickering while nothing is happening.
#[test]
fn the_open_image_viewer_does_not_re_transmit_on_every_frame() {
    let backend = test_backend(100, 32);
    let mut app = app_with_image(luxe_tui::graphics::ImageTier::Pixels);
    let key = app.newest_visible_image().expect("the transcript has an image");
    app.update(Msg::OpenImageViewer(key));

    render_app(&mut app, &backend);
    assert!(!app.images.drain_control().is_empty(), "the first frame transmits it");

    for frame in 0..4 {
        let buffer = render_app(&mut app, &backend);
        assert!(
            app.images.drain_control().is_empty(),
            "frame {frame} re-transmitted the image the viewer is already showing"
        );
        assert!(
            buffer.content.iter().any(|cell| cell.symbol().starts_with('\u{10EEEE}')),
            "and it is still on screen"
        );
    }
}

/// Closing the viewer puts the transcript's own copy back, at the transcript's size.
#[test]
fn closing_the_viewer_returns_the_transcript_to_its_own_size() {
    let backend = test_backend(100, 32);
    let mut app = app_with_image(luxe_tui::graphics::ImageTier::Pixels);
    let key = app.newest_visible_image().expect("an image");

    render_app(&mut app, &backend);
    let inline = app.images.fit(&key, 60, 12).expect("a transcript fit");

    app.update(Msg::OpenImageViewer(key.clone()));
    render_app(&mut app, &backend);
    app.images.drain_control();

    app.update(Msg::CloseImageViewer);
    assert!(app.overlays.image_viewer.is_none());
    let buffer = render_app(&mut app, &backend);
    assert!(
        buffer.content.iter().any(|cell| cell.symbol().starts_with('\u{10EEEE}')),
        "the transcript draws it again"
    );
    assert!(inline.rows >= 1, "and the transcript's own size is still a real one");
}

/// Which image rows the buffer is currently showing, in screen order.
///
/// A kitty placeholder addresses itself, so the buffer says exactly which row of the image each
/// cell is displaying — which is what makes the clipping checkable rather than a matter of opinion.
pub(super) fn painted_image_rows(buffer: &Buffer) -> Vec<u16> {
    let width = buffer.area.width as usize;
    buffer
        .content
        .chunks(width)
        .filter_map(|row| {
            let symbol = row
                .iter()
                .map(|cell| cell.symbol())
                .find(|symbol| symbol.starts_with('\u{10EEEE}'))?;
            // Second char is the row diacritic; its index in the protocol's alphabet is the row.
            let diacritic = symbol.chars().nth(1)?;
            u16::try_from(ROW_DIACRITICS.iter().position(|mark| *mark == diacritic)?).ok()
        })
        .collect()
}

/// The first few entries of the protocol's coordinate alphabet — enough for any capped image.
pub(super) const ROW_DIACRITICS: [char; 24] = [
    '\u{305}', '\u{30D}', '\u{30E}', '\u{310}', '\u{312}', '\u{33D}', '\u{33E}', '\u{33F}',
    '\u{346}', '\u{34A}', '\u{34B}', '\u{34C}', '\u{350}', '\u{351}', '\u{352}', '\u{357}',
    '\u{35B}', '\u{363}', '\u{364}', '\u{365}', '\u{366}', '\u{367}', '\u{368}', '\u{369}',
];
/// Scrolling a picture through a viewport made of text is the case the whole placement design
/// exists for. Only the visible rows are painted, they are the *right* rows, and when the image
/// leaves the viewport nothing of it is left behind — the ghost image that every cursor-positioned
/// approach has to clean up by hand, and that this one cannot produce.
#[test]
fn an_image_scrolls_through_the_viewport_without_leaving_anything_behind() {
    let backend = test_backend(100, 18);
    let mut app = app_with_image(luxe_tui::graphics::ImageTier::Pixels);
    for index in 0..40 {
        app.push_convo(ConvoBlock::Assistant(format!("line of prose number {index}")));
    }
    app.update(Msg::TerminalResized { width: 100, height: 18 });

    // Walk the transcript from the top, one line at a time, and watch the image go by.
    app.scroll_state.follow = false;
    let mut seen_partial_top = false;
    let mut seen_whole = false;
    let mut fit_rows = 0;

    for offset in 0..30 {
        app.scroll_state.offset = offset;
        let buffer = render_app(&mut app, &backend);
        let rows = painted_image_rows(&buffer);
        if rows.is_empty() {
            continue;
        }
        // Whatever is on screen is a contiguous run of the image's rows, in order.
        assert!(
            rows.windows(2).all(|pair| pair[1] == pair[0] + 1),
            "image rows on screen are contiguous and in order, got {rows:?}"
        );
        fit_rows = fit_rows.max(rows.last().copied().unwrap_or(0) + 1);
        if *rows.first().expect("non-empty") > 0 {
            seen_partial_top = true;
        }
        if rows.len() > 1 && rows.first() == Some(&0) {
            seen_whole = true;
        }
    }

    // And with the transcript scrolled well past it, not one cell of it survives. The image sits
    // at the top of this transcript, so "past it" is a large offset.
    app.scroll_state.offset = 40;
    let buffer = render_app(&mut app, &backend);
    let left_behind = painted_image_rows(&buffer);

    assert!(fit_rows > 1, "the image occupied several rows: {fit_rows}");
    assert!(seen_whole, "it was fully visible at some point");
    assert!(seen_partial_top, "and clipped at the top of the viewport at another");
    assert!(
        left_behind.is_empty(),
        "scrolled away entirely, no placeholder cell is left behind — the ghost image this \
         placement design exists to make impossible; found {left_behind:?}"
    );
}

/// The tool the reader never asked to watch. `activate_tools` loads a deferred tool's schema — a
/// fact about luxe's request budget, not a step in anyone's task — and it lands directly in front
/// of the tool that actually did something. A successful one leaves no card; a failed one does,
/// because a call nobody can see cannot explain the stall it caused.
#[test]
fn a_successful_schema_load_leaves_no_card_but_a_failed_one_does() {
    let backend = test_backend(100, 24);
    let mut app = create_app();
    app.update(Msg::Engine(luxe_core::Event::SessionLoaded {
        session_id: "session-a".into(),
        messages: vec![],
    }));

    let activity = |call: &str, name: &str, state, revision| {
        let mut activity = luxe_core::ToolActivity::running("session-a", call, name, name, 1);
        activity.state = state;
        activity.revision = revision;
        luxe_core::Event::ToolActivityUpdated(Box::new(activity))
    };

    app.update(Msg::Engine(activity("c1", "activate_tools", ToolActivityState::Running, 0)));
    app.update(Msg::Engine(activity("c1", "activate_tools", ToolActivityState::Succeeded, 1)));
    app.update(Msg::Engine(activity("c2", "screenshot", ToolActivityState::Succeeded, 1)));

    let rendered = rows_of(&render_app(&mut app, &backend)).join("\n");
    assert!(rendered.contains("screenshot"), "the tool that did something is on screen");
    assert!(
        !rendered.contains("activate_tools"),
        "the schema load that went fine is not: {rendered}"
    );
    // The state is still recorded, so nothing downstream lost track of the call.
    assert!(app.tool_activity.contains_key(&("session-a".into(), "c1".into())));

    // One that failed has to be visible — the model is about to do something inexplicable.
    app.update(Msg::Engine(activity("c3", "activate_tools", ToolActivityState::Failed, 1)));
    let rendered = rows_of(&render_app(&mut app, &backend)).join("\n");
    assert!(rendered.contains("activate_tools"), "a failed schema load explains itself");
}

/// Rows of the transcript that carry anything, as a crude "how much is on screen" measure.
pub(super) fn occupied_rows(buffer: &Buffer) -> usize {
    rows_of(buffer).iter().filter(|row| !row.trim().is_empty()).count()
}

/// A turn only ever adds. Content that appears and then vanishes reads as a glitch even when the
/// end state is right, and the transcript has several ways to do it by accident: the staggered
/// reveal, a card gaining its body, and — since images — a card growing by the height of a picture
/// the moment the pixels land. This drives the exact event order of a `screenshot` turn and asserts
/// the transcript never shrinks between frames.
#[test]
fn a_streaming_turn_never_takes_content_back() {
    use luxe_core::message::{ContentBlock, Message, Role, ToolOutput};

    let backend = test_backend(100, 40);
    let mut app = create_app();
    app.images = pinned_store(luxe_tui::graphics::ImageTier::Pixels);
    app.update(Msg::Engine(luxe_core::Event::SessionLoaded {
        session_id: "s".into(),
        messages: vec![],
    }));
    // The reveal is what stages content in; pace it off so this measures structure, not animation.
    app.reveal_catch_up();

    let mut frames: Vec<(&str, usize)> = Vec::new();
    let mut observe = |app: &mut App, label: &'static str| {
        app.reveal_catch_up();
        frames.push((label, occupied_rows(&render_app(app, &backend))));
    };

    app.push_convo(ConvoBlock::User {
        text: "how does the page look?".into(),
        attached: vec![],
        chips: Vec::new(),
    });
    observe(&mut app, "user message");

    app.update(Msg::Engine(luxe_core::Event::ThinkingDelta {
        block: 0,
        delta: "Checking whether the screenshot tool is loaded.".into(),
    }));
    observe(&mut app, "reasoning");

    let call = luxe_core::ToolActivity::running("s", "c1", "screenshot", "url · e.com", 1);
    app.update(Msg::Engine(luxe_core::Event::ToolActivityUpdated(Box::new(call.clone()))));
    observe(&mut app, "tool started");

    // The result commits first (this is where the pixels arrive), then the activity goes terminal.
    app.update(Msg::Engine(luxe_core::Event::MessageCommitted {
        session_id: "s".into(),
        entry_id: luxe_core::session::EntryId("e1".into()),
        message: Message {
            role: Role::Tool,
            content: vec![ContentBlock::ToolResult {
                call_id: "c1".into(),
                content: vec![
                    ToolOutput::Text("Captured url (320x160).".into()),
                    ToolOutput::Image {
                        data: split_png(320, 160),
                        mime: "image/png".into(),
                        name: None,
                    },
                ],
                is_error: false,
            }],
        },
    }));
    observe(&mut app, "image arrived");

    let mut done = call;
    done.revision = 1;
    done.state = ToolActivityState::Succeeded;
    done.elapsed_ms = Some(362);
    app.update(Msg::Engine(luxe_core::Event::ToolActivityUpdated(Box::new(done))));
    observe(&mut app, "tool finished");

    app.update(Msg::Engine(luxe_core::Event::TokenDelta {
        block: 1,
        delta: "A sparse white page with a centered heading.".into(),
    }));
    observe(&mut app, "answer");

    for pair in frames.windows(2) {
        let ((before, was), (after, now)) = (pair[0], pair[1]);
        assert!(
            now >= was,
            "the transcript shrank going from {before} ({was} rows) to {after} ({now} rows) — \
             content that appears and then vanishes reads as a glitch"
        );
    }
    assert!(frames.last().expect("frames").1 > frames[0].1, "and it grew overall: {frames:?}");
}
