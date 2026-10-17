//! Frame pins — every fixture's frame and hit registry, as a digest.
//!
//! The view's modules are split by responsibility, and a split that keeps the picture but drops a
//! click region, or pushes two in a different order, is invisible in a screenshot. So this draws
//! every fixture scenario at three sizes, every overlay over the chat and the start screen, and a
//! few transcript states (scrolled, searching, selecting), and records one digest of the cells
//! (symbol and style) and one of the hit registry per frame. `frame_pins.txt` is the record.
//!
//! `LUXE_FRAME_PINS=path` writes the current table; `LUXE_FRAME_PINS_DUMP=dir` writes each frame's
//! text and registry, so two checkouts' dumps can be diffed to see what moved.

use crate::app::App;
use crate::fixture::{seeded_app, FixtureScenario};
use crate::main_loop::overlay_matrix;
use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};
use std::time::Duration;

const SIZES: [(u16, u16); 3] = [(140, 44), (100, 30), (64, 20)];

/// Frames run before the pinned one, so a reveal mid-animation lands on the same frame every time.
const SETTLE_FRAMES: u32 = 28;

fn fnv(bytes: impl IntoIterator<Item = u8>, hash: &mut u64) {
    for byte in bytes {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
}

fn digest(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    fnv(text.bytes(), &mut hash);
    hash
}

/// A digest of every cell, except a kitty placeholder's foreground: that is the image's id, which
/// the store seeds at random so it cannot collide with another program's images.
fn buffer_digest(buffer: &Buffer) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    for cell in &buffer.content {
        let mut cell = cell.clone();
        if cell.symbol().starts_with('\u{10EEEE}') {
            cell.fg = ratatui::style::Color::Reset;
        }
        fnv(format!("{cell:?}").into_bytes(), &mut hash);
    }
    hash
}

/// `text` with each uuid replaced by its order of first appearance, since a fixture's findings get
/// fresh ids every run.
fn without_uuids(text: &str) -> String {
    let shape = |token: &[u8]| {
        token.len() == 36
            && token.iter().enumerate().all(|(index, byte)| match index {
                8 | 13 | 18 | 23 => *byte == b'-',
                _ => byte.is_ascii_hexdigit(),
            })
    };
    let mut seen: Vec<String> = Vec::new();
    let (mut out, mut rest) = (String::new(), text);
    while rest.len() >= 36 {
        if rest.is_char_boundary(36) && shape(&rest.as_bytes()[..36]) {
            let uuid = rest[..36].to_string();
            let ordinal = seen.iter().position(|known| *known == uuid).unwrap_or_else(|| {
                seen.push(uuid);
                seen.len() - 1
            });
            out.push_str(&format!("uuid#{ordinal}"));
            rest = &rest[36..];
            continue;
        }
        let next = rest.chars().next().map_or(1, char::len_utf8);
        out.push_str(&rest[..next]);
        rest = &rest[next..];
    }
    out + rest
}

fn buffer_text(buffer: &Buffer) -> String {
    let width = usize::from(buffer.area.width.max(1));
    buffer
        .content
        .chunks(width)
        .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Draws `app` the way the loop would after it settles, and returns the last frame.
fn draw(app: &mut App, (width, height): (u16, u16)) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a test terminal");
    let base = *app.status.frame_clock.get_or_insert_with(std::time::Instant::now);
    for step in 1..=SETTLE_FRAMES {
        app.status.frame_clock = Some(base + Duration::from_millis(12) * step);
        terminal.draw(|frame| super::render(frame, app)).expect("a frame");
    }
    terminal.backend().buffer().clone()
}

fn seeded(scenario: FixtureScenario, (width, height): (u16, u16)) -> App {
    let mut app = seeded_app("luxe", scenario).expect("the fixture");
    // A fixture that pinned its own clock timed its turns against it
    app.status.frame_clock.get_or_insert_with(std::time::Instant::now);
    app.update(crate::app::Msg::TerminalResized { width, height });
    app
}

struct Pin {
    name: String,
    frame: Buffer,
    hits: String,
}

fn pin(name: String, app: &mut App, size: (u16, u16)) -> Pin {
    let frame = draw(app, size);
    let hits = without_uuids(&format!("{:#?}", app.hit.borrow()));
    Pin { name, frame, hits }
}

fn mouse(app: &mut App, kind: MouseEventKind, column: u16, row: u16) {
    let event = MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE };
    overlay_matrix::send(app, Event::Mouse(event));
}

fn pins() -> Vec<Pin> {
    let mut pins = Vec::new();
    for &scenario in FixtureScenario::ALL {
        for size in SIZES {
            let mut app = seeded(scenario, size);
            pins.push(pin(format!("{scenario:?}@{}x{}", size.0, size.1), &mut app, size));
        }
    }
    for scenario in [FixtureScenario::ChatTools, FixtureScenario::Dashboard] {
        for overlay in overlay_matrix::OVERLAYS {
            let size = (100, 30);
            let mut app = seeded(scenario, size);
            overlay_matrix::open(&mut app, overlay);
            pins.push(pin(format!("{scenario:?}+{overlay}"), &mut app, size));
        }
    }
    let size = (64, 20);
    let mut app = seeded(FixtureScenario::ChatWideProse, size);
    draw(&mut app, size);
    app.scroll_state.up_by(7);
    pins.push(pin("ChatWideProse scrolled".into(), &mut app, size));

    let size = (100, 30);
    let mut app = seeded(FixtureScenario::ChatWideProse, size);
    app.search_state.searching = true;
    app.search_state.query = "the".into();
    pins.push(pin("ChatWideProse searching".into(), &mut app, size));

    let mut app = seeded(FixtureScenario::ChatWideProse, size);
    app.mouse_enabled = true;
    draw(&mut app, size);
    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), 12, 9);
    mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), 40, 11);
    pins.push(pin("ChatWideProse selecting".into(), &mut app, size));
    mouse(&mut app, MouseEventKind::Up(MouseButton::Left), 40, 11);
    pins.push(pin("ChatWideProse copied".into(), &mut app, size));

    let mut app = seeded(FixtureScenario::ChangesWide, (180, 44));
    pins.push(pin("ChangesWide@180x44".into(), &mut app, (180, 44)));
    pins.extend(changes_views());
    pins.extend(review_dialogs());
    pins.extend(review_diffs());
    pins.extend(seeded_overlays());
    pins
}

/// The changes pane unwrapped, and on its File view wrapped and not, with and without a cursor.
///
/// Wrapping is on by default, so the toggle turns it off; the narrow size is where lines overflow.
fn changes_views() -> Vec<Pin> {
    let mut pins = Vec::new();
    let size = (64, 20);
    let mut app = seeded(FixtureScenario::ChangesNarrow, size);
    if let Some(view) = app.changes.as_mut() {
        view.toggle_word_wrap();
    }
    pins.push(pin("ChangesNarrow unwrapped".into(), &mut app, size));
    let size = (140, 44);
    for (name, cursor, wrap) in [
        ("ChangesWide file", None, true),
        ("ChangesWide file cursor", Some(4), true),
        ("ChangesWide file unwrapped", Some(4), false),
    ] {
        let mut app = seeded(FixtureScenario::ChangesWide, size);
        if let Some(view) = app.changes.as_mut() {
            let long =
                format!("    let note = \"{}\";\n", "a line that runs past the pane ".repeat(6));
            let text = format!("fn main() {{\n{long}}}\n").repeat(30);
            let bytes = text.len() as u64;
            let file = luxe_core::project_files::ProjectFile {
                path: view.path.clone(),
                text,
                bytes,
                truncated: false,
                binary: false,
            };
            view.view = crate::changes::PeekView::File;
            view.text = crate::changes::TextLoad::Ready(Box::new(file));
            view.focused = cursor.is_some();
            view.cursor = cursor;
            if !wrap {
                view.toggle_word_wrap();
            }
        }
        pins.push(pin(name.into(), &mut app, size));
    }
    pins
}

/// A named way to open something over a seeded app.
type Opener = (&'static str, fn(&mut App));

/// The review's dialogs no fixture opens: help, a finding's details and its fix, the draft and
/// question composers, and the source picker.
fn review_dialogs() -> Vec<Pin> {
    let open: [Opener; 9] = [
        ("help", |app| app.guided_review_workspace.help_visible = true),
        ("details", |app| app.guided_review_workspace.finding_details_visible = true),
        ("fix", |app| app.guided_review_workspace.finding_fix_visible = true),
        ("draft", |app| {
            let files = app.guided_review.as_ref().map(|review| review.files.clone());
            app.guided_review_workspace.begin_finding_draft(&files.unwrap_or_default());
        }),
        ("question", |app| {
            let files = app.guided_review.as_ref().map(|review| review.files.clone());
            app.guided_review_workspace.begin_question(&files.unwrap_or_default());
        }),
        ("draft edit", |app| {
            let files = app.guided_review.as_ref().map(|review| review.files.clone());
            let workspace = &mut app.guided_review_workspace;
            workspace.begin_finding_draft(&files.unwrap_or_default());
            let id = app.review.session().and_then(|session| session.findings.first());
            app.guided_review_workspace.editing_finding_id = id.map(|finding| finding.id.clone());
        }),
        ("summary", |app| {
            app.guided_review_workspace.begin_review_summary();
        }),
        ("finish everything", |app| {
            let workspace = &mut app.guided_review_workspace;
            workspace.finish_review_choice = Some(crate::guided_review::FinishReviewChoice::Accept);
            workspace.finish_review_accept_confirmed = true;
            workspace.finish_offers_answers = true;
            workspace.finish_offers_opening = true;
            workspace.finish_offers_stack = true;
        }),
        ("picker", |app| {
            let selection = crate::guided_review::ReviewSourcePickerSelection::Base;
            app.guided_review_workspace.source_picker =
                crate::guided_review::ReviewSourcePickerState::Open { selection };
        }),
    ];
    let mut pins = Vec::new();
    for (name, open) in open {
        for size in [(140, 44), (100, 30)] {
            let mut app = seeded(FixtureScenario::GuidedReviewFindings, size);
            open(&mut app);
            pins.push(pin(format!("review {name}@{}x{}", size.0, size.1), &mut app, size));
        }
    }
    pins
}

/// A diff case: the scenario, a name, the size, and what to change before drawing.
type DiffCase = (FixtureScenario, &'static str, (u16, u16), &'static [fn(&mut App)]);

/// The review's diff wrapped (it starts clipped), and side by side with comment cards in it.
fn review_diffs() -> Vec<Pin> {
    fn wrap(app: &mut App) {
        app.guided_review_workspace.word_wrap = true;
    }
    fn split(app: &mut App) {
        app.guided_review_workspace.layout = luxe_core::config::DiffLayout::Split;
    }
    let cases: [DiffCase; 5] = [
        (FixtureScenario::GuidedReviewReady, "wrapped", (120, 40), &[wrap]),
        (FixtureScenario::GuidedReviewSplit, "wrapped", (170, 44), &[wrap]),
        (FixtureScenario::GuidedReviewFindings, "wrapped", (120, 40), &[wrap]),
        (FixtureScenario::GuidedReviewFindings, "split", (170, 44), &[split]),
        (FixtureScenario::GuidedReviewFindings, "split wrapped", (170, 44), &[split, wrap]),
    ];
    let mut pins = Vec::new();
    for (scenario, name, size, steps) in cases {
        let mut app = seeded(scenario, size);
        for step in steps {
            step(&mut app);
        }
        pins.push(pin(format!("{scenario:?} {name}"), &mut app, size));
    }
    pins
}

/// The overlays the site's captures open by name: each login state and a failed one, the GitHub
/// panel's views, and the `@` picker's tabs.
fn seeded_overlays() -> Vec<Pin> {
    let logins = [
        "login",
        "login-subscription",
        "login-local",
        "login-method",
        "login-secret",
        "login-device",
    ];
    let github = [
        "github",
        "github-detail",
        "github-search",
        "github-conversation",
        "github-draft",
        "github-issue",
        "github-pulls",
        "github-merged",
    ];
    // Not the plain `files` tab: it lists the directory the tests run in
    let tabs =
        ["files-query", "files-status", "skills", "mcp", "jira", "github", "github-signed-out"];
    let mut pins = Vec::new();
    for size in [(100, 30), (64, 20)] {
        for name in logins {
            let mut app = seeded(FixtureScenario::ChatIdle, size);
            let login = crate::fixture::LoginFixture::parse(name).expect("a login state");
            crate::fixture::seed_login_wizard(&mut app, login);
            pins.push(pin(format!("{name}@{}x{}", size.0, size.1), &mut app, size));
        }
        let mut app = seeded(FixtureScenario::ChatIdle, size);
        let login = crate::fixture::LoginFixture::parse("login").expect("a login state");
        crate::fixture::seed_login_wizard(&mut app, login);
        if let Some(wizard) = app.overlays.auth_wizard.as_mut() {
            let failed = luxe_core::auth::AuthError::UnknownProvider("nowhere".into());
            wizard.apply(crate::auth_wizard::AuthUiMsg::Finished(Err(failed)));
        }
        pins.push(pin(format!("login-failed@{}x{}", size.0, size.1), &mut app, size));
        for name in github {
            let mut app = seeded(FixtureScenario::ChatIdle, size);
            crate::fixture::seed_github(&mut app, name);
            pins.push(pin(format!("{name}@{}x{}", size.0, size.1), &mut app, size));
        }
        for tab in tabs {
            let mut app = seeded(FixtureScenario::ChatIdle, size);
            crate::fixture::seed_mention_picker(&mut app, tab);
            pins.push(pin(format!("mention-{tab}@{}x{}", size.0, size.1), &mut app, size));
        }
    }
    pins
}

/// Every fixture frame and its hit registry are what they were when the table was recorded.
///
/// The break this catches: a move between view modules that changes a cell, a style, a click
/// region or the order regions are pushed in (the topmost wins, so order is behavior).
#[test]
fn every_frame_and_hit_registry_is_as_recorded() {
    // The Setup footer and the permission card name the config file, which follows the
    // environment; under one home it reads `~/.config/…` on every machine. Nextest runs each test in
    // a process of its own, so neither this nor the glyph switches leak into another test.
    let home = luxe_core::user_dirs::home_dir().unwrap_or_default();
    let config_home = home.join(".config");
    let _env = crate::test_env::EnvGuard::set(&[
        ("LUXE_CONFIG_DIR", None),
        ("XDG_CONFIG_HOME", config_home.to_str()),
    ]);
    crate::glyphs::set_tier(crate::glyphs::GlyphTier::Nerd);
    crate::glyphs::set_braille(false);
    let pins = pins();
    if let Ok(dir) = std::env::var("LUXE_FRAME_PINS_DUMP") {
        std::fs::create_dir_all(&dir).expect("the dump directory");
        for pin in &pins {
            let body = format!("{}\n\n{}\n", buffer_text(&pin.frame), pin.hits);
            let file = pin.name.replace([' ', '+', '@'], "_");
            std::fs::write(format!("{dir}/{file}.txt"), body).expect("write a dump");
        }
    }
    let actual: String = pins
        .iter()
        .map(|pin| {
            let frame = buffer_digest(&pin.frame);
            format!("{}: frame={frame:016x} hits={:016x}\n", pin.name, digest(&pin.hits))
        })
        .collect();
    if let Ok(path) = std::env::var("LUXE_FRAME_PINS") {
        std::fs::write(path, &actual).expect("write the table");
    }
    let recorded = include_str!("frame_pins.txt");
    for (want, got) in recorded.lines().zip(actual.lines()) {
        assert_eq!(got, want, "a frame or its hit registry moved");
    }
    assert_eq!(recorded.lines().count(), actual.lines().count());
}
