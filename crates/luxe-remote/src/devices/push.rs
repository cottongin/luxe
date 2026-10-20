//! The push half of the device book: the bridge's signing key, and each device's subscription.
//!
//! Kept in the same file as the devices, so revoking a device takes its subscription with it, and
//! under the same lock, so two bridges starting at once agree on one key.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

use super::{Device, DeviceBook, KeptKey, Registry};
use crate::push::{Subscription, Vapid, VapidError};
use crate::RemoteError;

/// Why the push key couldn't be had.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PushKeyError {
    /// The device file couldn't be read or written
    #[error(transparent)]
    File(#[from] RemoteError),
    /// The key couldn't be made, or the kept one doesn't read
    #[error(transparent)]
    Key(#[from] VapidError),
}

impl Registry {
    /// The kept key, or a new one kept from now on.
    fn push_key_document(&mut self) -> Result<Vec<u8>, VapidError> {
        if let Some(KeptKey(kept)) = &self.push_key {
            return URL_SAFE_NO_PAD.decode(kept).map_err(|_| VapidError::Unreadable);
        }
        let (_, document) = Vapid::generate()?;
        self.push_key = Some(KeptKey(URL_SAFE_NO_PAD.encode(&document)));
        Ok(document)
    }

    /// Sets the subscription of device `id`, saying whether there is such a device.
    fn subscribe(&mut self, id: &str, subscription: Option<Subscription>) -> bool {
        let Some(device) = self.devices.iter_mut().find(|device| device.id == id) else {
            return false;
        };
        device.push = subscription;
        true
    }

    /// Drops device `id`'s subscription if it's still at `endpoint`, the one a push service
    /// reported gone; a device that subscribed again since keeps its new one.
    fn forget_endpoint(&mut self, id: &str, endpoint: &str) {
        let device = self.devices.iter_mut().find(|device| device.id == id);
        if let Some(device) = device.filter(|device| device.push_endpoint() == Some(endpoint)) {
            device.push = None;
        }
    }
}

impl Device {
    /// Where this device takes notifications, when it asked for them.
    pub(crate) fn subscription(&self) -> Option<&Subscription> {
        self.push.as_ref()
    }

    fn push_endpoint(&self) -> Option<&str> {
        self.push.as_ref().map(|subscription| subscription.endpoint.as_str())
    }
}

impl DeviceBook {
    /// The bridge's push signing key, made and kept the first time it's asked for.
    pub(crate) fn push_key(&self) -> Result<Vapid, PushKeyError> {
        let document = self.change(Registry::push_key_document)??;
        Ok(Vapid::from_pkcs8(&document)?)
    }

    /// Sends device `id`'s notifications to `subscription` from now on, or stops them with
    /// `None`. False when there's no such device.
    pub(crate) fn subscribe(
        &self,
        id: &str,
        subscription: Option<Subscription>,
    ) -> Result<bool, RemoteError> {
        self.change(|registry| registry.subscribe(id, subscription))
    }

    /// Forgets device `id`'s subscription at `endpoint`, which its push service says is gone.
    pub(crate) fn forget_endpoint(&self, id: &str, endpoint: &str) -> Result<(), RemoteError> {
        self.change(|registry| registry.forget_endpoint(id, endpoint))
    }
}

#[cfg(test)]
mod tests;
