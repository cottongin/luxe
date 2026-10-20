//! Web push — one notification posted to the push service a device subscribed through (RFC 8030).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use super::encryption::{encrypt, EncryptionError};
use super::notice::Notice;
use super::{Subscription, Vapid, VapidError};

/// How long a push service keeps a notification for a device that's offline, in seconds. Past an
/// hour, what it says has likely changed.
const TIME_TO_LIVE: u32 = 3600;

/// How long a signed token is good for. Apple takes up to a day.
const TOKEN_LIFETIME_SECS: u64 = 12 * 3600;

/// A token is signed again once it has less than this left. Apple asks for no new token more
/// often than hourly, which this keeps to.
const TOKEN_RENEWAL_SECS: u64 = 3600;

/// The longest a post to a push service may take, so a stalled one can't hold the rest back.
const POST_TIMEOUT: Duration = Duration::from_secs(15);

/// What a push service did with a notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delivery {
    /// Taken for delivery
    Taken,
    /// The subscription has expired or been withdrawn, and won't take any more
    Gone,
}

/// Why a notification wasn't posted.
#[derive(Debug, thiserror::Error)]
pub(crate) enum PushError {
    /// It couldn't be sealed for the device
    #[error(transparent)]
    Encryption(#[from] EncryptionError),
    /// The token couldn't be signed
    #[error(transparent)]
    Key(#[from] VapidError),
    /// The push service couldn't be reached
    #[error("couldn't reach the push service: {0}")]
    Network(#[from] reqwest::Error),
    /// The push service turned it down
    #[error("the push service refused it with {status}: {reason}")]
    Refused {
        /// Its status code
        status: u16,
        /// What it said, cut short
        reason: String,
    },
    /// The subscription's endpoint or keys don't read
    #[error("the subscription doesn't read")]
    BadSubscription,
}

/// What posts a notification. The bridge's is [`WebPush`]; tests post nowhere.
pub(crate) trait Courier: Send + Sync {
    /// Posts `notice` to `subscription`.
    fn deliver<'a>(
        &'a self,
        subscription: &'a Subscription,
        notice: &'a Notice,
    ) -> Pin<Box<dyn Future<Output = Result<Delivery, PushError>> + Send + 'a>>;
}

/// A signed token, and when it expires.
struct Token {
    header: String,
    expires_at: u64,
}

/// The bridge's courier: Web Push over HTTPS, signed with the bridge's key.
pub(crate) struct WebPush {
    client: reqwest::Client,
    vapid: Vapid,
    tokens: Mutex<HashMap<String, Token>>,
}

impl WebPush {
    /// A courier signing with `vapid`.
    pub(crate) fn new(vapid: Vapid) -> Result<Self, PushError> {
        let client = reqwest::Client::builder().timeout(POST_TIMEOUT).build()?;
        Ok(Self { client, vapid, tokens: Mutex::new(HashMap::new()) })
    }

    /// The post for `notice` to `subscription`, as of `now` (seconds since the Unix epoch).
    pub(crate) fn request(
        &self,
        subscription: &Subscription,
        notice: &Notice,
        now: u64,
    ) -> Result<reqwest::Request, PushError> {
        let audience = subscription.audience().ok_or(PushError::BadSubscription)?;
        let body = sealed(subscription, notice)?;
        let urgency = if notice.urgent { "high" } else { "normal" };
        let mut post = self.client.post(&subscription.endpoint).body(body);
        post = post.header("TTL", TIME_TO_LIVE).header("Urgency", urgency);
        post = post.header("Content-Encoding", "aes128gcm");
        post = post.header("Content-Type", "application/octet-stream");
        post = post.header("Authorization", self.authorization(&audience, now)?);
        post = post.header("Topic", topic(&notice.session));
        Ok(post.build()?)
    }

    /// The `Authorization` header for `audience`, reusing a token that has an hour left.
    fn authorization(&self, audience: &str, now: u64) -> Result<String, PushError> {
        let mut tokens = self.tokens.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(token) = tokens.get(audience) {
            if token.expires_at > now + TOKEN_RENEWAL_SECS {
                return Ok(token.header.clone());
            }
        }
        let expires_at = now + TOKEN_LIFETIME_SECS;
        let header = self.vapid.authorization(audience, expires_at)?;
        tokens.insert(audience.to_string(), Token { header: header.clone(), expires_at });
        Ok(header)
    }

    async fn post(
        &self,
        subscription: &Subscription,
        notice: &Notice,
    ) -> Result<Delivery, PushError> {
        let request = self.request(subscription, notice, unix_now())?;
        let response = self.client.execute(request).await?;
        let status = response.status();
        if status.is_success() {
            return Ok(Delivery::Taken);
        }
        if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
            return Ok(Delivery::Gone);
        }
        let reason = response.text().await.unwrap_or_default().chars().take(200).collect();
        Err(PushError::Refused { status: status.as_u16(), reason })
    }
}

impl Courier for WebPush {
    fn deliver<'a>(
        &'a self,
        subscription: &'a Subscription,
        notice: &'a Notice,
    ) -> Pin<Box<dyn Future<Output = Result<Delivery, PushError>> + Send + 'a>> {
        Box::pin(self.post(subscription, notice))
    }
}

/// `notice` as the service worker reads it, sealed for the subscription's device.
fn sealed(subscription: &Subscription, notice: &Notice) -> Result<Vec<u8>, PushError> {
    let (device_key, auth) = subscription.key_bytes().ok_or(PushError::BadSubscription)?;
    let message = serde_json::to_vec(notice).expect("a notice is plain strings");
    Ok(encrypt(&message, &device_key, &auth)?)
}

/// The `Topic` for a session's notifications, so a push service holding several for an offline
/// device passes on only the newest. Apple takes at most 32 base64url characters and reads them as
/// base64, refusing some lengths, so it's always 32: a hash of the id, which also keeps the id
/// itself from the push service.
fn topic(session: &str) -> String {
    use base64::Engine as _;
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(session.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&digest[..24])
}

fn unix_now() -> u64 {
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    since.map(|elapsed| elapsed.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests;
