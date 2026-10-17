//! Work the loop starts off the frame — listings, probes, copies and watching the desktop's
//! theme — and the channels each answers on.

use super::paste::ClipboardPaste;
use crate::app::{App, Msg};
use crate::auth_wizard::AuthUiMsg;
use crate::model_picker::ModelMenu;
use std::time::Duration;
use tokio::sync::mpsc;

/// What the loop listens to besides the engine and the terminal, and the senders its background
/// work answers on.
pub(super) struct Channels {
    /// A running login flow: progress lines, questions, and its final outcome
    pub(super) auth: mpsc::Receiver<AuthUiMsg>,
    pub(super) paste_tx: mpsc::Sender<ClipboardPaste>,
    pub(super) paste: mpsc::Receiver<ClipboardPaste>,
    /// Catalogs rebuilt off the frame, after a local server answered what it is serving
    menu_tx: mpsc::Sender<ModelMenu>,
    pub(super) menu: mpsc::Receiver<ModelMenu>,
    /// Listings read off the frame: a mention source's rows, the project's files, its sessions
    listed_tx: mpsc::Sender<Msg>,
    pub(super) listed: mpsc::Receiver<Msg>,
    pub(super) desktop: mpsc::Receiver<()>,
    desktop_watch: Option<tokio::task::JoinHandle<()>>,
    /// Copies that no local clipboard backend accepted, routed back to the main thread for OSC 52
    copy_failures_tx: mpsc::Sender<String>,
    copy_failures: mpsc::Receiver<String>,
}

impl Channels {
    /// Opens the loop's channels around `auth`, the login flows' receiver.
    pub(super) fn open(auth: mpsc::Receiver<AuthUiMsg>) -> Self {
        let (paste_tx, paste) = mpsc::channel(10);
        let (menu_tx, menu) = mpsc::channel(4);
        let (listed_tx, listed) = mpsc::channel(4);
        // Omarchy re-themes the whole desktop at once, and a session left in the old palette is
        // the one window that did not follow. Watched only when this is Omarchy, off the frame.
        let (desktop_tx, desktop) = mpsc::channel(1);
        let desktop_watch = watch_desktop_theme(desktop_tx);
        let (copy_failures_tx, copy_failures) = mpsc::channel(4);
        Self {
            auth,
            paste_tx,
            paste,
            menu_tx,
            menu,
            listed_tx,
            listed,
            desktop,
            desktop_watch,
            copy_failures_tx,
            copy_failures,
        }
    }

    /// Stops watching the desktop's theme.
    pub(super) fn close(self) {
        if let Some(watch) = self.desktop_watch {
            watch.abort();
        }
    }
}

/// Starts whatever the app asked for that runs off the frame: a mention source's listing, the
/// project's files, its sessions, a model probe and a copy.
pub(super) fn spawn_requested(app: &mut App, channels: &Channels) {
    // The picker is drawn from what is on disk and corrected when the server answers, so this
    // never delays a frame — and a server that is switched off simply never answers.
    // A source is a program the reader named; it runs off the render thread, bounded
    if let Some(source) = app.outbox.mention_source.take() {
        let tx = channels.listed_tx.clone();
        tokio::spawn(async move {
            let cwd = std::env::current_dir().unwrap_or_default();
            let answer = luxe_core::mention_source::list(&source, &cwd)
                .await
                .map_err(|error| capitalized(&error.to_string()));
            let _ = tx.send(Msg::MentionsListed { kind: source.kind(), answer }).await;
        });
    }

    // The `@` picker's files, read off the render thread: a large checkout takes a while
    if app.file_listing.start(std::time::Instant::now()) {
        let tx = channels.listed_tx.clone();
        tokio::spawn(async move {
            let cwd = std::env::current_dir().unwrap_or_default();
            let files = tokio::task::spawn_blocking(move || crate::ref_provider::list_files(&cwd))
                .await
                .ok();
            let _ = tx.send(Msg::FilesListed(files)).await;
        });
    }

    // `/sessions` opened on nothing and fills when this lands
    if let Some(request) = app.outbox.session_catalog.take() {
        if let Some(source) = app.session_catalog.source.clone() {
            let tx = channels.listed_tx.clone();
            tokio::spawn(async move {
                let catalog = source().await;
                let _ = tx.send(Msg::SessionsListed { request, catalog }).await;
            });
        }
    }

    if std::mem::take(&mut app.outbox.model_probe) {
        if let Some(probe) = app.model_sources.as_ref().and_then(|sources| sources.probe.clone()) {
            let tx = channels.menu_tx.clone();
            tokio::spawn(async move {
                let menu = probe().await;
                let _ = tx.send(menu).await;
            });
        }
    }

    // Copies go through the one shared sink (see `clipboard::create_clipboard`), off the render
    // thread. A failure reports back so the OSC 52 last resort can run *here*, where writing to
    // the terminal can't interleave with a frame.
    if let Some(text) = app.outbox.copy.take() {
        let sink = app.clipboard_sink.clone();
        let failures = channels.copy_failures_tx.clone();
        tokio::spawn(async move {
            if let Err(error) = sink.write_text(&text).await {
                tracing::debug!(%error, "no system clipboard took it; trying the terminal");
                let _ = failures.send(text).await;
            }
        });
    }
}

/// Asks the terminal itself to take the copies no local clipboard did, and answers whether the
/// screen needs repainting.
///
/// OSC 52 works over ssh and through a multiplexer. A terminal that ignores it may echo the
/// payload, so the screen is cleared after.
pub(super) fn write_failed_copies(
    term: &mut crate::terminal::Tui,
    channels: &mut Channels,
) -> bool {
    let mut repaint = false;
    while let Ok(text) = channels.copy_failures.try_recv() {
        if let Some(sequence) = crate::clipboard::osc52_sequence(&text) {
            use std::io::Write;
            let mut out = std::io::stdout();
            if out.write_all(sequence.as_bytes()).and_then(|()| out.flush()).is_err() {
                tracing::warn!("terminal clipboard (OSC 52) write failed");
            }
            let _ = term.clear();
            // The screen is blank until something repaints it, and the copy came from a channel
            // rather than an event, so nothing else here would ask for a frame.
            repaint = true;
        }
    }
    repaint
}

/// How often the desktop's theme is checked. A re-theme is a deliberate act that takes Omarchy a
/// moment itself, so a couple of seconds reads as following it; each check is two `stat`s.
const DESKTOP_THEME_POLL: Duration = Duration::from_secs(2);

/// Watches Omarchy's applied theme and says when it changes, or does nothing off Omarchy.
///
/// Polled rather than watched with inotify: the applied theme is a directory Omarchy replaces
/// wholesale, which a watch on it loses, and a stamp of two files is simpler to get right.
fn watch_desktop_theme(changed: mpsc::Sender<()>) -> Option<tokio::task::JoinHandle<()>> {
    use luxe_core::omarchy::DesktopTheme;
    let stamp = || DesktopTheme::current().map(|desktop| desktop.stamp());
    let mut last = stamp()?;
    Some(tokio::spawn(async move {
        loop {
            tokio::time::sleep(DESKTOP_THEME_POLL).await;
            let Ok(Some(now)) = tokio::task::spawn_blocking(stamp).await else { continue };
            if now != last {
                last = now;
                if changed.send(()).await.is_err() {
                    return;
                }
            }
        }
    }))
}

/// A sentence from an error message, which starts lowercase by convention.
fn capitalized(message: &str) -> String {
    let mut chars = message.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str().trim_end_matches('.')),
        None => String::new(),
    }
}
