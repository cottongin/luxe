//! local_endpoint — the way into a running luxe from the same machine, and the list of them.
//!
//! Each interactive luxe serves its protocol here, so another front-end (the `luxe remote` bridge,
//! `luxe join`, a script) can join the session it is running. It is local IPC, not network I/O: a
//! Unix socket in a folder only this user can open, or on Windows a named pipe only this user can
//! open and no other machine can reach. This module is the one owner of those platform details.

use std::path::Path;
use tokio::io::{AsyncRead, AsyncWrite};

mod ancestry;
mod live;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

pub use ancestry::descends_from;
pub use live::{live_sessions, running_sessions, LiveSession, Publication};

#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
use windows as platform;

/// Both directions of a connection, whatever the platform carries them on.
pub trait LocalIo: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T: AsyncRead + AsyncWrite + Unpin + Send> LocalIo for T {}

/// A connection to or from a running luxe.
pub type LocalStream = Box<dyn LocalIo>;

/// A client that connected, with its process id where the platform reports one.
pub struct Joined {
    /// The connection
    pub stream: LocalStream,
    /// The client's process id, for the check that it is not one of this luxe's own children
    pub peer: Option<u32>,
}

/// Why an endpoint could not be opened, accepted on, or reached.
#[derive(Debug, thiserror::Error)]
pub enum EndpointError {
    /// The operating system refused. Args: what was being done, and its error.
    #[error("{action}: {source}")]
    Io {
        /// What was being done, as a phrase
        action: String,
        /// The operating system's error
        #[source]
        source: std::io::Error,
    },
}

impl EndpointError {
    /// A `map_err` adapter naming what was being done.
    fn io(action: impl Into<String>) -> impl FnOnce(std::io::Error) -> Self {
        let action = action.into();
        move |source| Self::Io { action, source }
    }
}

/// This luxe's endpoint, open for clients until it is dropped.
pub struct EndpointListener(platform::Listener);

impl EndpointListener {
    /// Opens an endpoint for this process. On Unix its socket goes in `dir`, which is created
    /// private to this user; a Windows pipe has a name rather than a place.
    pub fn bind(dir: &Path) -> Result<Self, EndpointError> {
        platform::Listener::bind(dir).map(Self)
    }

    /// What a client passes to [`connect`]: a socket path, or a pipe name.
    pub fn address(&self) -> &str {
        self.0.address()
    }

    /// The next client to connect, running as this user.
    pub async fn accept(&mut self) -> Result<Joined, EndpointError> {
        self.0.accept().await
    }
}

/// Connects to the endpoint at `address`, as [`EndpointListener::address`] gave it.
pub async fn connect(address: &str) -> Result<LocalStream, EndpointError> {
    platform::connect(address).await
}

#[cfg(test)]
mod tests;
