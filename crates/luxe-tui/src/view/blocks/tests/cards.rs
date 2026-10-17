//! Cards — notices, references and the tool card in each state.

use super::*;

/// A command a notice names is drawn as inline code, backticks dropped; the rest stays muted.
#[test]
fn a_notice_draws_its_commands_as_code() {
    let styles = &theme();
    let lines = system_lines("", "Use `luxe --no-tool <name>` for one", styles.muted, styles);
    let spans = &lines[0].spans;
    let code: Vec<&str> = spans
        .iter()
        .filter(|span| span.style == styles.md_code)
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(code, ["luxe --no-tool <name>"]);
    assert!(spans.iter().all(|span| !span.content.contains('`')), "{spans:?}");
    assert_eq!(spans[0].style, styles.muted);
}

/// A reference in a sent message stands apart from the accent prose around it: body ink, bold,
/// and no tint, which on the card's own band was too faint to read.
#[test]
fn a_reference_in_a_message_stands_apart() {
    let styles = &theme();
    let text = "fix acme/todo#12 with test-first";
    let lines = chipped_lines(text, &[4..16, 22..32], styles.accent, styles);
    assert_eq!(lines.len(), 1);
    let spans = &lines[0].spans;
    let texts: Vec<&str> = spans.iter().map(|span| span.content.as_ref()).collect();
    assert_eq!(texts, ["fix ", "acme/todo#12", " with ", "test-first"]);
    assert_eq!(spans[1].style.fg, styles.text.fg, "the body ink");
    assert_ne!(spans[1].style.fg, spans[0].style.fg, "not the sentence's accent");
    assert!(spans[1].style.add_modifier.contains(Modifier::BOLD), "in bold");
    assert_eq!(spans[1].style.bg, styles.accent.bg, "on the card, not a tint");
    assert!(!spans[0].style.add_modifier.contains(Modifier::BOLD), "the prose is not a chip");
    // A range off this text's character boundaries is not trusted
    let inside_the_e = std::ops::Range { start: 1, end: 2 };
    let odd = chipped_lines("é", std::slice::from_ref(&inside_the_e), styles.accent, styles);
    assert_eq!(odd[0].spans.len(), 1);
}

/// A transcript with no images in it, which is every block these tests build.
pub(super) static NO_IMAGES: OnceLock<crate::graphics::ImageStore> = OnceLock::new();
pub(super) static NO_TOOL_IMAGES: OnceLock<
    std::collections::HashMap<(String, String), Vec<ImageKey>>,
> = OnceLock::new();
pub(super) fn theme() -> ThemeStyles {
    let registry = luxe_core::theme::ThemeRegistry::new();
    let resolved = registry.resolve("nord").expect("a bundled theme");
    ThemeStyles::from_resolved(&resolved, "nord")
}

pub(super) fn tool_item(name: &str, state: luxe_core::ToolActivityState) -> ConvoBlock {
    let mut activity =
        luxe_core::ToolActivity::running("session-a", "call-a", name, "cargo test", 1);
    activity.state = state;
    activity.elapsed_ms = Some(842);
    ConvoBlock::Tool(Box::new(crate::app::ToolItem {
        activity,
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    }))
}

pub(super) fn context(styles: &ThemeStyles, spinner_frame: usize) -> BlockContext<'_> {
    BlockContext {
        styles,
        spinner_frame,
        waiting_on_user: false,
        never_sent: false,
        expanded: false,
        images: NO_IMAGES.get_or_init(crate::graphics::ImageStore::default),
        tool_images: NO_TOOL_IMAGES.get_or_init(std::collections::HashMap::new),
        image_bounds: (80, 24),
        card_width: 80,
        prose_width: 80,
        diff_layout: luxe_core::config::DiffLayout::Auto,
        tool_output_expanded: false,
        expand_chord: Some("ctrl+o".into()),
        running_ms: None,
    }
}

pub(super) fn header(block: &ConvoBlock, styles: &ThemeStyles) -> Line<'static> {
    lines(block, &context(styles, 0)).remove(0).line
}

/// The card's three voices: the tool in bold state color, the subject in body text, the clock
/// muted. A reader's eye has to land on the command, not on `842ms`.
#[test]
fn a_tool_card_reads_in_three_voices() {
    let styles = theme();
    let line = header(&tool_item("bash", luxe_core::ToolActivityState::Succeeded), &styles);

    let name = &line.spans[0];
    assert_eq!(name.content.as_ref(), format!("{} bash", tool_glyph("bash")));
    assert!(name.style.add_modifier.contains(Modifier::BOLD), "the tool name is bold");
    assert_eq!(name.style.fg, styles.success.fg, "a landed call takes its outcome color");

    let subject = &line.spans[1];
    assert_eq!(subject.content.as_ref(), "  cargo test");
    assert_eq!(subject.style.fg, styles.text.fg, "the subject is plain body text");
    assert!(!subject.style.add_modifier.contains(Modifier::BOLD));

    let duration = &line.spans[2];
    assert_eq!(duration.content.as_ref(), "  842ms", "held off by two spaces, not one");
    assert_eq!(duration.style.fg, styles.muted.fg, "the clock is de-emphasized");
}

/// A running call keeps the spinner *and* its name in the warning color, so the eye still finds
/// the in-flight card while the subject and clock read as they do everywhere else.
#[test]
fn a_running_card_spins_in_the_warning_color() {
    let styles = theme();
    let block = tool_item("bash", luxe_core::ToolActivityState::Running);

    let first = header(&block, &styles);
    assert_eq!(first.spans[0].style.fg, styles.warning.fg);
    assert!(first.spans[0].style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(first.spans[1].style.fg, styles.text.fg);
    assert_ne!(
        first.spans[0].content,
        lines(&block, &context(&styles, 3)).remove(0).line.spans[0].content,
        "the leading glyph animates while the call is in flight"
    );
}

/// A call the model is still writing has a card but nothing yet to say about it. The header is
/// its mark and the tool, and no more — a subject slot filled with the tool's own name would
/// read `write  write`.
#[test]
fn a_card_for_an_unnamed_call_is_its_mark_and_the_tool() {
    let styles = theme();
    let card = |label: &str| {
        ConvoBlock::Tool(Box::new(crate::app::ToolItem {
            activity: luxe_core::ToolActivity::composing("session-a", "call-a", "write", label, 1),
            expanded: false,
            details_toggled: false,
            waited_on_user_ms: 0,
        }))
    };

    let line = header(&card(""), &styles);
    assert_eq!(line.spans.len(), 1, "{line}");
    assert_eq!(line.spans[0].content.as_ref(), format!("{} write", crate::statusline::spinner(0)));

    // Then the path arrives and takes the subject slot, on the same row.
    assert_eq!(header(&card("notes.rs"), &styles).spans[1].content.as_ref(), "  notes.rs");
}

/// Exactly one card looks busy, and it is whichever one is actually moving.
///
/// The model writing a call and the tool running it are both work, and the card says so the
/// same way for each. Only a call written in full and waiting its turn is idle. Both halves of
/// this were reported wrong: every un-started call drawn as running put two spinning `bash`
/// cards side by side, and every un-started call drawn as waiting put a paused mark on a card
/// that was visibly filling in.
#[test]
fn a_card_looks_busy_exactly_when_it_is() {
    let styles = theme();
    let card = |state| {
        ConvoBlock::Tool(Box::new(crate::app::ToolItem {
            activity: luxe_core::ToolActivity {
                state,
                ..luxe_core::ToolActivity::running("s", "c", "bash", "vainfo", 1)
            },
            expanded: false,
            details_toggled: false,
            waited_on_user_ms: 0,
        }))
    };
    let spinner = format!("{} bash", crate::statusline::spinner(0));

    for busy in [luxe_core::ToolActivityState::Running, luxe_core::ToolActivityState::Composing] {
        let line = header(&card(busy), &styles);
        assert_eq!(line.spans[0].content.as_ref(), spinner, "{busy:?} is work in progress");
        assert_eq!(line.spans[0].style.fg, styles.warning.fg, "{busy:?}");
        // And it animates, or the spinner is a picture of a stalled thing.
        let later = lines(&card(busy), &context(&styles, 3)).remove(0).line;
        assert_ne!(line.spans[0].content, later.spans[0].content, "{busy:?}");
    }

    let queued = card(luxe_core::ToolActivityState::Queued);
    let line = header(&queued, &styles);
    assert_eq!(
        line.spans[0].content.as_ref(),
        format!("{} bash", Icon::Queued.glyph()),
        "a call waiting its turn wears the mark a queued submission does",
    );
    assert_eq!(line.spans[0].style.fg, styles.muted.fg, "and it recedes");
    let later = lines(&queued, &context(&styles, 3)).remove(0).line;
    assert_eq!(line.spans[0].content, later.spans[0].content, "nothing is happening to it");
}

/// A header subject is a preview for a card that shows nothing else. Where the body *is* the
/// thing the subject was cut out of, the preview is the same string rendered worse, directly
/// above the real one.
#[test]
fn a_header_does_not_restate_the_body_under_it() {
    let styles = theme();
    let card = |tool: &str, arguments: serde_json::Value| {
        let mut activity = luxe_core::ToolActivity::running(
            "s",
            "c",
            tool,
            luxe_core::describe_call(tool, &arguments, "/project"),
            1,
        );
        activity.state = luxe_core::ToolActivityState::Succeeded;
        activity.elapsed_ms = Some(5);
        activity.card_body = luxe_core::tool_card_body(tool, &arguments, "/project");
        ConvoBlock::Tool(Box::new(crate::app::ToolItem {
            activity,
            expanded: false,
            details_toggled: false,
            waited_on_user_ms: 0,
        }))
    };

    // A multi-line command is drawn in full, highlighted, right below the header.
    let script = card("bash", serde_json::json!({"command": "make build\nmake test"}));
    let row = header(&script, &styles);
    assert_eq!(row.spans.len(), 2, "the tool and the clock, and nothing else: {row}");
    assert_eq!(row.spans[1].content.as_ref(), "  5ms");
    let rendered = lines(&script, &context(&styles, 0));
    assert!(rendered[1].line.to_string().contains("make build"), "the body has the command");

    // Same for the queries a search ran: the header named the first and the body lists them.
    let searched = card("web_search", serde_json::json!({"queries": ["one", "two"]}));
    assert_eq!(header(&searched, &styles).spans.len(), 2);

    // A one-line command has no body, so the header is the only place it can be.
    let one = card("bash", serde_json::json!({"command": "make build"}));
    assert_eq!(header(&one, &styles).spans[1].content.as_ref(), "  make build");

    // And a `write`'s subject is the path while its body is the file: different things.
    let written = card("write", serde_json::json!({"path": "a.rs", "content": "fn main() {}"}));
    assert_eq!(header(&written, &styles).spans[1].content.as_ref(), "  a.rs");

    // So is a described command: the header says what it is for, the body what ran, folded
    let described = card(
        "bash",
        serde_json::json!({"description": "Build it", "command": "make build\nmake test"}),
    );
    assert_eq!(header(&described, &styles).spans[1].content.as_ref(), "  Build it");
    let rendered = lines(&described, &context(&styles, 0));
    assert!(rendered[1].line.to_string().contains("make build"), "the command's first line");
    assert!(
        !rendered.iter().any(|row| row.line.to_string().contains("make test")),
        "the rest is folded"
    );
}

/// A call in flight reports how long it has been going, which is when that answers something.
///
/// The projector only times a call once it has *finished*, so a card used to show no figure at
/// all for the whole time "is this taking long?" was the live question — and then show one the
/// moment nobody was asking.
/// A call waiting on the reader stands still: its own glyph, in the running color, rather than a
/// spinner hurrying someone who is reading a question. Busy again, it spins again.
#[test]
fn a_call_waiting_on_you_does_not_spin() {
    let styles = theme();
    let question = ConvoBlock::Tool(Box::new(crate::app::ToolItem {
        activity: luxe_core::ToolActivity::running("s", "c", "question", "Which runtime?", 1),
        expanded: false,
        details_toggled: false,
        waited_on_user_ms: 0,
    }));
    let icon = |waiting: bool, frame: usize| {
        let ctx = BlockContext { waiting_on_user: waiting, ..context(&styles, frame) };
        let header = lines(&question, &ctx).remove(0).line;
        (header.spans[0].content.to_string(), header.spans[0].style)
    };
    let (still, style) = icon(true, 0);
    assert!(still.contains(tool_glyph("question")), "{still:?}");
    assert_eq!(icon(true, 7), (still.clone(), style), "the same on every frame");
    let (spinning, spinning_style) = icon(false, 0);
    assert_ne!(spinning, still, "a call nobody waits on spins");
    assert_ne!(icon(false, 7).0, spinning, "and moves");
    assert_eq!(style, spinning_style, "in the running color either way");
}

#[test]
fn a_call_in_flight_reports_how_long_it_has_been_going() {
    let styles = theme();
    let card = |state| {
        let mut activity = luxe_core::ToolActivity::running("s", "c", "bash", "make test", 1);
        activity.state = state;
        activity.elapsed_ms =
            matches!(state, luxe_core::ToolActivityState::Succeeded).then_some(2_500);
        ConvoBlock::Tool(Box::new(crate::app::ToolItem {
            activity,
            expanded: false,
            details_toggled: false,
            waited_on_user_ms: 0,
        }))
    };
    let with_running = |block: &ConvoBlock, running_ms| {
        let ctx = BlockContext { running_ms, ..context(&styles, 0) };
        lines(block, &ctx).remove(0).line
    };

    let running = card(luxe_core::ToolActivityState::Running);
    let live = with_running(&running, Some(12_000));
    assert_eq!(live.spans[2].content.as_ref(), "  12s", "{live}");

    assert!(!live.to_string().contains('.'), "a live figure with a tenths digit: {live}");

    // Before the front-end has a start to measure from, it says nothing rather than `0ms`.
    let cold = with_running(&running, None);
    assert_eq!(cold.spans.len(), 2, "no clock yet: {cold}");

    // And for its first second it says nothing either: the only reading available down there
    // is milliseconds, which on a card redrawn every frame is three digits of noise beside a
    // spinner that is already reporting the same fact.
    let just_started = with_running(&running, Some(740));
    assert_eq!(just_started.spans.len(), 2, "under a second, no figure: {just_started}");
    assert_eq!(
        with_running(&running, Some(1_000)).spans[2].content.as_ref(),
        "  1s",
        "and then it starts counting"
    );

    // Once it lands, the figure is the one the projector measured — not whatever the last
    // frame happened to compute.
    let done = card(luxe_core::ToolActivityState::Succeeded);
    assert_eq!(with_running(&done, Some(99_000)).spans[2].content.as_ref(), "  2s");
}

/// A card reports the time the *tool* spent, not the time the reader spent deciding.
#[test]
fn a_card_does_not_report_the_readers_own_deliberation() {
    let styles = theme();
    let card = |tool: &str, waited_on_user_ms| {
        let mut activity = luxe_core::ToolActivity::running("s", "c", tool, "subject", 1);
        activity.state = luxe_core::ToolActivityState::Succeeded;
        activity.elapsed_ms = Some(31_000);
        ConvoBlock::Tool(Box::new(crate::app::ToolItem {
            activity,
            expanded: false,
            details_toggled: false,
            waited_on_user_ms,
        }))
    };

    // Thirty of those thirty-one seconds were a permission prompt on screen.
    let gated = header(&card("bash", 30_000), &styles);
    assert_eq!(gated.spans[2].content.as_ref(), "  1s");
    // Nothing waited on, nothing to subtract.
    let plain = header(&card("bash", 0), &styles);
    assert_eq!(plain.spans[2].content.as_ref(), "  31s");

    // A `question` is that case taken to its limit: the whole call is a person thinking, so
    // there is no tool time to report at all.
    let asked = header(&card("question", 0), &styles);
    assert_eq!(asked.spans.len(), 2, "the tool and the prompt, no clock: {asked}");
    assert_eq!(asked.spans[1].content.as_ref(), "  subject");
}

/// A long subject, a question or a command, wraps under itself rather than back under the
/// tool's name, so the card's header still reads as one block.
#[test]
fn a_card_header_wraps_under_its_subject() {
    let styles = theme();
    for tool in ["question", "bash"] {
        let mut activity = luxe_core::ToolActivity::running("s", "c", tool, "a long subject", 1);
        activity.state = luxe_core::ToolActivityState::Succeeded;
        let block = ConvoBlock::Tool(Box::new(crate::app::ToolItem {
            activity,
            expanded: false,
            details_toggled: false,
            waited_on_user_ms: 0,
        }));
        let rendered = lines(&block, &context(&styles, 0));
        assert_eq!(rendered[0].hang, Some(header_subject_column(tool)), "{tool}");
        assert!(rendered[1..].iter().all(|line| line.hang.is_none()), "only the header hangs");
    }
}

/// The band's wash says how it went, so the glyph is free to say *what ran* — except for the
/// two states that get no wash at all.
#[test]
fn a_landed_card_shows_its_tool_not_a_tick() {
    let styles = theme();
    for state in [luxe_core::ToolActivityState::Succeeded, luxe_core::ToolActivityState::Failed] {
        for tool in ["read", "bash", "grep"] {
            let line = header(&tool_item(tool, state), &styles);
            assert_eq!(
                line.spans[0].content.as_ref(),
                format!("{} {tool}", tool_glyph(tool)),
                "{tool} in {state:?} should wear its own icon"
            );
        }
    }
    // Cancelled and interrupted calls keep a state glyph: nothing else on the card says they
    // never finished.
    let cancelled = header(&tool_item("read", luxe_core::ToolActivityState::Cancelled), &styles);
    assert!(cancelled.spans[0].content.starts_with('⊘'));
}
