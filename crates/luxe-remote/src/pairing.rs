//! Pairing — a device trades an invitation for a credential of its own.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;

use crate::access::{credential_cookie, Login};
use crate::routes::Bridged;
use crate::RemoteError;

/// Longest name a device keeps; it is a label in a list, not a description.
const NAME_LIMIT: usize = 40;

/// What the pairing page sends.
#[derive(Debug, Deserialize)]
pub(crate) struct PairRequest {
    invitation: String,
    #[serde(default)]
    name: String,
}

/// Redeems the invitation and sets the device's credential as a cookie the page never sees.
pub(crate) async fn pair(
    State(bridged): State<Arc<Bridged>>,
    Extension(Login(login)): Extension<Login>,
    Json(request): Json<PairRequest>,
) -> Response {
    let devices = bridged.devices.clone();
    let name = device_name(&request.name);
    let paired =
        tokio::task::spawn_blocking(move || devices.pair(request.invitation.trim(), &name, &login))
            .await;
    match paired {
        Ok(outcome) => answer(outcome),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

fn answer(paired: Result<(crate::devices::Device, String), RemoteError>) -> Response {
    match paired {
        Ok((device, credential)) => {
            tracing::info!(device = %device.name, id = %device.id, "a device paired");
            let cookie = [(header::SET_COOKIE, credential_cookie(&credential))];
            let body = serde_json::json!({ "id": device.id, "name": device.name });
            (cookie, Json(body)).into_response()
        }
        Err(RemoteError::Pairing(refused)) => {
            (StatusCode::BAD_REQUEST, refused.to_string()).into_response()
        }
        Err(error) => {
            tracing::error!(%error, "could not record a paired device");
            (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response()
        }
    }
}

/// The name a device asked for, made fit for a list: one line, bounded, never empty.
fn device_name(asked: &str) -> String {
    let name: String =
        asked.chars().map(|c| if c.is_control() { ' ' } else { c }).take(NAME_LIMIT).collect();
    let name = name.trim();
    if name.is_empty() {
        "Device".to_string()
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device's name is a single bounded line, so `luxe remote devices` stays a list.
    ///
    /// The break this catches: storing the name as sent, so a newline forges a second row in the
    /// listing, or an empty one leaves a row with nothing to recognize it by.
    #[test]
    fn a_device_name_is_one_short_line() {
        assert_eq!(device_name("Evan's iPhone"), "Evan's iPhone");
        assert_eq!(device_name("phone\nabc1234  Fake"), "phone abc1234  Fake");
        assert_eq!(device_name("   "), "Device");
        assert_eq!(device_name(&"x".repeat(100)).len(), NAME_LIMIT);
    }
}
