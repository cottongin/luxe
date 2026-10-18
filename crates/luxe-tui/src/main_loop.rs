//! Main event loop for the TUI.
//!
//! Selects over crossterm events and the core Engine event channel,
//! driving the Elm-style update cycle.

mod background;
mod click;
mod keys;
mod launch;
mod mouse;
mod pacing;
mod paste;

use crate::app::{App, Msg};
use crate::workspace_layout::Pane;
use crate::{terminal, view};
use background::Channels;
use click::{click, clicked_target, wheel};
use crossterm::event::EventStream;
#[cfg(test)]
use crossterm::event::{Event, MouseButton, MouseEvent, MouseEventKind};
#[cfg(test)]
pub(crate) use keys::collect_exit_for_tests;
#[cfg(test)]
use keys::{apply_terminal_event, collect_exit};
pub use keys::{route_key, RouteAction, ScrollAction};
use launch::{run_launch, Launch, MouseCapture};
use luxe_core::config::Config;
use luxe_core::engine::Action;
use luxe_core::keymap::{load_keybindings, KeyMap};
pub use mouse::{route_mouse, route_mouse_at};
#[cfg(test)]
use pacing::may_coalesce;
use pacing::{
    apply_engine_events, apply_terminal_batch, next_event, reveal_sleep, reveal_wait, FRAME_BUDGET,
    REVEAL_FRAME,
};
pub(crate) use pacing::{frame_decision, FrameDecision, Wake};
use paste::spawn_clipboard_read;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::interval;

/// The animation tick, about 30 a second: what every timed effect counts in, the wordmark's lights
/// and the spinner included. Public so a recording of one, the README's wordmark, keeps the same
/// time.
pub const ANIMATION_TICK: Duration = Duration::from_millis(33);

/// Why the TUI returned control to the CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TuiExit {
    /// The user quit Luxe.
    Quit,
    /// Open a file in the reader's editor, then come back.
    ///
    /// Args: `(path)` — chosen on the Configuration surface or in the context-file list, both of
    /// which know where the file they named lives. Not only config any more, which is why it is no
    /// longer called that.
    OpenInEditor(PathBuf),
    /// End the session and remove its worktree, whose work is all committed.
    ///
    /// Args: `(path)` — the worktree, which the CLI removes once the process has left it.
    RemoveWorktree(PathBuf),
    /// Check a pull request out in a worktree and start a new session there, fixing it.
    FixPullRequest(FixLaunch),
    /// Start a new session on an issue, in a worktree of its own, as `luxe --issue` does.
    ///
    /// Args: `(number)` — the issue, on the checkout's repository.
    TakeUpIssue(u64),
    /// Leave this session for another of yours, in its own worktree when it has one.
    ///
    /// Args: `(launch)` — the session, and the draft and mode the reader carries into it.
    ResumeSession(ResumeLaunch),
    /// Leave Luxe and run `luxe update`, which needs the terminal for the package manager.
    Update,
}

/// A pull request to fix in a new session, and the comments written reading it, which become the
/// agent's work there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixLaunch {
    /// Number within its repository
    pub number: u64,
    /// Unsent comments from the review it was read in
    pub carried: Vec<luxe_core::guided_review::ReviewFinding>,
}

/// Another of your sessions to leave this one for, and the draft the reader had in hand when they
/// chose it, which a session opened in place would keep. The mode, model and thinking level travel
/// with the engine's own carryover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeLaunch {
    /// The session to resume
    pub session: String,
    /// The composer's draft, expanded
    pub draft: String,
    /// The images attached to the draft
    pub images: Vec<crate::editor::AttachedImage>,
}

/// What the TUI opens on.
#[derive(Debug)]
pub enum Opening {
    /// The dashboard
    Dashboard(Box<crate::dashboard::DashboardState>),
    /// Straight into the conversation
    Chat,
    /// A session the CLI resumed, taking up what the reader carried out of the surface they left
    Resumed(ResumeLaunch),
    /// A checked-out pull request's review, adopting the comments carried to it
    FixPullRequest(FixLaunch),
    /// The setup flow alone, for `luxe init`: finishing it leaves
    Setup,
}

/// What the CLI reads from disk on the TUI's behalf, again each time a picker asks for it.
pub struct Catalogs {
    /// The models the model picker offers
    pub models: crate::model_picker::ModelSources,
    /// The project's sessions, for `/sessions`; `None` where there are none to switch to
    pub sessions: Option<crate::dashboard::SessionCatalogSource>,
}

/// What a launch opens on, once the credential state is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartupSurface {
    /// Whatever the caller set up — the dashboard, or straight into the conversation.
    AsGiven,
    /// The Configuration surface, because nothing can authenticate a turn and nothing else on
    /// screen would say so.
    Configuration,
}

/// Where a launch lands.
///
/// The dashboard is the answer whenever there is one: everyone sees the same surface, and a machine
/// with no credential sees it leading with `Set up a provider`. Setup only takes the screen when
/// there is no dashboard to say it — `"dashboard": false`, or a resumed session — because the
/// alternative is a composer that answers the first message with an auth error.
pub(crate) fn startup_surface(
    configured: bool,
    has_dashboard: bool,
    headless: bool,
) -> StartupSurface {
    match !configured && !headless && !has_dashboard {
        true => StartupSurface::Configuration,
        false => StartupSurface::AsGiven,
    }
}

/// Whether this is the first launch on the machine: no `config.json` yet, and someone at the
/// keyboard to ask.
///
/// Strict on purpose. A question asked of a terminal nobody is watching stops a container or a CI
/// job until something answers it, so anything not interactive never sees it — and skipping it
/// writes the file, so it is asked once.
fn is_first_run(headless: bool) -> bool {
    use std::io::IsTerminal;
    !headless
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && !luxe_core::config::Config::global_path().exists()
}

/// Run the TUI event loop.
///
/// `model` labels the status line, and context % fills in from the engine's `ContextUsage` events
/// as turns run. `catalogs` are read again whenever a picker opens, so config and catalog edits and
/// sessions started elsewhere land without a relaunch.
pub async fn run(
    event_rx: mpsc::Receiver<luxe_core::event::Event>,
    engine_tx: mpsc::Sender<Action>,
    status: crate::statusline::StatusContext,
    opening: Opening,
    catalogs: Catalogs,
    auth: Option<crate::auth_wizard::AuthServiceHandle>,
    headless: bool,
) -> Result<TuiExit, anyhow::Error> {
    let mut term = terminal::init()?;
    // an error out of the loop below leaves the terminal as it found it, as a panic does
    let restore = terminal::RestoreOnDrop::armed();
    let mut app = setup_app(&term, engine_tx, status, catalogs, headless).await?;
    let auth_rx = open_first_surface(&mut app, opening, auth, headless);
    let mut channels = Channels::open(auth_rx);
    let mut sources = Sources {
        event_rx,
        terminal_events: Some(EventStream::new()),
        tick: interval(ANIMATION_TICK),
        // One listener for the run, rather than a fresh registration on every pass of a loop that
        // turns over as fast as the terminal can deliver keys.
        interrupted: Box::pin(tokio::signal::ctrl_c()),
    };

    // Reconcile terminal mouse capture to the configured default before the first draw.
    let mut capture = MouseCapture::default();
    if let Some(on) = capture.change(app.mouse_enabled) {
        let _ = terminal::set_mouse_capture(on);
    }
    view::draw(&mut term, &mut app)?;
    // Inside a herdr pane, herdr follows the agent's state; outside one this is `None`
    let mut herdr = crate::herdr::Reporter::start(app.config.herdr() && !headless);

    let mut exit = TuiExit::Quit;
    let mut pacing = Pacing { last_draw: std::time::Instant::now(), pending_frame: false };
    loop {
        // While the reveal owes the screen text it sets the frame rate, in place of the 33 ms tick.
        let revealing = app.reveal_pending();
        let reveal_frame_wait = reveal_wait(revealing, pacing.last_draw.elapsed());
        let woke = select_once(
            &mut app,
            &mut sources,
            &mut channels,
            &mut exit,
            &mut pacing,
            reveal_frame_wait,
        );
        let Some(woke) = woke.await else { break };
        if std::mem::take(&mut app.outbox.clipboard_paste) {
            spawn_clipboard_read(channels.paste_tx.clone());
        }
        // The terminal goes to the program and comes back; the engine never stops underneath
        if let Some(launch) = Launch::take(&mut app) {
            let (events, rx) = (&mut sources.terminal_events, &mut sources.event_rx);
            run_launch(&mut term, events, rx, &mut app, launch, &mut capture).await?;
            pacing.last_draw = std::time::Instant::now();
            continue;
        }
        background::spawn_requested(&mut app, &channels);
        pacing.pending_frame |= background::write_failed_copies(&mut term, &mut channels);
        if let Some(herdr) = herdr.as_mut() {
            herdr.report(app.agent_state(), app.session_id());
        }
        if app.should_quit {
            break;
        }
        // Apply any runtime mouse-capture toggle (`/mouse`, ToggleMouse) before the next draw.
        if let Some(on) = capture.change(app.mouse_enabled) {
            let _ = terminal::set_mouse_capture(on);
        }
        draw_frame(&mut term, &mut app, &woke, revealing, &mut pacing)?;
    }
    channels.close();
    crate::frame_profile::write();
    // Every other exit comes straight back to a new TUI in the same pane, which reports again
    if let (Some(herdr), TuiExit::Quit) = (herdr, &exit) {
        herdr.release().await;
    }

    // Hand back every image the terminal is holding for us before leaving the alternate screen.
    // Leaving the screen drops the placements; the pixels behind them would stay in its memory.
    app.images.release_all();
    let _ = terminal::write_graphics(&app.images.drain_control());

    restore.finish()?;

    Ok(exit)
}

/// Builds the app the loop drives: config, keymap, the persisted menus and pane widths, the
/// catalogs, the terminal's size, and what the CLI already knows for the status line.
async fn setup_app(
    term: &terminal::Tui,
    engine_tx: mpsc::Sender<Action>,
    status: crate::statusline::StatusContext,
    catalogs: Catalogs,
    headless: bool,
) -> std::io::Result<App> {
    // Load UI config + keymap (defaults + layers). Apply user keybindings overrides from
    // `keybindings.json` (global + project) so a rebound key fires the right action from launch.
    let repo_root = std::env::current_dir().unwrap_or_default();
    let config = Config::load(&repo_root);
    let keymap = KeyMap::defaults().with_overrides(load_keybindings(&repo_root));
    // Resolve the icon vocabulary before the first frame: it is a property of the terminal and the
    // config, and every surface reads it while drawing.
    crate::glyphs::resolve_vocabulary(config.glyphs());

    let mut app = App::new(engine_tx, keymap, config);
    // What the palette and `/` ran last time leads their menus this time
    app.recent_commands =
        crate::commands::RecentCommands::persisted(luxe_core::runtime_state::RuntimeState::path());
    // And the panes come back at the widths they were dragged to
    app.panes = crate::workspace_layout::PaneLayout::persisted(
        luxe_core::runtime_state::RuntimeState::path(),
    );
    app.guided_review_workspace.set_guide_width(app.panes.width(Pane::ReviewGuide));
    let Catalogs { models, sessions } = catalogs;
    let menu = (models.build)();
    app.set_model_catalog(menu.catalog, menu.active);
    app.model_sources = Some(models);
    app.session_catalog.source = sessions;
    let terminal_size = term.size()?;
    app.update(Msg::TerminalResized { width: terminal_size.width, height: terminal_size.height });
    app.clipboard_sink = crate::clipboard::create_clipboard().await;
    // And the real opener: nothing short of a launch may start a browser or a viewer
    app.opener = crate::app::ExternalOpener::Desktop;
    app.status.model = status.model;
    app.status.provider = status.provider;
    app.status.context_window = status.context_window;
    app.status.thinking = status.thinking;
    app.status.git_branch = status.git_branch;
    app.status.git_dirty = status.git_dirty;
    app.status.project = status.project;
    app.status.cwd = status.cwd;
    // Launched in walk-away mode: the engine was told when it was built, and this is the TUI's own
    // copy, which is what the bar and the rail draw from.
    app.permission_auto_approve = status.auto_approve;
    app.headless = headless;
    Ok(app)
}

/// Opens what the launch lands on and hands the app its login service, answering with the
/// receiver its login flows report on.
fn open_first_surface(
    app: &mut App,
    opening: Opening,
    auth: Option<crate::auth_wizard::AuthServiceHandle>,
    headless: bool,
) -> mpsc::Receiver<crate::auth_wizard::AuthUiMsg> {
    // Show the dashboard if provided (and not suppressed by headless/resume/prompt).
    let first_run = matches!(opening, Opening::Dashboard(_)) && is_first_run(headless);
    let setup_only = matches!(opening, Opening::Setup);
    match opening {
        Opening::Dashboard(dashboard_state) => app.enter_dashboard(*dashboard_state),
        Opening::Chat | Opening::Setup => {}
        Opening::FixPullRequest(launch) => app.open_pull_request_fix(launch),
        Opening::Resumed(launch) => app.carry_in(launch),
    }

    // Login flows run on their own task and report back here. Bounded: a flow that outpaces the
    // render loop is reporting progress nobody can read.
    let (auth_tx, auth_rx) = mpsc::channel(16);
    let configured = auth.as_ref().is_none_or(|service| service.any_configured());
    app.auth_service = auth;
    app.auth_tx = Some(auth_tx);
    // Nothing configured anywhere: the dashboard leads with setup, and without a dashboard the
    // Configuration surface opens in its place — either way the first thing on screen is the fix,
    // rather than an auth error on the first message.
    app.set_provider_configured(configured);
    // The first launch asks its four questions once, over the start screen it then lands on; a
    // machine that skipped them is still told it has no provider, by the list's first row
    if setup_only {
        app.open_setup(crate::configuration::SetupMode::Init);
    } else if first_run {
        app.open_setup(crate::configuration::SetupMode::FirstRun);
    } else if startup_surface(configured, app.dashboard.is_some(), headless)
        == StartupSurface::Configuration
    {
        app.open_configuration();
    }
    auth_rx
}

/// What the loop waits on besides its background channels.
struct Sources {
    event_rx: mpsc::Receiver<luxe_core::event::Event>,
    /// An `Option` so a launcher can drop it outright — see `launcher::run` for why it must
    terminal_events: Option<EventStream>,
    tick: tokio::time::Interval,
    interrupted: std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<()>> + Send>>,
}

/// When the last frame was drawn, and whether a change is still waiting for one.
struct Pacing {
    last_draw: std::time::Instant,
    /// A change the frame budget deferred. The animation tick would otherwise be the only thing
    /// that ever draws it, and a tick only draws while something is animating.
    pending_frame: bool,
}

/// What woke one iteration, and what the frame profile records about it.
struct Woke {
    wake: Wake,
    update_us: u128,
    coalesced: usize,
}

/// Waits for the next thing to happen and applies it to the app; `None` once ctrl+c asks to quit.
async fn select_once(
    app: &mut App,
    sources: &mut Sources,
    channels: &mut Channels,
    exit: &mut TuiExit,
    pacing: &mut Pacing,
    reveal_frame_wait: Option<std::time::Duration>,
) -> Option<Woke> {
    let mut woke = Woke { wake: Wake::Event, update_us: 0, coalesced: 0 };
    tokio::select! {
        Some(event) = sources.event_rx.recv() => {
            apply_engine_events(app, &mut sources.event_rx, event, exit);
        }

        Some(Ok(terminal_event)) = next_event(&mut sources.terminal_events) => {
            let started = std::time::Instant::now();
            let events = &mut sources.terminal_events;
            let (idle_motion, coalesced) =
                apply_terminal_batch(app, events, terminal_event, exit).await;
            // Pointer motion that changes nothing (no hover target, no selection) must not cost a
            // full transcript relayout; anything else is drawn on the spot.
            woke.wake = if idle_motion { Wake::IdleMotion } else { Wake::Input };
            woke.coalesced = coalesced;
            woke.update_us = started.elapsed().as_micros();
        }

        // Periodic tick: advances animation phase, and paints only if something is moving.
        _ = sources.tick.tick() => {
            woke.wake = Wake::Tick;
            let items = app.convo.len();
            app.update(Msg::Tick);
            // A tick is bookkeeping, except when it put something in the transcript
            pacing.pending_frame |= app.convo.len() != items;
        }

        // The reveal's own clock, armed only while it owes the screen text. It asks for a frame
        // and nothing else: a `Tick` here would run the spinner and the activity comet at nearly
        // three times the speed they were drawn for.
        () = reveal_sleep(reveal_frame_wait) => woke.wake = Wake::Tick,

        Some(message) = channels.auth.recv() => app.update(Msg::Auth(message)),
        Some(menu) = channels.menu.recv() => app.update(Msg::ModelCatalogRefreshed(menu.catalog)),
        Some(listed) = channels.listed.recv() => app.update(listed),
        Some(()) = channels.desktop.recv() => app.update(Msg::DesktopThemeChanged),
        Some(payload) = channels.paste.recv() => paste::deliver(app, payload),

        _ = &mut sources.interrupted => {
            tracing::info!("ctrl+c received, quitting");
            return None;
        }
    }
    Some(woke)
}

/// Paints, defers or skips the frame for one iteration, per [`frame_decision`].
fn draw_frame(
    term: &mut terminal::Tui,
    app: &mut App,
    woke: &Woke,
    revealing: bool,
    pacing: &mut Pacing,
) -> std::io::Result<()> {
    let budget = if revealing { REVEAL_FRAME } else { FRAME_BUDGET };
    let since_draw = pacing.last_draw.elapsed();
    match frame_decision(woke.wake, app.animating(), pacing.pending_frame, since_draw, budget) {
        FrameDecision::Skip => {}
        FrameDecision::Defer => pacing.pending_frame = true,
        FrameDecision::Draw => {
            let started = std::time::Instant::now();
            view::draw(term, app)?;
            crate::frame_profile::note_frame(crate::frame_profile::Frame {
                wake: match woke.wake {
                    Wake::Input => "input",
                    Wake::Event => "event",
                    Wake::Tick => "tick",
                    Wake::IdleMotion => "motion",
                },
                update_us: woke.update_us,
                draw_us: started.elapsed().as_micros(),
                coalesced: woke.coalesced,
            });
            pacing.pending_frame = false;
            pacing.last_draw = std::time::Instant::now();
        }
    }
    Ok(())
}

#[cfg(test)]
mod input_tests;
#[cfg(test)]
mod launch_tests;
#[cfg(test)]
pub(crate) mod overlay_matrix;
#[cfg(test)]
mod tests;
