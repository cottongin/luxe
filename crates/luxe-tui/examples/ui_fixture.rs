//! ui_fixture — static production-renderer screenshots for UI review.

use anyhow::{bail, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::{execute, terminal::SetTitle};
use luxe_tui::fixture::{seeded_app, FixtureScenario};
use luxe_tui::main_loop::{route_mouse, RouteAction, ScrollAction};
use luxe_tui::{terminal, view};
use std::env;
use std::io;
use std::time::Duration;

fn main() -> Result<()> {
    let (theme, scenario) = parse_args()?;
    // The fixture keeps clear of user config on purpose, so the glyph vocabulary gets its own knob:
    // `LUXE_FIXTURE_GLYPHS=nerd|nerd-wide|unicode|ascii` captures a surface at one fidelity.
    if let Ok(glyphs) = env::var("LUXE_FIXTURE_GLYPHS") {
        use luxe_tui::glyphs::{set_tier, GlyphTier};
        set_tier(match glyphs.as_str() {
            "nerd-wide" | "nerd_wide" => GlyphTier::NerdWide,
            "unicode" => GlyphTier::Unicode,
            "ascii" => GlyphTier::Ascii,
            _ => GlyphTier::Nerd,
        });
    }
    let mut app = seeded_app(&theme, scenario).map_err(anyhow::Error::msg)?;
    // `LUXE_FIXTURE_DESKTOP=<name>` paints the frame in an Omarchy theme from this machine, turned
    // into Luxe's the way a session following the desktop does, so a theme no built-in matches can
    // be judged without applying it
    if let Ok(name) = env::var("LUXE_FIXTURE_DESKTOP") {
        app.theme_styles = desktop_styles(&name)?;
    }
    // The same knob for the other thing the terminal decides for us:
    // `LUXE_FIXTURE_IMAGES=pixels|blocks|off` captures an image surface at one fidelity.
    if let Ok(images) = env::var("LUXE_FIXTURE_IMAGES") {
        use luxe_tui::graphics::ImageTier;
        app.images.set_tier(match images.as_str() {
            "pixels" => ImageTier::Pixels,
            "off" => ImageTier::Text,
            _ => ImageTier::Blocks,
        });
    }
    let scenario_name = scenario.name();

    execute!(io::stdout(), SetTitle(format!("Luxe fixture: {scenario_name}")))?;
    let mut terminal = terminal::init()?;
    let result = run(&mut terminal, &mut app);
    terminal::restore()?;
    result
}

fn run(terminal: &mut terminal::Tui, app: &mut luxe_tui::app::App) -> Result<()> {
    let size = terminal.size()?;
    app.update(luxe_tui::app::Msg::TerminalResized { width: size.width, height: size.height });
    // Optional overlay to open at startup for screenshot review (e.g. LUXE_FIXTURE_OVERLAY=palette).
    // A subscription's remaining allowance takes the statusline's cost slot; seed it on demand.
    if env::var("LUXE_FIXTURE_PLAN").is_ok() {
        luxe_tui::fixture::seed_plan_usage(app);
    }
    // The dock at a width a reader dragged it to, e.g. `LUXE_FIXTURE_DOCK_WIDTH=80`
    if let Some(width) = env::var("LUXE_FIXTURE_DOCK_WIDTH").ok().and_then(|w| w.parse().ok()) {
        app.panes.set(luxe_tui::workspace_layout::Pane::Dock, Some(width));
    }
    // The File view on a real file, e.g. `LUXE_FIXTURE_FILE=crates/luxe-tui/src/files.rs`, with
    // `LUXE_FIXTURE_PICK=40-52` picking lines in it
    if let Ok(path) = env::var("LUXE_FIXTURE_FILE") {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let pick = env::var("LUXE_FIXTURE_PICK").ok().and_then(|pick| {
            let (first, last) = pick.split_once('-').unwrap_or((&pick, &pick));
            Some((first.parse().ok()?, last.parse().ok()?))
        });
        luxe_tui::fixture::seed_file_view(app, &path, &text, pick);
    }
    // Where the keyboard is: `LUXE_FIXTURE_FOCUS=files` for the tree, `=peek:44` for the preview
    // with its cursor on line 44
    if let Ok(focus) = env::var("LUXE_FIXTURE_FOCUS") {
        match focus.split_once(':').unwrap_or((&focus, "")) {
            ("files", _) => app.update(luxe_tui::app::Msg::FocusFiles),
            ("peek", line) => luxe_tui::fixture::focus_preview(app, line.parse().ok()),
            // `filter:dock` filters the tree over this checkout's files
            ("filter", query) => {
                let cwd = env::current_dir().unwrap_or_default();
                let files = luxe_tui::ref_provider::list_files(&cwd);
                luxe_tui::fixture::filter_tree(app, &files, query);
            }
            // `visual:12-16` selects those lines in visual-line mode
            ("visual", range) => {
                let (from, to) = range.split_once('-').unwrap_or((range, range));
                if let (Ok(from), Ok(to)) = (from.parse(), to.parse()) {
                    luxe_tui::fixture::preview_visual(app, from, to);
                }
            }
            _ => {}
        }
    }
    // Todo subjects as long as a real plan writes them, to see the checklist wrap
    if env::var("LUXE_FIXTURE_LONG_TODOS").is_ok() {
        if let Some(snapshot) = app.workspace.as_mut() {
            let lengthen = |subject: &mut String| {
                subject.push_str(" across the backend, the frontend and every integration test");
            };
            snapshot.todos.items.iter_mut().for_each(|item| lengthen(&mut item.subject));
            if let Some(current) = snapshot.todos.current.as_mut() {
                lengthen(&mut current.subject);
            }
        }
    }
    if let Ok(overlay) = env::var("LUXE_FIXTURE_OVERLAY") {
        if let Some(login) = luxe_tui::fixture::LoginFixture::parse(&overlay) {
            luxe_tui::fixture::seed_login_wizard(app, login);
        }
        // The `@` picker on each of its tabs, and GitHub when `gh` cannot list anything
        if let Some(tab) = overlay.strip_prefix("mention-") {
            luxe_tui::fixture::seed_mention_picker(app, tab);
        }
        // `/github` and `/pr`, each in a state worth a look: `github`, `github-detail`,
        // `github-conversation`, `github-draft`, `github-issue`, `github-pulls`, `github-search`,
        // `github-merged`, `pr`, `pr-merge`
        if overlay.starts_with("pr") || overlay.starts_with("github") {
            luxe_tui::fixture::seed_github(app, &overlay);
        }
        // Keys the review answers to, as a reader would press them: `help` for `?`
        if overlay == "help" {
            app.update(luxe_tui::app::Msg::Key(KeyEvent::new(
                KeyCode::Char('?'),
                KeyModifiers::NONE,
            )));
        }
        if overlay == "palette" {
            app.update(luxe_tui::app::Msg::Key(KeyEvent::new(
                KeyCode::Char('k'),
                KeyModifiers::CONTROL,
            )));
        }
    }
    // Demo the file list at scale: clone the fixture file into many deeply-nested long paths so
    // the rail's one-line truncation + colored counts are visible for screenshot review.
    let many_files = env::var("LUXE_FIXTURE_MANYFILES").ok();
    if let Some(mode) = many_files.as_deref() {
        use luxe_core::guided_review::{FileId, ReviewFileStatus, ReviewItemRef};
        if let Some(review) = app.guided_review.as_mut() {
            let template = review.files[0].clone();
            let specs: &[(&str, u64, u64, ReviewFileStatus)] = &[
                ("crates/luxe-core/src/keymap/actions.rs", 5, 0, ReviewFileStatus::Modified),
                ("crates/luxe-core/src/config.rs", 109, 2, ReviewFileStatus::Modified),
                ("crates/luxe-tui/src/app/commands.rs", 3, 0, ReviewFileStatus::Modified),
                ("crates/luxe-tui/src/model_picker.rs", 51, 16, ReviewFileStatus::Modified),
                ("crates/luxe-tui/src/session_picker.rs", 167, 39, ReviewFileStatus::Modified),
                (
                    "crates/luxe-tui/src/view/guided_review/context.rs",
                    151,
                    30,
                    ReviewFileStatus::Modified,
                ),
                (
                    "crates/luxe-tui/src/view/guided_review/diff_render.rs",
                    330,
                    116,
                    ReviewFileStatus::Modified,
                ),
                ("crates/luxe-tui/src/themes/rose-pine.json", 2, 0, ReviewFileStatus::Added),
                ("docs/design/legacy-review-notes.md", 0, 84, ReviewFileStatus::Deleted),
            ];
            for (path, additions, deletions, status) in specs {
                let mut file = template.clone();
                file.id = FileId::from_path_lineage(None, Some(std::path::Path::new(path)));
                file.new_path = Some(path.into());
                file.old_path = Some(path.into());
                file.additions = Some(*additions);
                file.deletions = Some(*deletions);
                file.status = *status;
                if let Some(chapter) = review.chapters.first_mut() {
                    chapter.items.push(ReviewItemRef::File(file.id.clone()));
                }
                review.files.push(file);
                // Keep the review summary consistent with the injected files.
                review.stats.files += 1;
                review.stats.additions = Some(review.stats.additions.unwrap_or(0) + additions);
                review.stats.deletions = Some(review.stats.deletions.unwrap_or(0) + deletions);
            }
        }
        // `LUXE_FIXTURE_MANYFILES=hidden` puts the guide away, so the injected list is read from
        // the diff pane's own header instead.
        if mode == "hidden" {
            app.update(luxe_tui::app::Msg::GuidedReviewToggleGuide);
        }
    }
    // Demo the intra-line marks: replace the first hunk with pairs the fixture's single-token edit
    // cannot show — two separate edits in one line, an edit that only moves punctuation, a rewrite
    // that must come back unmarked. `LUXE_FIXTURE_INTRALINE=1`.
    if env::var("LUXE_FIXTURE_INTRALINE").is_ok() {
        use luxe_core::guided_review::{ReviewLine, ReviewLineKind};
        let pairs = [
            (
                "/// Takes a string like '2' and returns it.",
                "/// Takes a number like 2 and returns it.",
            ),
            ("    pub category: String,", "    pub category: NotificationCategory,"),
            (
                "    let label: String = category.to_string();",
                "    let label = category.display_name();",
            ),
            ("        if ready && !stale {", "        if ready {"),
            ("    send(payload, Priority::Low)?;", "    dispatch(payload).await?;"),
        ];
        if let Some(hunk) = app
            .guided_review
            .as_mut()
            .and_then(|review| review.files.first_mut().and_then(|file| file.hunks.first_mut()))
        {
            hunk.lines = pairs
                .iter()
                .enumerate()
                .flat_map(|(index, (removed, added))| {
                    let number = index as u32 * 2 + 1;
                    [
                        ReviewLine {
                            old_line: Some(number),
                            new_line: None,
                            kind: ReviewLineKind::Deletion,
                            text: (*removed).into(),
                        },
                        ReviewLine {
                            old_line: None,
                            new_line: Some(number),
                            kind: ReviewLineKind::Addition,
                            text: (*added).into(),
                        },
                    ]
                })
                .collect();
        }
    }
    // Demo tab-indented source, which is what exposed the expansion defect: `LUXE_FIXTURE_TABS=1`.
    if env::var("LUXE_FIXTURE_TABS").is_ok() {
        use luxe_core::guided_review::{ReviewLine, ReviewLineKind};
        if let Some(hunk) = app
            .guided_review
            .as_mut()
            .and_then(|review| review.files.first_mut().and_then(|file| file.hunks.first_mut()))
        {
            hunk.lines = (1..=5u32)
                .flat_map(|depth| {
                    let indent = "\t".repeat(depth as usize);
                    [
                        ReviewLine {
                            old_line: Some(depth * 2),
                            new_line: None,
                            kind: ReviewLineKind::Deletion,
                            text: format!("{indent}<div class=\"grid grid-cols-2 gap-4\">"),
                        },
                        ReviewLine {
                            old_line: None,
                            new_line: Some(depth * 2),
                            kind: ReviewLineKind::Addition,
                            text: format!("{indent}<div class=\"form-control\">"),
                        },
                    ]
                })
                .collect();
        }
    }
    // Demo the review word-wrap: inject a long addition line and turn wrapping on so it is visible
    // for screenshot review (the fixture's own lines are short).
    if env::var("LUXE_FIXTURE_WRAP").is_ok() {
        if let Some(line) = app.guided_review.as_mut().and_then(|review| {
            review.files.first_mut().and_then(|file| file.hunks.first_mut()).and_then(|hunk| {
                hunk.lines
                    .iter_mut()
                    .find(|line| line.kind == luxe_core::guided_review::ReviewLineKind::Addition)
            })
        }) {
            line.text = "let very_long_identifier = compute_something(alpha, beta, gamma, delta, epsilon, zeta, eta, theta, iota, kappa);".into();
        }
        app.update(luxe_tui::app::Msg::Key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE)));
    }
    // A scenario with a pinned clock is mid-animation on purpose, so run the reveal forward the way
    // the event loop does — one frame every 12 ms — and then stop, leaving a frame that is the same
    // every time it is captured. Stepping matters: a frame's timestep is capped, so jumping the clock
    // would land somewhere else entirely.
    if let Some(base) = app.status.frame_clock {
        for step in 1..=REVEAL_FRAMES {
            app.status.frame_clock = Some(base + Duration::from_millis(12) * step);
            draw(terminal, app)?;
        }
    }
    // A named wordmark frame is a still: the shine is a pure function of the tick, so holding the
    // tick holds the frame, and the capture is the same every time instead of wherever ~1s of
    // ticking happened to land.
    let pinned = env::var("LUXE_FIXTURE_BANNER_TICK").is_ok();
    // Capture the mouse, so a pointer gesture is reviewable the way a keystroke is. This is the
    // only place the production mouse router meets a real terminal without a live engine behind it.
    terminal::set_mouse_capture(true)?;
    loop {
        draw(terminal, app)?;
        // Poll on the production tick, and tick the app when nothing arrives, so animated chrome
        // (the activity bar, tool-card spinners) runs at the speed it will really run at.
        if event::poll(Duration::from_millis(33))? {
            match event::read()? {
                // Esc closes whatever the app has open before it closes the fixture, or a
                // surface that exists to be reviewed can never be reviewed.
                Event::Key(key)
                    if matches!(key.code, KeyCode::Esc | KeyCode::Char('q'))
                        && app.overlays.image_viewer.is_none() =>
                {
                    return Ok(());
                }
                Event::Key(key)
                    if key.code == KeyCode::Char('b')
                        && key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    app.update(luxe_tui::app::Msg::GuidedReviewToggleSplit);
                }
                // Reflow on window resize so responsive layouts (dock, dashboard) can be
                // reviewed at any size, not just the size present at launch.
                Event::Resize(width, height) => {
                    app.update(luxe_tui::app::Msg::TerminalResized { width, height });
                }
                // Forward every other key to the app so overlays (palette via Ctrl+K, pickers,
                // in-review actions) can be driven for screenshot review.
                Event::Key(key) => {
                    app.update(luxe_tui::app::Msg::Key(key));
                }
                // Pointer gestures go through the production router, so drag-to-select,
                // multi-click, and click-to-expand can be reviewed here too.
                Event::Mouse(mouse) => {
                    let action = route_mouse(app, &mouse);
                    apply(app, action);
                }
                _ => {}
            }
        } else if !pinned {
            app.update(luxe_tui::app::Msg::Tick);
        }
    }
}

/// Applies what the mouse router asked for. The fixture has no engine, so the outcomes that end a
/// session or reopen a project are unreachable here and ignored.
fn apply(app: &mut luxe_tui::app::App, action: RouteAction) {
    match action {
        RouteAction::Msg(message) => app.update(*message),
        RouteAction::Scroll(ScrollAction::Up(lines)) => app.scroll_state.up_by(lines),
        RouteAction::Scroll(ScrollAction::Down(lines)) => app.scroll_state.down_by(lines),
        RouteAction::Scroll(ScrollAction::PageUp) => app.scroll_state.page_up(),
        RouteAction::Scroll(ScrollAction::PageDown) => app.scroll_state.page_down(),
        RouteAction::Scroll(ScrollAction::ToBottom) => app.scroll_state.to_bottom(),
        RouteAction::Scroll(ScrollAction::DragTo { row, height }) => {
            app.scroll_state.drag_to(row, height);
        }
        _ => {}
    }
}

/// Frames a clock-pinned scenario is stepped before it is captured — about a third of a second,
/// far enough in that the reveal has a settled beginning and a fading tail to look at.
const REVEAL_FRAMES: u32 = 28;

fn parse_args() -> Result<(String, FixtureScenario)> {
    let mut args = env::args().skip(1);
    let Some(flag) = args.next() else {
        bail!(usage());
    };
    if flag == "--help" || flag == "-h" {
        println!("{}", usage());
        std::process::exit(0);
    }
    if flag != "--theme" {
        bail!(usage());
    }
    let Some(theme) = args.next() else {
        bail!(usage());
    };
    let Some(scenario) = args.next() else {
        bail!(usage());
    };
    if args.next().is_some() {
        bail!(usage());
    }

    Ok((theme, scenario.parse().map_err(anyhow::Error::msg)?))
}

fn usage() -> String {
    let names: Vec<&str> = FixtureScenario::ALL.iter().map(|scenario| scenario.name()).collect();
    format!(
        "usage: cargo run -p luxe-tui --example ui_fixture -- --theme <built-in-id> <{}>",
        names.join("|")
    )
}

/// The styles of the Omarchy theme `name`, installed or the user's own.
fn desktop_styles(name: &str) -> Result<luxe_tui::theme::styles::ThemeStyles> {
    let home = luxe_core::user_dirs::home_dir().unwrap_or_default();
    let dir = [".config/omarchy/themes", ".local/share/omarchy/themes"]
        .iter()
        .map(|root| home.join(root).join(name))
        .find(|dir| dir.is_dir());
    let Some(dir) = dir else { bail!("no Omarchy theme named {name}") };
    let desktop = luxe_core::omarchy::DesktopTheme { name: name.to_owned(), dir };
    let registry = luxe_core::theme::ThemeRegistry::new().with_desktop(Some(&desktop));
    let Some(resolved) = registry.resolve(luxe_core::omarchy::DESKTOP_THEME) else {
        bail!("{name} has no colors Luxe can read")
    };
    Ok(luxe_tui::theme::styles::ThemeStyles::from_resolved(&resolved, name))
}

/// Draws a frame, with the headings in lower case under `LUXE_FIXTURE_HEADINGS=lower`.
///
/// For weighing a look before building it: the frame is drawn as shipped and its headings are
/// rewritten in the buffer, so the comparison is the same frame with one thing changed. A heading
/// is told by how it is drawn, a bold word of three or more capitals standing alone, which is
/// every section badge and pane tab and no code a preview could be showing.
fn draw(terminal: &mut terminal::Tui, app: &mut luxe_tui::app::App) -> io::Result<()> {
    if env::var("LUXE_FIXTURE_HEADINGS").as_deref() != Ok("lower") {
        return view::draw(terminal, app);
    }
    terminal::draw_synchronized(terminal, |frame| {
        view::render(frame, app);
        lower_headings(frame.buffer_mut());
    })
}

fn lower_headings(buffer: &mut ratatui::buffer::Buffer) {
    use ratatui::style::Modifier;
    let area = buffer.area;
    for y in area.top()..area.bottom() {
        let mut x = area.left();
        while x < area.right() {
            let heading = |cell: &ratatui::buffer::Cell| {
                cell.modifier.contains(Modifier::BOLD)
                    && cell.symbol().chars().all(|c| c.is_ascii_uppercase())
                    && !cell.symbol().is_empty()
            };
            let start = x;
            while x < area.right() && heading(&buffer[(x, y)]) {
                x += 1;
            }
            let alone = |at: u16| {
                at < area.left()
                    || at >= area.right()
                    || !buffer[(at, y)].symbol().chars().all(char::is_alphanumeric)
                    || buffer[(at, y)].symbol().is_empty()
            };
            if x - start >= 3 && alone(start.wrapping_sub(1)) && alone(x) {
                for column in start..x {
                    let lower = buffer[(column, y)].symbol().to_ascii_lowercase();
                    buffer[(column, y)].set_symbol(&lower);
                }
            }
            x = x.max(start + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_describes_stable_fixture_interface() {
        assert!(usage().contains("--theme <built-in-id>"));
        assert!(usage().contains("chat-tool-error"));
        assert!(usage().contains("chat-tool-run"));
        assert!(usage().contains("guided-review-wide"));
        assert!(usage().contains("guided-review-findings-empty"));
        assert!(usage().contains("guided-review-findings"));
        assert!(usage().contains("guided-review-needs-verification"));
        assert!(usage().contains("guided-review-finish"));
        assert!(usage().contains("guided-review-medium"));
        assert!(usage().contains("guided-review-narrow"));
        assert!(usage().contains("dock-compact"));

        // Every name this offers has to open something. The list had drifted: it advertised a
        // `guided-review-skim` that no longer exists, so the one place a user looks up a surface
        // was naming one they could not reach.
        let usage = usage();
        let offered = usage
            .rsplit_once('<')
            .map(|(_, tail)| tail.trim_end_matches('>'))
            .expect("the scenario list");
        let mut advertised = Vec::new();
        for name in offered.split('|') {
            let scenario = name.parse::<FixtureScenario>();
            assert!(scenario.is_ok(), "usage offers unknown {name:?}");
            advertised.push(scenario.unwrap());
        }

        // And the list checks walk has to be the same one, or a scenario dodges every check by
        // being in neither: `FixtureScenario::ALL` is what the fixture's own invariants run over.
        for scenario in FixtureScenario::ALL {
            assert!(advertised.contains(scenario), "usage never offers {scenario:?}");
        }
        assert_eq!(advertised.len(), FixtureScenario::ALL.len(), "the two lists have drifted");
    }
}
