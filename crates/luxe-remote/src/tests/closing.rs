//! How the bridge closes a device's socket, which is how the app tells leaving, an ended session
//! and being unpaired apart from a dropped connection.

use super::*;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;

/// The close frame the device reads next, skipping any protocol frames before it.
async fn close_frame(device: &mut DeviceSocket) -> Option<CloseFrame> {
    let wait = async {
        loop {
            match device.next().await {
                Some(Ok(Message::Close(frame))) => return frame,
                Some(Ok(_)) => continue,
                other => panic!("the socket ended without a close frame: {other:?}"),
            }
        }
    };
    tokio::time::timeout(REVOCATION_CHECK * 3, wait).await.expect("the socket stayed open")
}

/// A device that closes its socket gets the close answered, not a dropped connection.
///
/// The break this catches: dropping the socket as soon as the device's close arrives, so every
/// time the app leaves it sees code 1006 and can't tell that from losing the network.
#[tokio::test]
async fn a_device_that_closes_is_answered() {
    let bridge = harness().await;
    let mut session = fake_session(bridge.runtime.path());
    let (mut device, _joined) = bridge.join(&mut session).await;

    device.close(None).await.unwrap();
    let answer = tokio::time::timeout(REVOCATION_CHECK, device.next()).await.expect("no answer");
    assert!(matches!(answer, Some(Ok(Message::Close(_)))), "answered with {answer:?}");
}

/// A session that ends closes the device's socket normally, saying why.
///
/// The break this catches: dropping the socket when the session goes, so the app retries a
/// session that is gone instead of saying it ended.
#[tokio::test]
async fn a_session_that_ends_closes_the_socket_normally() {
    let bridge = harness().await;
    let mut session = fake_session(bridge.runtime.path());
    let (mut device, joined) = bridge.join(&mut session).await;

    drop(joined);
    let frame = close_frame(&mut device).await.expect("a close code");
    assert_eq!(frame.code, CloseCode::Normal);
    assert_eq!(frame.reason, "the session ended");
}

/// Unpairing a connected device closes its open socket, with the code that says so.
///
/// The break this catches: checking the device only at the upgrade, so a lost phone keeps
/// steering a session it already joined; or closing it like any other ending, so the app keeps
/// retrying with a credential that will never work again instead of asking to be paired.
#[tokio::test]
async fn an_unpaired_device_is_told_so() {
    let bridge = harness().await;
    let mut session = fake_session(bridge.runtime.path());
    let (mut device, _joined) = bridge.join(&mut session).await;

    let id = bridge.devices.devices()[0].id.clone();
    bridge.devices.revoke(&id).unwrap();
    let frame = close_frame(&mut device).await.expect("a close code");
    assert_eq!(u16::from(frame.code), relay::UNPAIRED);
    assert_eq!(frame.reason, "this device was unpaired");
}
