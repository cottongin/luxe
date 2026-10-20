//! Watch — the bridge's own reader on every running session, while any device wants notifications.
//!
//! A phone's socket lasts only while the app is on screen, so nothing would hear a prompt
//! arrive while it's in a pocket. Instead the bridge joins each running session itself, as a
//! watch-only client, which the terminal doesn't announce and which can't act. It reads the
//! stream and sends nothing back. With no device subscribed it joins nothing, and leaves what it
//! had joined.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use luxe_core::local_endpoint::{self, LiveSession};
use luxe_core::Event;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::notice::{Heard, Notice, Reading, PROMPT_GRACE};
use crate::devices::DeviceBook;

/// How often the running sessions and the subscriptions are looked at again.
pub(crate) const WATCH_CHECK: Duration = Duration::from_secs(5);

/// Who the watcher joins as: unannounced, so its name shows only in the session's log.
pub(crate) const WATCHER: crate::relay::Introduction<'static> =
    crate::relay::Introduction { name: "Notifications", watch_only: true, yours: false };

/// Keeps a reader on each running session while any device is subscribed, sending what they hear
/// to `notices`, until the task is dropped.
pub(crate) async fn watch_sessions(
    runtime_dir: PathBuf,
    devices: DeviceBook,
    notices: mpsc::Sender<Notice>,
) {
    let mut watching: HashMap<String, Watcher> = HashMap::new();
    loop {
        let running = if anyone_subscribed(&devices).await {
            running(runtime_dir.clone()).await
        } else {
            Vec::new()
        };
        keep_watching(&mut watching, running, &notices);
        tokio::time::sleep(WATCH_CHECK).await;
    }
}

/// Stops the readers of sessions no longer running (and of any that quit), and starts one on each
/// session that has none, keyed by endpoint: a session's id changes when the terminal switches.
fn keep_watching(
    watching: &mut HashMap<String, Watcher>,
    running: Vec<LiveSession>,
    notices: &mpsc::Sender<Notice>,
) {
    let live = |address: &String| running.iter().any(|session| &session.address == address);
    watching.retain(|address, watcher| live(address) && !watcher.0.is_finished());
    for session in running {
        let notices = notices.clone();
        watching
            .entry(session.address.clone())
            .or_insert_with(|| Watcher(tokio::spawn(watch_session(session, notices))));
    }
}

/// A task stopped when dropped: a session's reader, or the notifications as a whole.
pub(crate) struct Watcher(pub(super) tokio::task::JoinHandle<()>);

impl Drop for Watcher {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn anyone_subscribed(devices: &DeviceBook) -> bool {
    let devices = devices.clone();
    let paired = tokio::task::spawn_blocking(move || devices.devices()).await;
    paired.is_ok_and(|paired| paired.iter().any(|device| device.subscription().is_some()))
}

async fn running(runtime_dir: PathBuf) -> Vec<LiveSession> {
    // reads a folder and, on Windows, asks `tasklist` about each pid
    let found = tokio::task::spawn_blocking(move || local_endpoint::running_sessions(&runtime_dir));
    found.await.unwrap_or_default()
}

/// Reads `session` until it ends, sending its notices on, each prompt only once it's waited
/// [`PROMPT_GRACE`] unanswered.
pub(crate) async fn watch_session(session: LiveSession, notices: mpsc::Sender<Notice>) {
    let Some(mut lines) = join(&session).await else { return };
    let mut reading = Reading::new(&session.session_id, &session.cwd);
    let mut held = Held::default();
    loop {
        // both are cancel-safe: a line stays buffered, and a held prompt is taken only once due
        let heard = tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(line)) => {
                    decode(&line).and_then(|event| reading.read(&event, Instant::now()))
                }
                Ok(None) | Err(_) => return,
            },
            notice = held.due() => Some(Heard::Now(notice)),
        };
        if !held.take(heard, &notices).await {
            return;
        }
    }
}

/// The session's stream, joined as the watcher.
async fn join(
    session: &LiveSession,
) -> Option<tokio::io::Lines<BufReader<local_endpoint::LocalStream>>> {
    match crate::routes::open(&session.address, &WATCHER).await {
        Ok(stream) => Some(BufReader::new(stream).lines()),
        Err(error) => {
            tracing::warn!(%error, session = %session.session_id, "couldn't watch a session");
            None
        }
    }
}

fn decode(line: &str) -> Option<Event> {
    serde_json::from_str(line)
        .inspect_err(|error| tracing::debug!(%error, "a session line that isn't an event"))
        .ok()
}

/// Prompts waiting out their grace.
#[derive(Default)]
struct Held {
    prompts: Vec<(Instant, String, Notice)>,
}

impl Held {
    /// Acts on what was heard: sends it, holds it or lets one go. False once nobody's listening.
    async fn take(&mut self, heard: Option<Heard>, notices: &mpsc::Sender<Notice>) -> bool {
        match heard {
            Some(Heard::Now(notice)) => return notices.send(notice).await.is_ok(),
            Some(Heard::Prompt(id, notice)) => {
                self.prompts.push((Instant::now() + PROMPT_GRACE, id, notice));
            }
            Some(Heard::Closed(id)) => self.prompts.retain(|(_, held, _)| *held != id),
            None => {}
        }
        true
    }

    /// The next prompt to come due, once it has; never, with none held.
    async fn due(&mut self) -> Notice {
        let Some(next) = self.prompts.iter().map(|(due, ..)| *due).min() else {
            return std::future::pending().await;
        };
        tokio::time::sleep_until(next).await;
        let at = self.prompts.iter().position(|(due, ..)| *due == next);
        // only `take` drops a prompt, and it can't run while this waits
        let at = at.expect("a due prompt is still held");
        self.prompts.remove(at).2
    }
}

#[cfg(test)]
mod tests;
