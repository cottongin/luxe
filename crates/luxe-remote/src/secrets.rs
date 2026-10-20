//! Secrets — the random strings pairing hands out, and the digests the host keeps instead.

use base64::Engine as _;
use sha2::{Digest, Sha256};

/// A fresh secret: 32 random bytes, URL-safe, so it fits in a link and a cookie as it is.
pub(crate) fn random_secret() -> String {
    let mut bytes = [0u8; 32];
    // a guessable credential would be a hole, not an inconvenience
    getrandom::fill(&mut bytes).expect("OS randomness unavailable");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A short random id to name a device by in a list.
pub(crate) fn random_id() -> String {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).expect("OS randomness unavailable");
    base16ct::lower::encode_string(&bytes)
}

/// What the host stores in place of `secret`. A plain hash is enough: the secret is 256 random
/// bits, so there is nothing to guess that a slow hash would protect.
pub(crate) fn digest(secret: &str) -> String {
    base16ct::lower::encode_string(&Sha256::digest(secret.as_bytes()))
}
