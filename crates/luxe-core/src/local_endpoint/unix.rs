//! The Unix endpoint: a socket in a folder only this user can open.

use super::{EndpointError, Joined, LocalStream};
use std::path::{Path, PathBuf};
use tokio::net::{UnixListener, UnixStream};

/// A socket named for this process, removed when the listener is dropped.
pub(super) struct Listener {
    listener: UnixListener,
    path: PathBuf,
    address: String,
}

impl Listener {
    pub(super) fn bind(dir: &Path) -> Result<Self, EndpointError> {
        crate::fs_atomic::create_private_dir(dir)
            .map_err(EndpointError::io(format!("create {}", dir.display())))?;
        let path = dir.join(format!("{}.sock", std::process::id()));
        // left by an earlier process with this id; only this user can write in this folder
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)
            .map_err(EndpointError::io(format!("listen on {}", path.display())))?;
        restrict(&path)?;
        let address = path.to_string_lossy().into_owned();
        Ok(Self { listener, path, address })
    }

    pub(super) fn address(&self) -> &str {
        &self.address
    }

    pub(super) async fn accept(&mut self) -> Result<Joined, EndpointError> {
        loop {
            let (stream, _) =
                self.listener.accept().await.map_err(EndpointError::io("accept a client"))?;
            if let Some(joined) = admit(stream) {
                return Ok(joined);
            }
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// `stream` as a client, unless it runs as another user or will not say who it is.
///
/// The folder already keeps other users out; this holds if it ever does not.
fn admit(stream: UnixStream) -> Option<Joined> {
    let credentials = match stream.peer_cred() {
        Ok(credentials) => credentials,
        Err(error) => {
            tracing::warn!(%error, "refused a client whose credentials could not be read");
            return None;
        }
    };
    if credentials.uid() != rustix::process::geteuid().as_raw() {
        tracing::warn!(uid = credentials.uid(), "refused a client running as another user");
        return None;
    }
    let peer = credentials.pid().and_then(|pid| u32::try_from(pid).ok());
    Some(Joined { stream: Box::new(stream), peer })
}

/// Owner read and write only. The folder is private already; this is the second lock.
fn restrict(path: &Path) -> Result<(), EndpointError> {
    use std::os::unix::fs::PermissionsExt;
    let owner_only = std::fs::Permissions::from_mode(crate::fs_atomic::OWNER_ONLY);
    std::fs::set_permissions(path, owner_only)
        .map_err(EndpointError::io(format!("restrict {}", path.display())))
}

pub(super) async fn connect(address: &str) -> Result<LocalStream, EndpointError> {
    let stream = UnixStream::connect(address)
        .await
        .map_err(EndpointError::io(format!("connect to {address}")))?;
    Ok(Box::new(stream))
}
