use super::*;

const START: u64 = 1_800_000_000;

fn device(credential: &str) -> Device {
    Device {
        id: "a1b2c3d4".into(),
        name: "iPhone".into(),
        login: "alex@github".into(),
        paired_at: START,
        credential_sha256: digest(credential),
        push: None,
    }
}

/// An invitation pairs one device, once.
///
/// The break this catches: leaving the invitation in place after it is used, so a link that
/// leaked from a screenshot or a shared photo pairs a second device.
#[test]
fn an_invitation_pairs_one_device_once() {
    let mut registry = Registry::default();
    registry.invite("token", START);
    assert!(registry.redeem("token", device("first"), START + 1).is_ok());
    let again = registry.redeem("token", device("second"), START + 2);
    assert_eq!(again, Err(PairingError::InvitationUnusable));
    assert_eq!(registry.devices.len(), 1);
}

/// An invitation is good for ten minutes and no longer.
///
/// The break this catches: never expiring invitations, so one printed and forgotten stays a way
/// in for good.
#[test]
fn an_invitation_expires() {
    let mut registry = Registry::default();
    registry.invite("token", START);
    let late = START + INVITATION_LIFETIME_SECS;
    assert_eq!(registry.redeem("token", device("c"), late), Err(PairingError::InvitationUnusable));
    registry.invite("fresh", START);
    let in_time = START + INVITATION_LIFETIME_SECS - 1;
    assert!(registry.redeem("fresh", device("c"), in_time).is_ok());
}

/// A device signs in with its own credential, and not after it is revoked.
///
/// The break this catches: revocation that leaves the device's credential working.
#[test]
fn a_revoked_device_no_longer_signs_in() {
    let mut registry = Registry::default();
    registry.invite("token", START);
    registry.redeem("token", device("credential"), START).unwrap();
    assert_eq!(registry.holder("credential").map(|device| device.name.as_str()), Some("iPhone"));
    assert!(registry.holder("another").is_none());
    assert!(registry.revoke("a1b2c3d4").is_some());
    assert!(registry.holder("credential").is_none());
    assert!(registry.revoke("a1b2c3d4").is_none(), "nothing left to revoke");
}

/// The file keeps digests: neither the invitation nor the credential can be read back out of it.
///
/// The break this catches: storing either as it was issued, so anyone who reads the file (a
/// backup, a synced dotfiles repository) can pair or sign in as the device.
#[test]
fn the_file_holds_no_secret_that_signs_in() {
    let dir = tempfile::tempdir().unwrap();
    let book = DeviceBook::at(dir.path().join("remote.json"));
    let token = book.invite().unwrap();
    let unused = book.invite().unwrap();
    let (device, credential) = book.pair(&token, "iPad", "alex@github").unwrap();
    let written = std::fs::read_to_string(dir.path().join("remote.json")).unwrap();
    for secret in [&token, &unused, &credential] {
        assert!(!written.contains(secret.as_str()), "{secret} is in the file");
    }
    assert_eq!(book.holder(&credential), Some(device));
}
