//! luxe remote — the bridge between your own devices and the sessions running on this machine.
//!
//! Each running luxe shares its session on a local endpoint only this user can open
//! (`luxe_core::local_endpoint`). The bridge is the one thing that puts those sessions on a
//! network, and only on the loopback interface: Tailscale Serve carries it to your tailnet and does
//! the HTTPS. A phone lists the sessions over HTTP and joins one over a WebSocket, which the bridge
//! pipes to that session's endpoint a protocol line per text frame. What crosses is the published
//! `--rpc` protocol, untouched, so the bridge holds no session state of its own.
//!
//! Who gets in is the `access` module's call: a tailnet login on the allowlist, and a device paired
//! through [`DeviceBook`].

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

mod access;
mod app;
mod devices;
mod folders;
mod home_screen;
mod launch;
mod pairing;
mod push;
mod relay;
mod render;
mod routes;
mod secrets;
pub mod service;
mod sessions;
mod tailnet;

pub use devices::{Device, DeviceBook, PairingError, INVITATION_LIFETIME_SECS};
pub use render::{
    ContextUsage, FileMark, GithubMarks, MarkHue, Renderer, StatusFacts, StatusSegment, StatusTone,
    TileColors, Voice,
};
pub use tailnet::{publish, published, this_machine, Published, Tailnet, TailnetError};

/// How often the bridge rereads the paired devices to close a revoked one's sockets.
const REVOCATION_CHECK: Duration = Duration::from_secs(2);

/// Who may use the bridge.
#[derive(Debug, Clone)]
pub struct Access {
    /// Tailnet logins allowed in, as Tailscale Serve reports them (`alex@github`)
    pub logins: Vec<String>,
    /// The paired devices
    pub devices: DeviceBook,
}

/// Why the bridge could not start or keep serving.
#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    /// The address is reachable from beyond this machine
    #[error(
        "refusing to listen on {0}: the bridge listens on this machine only, and Tailscale Serve \
         takes it to your devices"
    )]
    NotLoopback(SocketAddr),
    /// The port could not be bound or served
    #[error("could not {action} on {address}: {source}")]
    Io {
        /// What was being done
        action: &'static str,
        /// Where
        address: SocketAddr,
        /// The operating system's reason
        #[source]
        source: std::io::Error,
    },
    /// The paired-device file could not be read or written
    #[error("paired devices: {0}")]
    Devices(#[from] luxe_core::json_file::FileError),
    /// An invitation was turned down
    #[error(transparent)]
    Pairing(#[from] PairingError),
}

/// A bridge bound to its port, ready to serve.
pub struct Bridge {
    listener: tokio::net::TcpListener,
    address: SocketAddr,
    runtime_dir: PathBuf,
    access: Access,
    launcher: Option<launch::Launcher>,
    renderer: std::sync::Arc<dyn Renderer>,
    presence: push::Presence,
    courier: Option<std::sync::Arc<dyn push::Courier>>,
    machine: Option<String>,
}

impl Bridge {
    /// Binds `address`, which must be on the loopback interface, to serve the sessions recorded in
    /// `runtime_dir` to those `access` lets in.
    ///
    /// Loopback is enforced here rather than left to a default: anything wider would put every
    /// session in reach of the local network, with only the device check in front of it.
    pub async fn bind(
        address: SocketAddr,
        runtime_dir: PathBuf,
        access: Access,
    ) -> Result<Self, RemoteError> {
        let (listener, address) = listen_on(address).await?;
        Ok(Self::listening(listener, address, runtime_dir, access))
    }

    /// A bridge on `listener` with nothing added yet: plain text, no launcher, no name.
    fn listening(
        listener: tokio::net::TcpListener,
        address: SocketAddr,
        runtime_dir: PathBuf,
        access: Access,
    ) -> Self {
        Self {
            listener,
            address,
            runtime_dir,
            access,
            launcher: None,
            renderer: std::sync::Arc::new(render::PlainText),
            presence: push::Presence::default(),
            courier: None,
            machine: None,
        }
    }

    /// Names the computer it serves, as its tailnet does, on the phone's app and its
    /// notifications: one phone can have an app for each of several computers.
    pub fn named(mut self, machine: impl Into<String>) -> Self {
        self.machine = Some(machine.into());
        self
    }

    /// Renders messages for the app with `renderer`. Without one the app shows plain text.
    pub fn rendering_with(mut self, renderer: std::sync::Arc<dyn Renderer>) -> Self {
        self.renderer = renderer;
        self
    }

    /// Lets devices start sessions: in a herdr pane when herdr is running, otherwise by running
    /// `program`, the bridge's own `luxe`, as `luxe --rpc`.
    pub fn starting_sessions(self, program: PathBuf) -> Self {
        let folders = folders::FolderBook::at(folders::FolderBook::default_path());
        self.with_launcher(launch::Launcher::new(program, launch::Herdr::WhenRunning, folders))
    }

    pub(crate) fn with_launcher(mut self, launcher: launch::Launcher) -> Self {
        self.launcher = Some(launcher);
        self
    }

    /// Posts notifications through `courier` instead of Web Push.
    #[cfg(test)]
    pub(crate) fn notifying_through(mut self, courier: std::sync::Arc<dyn push::Courier>) -> Self {
        self.courier = Some(courier);
        self
    }

    /// Which devices have which sessions open.
    #[cfg(test)]
    pub(crate) fn presence(&self) -> push::Presence {
        self.presence.clone()
    }

    /// Where it is listening, with the port the system picked when asked for port 0.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    /// Serves until `shutdown` resolves.
    pub async fn serve(
        self,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Result<(), RemoteError> {
        let address = self.address;
        let devices = self.access.devices.clone();
        let (runtime_dir, presence) = (self.runtime_dir.clone(), self.presence.clone());
        let (courier, machine) = (self.courier.clone(), self.machine.clone());
        let _notifying = push::start(runtime_dir, devices, presence, courier, machine);
        let (listener, app) = self.into_router();
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(|source| RemoteError::Io { action: "serve", address, source })
    }

    fn into_router(self) -> (tokio::net::TcpListener, axum::Router) {
        let app = routes::router(routes::Bridged {
            runtime_dir: self.runtime_dir,
            logins: self.access.logins,
            devices: self.access.devices,
            launcher: self.launcher,
            renderer: self.renderer,
            presence: self.presence,
            machine: self.machine,
        });
        (self.listener, app)
    }
}

/// A listener on `address`, which must be on the loopback interface, and where it landed.
async fn listen_on(
    address: SocketAddr,
) -> Result<(tokio::net::TcpListener, SocketAddr), RemoteError> {
    if !address.ip().is_loopback() {
        return Err(RemoteError::NotLoopback(address));
    }
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|source| RemoteError::Io { action: "listen", address, source })?;
    let landed = listener.local_addr().map_err(|source| RemoteError::Io {
        action: "read the address",
        address,
        source,
    })?;
    Ok((listener, landed))
}

#[cfg(test)]
mod tests;
