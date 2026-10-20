//! The routes a device turns its notifications on and off with, behind the paired-device check.
//!
//! `GET /api/push` gives the key to subscribe with and whether this device is subscribed,
//! `PUT /api/push` takes the subscription the browser made, and `DELETE /api/push` stops them.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};

use super::Subscription;
use crate::access::Refusal;
use crate::devices::Device;
use crate::routes::Bridged;

/// What the app needs to subscribe, or to show that it has.
#[derive(serde::Serialize)]
struct PushStatus {
    /// The bridge's public key, base64url, for `PushManager.subscribe`
    key: String,
    /// Whether the bridge has a subscription for this device
    subscribed: bool,
}

/// The bridge's push key, and whether this device is subscribed.
pub(crate) async fn status(
    State(bridged): State<Arc<Bridged>>,
    Extension(device): Extension<Device>,
) -> Response {
    let devices = bridged.devices.clone();
    let key = tokio::task::spawn_blocking(move || devices.push_key()).await;
    match key {
        Ok(Ok(vapid)) => {
            let subscribed = device.subscription().is_some();
            Json(PushStatus { key: vapid.public_key(), subscribed }).into_response()
        }
        Ok(Err(error)) => failed(&error),
        Err(error) => failed(&error),
    }
}

/// Sends this device's notifications to the subscription its browser made.
pub(crate) async fn subscribe(
    State(bridged): State<Arc<Bridged>>,
    Extension(device): Extension<Device>,
    Json(subscription): Json<Subscription>,
) -> Response {
    match subscription.checked() {
        Ok(subscription) => keep(&bridged, &device, Some(subscription)).await,
        Err(refused) => {
            tracing::warn!(device = %device.name, %refused, "refused a push subscription");
            (StatusCode::BAD_REQUEST, refused.to_string()).into_response()
        }
    }
}

/// Stops this device's notifications.
pub(crate) async fn unsubscribe(
    State(bridged): State<Arc<Bridged>>,
    Extension(device): Extension<Device>,
) -> Response {
    keep(&bridged, &device, None).await
}

async fn keep(bridged: &Bridged, device: &Device, subscription: Option<Subscription>) -> Response {
    let (devices, id) = (bridged.devices.clone(), device.id.clone());
    let on = subscription.is_some();
    let kept = tokio::task::spawn_blocking(move || devices.subscribe(&id, subscription)).await;
    match kept {
        Ok(Ok(true)) => {
            tracing::info!(device = %device.name, on, "a device changed its notifications");
            StatusCode::NO_CONTENT.into_response()
        }
        // revoked since the request was let in
        Ok(Ok(false)) => Refusal::NotPaired.into_response(),
        Ok(Err(error)) => failed(&error),
        Err(error) => failed(&error),
    }
}

fn failed(error: &dyn std::error::Error) -> Response {
    tracing::error!(%error, "couldn't read or keep a push subscription");
    let reason = format!("couldn't keep notifications: {error}");
    (StatusCode::INTERNAL_SERVER_ERROR, reason).into_response()
}
