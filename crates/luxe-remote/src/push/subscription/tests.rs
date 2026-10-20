use super::*;

/// A subscription in the shape the Push API gives `PushSubscription.toJSON()`, with an endpoint
/// on Apple's push service and RFC 8291's example keys. Not captured from a device: swap in a real
/// one when there is one.
pub(crate) const FROM_SAFARI: &str = r#"{"endpoint":"https://web.push.apple.com/QGuQyavXutnMH8nrLlUjPZe_eVbc4ndHqoGAHGBrP2bIG7WY_DuCO7B0fhd7QAEXMaChjqCnDiuWvDEvkFvYg1vbqgxQp9zc5uj2BLsdLUn6R7Kp5YnWzMw6V_Ekd8CKk8Q3t6c2RU1yhp9eu5fcWkoUd3pUwP7sOTzmjVPCQCmY","keys":{"p256dh":"BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4","auth":"BTBZMqHH6r4Tts7J_aSIgg"},"expirationTime":null}"#;

fn safari() -> Subscription {
    serde_json::from_str(FROM_SAFARI).expect("Safari's shape reads")
}

fn at(endpoint: &str) -> Subscription {
    Subscription { endpoint: endpoint.into(), ..safari() }
}

/// The break this catches: reading only some other browser's shape, so Safari's subscription
/// (the one an iPhone sends) is refused or loses its keys.
#[test]
fn a_subscription_from_safari_is_taken() {
    let subscription = safari().checked().expect("it's Apple's push service");
    assert_eq!(subscription.audience().as_deref(), Some("https://web.push.apple.com"));
}

/// The break this catches: a host check that's a plain suffix or substring match, so a name
/// that only ends like a push service's (or contains one) is posted to.
#[test]
fn only_a_push_services_own_hosts_are_taken() {
    for endpoint in [
        "https://fcm.googleapis.com/fcm/send/abc",
        "https://updates.push.services.mozilla.com/wpush/v2/abc",
        "https://wns2-par02p.notify.windows.com/w/?token=abc",
    ] {
        assert!(at(endpoint).checked().is_ok(), "{endpoint}");
    }
    for endpoint in [
        "https://evilpush.apple.com.example.net/x",
        "https://notpush.apple.com/x",
        "https://example.net/web.push.apple.com",
        "https://192.168.1.1/push",
    ] {
        let refused = at(endpoint).checked();
        assert!(matches!(refused, Err(SubscriptionError::UnknownService(_))), "{endpoint}");
    }
}

/// The break this catches: posting over plain HTTP, or to a push service's host on another port,
/// where something other than the push service may be listening.
#[test]
fn an_endpoint_must_be_https_on_its_usual_port() {
    for endpoint in ["http://web.push.apple.com/x", "https://web.push.apple.com:8443/x", "nonsense"]
    {
        assert_eq!(at(endpoint).checked(), Err(SubscriptionError::NotHttps), "{endpoint}");
    }
}

/// The break this catches: taking keys that can't seal a message, so every notification to the
/// device fails later, with nothing said when it subscribed.
#[test]
fn keys_that_cant_seal_a_message_are_refused() {
    let bad = |p256dh: &str, auth: &str| {
        let keys = SubscriptionKeys { p256dh: p256dh.into(), auth: auth.into() };
        Subscription { keys, ..safari() }.checked()
    };
    let good = safari().keys;
    assert_eq!(bad("not base64!", &good.auth), Err(SubscriptionError::BadKeys));
    assert_eq!(bad(&good.p256dh[..60], &good.auth), Err(SubscriptionError::BadKeys));
    assert_eq!(bad(&good.p256dh, "c2hvcnQ"), Err(SubscriptionError::BadKeys));
    let compressed = URL_SAFE_NO_PAD.encode([[2u8].as_slice(), &[9; 64]].concat());
    assert_eq!(bad(&compressed, &good.auth), Err(SubscriptionError::BadKeys));
}
