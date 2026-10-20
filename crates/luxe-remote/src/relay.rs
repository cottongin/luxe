//! Relay — one device's WebSocket piped to one session's local endpoint.
//!
//! The protocol is a JSON object per line, and a WebSocket already frames messages, so a line
//! becomes a text frame and a text frame becomes a line. Nothing the device sends is parsed: the
//! session answers a bad frame itself, with `action_rejected`, as it would for any client. The one
//! line held back is a session refusing the bridge's own `introduce`, which the device never sent.

use axum::extract::ws::{close_code, CloseFrame, Message, WebSocket};
use luxe_core::local_endpoint::LocalStream;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, WriteHalf};

/// The close code a device gets when it is unpaired while connected, from the range RFC 6455 leaves
/// to applications, so the app can say so instead of trying again.
pub(crate) const UNPAIRED: u16 = 4001;

/// How long to wait for the device to answer the bridge's close before dropping the connection.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// Why a relay stopped, which decides how the device's socket is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    /// The device closed its socket, or lost it
    DeviceLeft,
    /// The session's endpoint closed
    SessionEnded,
    /// The device was unpaired while connected
    Unpaired,
}

/// Tells the session who is joining, so it can say "Approved from iPhone" rather than naming the
/// bridge's process.
pub(crate) async fn introduce(
    session: &mut LocalStream,
    introduction: &Introduction<'_>,
) -> std::io::Result<()> {
    let frame = luxe_core::event::Action::Introduce {
        name: introduction.name.to_string(),
        watch_only: introduction.watch_only,
        yours: introduction.yours,
    };
    let frame = serde_json::to_string(&frame).map_err(std::io::Error::other)?;
    session.write_all(one_line(&frame).as_bytes()).await
}

/// Who the bridge joins a session as.
pub(crate) struct Introduction<'a> {
    /// The device it relays for, or the watcher's name
    pub(crate) name: &'a str,
    /// Joins unannounced and can't act, as the notification watcher does
    pub(crate) watch_only: bool,
    /// The device was paired by the computer's owner, the person at its terminal
    pub(crate) yours: bool,
}

/// Pipes `socket` and `session` both ways until either side ends, or `until` resolves (the device
/// was revoked), then ends both.
pub(crate) async fn relay(
    mut socket: WebSocket,
    session: LocalStream,
    until: impl std::future::Future<Output = ()>,
) {
    let (from_session, mut to_session) = tokio::io::split(session);
    let ending = pump(&mut socket, from_session, &mut to_session, until).await;
    let _ = to_session.shutdown().await;
    close(socket, ending).await;
}

/// Carries lines and frames across until one side stops, and says which.
async fn pump(
    socket: &mut WebSocket,
    from_session: tokio::io::ReadHalf<LocalStream>,
    to_session: &mut WriteHalf<LocalStream>,
    until: impl std::future::Future<Output = ()>,
) -> Ending {
    let mut lines = BufReader::new(from_session).lines();
    tokio::pin!(until);
    // Both reads are cancel-safe, so a line or frame the other branch beat is never lost
    loop {
        let stopped = tokio::select! {
            line = lines.next_line() => line_to_device(socket, line).await,
            message = socket.recv() => frame_to_session(to_session, message).await,
            () = &mut until => Some(Ending::Unpaired),
        };
        if let Some(ending) = stopped {
            return ending;
        }
    }
}

async fn line_to_device(
    socket: &mut WebSocket,
    line: std::io::Result<Option<String>>,
) -> Option<Ending> {
    match line {
        Ok(Some(line)) if refuses_introduction(&line) => None,
        Ok(Some(line)) => {
            socket.send(Message::Text(line.into())).await.err().map(|_| Ending::DeviceLeft)
        }
        Ok(None) => Some(Ending::SessionEnded),
        Err(error) => {
            tracing::warn!(%error, "stopped reading the session");
            Some(Ending::SessionEnded)
        }
    }
}

/// Whether `line` is the session refusing the bridge's `introduce`. Only a session started before
/// its luxe knew the action does, and it carries on without the device's name; the refusal is of a
/// frame the device never sent, so showing it there reads as a fault. Decoded only when it could
/// be one, so every other line still passes untouched.
fn refuses_introduction(line: &str) -> bool {
    if !line.contains("action_rejected") {
        return false;
    }
    matches!(
        serde_json::from_str::<luxe_core::event::Event>(line),
        Ok(luxe_core::event::Event::ActionRejected { kind: Some(kind), .. }) if kind == "introduce"
    )
}

async fn frame_to_session(
    to_session: &mut WriteHalf<LocalStream>,
    message: Option<Result<Message, axum::Error>>,
) -> Option<Ending> {
    let text = match message {
        Some(Ok(Message::Text(text))) => text,
        // Pings are answered by axum; binary has no meaning in a line protocol
        Some(Ok(Message::Binary(_) | Message::Ping(_) | Message::Pong(_))) => return None,
        Some(Ok(Message::Close(_)) | Err(_)) | None => return Some(Ending::DeviceLeft),
    };
    let written = to_session.write_all(one_line(&text).as_bytes()).await;
    written.err().map(|_| Ending::SessionEnded)
}

/// Closes the device's socket the way `ending` calls for, so the app can tell leaving, a session
/// that ended and being unpaired apart from a dropped connection, which is the one worth retrying.
async fn close(mut socket: WebSocket, ending: Ending) {
    let frame = |code, reason: &str| Some(CloseFrame { code, reason: reason.into() });
    let frame = match ending {
        // tungstenite refuses to send once the device has closed; it queued its own answer, and
        // the read below sends it
        Ending::DeviceLeft => None,
        Ending::SessionEnded => frame(close_code::NORMAL, "the session ended"),
        Ending::Unpaired => frame(UNPAIRED, "this device was unpaired"),
    };
    if frame.is_some() && socket.send(Message::Close(frame)).await.is_err() {
        return;
    }
    let answered = async { while let Some(Ok(_)) = socket.recv().await {} };
    let _ = tokio::time::timeout(CLOSE_WAIT, answered).await;
}

/// `frame` as exactly one protocol line.
///
/// JSON can hold a raw line break only as whitespace between tokens (inside a string it is
/// escaped), so turning breaks into spaces keeps a pretty-printed frame one action instead of
/// several broken ones.
fn one_line(frame: &str) -> String {
    let mut line: String =
        frame.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
    line.push('\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pretty-printed frame reaches the session as the one line it is.
    ///
    /// The break this catches: writing the frame as it came, so a client that sends indented JSON
    /// gets several `action_rejected` answers and nothing done.
    /// An older session's refusal of the bridge's `introduce` is held back, and every other
    /// refusal goes through.
    ///
    /// The break this catches: a device joining a session that predates `introduce` shown
    /// "Refused: no such action `introduce`" for a frame it never sent, or the device's own
    /// refused frames (and the message words alone) swallowed too.
    #[test]
    fn only_the_refusal_of_the_introduction_is_held_back() {
        let refused = |kind: &str| {
            format!("{{\"kind\":\"action_rejected\",\"content\":{{\"kind\":\"{kind}\",\"reason\":\"no such action `{kind}`\"}}}}")
        };
        assert!(refuses_introduction(&refused("introduce")));
        assert!(!refuses_introduction(&refused("submit_user_message")));
        let unparsed =
            r#"{"kind":"action_rejected","content":{"reason":"no such action `introduce`"}}"#;
        assert!(
            !refuses_introduction(unparsed),
            "the words alone don't make it the introduction's"
        );
        assert!(!refuses_introduction(r#"{"kind":"turn_started"}"#));
    }

    #[test]
    fn a_frame_is_one_line_however_it_was_printed() {
        let pretty = "{\n  \"kind\": \"cancel\"\r\n}";
        assert_eq!(one_line(pretty), "{   \"kind\": \"cancel\"  }\n");
        let text = r#"{"kind":"submit_user_message","content":{"text":"a\nb"}}"#;
        assert_eq!(one_line(text), format!("{text}\n"), "an escaped break is not a break");
    }
}
