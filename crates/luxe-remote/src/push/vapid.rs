//! VAPID — how the bridge proves to a push service that it's the server a device subscribed to.
//!
//! A device subscribes with the bridge's public key, and each push carries a short token signed
//! with the private one (RFC 8292). The token names the push service it's for and when it
//! expires, and as its contact the project's homepage: nothing about this machine or its owner.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};

/// The contact a token names. A push service wants a URL or `mailto:` there.
const CONTACT: &str = env!("CARGO_PKG_HOMEPAGE");

/// Why the bridge's push key couldn't be made, read or used.
#[derive(Debug, thiserror::Error)]
pub(crate) enum VapidError {
    /// The stored key isn't a P-256 key in PKCS#8
    #[error("the stored push key isn't a P-256 private key")]
    Unreadable,
    /// The system's randomness failed
    #[error("the system's random number source failed")]
    Random,
}

/// The bridge's push signing key.
pub(crate) struct Vapid {
    pair: EcdsaKeyPair,
    random: SystemRandom,
}

impl Vapid {
    /// A new key, and the PKCS#8 document to keep it in.
    pub(crate) fn generate() -> Result<(Self, Vec<u8>), VapidError> {
        let random = SystemRandom::new();
        let document = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &random)
            .map_err(|_| VapidError::Random)?;
        let kept = document.as_ref().to_vec();
        Ok((Self::from_pkcs8(&kept)?, kept))
    }

    /// The key kept in `document`.
    pub(crate) fn from_pkcs8(document: &[u8]) -> Result<Self, VapidError> {
        let random = SystemRandom::new();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, document, &random)
            .map_err(|_| VapidError::Unreadable)?;
        Ok(Self { pair, random })
    }

    /// The public key, base64url, as a device passes it to `PushManager.subscribe`.
    pub(crate) fn public_key(&self) -> String {
        URL_SAFE_NO_PAD.encode(self.pair.public_key().as_ref())
    }

    /// The `Authorization` header for the push service at origin `audience`, good until
    /// `expires_at` (seconds since the Unix epoch).
    pub(crate) fn authorization(
        &self,
        audience: &str,
        expires_at: u64,
    ) -> Result<String, VapidError> {
        let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = serde_json::json!({ "aud": audience, "exp": expires_at, "sub": CONTACT });
        let claims = URL_SAFE_NO_PAD.encode(claims.to_string());
        let signed = format!("{header}.{claims}");
        let signature =
            self.pair.sign(&self.random, signed.as_bytes()).map_err(|_| VapidError::Random)?;
        let signature = URL_SAFE_NO_PAD.encode(signature.as_ref());
        Ok(format!("vapid t={signed}.{signature}, k={}", self.public_key()))
    }
}

#[cfg(test)]
mod tests;
