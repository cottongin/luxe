//! The Windows endpoint: a named pipe only this user can open, and no other machine can reach.
//!
//! The defaults are wrong for this. A pipe's default security grants read access to Everyone and
//! the anonymous account, and reading is the whole transcript; and a pipe can be opened across the
//! network while the Server service runs. So every instance is created with a descriptor granting
//! this user alone, remote clients are refused, and the first instance must be new, so nothing can
//! claim the name before Luxe does.

use super::{EndpointError, Joined, LocalStream};
use std::path::Path;
use std::time::Duration;
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};
use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;

mod security;

pub(super) use security::process_parents;

/// How long a client waits for the server's next instance, which only lags the last connection.
const BUSY_RETRIES: u32 = 40;

/// A pipe with an instance always waiting for the next client.
pub(super) struct Listener {
    next: NamedPipeServer,
    address: String,
    descriptor: security::OwnerOnly,
}

impl Listener {
    /// `_dir` has no part: a pipe has a name rather than a place.
    pub(super) fn bind(_dir: &Path) -> Result<Self, EndpointError> {
        let descriptor = security::OwnerOnly::new()
            .map_err(EndpointError::io("describe who may open the pipe"))?;
        let address = format!(r"\\.\pipe\luxe-{}", uuid::Uuid::new_v4().simple());
        let next = create(&address, &descriptor, true)?;
        Ok(Self { next, address, descriptor })
    }

    pub(super) fn address(&self) -> &str {
        &self.address
    }

    pub(super) async fn accept(&mut self) -> Result<Joined, EndpointError> {
        self.next.connect().await.map_err(EndpointError::io("accept a client"))?;
        let waiting = create(&self.address, &self.descriptor, false)?;
        let connected = std::mem::replace(&mut self.next, waiting);
        let peer = security::client_process_id(&connected);
        Ok(Joined { stream: Box::new(connected), peer })
    }
}

/// One instance of the pipe at `address`; `first` refuses a name something else already holds.
fn create(
    address: &str,
    descriptor: &security::OwnerOnly,
    first: bool,
) -> Result<NamedPipeServer, EndpointError> {
    let mut options = ServerOptions::new();
    options.first_pipe_instance(first).reject_remote_clients(true);
    descriptor.create(&options, address).map_err(EndpointError::io(format!("open {address}")))
}

pub(super) async fn connect(address: &str) -> Result<LocalStream, EndpointError> {
    for _ in 0..BUSY_RETRIES {
        match ClientOptions::new().open(address) {
            Ok(client) => return Ok(Box::new(client)),
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return Err(EndpointError::io(format!("connect to {address}"))(error)),
        }
    }
    let busy = std::io::Error::from_raw_os_error(ERROR_PIPE_BUSY as i32);
    Err(EndpointError::io(format!("connect to {address}"))(busy))
}
