//! Guest — who a joined client is, as the terminal names it: the name it introduced itself by, or
//! its process id.
//!
//! The bridge introduces each device it relays for, so the terminal says "Approved from iPhone"
//! rather than naming the bridge's process. The name is cosmetic, since any program running as
//! you could claim one, and the ancestry check goes by process id, not by name.
//!
//! A client that opens by introducing itself as watch-only, like the bridge's notification
//! watcher, isn't announced. Being unseen is why it can't act: everything it sends after that is
//! dropped here, before the protocol sees it.

use luxe_core::local_endpoint::descends_from;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::watch;

/// How long a new client gets to introduce itself before the terminal says it joined.
pub(super) const INTRODUCTION_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

/// The longest name the terminal shows.
const NAME_LIMIT: usize = 40;

/// What a client said about itself.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Introduction {
    pub(super) name: String,
    pub(super) watch_only: bool,
    /// Paired by the person at the terminal, so its comings and goings needn't be told
    pub(super) yours: bool,
}

/// Who joined.
#[derive(Clone)]
pub(super) struct Guest {
    pid: Option<u32>,
    said: watch::Receiver<Option<Introduction>>,
}

impl Guest {
    /// A guest known by `pid` until it introduces itself through the returned sender.
    pub(super) fn new(pid: Option<u32>) -> (Self, watch::Sender<Option<Introduction>>) {
        let (named, said) = watch::channel(None);
        (Self { pid, said }, named)
    }

    /// Whether the client opened as watch-only, so it isn't announced.
    pub(super) fn watch_only(&self) -> bool {
        self.said.borrow().as_ref().is_some_and(|said| said.watch_only)
    }

    /// Whether the bridge said the person at the terminal paired it. Cosmetic, as the name is: it
    /// decides only whether joining earns a line in the transcript.
    pub(super) fn yours(&self) -> bool {
        self.said.borrow().as_ref().is_some_and(|said| said.yours)
    }

    /// Whether the client is `agent` or a process it started, directly or not.
    ///
    /// A tripwire, not a wall: a command can detach from its parent first. Unknown ancestry
    /// passes, since refusing it would refuse every client on a system that hides it.
    pub(super) async fn descends_from(&self, agent: u32) -> bool {
        let Some(peer) = self.pid else { return false };
        let ancestry = tokio::task::spawn_blocking(move || descends_from(peer, agent));
        ancestry.await.unwrap_or(false)
    }

    /// Waits up to [`INTRODUCTION_WAIT`] for a name, so "joined" names the device.
    pub(super) async fn introduced(&mut self) {
        let named = self.said.wait_for(Option::is_some);
        let _ = tokio::time::timeout(INTRODUCTION_WAIT, named).await;
    }
}

impl std::fmt::Display for Guest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(said) = self.said.borrow().as_ref() {
            return f.write_str(&said.name);
        }
        match self.pid {
            Some(pid) => write!(f, "a client (pid {pid})"),
            None => f.write_str("a client"),
        }
    }
}

/// Copies the client's lines to `protocol`, keeping its introductions for `named`.
///
/// Read here rather than in the protocol because the name is the share layer's business: the
/// engine has no use for it, and the client that started a session has nobody to introduce
/// itself to. Once a client opens as watch-only, nothing more of it is read for the protocol.
pub(super) async fn sift(
    client: impl AsyncRead + Unpin,
    mut protocol: impl AsyncWrite + Unpin,
    named: watch::Sender<Option<Introduction>>,
) {
    let mut lines = BufReader::new(client).lines();
    let mut opening = true;
    while let Ok(Some(line)) = lines.next_line().await {
        if !for_protocol(&line, &mut opening, &named) {
            continue;
        }
        if protocol.write_all(format!("{line}\n").as_bytes()).await.is_err() {
            return;
        }
    }
}

/// Whether `line` goes on to the protocol: not when it's an introduction, which `named` keeps,
/// nor when the client opened as watch-only.
fn for_protocol(
    line: &str,
    opening: &mut bool,
    named: &watch::Sender<Option<Introduction>>,
) -> bool {
    if named.borrow().as_ref().is_some_and(|said| said.watch_only) {
        tracing::warn!("dropped a line from a watch-only client");
        return false;
    }
    let first = std::mem::take(opening);
    let Some(mut said) = introduction(line) else { return true };
    said.watch_only &= first;
    named.send_replace(Some(said));
    false
}

/// What an `introduce` frame says, its name made fit for one line of the terminal.
///
/// Anything else, a malformed introduction included, is `None` and goes on to the protocol, which
/// refuses it by name.
fn introduction(line: &str) -> Option<Introduction> {
    if !line.contains("introduce") {
        return None;
    }
    let luxe_core::event::Action::Introduce { name, watch_only, yours } =
        serde_json::from_str(line).ok()?
    else {
        return None;
    };
    let name: String = name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let name: String = name.trim().chars().take(NAME_LIMIT).collect();
    (!name.is_empty()).then_some(Introduction { name, watch_only, yours })
}

#[cfg(test)]
mod tests;
