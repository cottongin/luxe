//! Devices — the phones and tablets paired with this machine, and the invitations that pair them.
//!
//! An invitation is one use and ten minutes. Redeeming it gives the device a credential of its own,
//! which the host keeps only as a digest, so the file holds nothing a reader could sign in with.
//! The file is shared between the running bridge and `luxe remote devices`/`pair`, so every
//! change goes through core's locked read-modify-write and the bridge reads it fresh each time:
//! a device revoked from another terminal is refused on its next request.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use luxe_core::json_file::{FileError, FileMode, JsonFile};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::secrets::{digest, random_id, random_secret};
use crate::RemoteError;

/// How long an invitation can be redeemed for.
pub const INVITATION_LIFETIME_SECS: u64 = 10 * 60;

/// A paired phone or tablet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// Short id to revoke it by
    pub id: String,
    /// What it called itself when it paired
    pub name: String,
    /// The tailnet login that paired it
    pub login: String,
    /// When it paired, in seconds since the Unix epoch
    pub paired_at: u64,
    credential_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    push: Option<crate::push::Subscription>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Invitation {
    token_sha256: String,
    expires_at: u64,
}

/// What `remote.json` holds.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Registry {
    #[serde(default)]
    devices: Vec<Device>,
    #[serde(default)]
    invitations: Vec<Invitation>,
    /// The bridge's push signing key, made when a device first asks for it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    push_key: Option<KeptKey>,
}

/// A private key as the file keeps it, base64url PKCS#8, which never prints.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
struct KeptKey(String);

impl std::fmt::Debug for KeptKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KeptKey(..)")
    }
}

/// Why an invitation was turned down.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PairingError {
    /// Never issued, already used, or expired: the three look the same on purpose
    #[error(
        "this invitation has expired or was already used; run `luxe remote pair` for a new one"
    )]
    InvitationUnusable,
}

impl Registry {
    fn invite(&mut self, token: &str, now: u64) {
        self.forget_expired(now);
        let expires_at = now + INVITATION_LIFETIME_SECS;
        self.invitations.push(Invitation { token_sha256: digest(token), expires_at });
    }

    /// Trades `token` for the `paired` device, spending the invitation.
    fn redeem(&mut self, token: &str, paired: Device, now: u64) -> Result<Device, PairingError> {
        self.forget_expired(now);
        let wanted = digest(token);
        let at = self.invitations.iter().position(|invitation| invitation.token_sha256 == wanted);
        let Some(at) = at else {
            return Err(PairingError::InvitationUnusable);
        };
        self.invitations.remove(at);
        self.devices.push(paired.clone());
        Ok(paired)
    }

    fn holder(&self, credential: &str) -> Option<&Device> {
        let wanted = digest(credential);
        self.devices.iter().find(|device| device.credential_sha256 == wanted)
    }

    fn revoke(&mut self, id: &str) -> Option<Device> {
        let at = self.devices.iter().position(|device| device.id == id)?;
        Some(self.devices.remove(at))
    }

    fn forget_expired(&mut self, now: u64) {
        self.invitations.retain(|invitation| invitation.expires_at > now);
    }
}

/// The paired devices and open invitations, in a file only this user can read.
#[derive(Debug, Clone)]
pub struct DeviceBook {
    file: JsonFile,
}

impl DeviceBook {
    /// The book in `path`, created on first write.
    pub fn at(path: PathBuf) -> Self {
        Self { file: JsonFile::new(path, FileMode::OwnerOnly) }
    }

    /// Where it lives unless told otherwise: `remote.json` beside luxe's other settings.
    pub fn default_path() -> PathBuf {
        luxe_core::user_dirs::luxe_config_dir().join("remote.json")
    }

    /// A new invitation, good for one device for ten minutes. The token is returned once and kept
    /// only as a digest.
    pub fn invite(&self) -> Result<String, RemoteError> {
        let token = random_secret();
        self.change(|registry| registry.invite(&token, now()))?;
        Ok(token)
    }

    /// Every paired device, oldest first.
    pub fn devices(&self) -> Vec<Device> {
        self.load().devices
    }

    /// Unpairs the device with this id, returning it, or `None` when there is no such device.
    pub fn revoke(&self, id: &str) -> Result<Option<Device>, RemoteError> {
        self.change(|registry| registry.revoke(id))
    }

    /// Redeems `token` for a new device, returning it and the credential it signs in with.
    pub(crate) fn pair(
        &self,
        token: &str,
        name: &str,
        login: &str,
    ) -> Result<(Device, String), RemoteError> {
        let credential = random_secret();
        let paired = Device {
            id: random_id(),
            name: name.to_string(),
            login: login.to_string(),
            paired_at: now(),
            credential_sha256: digest(&credential),
            push: None,
        };
        let device = self.change(|registry| registry.redeem(token, paired, now()))??;
        Ok((device, credential))
    }

    /// The device `credential` belongs to.
    pub(crate) fn holder(&self, credential: &str) -> Option<Device> {
        self.load().holder(credential).cloned()
    }

    fn load(&self) -> Registry {
        let Some(map) = self.file.read() else {
            return Registry::default();
        };
        serde_json::from_value(Value::Object(map))
            .inspect_err(|error| tracing::warn!(%error, "the paired-device file doesn't read"))
            .unwrap_or_default()
    }

    fn change<T>(&self, edit: impl FnOnce(&mut Registry) -> T) -> Result<T, RemoteError> {
        let path = self.file.path().to_path_buf();
        let outcome = self.file.modify(|map| {
            let mut registry: Registry = serde_json::from_value(Value::Object(std::mem::take(map)))
                .map_err(|error| FileError::Unparseable {
                    path,
                    reason: format!("not a paired-device list ({error})"),
                })?;
            let outcome = edit(&mut registry);
            *map = into_map(&registry);
            Ok(outcome)
        })?;
        Ok(outcome)
    }
}

fn into_map(registry: &Registry) -> Map<String, Value> {
    match serde_json::to_value(registry) {
        Ok(Value::Object(map)) => map,
        _ => unreachable!("a struct of vectors serializes to an object"),
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs()).unwrap_or(0)
}

mod push;

#[cfg(test)]
mod tests;
