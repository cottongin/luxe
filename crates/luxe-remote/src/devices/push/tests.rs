use super::*;
use crate::push::Subscription;

fn subscription(endpoint: &str) -> Subscription {
    let keys = crate::push::SubscriptionKeys {
        p256dh: "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4"
            .into(),
        auth: "BTBZMqHH6r4Tts7J_aSIgg".into(),
    };
    Subscription { endpoint: endpoint.into(), keys }
}

/// A book with one paired device, and that device.
fn paired() -> (tempfile::TempDir, DeviceBook, Device) {
    let dir = tempfile::tempdir().unwrap();
    let book = DeviceBook::at(dir.path().join("remote.json"));
    let token = book.invite().unwrap();
    let (device, _) = book.pair(&token, "iPhone", "alex@github").unwrap();
    (dir, book, device)
}

fn kept(book: &DeviceBook, id: &str) -> Option<Subscription> {
    let devices = book.devices();
    devices.iter().find(|device| device.id == id)?.subscription().cloned()
}

/// The break this catches: a new key on every ask, so each subscription is to a key the bridge
/// no longer signs with and Apple refuses every push (`VapidPkHashMismatch`).
#[test]
fn the_push_key_is_made_once_and_kept() {
    let (_dir, book, _) = paired();
    let first = book.push_key().unwrap().public_key();
    assert_eq!(book.push_key().unwrap().public_key(), first);
    let reopened = DeviceBook::at(book.file.path().to_path_buf());
    assert_eq!(reopened.push_key().unwrap().public_key(), first);
}

/// The break this catches: a subscription kept apart from its device, so a revoked phone goes on
/// being told what your sessions are waiting for.
#[test]
fn a_subscription_goes_with_its_device() {
    let (_dir, book, device) = paired();
    let apple = subscription("https://web.push.apple.com/a");
    assert!(book.subscribe(&device.id, Some(apple.clone())).unwrap());
    assert_eq!(kept(&book, &device.id), Some(apple));
    assert!(book.subscribe(&device.id, None).unwrap());
    assert_eq!(kept(&book, &device.id), None, "turned off");
    assert!(!book.subscribe("nobody", Some(subscription("https://web.push.apple.com/b"))).unwrap());
}

/// The break this catches: a late "gone" for an old endpoint wiping the subscription the device
/// made since, so it stops getting notifications it just turned on.
#[test]
fn a_gone_endpoint_forgets_only_that_subscription() {
    let (_dir, book, device) = paired();
    let fresh = subscription("https://web.push.apple.com/new");
    book.subscribe(&device.id, Some(fresh.clone())).unwrap();
    book.forget_endpoint(&device.id, "https://web.push.apple.com/old").unwrap();
    assert_eq!(kept(&book, &device.id), Some(fresh));
    book.forget_endpoint(&device.id, "https://web.push.apple.com/new").unwrap();
    assert_eq!(kept(&book, &device.id), None);
}

/// The file names what it keeps as below, and the key never prints.
///
/// The break this catches: a renamed field, which a round trip can't see but which loses every
/// device's subscription and the key (so every subscription with it) on upgrade; or the key in a
/// log line through `Debug`.
#[test]
fn the_file_keeps_the_key_and_each_subscription_by_name() {
    let (dir, book, device) = paired();
    book.subscribe(&device.id, Some(subscription("https://web.push.apple.com/a"))).unwrap();
    book.push_key().unwrap();
    let written = std::fs::read_to_string(dir.path().join("remote.json")).unwrap();
    let written: serde_json::Value = serde_json::from_str(&written).unwrap();
    let push = &written["devices"][0]["push"];
    assert_eq!(push["endpoint"], "https://web.push.apple.com/a");
    assert_eq!(push["keys"]["auth"], "BTBZMqHH6r4Tts7J_aSIgg");
    let key = written["push_key"].as_str().expect("the key is kept as text");
    let registry = book.load();
    assert!(!format!("{registry:?}").contains(key), "the key printed");
}
