//! Encryption — a push message sealed for one device, as RFC 8291 has it.
//!
//! The push service carries the message and can't read it: each one is sealed with a key agreed
//! between a throwaway key of the bridge's and the key the device's browser made when it
//! subscribed, mixed with the subscription's own secret.

use ring::rand::SecureRandom;
use ring::{aead, agreement, hkdf, rand};

/// The record size a message declares. One record holds the whole message, which a push service
/// caps at 4 KiB anyway.
const RECORD_SIZE: u32 = 4096;

/// Bytes a record adds to its message: the delimiter that ends the last record, and the AES-GCM
/// tag.
const RECORD_OVERHEAD: usize = 1 + 16;

/// Why a message couldn't be sealed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum EncryptionError {
    /// The subscription's key isn't a P-256 point
    #[error("the subscription's key isn't a P-256 public key")]
    BadSubscriptionKey,
    /// The message won't fit one record
    #[error("a push message of {0} bytes is too long for one record")]
    TooLong(usize),
    /// The system's randomness failed
    #[error("the system's random number source failed")]
    Random,
}

/// The two secrets a message is sealed with.
#[derive(Debug, PartialEq, Eq)]
struct ContentKeys {
    key: [u8; 16],
    nonce: [u8; 12],
}

/// `message` sealed for the subscription with public key `device_key` and secret `auth`, as the
/// body of an `aes128gcm` push request.
pub(crate) fn encrypt(
    message: &[u8],
    device_key: &[u8],
    auth: &[u8],
) -> Result<Vec<u8>, EncryptionError> {
    if message.len() + RECORD_OVERHEAD > RECORD_SIZE as usize {
        return Err(EncryptionError::TooLong(message.len()));
    }
    let random = rand::SystemRandom::new();
    let (ours, our_key) = throwaway_key(&random)?;
    let mut salt = [0u8; 16];
    random.fill(&mut salt).map_err(|_| EncryptionError::Random)?;
    let theirs = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, device_key);
    let keys = agreement::agree_ephemeral(ours, &theirs, |shared| {
        content_keys(shared, auth, device_key, our_key.as_ref(), &salt)
    })
    .map_err(|_| EncryptionError::BadSubscriptionKey)?;
    Ok(seal(&keys, &salt, our_key.as_ref(), message))
}

/// A key pair for one message, so a message read once its key leaks says nothing of the others.
fn throwaway_key(
    random: &rand::SystemRandom,
) -> Result<(agreement::EphemeralPrivateKey, agreement::PublicKey), EncryptionError> {
    let ours = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, random)
        .map_err(|_| EncryptionError::Random)?;
    let public = ours.compute_public_key().map_err(|_| EncryptionError::Random)?;
    Ok((ours, public))
}

/// The content keys from the agreed secret, RFC 8291 section 3.4: the subscription's secret and
/// both public keys are mixed in first, then the message's salt.
fn content_keys(
    shared: &[u8],
    auth: &[u8],
    device_key: &[u8],
    our_key: &[u8],
    salt: &[u8],
) -> ContentKeys {
    let key_info = [b"WebPush: info\0".as_slice(), device_key, our_key].concat();
    let combined = hkdf::Salt::new(hkdf::HKDF_SHA256, auth).extract(shared);
    let keying: [u8; 32] = expand(&combined, &key_info);
    let content = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(&keying);
    ContentKeys {
        key: expand(&content, b"Content-Encoding: aes128gcm\0"),
        nonce: expand(&content, b"Content-Encoding: nonce\0"),
    }
}

/// The header (salt, record size, the bridge's key) and then the one sealed record.
fn seal(keys: &ContentKeys, salt: &[u8; 16], our_key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut record = [message, &[2]].concat();
    let unbound =
        aead::UnboundKey::new(&aead::AES_128_GCM, &keys.key).expect("16 bytes is an AES-128 key");
    let nonce = aead::Nonce::assume_unique_for_key(keys.nonce);
    aead::LessSafeKey::new(unbound)
        .seal_in_place_append_tag(nonce, aead::Aad::empty(), &mut record)
        .expect("one record is far below AES-GCM's length limit");
    let key_length = u8::try_from(our_key.len()).expect("a P-256 point is 65 bytes");
    let header = [salt.as_slice(), &RECORD_SIZE.to_be_bytes(), &[key_length], our_key].concat();
    [header, record].concat()
}

/// `N` bytes of HKDF output.
fn expand<const N: usize>(key: &hkdf::Prk, info: &[u8]) -> [u8; N] {
    struct Length(usize);
    impl hkdf::KeyType for Length {
        fn len(&self) -> usize {
            self.0
        }
    }
    let mut out = [0u8; N];
    key.expand(&[info], Length(N))
        .and_then(|okm| okm.fill(&mut out))
        .expect("HKDF-SHA-256 gives up to 8160 bytes");
    out
}

#[cfg(test)]
pub(crate) mod tests;
