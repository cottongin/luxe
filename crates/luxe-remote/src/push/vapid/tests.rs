use super::*;
use ring::signature::{UnparsedPublicKey, ECDSA_P256_SHA256_FIXED};

const APPLE: &str = "https://web.push.apple.com";

/// The token and key out of an `Authorization` header.
fn parts(header: &str) -> (String, Vec<u8>) {
    let rest = header.strip_prefix("vapid t=").expect("a vapid header");
    let (token, key) = rest.split_once(", k=").expect("a token and a key");
    (token.to_string(), URL_SAFE_NO_PAD.decode(key).expect("the key is base64url"))
}

fn decoded(segment: &str) -> serde_json::Value {
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(segment).unwrap()).unwrap()
}

/// The token says what Apple checks: ES256, the push service it's for, the expiry, and a URL to
/// reach the sender.
///
/// The break this catches: a claim renamed or dropped, or a contact that isn't a URL, which Apple
/// answers with 403 `BadJwtToken` for every push.
#[test]
fn the_token_carries_what_apple_checks() {
    let (vapid, _) = Vapid::generate().unwrap();
    let (token, _) = parts(&vapid.authorization(APPLE, 1_800_000_000).unwrap());
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(segments.len(), 3, "{token}");
    assert_eq!(decoded(segments[0]), serde_json::json!({ "typ": "JWT", "alg": "ES256" }));
    let claims =
        serde_json::json!({ "aud": APPLE, "exp": 1_800_000_000u64, "sub": "https://luxe.dev" });
    assert_eq!(decoded(segments[1]), claims);
}

/// The signature checks out against the key the header gives, with ring's verifier rather than
/// anything of the bridge's.
///
/// The break this catches: signing other bytes than the token's first two parts, a DER signature
/// where JWS wants the fixed 64 bytes, or the header naming another key.
#[test]
fn the_token_is_signed_by_the_key_it_names() {
    let (vapid, _) = Vapid::generate().unwrap();
    let (token, key) = parts(&vapid.authorization(APPLE, 1_800_000_000).unwrap());
    let (signed, signature) = token.rsplit_once('.').unwrap();
    let signature = URL_SAFE_NO_PAD.decode(signature).unwrap();
    let verifier = UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &key);
    assert!(verifier.verify(signed.as_bytes(), &signature).is_ok());
    assert_eq!(URL_SAFE_NO_PAD.encode(&key), vapid.public_key());
}

/// The key a device subscribed with is the one the bridge signs with after a restart.
///
/// The break this catches: the kept document not being the key in use, so every subscription is
/// refused (`VapidPkHashMismatch`) once the bridge restarts.
#[test]
fn a_kept_key_is_the_same_key() {
    let (vapid, document) = Vapid::generate().unwrap();
    let kept = Vapid::from_pkcs8(&document).unwrap();
    assert_eq!(kept.public_key(), vapid.public_key());
    let key = URL_SAFE_NO_PAD.decode(kept.public_key()).unwrap();
    assert_eq!((key.len(), key[0]), (65, 4), "an uncompressed P-256 point");
}

/// The break this catches: a damaged key file taken for a key, or a panic, rather than an error
/// the bridge can report.
#[test]
fn a_damaged_key_is_an_error() {
    assert!(matches!(Vapid::from_pkcs8(b"not a key"), Err(VapidError::Unreadable)));
}
