use super::*;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

/// RFC 8291 Appendix A, whose values every test here is held to.
mod rfc {
    pub(super) const MESSAGE: &str = "When I grow up, I want to be a watermelon";
    pub(super) const OUR_KEY: &str =
        "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
    pub(super) const DEVICE_KEY: &str =
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    pub(super) const SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";
    pub(super) const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
    pub(super) const SHARED: &str = "kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs";
    pub(super) const CONTENT_KEY: &str = "oIhVW04MRdy2XN9CiKLxTg";
    pub(super) const NONCE: &str = "4h_95klXJ5E_qnoN";
    pub(super) const HEADER: &str = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
    pub(super) const SEALED: &str =
        "8pfeW0KbunFT06SuDKoJH9Ql87S1QUrdirN6GcG7sFz1y1sqLgVi1VhjVkHsUoEsbI_0LpXMuGvnzQ";
}

fn bytes(base64url: &str) -> Vec<u8> {
    URL_SAFE_NO_PAD.decode(base64url).expect("the RFC's values are base64url")
}

fn rfc_keys() -> ContentKeys {
    let (shared, auth) = (bytes(rfc::SHARED), bytes(rfc::AUTH));
    let (device, ours, salt) = (bytes(rfc::DEVICE_KEY), bytes(rfc::OUR_KEY), bytes(rfc::SALT));
    content_keys(&shared, &auth, &device, &ours, &salt)
}

/// The key and nonce come out as the RFC's do from its agreed secret.
///
/// The break this catches: any slip in the key schedule (the info strings, which key goes first,
/// salt and secret swapped), each of which a device rejects as a message it can't open.
#[test]
fn the_content_keys_are_the_rfcs() {
    let keys = rfc_keys();
    assert_eq!(keys.key.as_slice(), bytes(rfc::CONTENT_KEY));
    assert_eq!(keys.nonce.as_slice(), bytes(rfc::NONCE));
}

/// The sealed body is the RFC's header and ciphertext, byte for byte.
///
/// The break this catches: a wrong header field (record size, key length, the device's key where
/// ours belongs) or a missing end-of-record delimiter.
#[test]
fn a_message_is_sealed_as_the_rfcs() {
    let salt: [u8; 16] = bytes(rfc::SALT).try_into().expect("16 bytes");
    let body = seal(&rfc_keys(), &salt, &bytes(rfc::OUR_KEY), rfc::MESSAGE.as_bytes());
    assert_eq!(body, [bytes(rfc::HEADER), bytes(rfc::SEALED)].concat());
}

/// A device opens what `encrypt` sealed for it, agreeing its key with the one in the header.
///
/// The break this catches: `encrypt` mixing the keys in the wrong order or putting the wrong key
/// in the header, which the pinned halves above can't see.
#[test]
fn a_device_opens_what_was_sealed_for_it() {
    let random = rand::SystemRandom::new();
    let device = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &random).unwrap();
    let device_key = device.compute_public_key().unwrap();
    let auth = [7u8; 16];
    let body = encrypt(b"Approve bash", device_key.as_ref(), &auth).unwrap();
    assert_eq!(open(device, device_key.as_ref(), &auth, &body), b"Approve bash\x02");
}

/// What the device with private key `device` reads from `body`, as its browser would: the key in
/// the header agreed with its own, and the one record opened, delimiter and all.
pub(crate) fn open(
    device: agreement::EphemeralPrivateKey,
    device_key: &[u8],
    auth: &[u8],
    body: &[u8],
) -> Vec<u8> {
    let (salt, our_key, sealed) = (&body[..16], &body[21..86], &body[86..]);
    let ours = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, our_key);
    let keys = agreement::agree_ephemeral(device, &ours, |shared| {
        content_keys(shared, auth, device_key, our_key, salt)
    })
    .expect("the header's key is on the curve");
    let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &keys.key).unwrap());
    let mut record = sealed.to_vec();
    let nonce = aead::Nonce::assume_unique_for_key(keys.nonce);
    key.open_in_place(nonce, aead::Aad::empty(), &mut record).expect("it opens").to_vec()
}

/// The break this catches: sealing a message longer than its record says, which the push service
/// refuses, rather than saying why here.
#[test]
fn a_message_too_long_for_one_record_is_refused() {
    let device = [4u8; 65];
    let longest = vec![b'x'; RECORD_SIZE as usize - RECORD_OVERHEAD];
    assert!(!matches!(encrypt(&longest, &device, &[0; 16]), Err(EncryptionError::TooLong(_))));
    let longer = vec![b'x'; longest.len() + 1];
    assert!(matches!(encrypt(&longer, &device, &[0; 16]), Err(EncryptionError::TooLong(_))));
}

/// The break this catches: a key that isn't on the curve sealed for anyway, sending a message no
/// device can open.
#[test]
fn a_key_off_the_curve_is_refused() {
    let sealed = encrypt(b"hi", &[4u8; 65], &[0; 16]);
    assert!(matches!(sealed, Err(EncryptionError::BadSubscriptionKey)));
}
