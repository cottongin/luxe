use super::*;
use crate::push::encryption::tests::open;
use crate::push::SubscriptionKeys;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ring::agreement::{EphemeralPrivateKey, ECDH_P256};

const NOW: u64 = 1_800_000_000;
const SESSION: &str = "5898d57b-0000-4000-8000-000000000001";

/// A device's private key and the subscription its browser would make with it.
fn device() -> (EphemeralPrivateKey, Subscription) {
    let private = EphemeralPrivateKey::generate(&ECDH_P256, &ring::rand::SystemRandom::new());
    let private = private.unwrap();
    let public = private.compute_public_key().unwrap();
    let keys = SubscriptionKeys {
        p256dh: URL_SAFE_NO_PAD.encode(public.as_ref()),
        auth: URL_SAFE_NO_PAD.encode([7u8; 16]),
    };
    (private, Subscription { endpoint: "https://web.push.apple.com/abc".into(), keys })
}

fn courier() -> WebPush {
    WebPush::new(Vapid::generate().unwrap().0).unwrap()
}

fn notice(urgent: bool) -> Notice {
    let (title, body) = ("luxe-dev".into(), "Approve bash".into());
    Notice { title, body, session: SESSION.into(), urgent }
}

fn header<'a>(request: &'a reqwest::Request, name: &str) -> &'a str {
    request.headers()[name].to_str().unwrap()
}

/// The post goes to the subscription with the headers a push service requires, and the device
/// opens the body to the notice the service worker shows.
///
/// The break this catches: a missing `TTL` (Apple's `BadTtl`) or encoding, the wrong urgency, or a
/// body sealed for someone else or carrying fields the worker doesn't read.
#[test]
fn a_post_carries_what_the_push_service_and_the_device_need() {
    let (private, subscription) = device();
    let request = courier().request(&subscription, &notice(true), NOW).unwrap();
    assert_eq!(request.url().as_str(), subscription.endpoint);
    assert_eq!(header(&request, "TTL"), "3600");
    assert_eq!(header(&request, "Urgency"), "high");
    assert_eq!(header(&request, "Content-Encoding"), "aes128gcm");
    assert_eq!(header(&request, "Topic"), "DJZtLYoay56CTZC7EDzavnWaSjSnBF8s");
    assert!(header(&request, "Authorization").starts_with("vapid t="));
    let body = request.body().and_then(reqwest::Body::as_bytes).unwrap();
    let (device_key, auth) = subscription.key_bytes().unwrap();
    let opened = open(private, &device_key, &auth, body);
    let shown: serde_json::Value = serde_json::from_slice(&opened[..opened.len() - 1]).unwrap();
    let expected =
        serde_json::json!({ "title": "luxe-dev", "body": "Approve bash", "session": SESSION });
    assert_eq!(shown, expected);
    let later = courier().request(&subscription, &notice(false), NOW).unwrap();
    assert_eq!(header(&later, "Urgency"), "normal");
}

/// The claims inside a request's token.
fn claims(request: &reqwest::Request) -> serde_json::Value {
    let token = header(request, "Authorization").strip_prefix("vapid t=").unwrap();
    let claims = token.split('.').nth(1).unwrap();
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims).unwrap()).unwrap()
}

/// The break this catches: a token signed for every post, which Apple asks servers not to do more
/// than hourly, or one kept past its expiry, which it refuses; or a token for the wrong service.
#[test]
fn a_token_is_reused_until_it_has_an_hour_left() {
    let (_, subscription) = device();
    let courier = courier();
    let first = courier.request(&subscription, &notice(true), NOW).unwrap();
    let token = |request: &reqwest::Request| header(request, "Authorization").to_string();
    let expected = serde_json::json!({ "aud": "https://web.push.apple.com", "exp": NOW + TOKEN_LIFETIME_SECS, "sub": "https://luxe.dev" });
    assert_eq!(claims(&first), expected);
    let renew_at = NOW + TOKEN_LIFETIME_SECS - TOKEN_RENEWAL_SECS;
    let reused = courier.request(&subscription, &notice(true), renew_at - 1).unwrap();
    assert_eq!(token(&reused), token(&first));
    let renewed = courier.request(&subscription, &notice(true), renew_at).unwrap();
    assert_eq!(claims(&renewed)["exp"], renew_at + TOKEN_LIFETIME_SECS);
}

/// The topic is 32 base64url characters whatever the id, and differs between sessions.
///
/// The break this catches: a session id passed through whole, whose 36 characters Apple refuses
/// (`BadWebPushTopic`), a shorter one at a length Apple can't read as base64, or one topic for
/// every session, so a newer notice from one session replaces another's.
#[test]
fn a_topic_is_always_32_url_safe_characters() {
    assert_eq!(topic(SESSION), "DJZtLYoay56CTZC7EDzavnWaSjSnBF8s");
    assert_eq!(topic("short-1"), "GA5_ZXQcTDu9kMi-CVbgVxwQ_TCZQ-fp");
    assert_eq!(topic("").len(), 32);
}

/// A push service on this machine that answers every post with the status in its path.
async fn push_service() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let answer = |axum::extract::Path(status): axum::extract::Path<u16>| async move {
        (axum::http::StatusCode::from_u16(status).unwrap(), "BadDeviceToken")
    };
    let app = axum::Router::new().route("/{status}", axum::routing::post(answer));
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{address}")
}

async fn delivered(courier: &WebPush, service: &str, status: u16) -> String {
    let (_, mut subscription) = device();
    subscription.endpoint = format!("{service}/{status}");
    format!("{:?}", courier.deliver(&subscription, &notice(false)).await)
}

/// The break this catches: a dropped subscription (404 or 410) taken as a failure, so it's posted
/// to forever, or any other refusal taken as gone, so one bad request forgets a working device.
#[tokio::test]
async fn what_the_push_service_answers_is_told_apart() {
    let (courier, service) = (courier(), push_service().await);
    assert_eq!(delivered(&courier, &service, 201).await, "Ok(Taken)");
    assert_eq!(delivered(&courier, &service, 404).await, "Ok(Gone)");
    assert_eq!(delivered(&courier, &service, 410).await, "Ok(Gone)");
    let refused = delivered(&courier, &service, 400).await;
    assert!(refused.contains("status: 400") && refused.contains("BadDeviceToken"), "{refused}");
}
