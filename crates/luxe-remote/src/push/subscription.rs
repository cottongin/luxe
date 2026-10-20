//! Subscription — where a device asked to be sent notifications, and the keys to seal them with.
//!
//! The device names the address the bridge will post to, so the address is held to the push
//! services browsers use: HTTPS, on a known host. Anything else would let a paired device point
//! the bridge at a machine on your network.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};

/// The push services a subscription may name, each with its subdomains: Apple's (Safari, every
/// browser on iOS), Google's (Chrome), Mozilla's (Firefox) and Microsoft's (Edge).
const PUSH_SERVICES: &[&str] =
    &["push.apple.com", "fcm.googleapis.com", "push.services.mozilla.com", "notify.windows.com"];

/// A device's push subscription, as `PushSubscription.toJSON()` gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Subscription {
    /// Where the push service takes this device's notifications
    pub(crate) endpoint: String,
    /// The keys the browser made for it
    pub(crate) keys: SubscriptionKeys,
}

/// The browser's half of the encryption, base64url.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SubscriptionKeys {
    /// Its P-256 public key
    pub(crate) p256dh: String,
    /// The secret shared with the bridge alone
    pub(crate) auth: String,
}

/// Why a subscription was turned down.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum SubscriptionError {
    /// The endpoint isn't an HTTPS URL on the default port
    #[error("a push endpoint must be an https:// address")]
    NotHttps,
    /// The endpoint's host isn't a push service a browser uses
    #[error("{0} isn't a push service this bridge sends to")]
    UnknownService(String),
    /// The keys aren't a P-256 point and a 16-byte secret
    #[error("the subscription's keys aren't the ones Web Push uses")]
    BadKeys,
}

impl Subscription {
    /// This subscription, if it names a push service and carries usable keys.
    pub(crate) fn checked(self) -> Result<Self, SubscriptionError> {
        let host = push_host(&self.endpoint)?;
        if !PUSH_SERVICES.iter().any(|service| is_within(&host, service)) {
            return Err(SubscriptionError::UnknownService(host));
        }
        let (public, auth) = self.key_bytes().ok_or(SubscriptionError::BadKeys)?;
        if public.len() != 65 || public[0] != 4 || auth.len() != 16 {
            return Err(SubscriptionError::BadKeys);
        }
        Ok(self)
    }

    /// The browser's public key and the shared secret, decoded.
    pub(crate) fn key_bytes(&self) -> Option<(Vec<u8>, Vec<u8>)> {
        let public = URL_SAFE_NO_PAD.decode(&self.keys.p256dh).ok()?;
        let auth = URL_SAFE_NO_PAD.decode(&self.keys.auth).ok()?;
        Some((public, auth))
    }

    /// The push service's origin, which a VAPID token must name.
    pub(crate) fn audience(&self) -> Option<String> {
        let url = reqwest::Url::parse(&self.endpoint).ok()?;
        Some(url.origin().ascii_serialization())
    }
}

/// The host of `endpoint`, when it's HTTPS on the default port.
fn push_host(endpoint: &str) -> Result<String, SubscriptionError> {
    let url = reqwest::Url::parse(endpoint).map_err(|_| SubscriptionError::NotHttps)?;
    if url.scheme() != "https" || url.port().is_some() {
        return Err(SubscriptionError::NotHttps);
    }
    let host = url.host_str().ok_or(SubscriptionError::NotHttps)?;
    Ok(host.to_ascii_lowercase())
}

/// Whether `host` is `service` or one of its subdomains.
fn is_within(host: &str, service: &str) -> bool {
    host == service || host.strip_suffix(service).is_some_and(|rest| rest.ends_with('.'))
}

#[cfg(test)]
pub(crate) mod tests;
