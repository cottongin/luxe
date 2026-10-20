//! Push — notifications on your devices when a session wants you, sent by the bridge itself.
//!
//! Web Push, as iOS has offered it to home-screen web apps since 16.4: a device that opted in
//! gives the bridge a subscription, and the bridge posts each notification, sealed so only that
//! device can read it, straight to the push service the subscription names (Apple's, for an
//! iPhone). There is no Luxe server in between, and the push service sees when, not what.
//!
//! [`watch`] hears what the sessions are waiting for, and [`notify`] takes each notice to every
//! subscribed device that doesn't have that session open in front of it.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc;

pub(crate) mod api;
mod encryption;
mod notice;
mod presence;
mod subscription;
mod vapid;
mod watch;
mod web_push;

pub(crate) use presence::Presence;
pub(crate) use subscription::Subscription;
#[cfg(test)]
pub(crate) use subscription::{tests::FROM_SAFARI, SubscriptionKeys};
#[cfg(test)]
pub(crate) use tests::Recording;
pub(crate) use vapid::{Vapid, VapidError};
pub(crate) use web_push::Courier;

use crate::devices::{Device, DeviceBook};
use notice::Notice;
use web_push::{Delivery, WebPush};

/// Notices heard but not yet posted. A few sessions, each with a prompt or two at a time.
const NOTICE_BUFFER: usize = 64;

/// Notifications from the sessions under `runtime_dir` to the devices in `devices`, until the
/// returned guard drops. They go through `courier`, or Web Push signed with the bridge's key, each
/// titled with `machine` when the bridge has a name.
pub(crate) fn start(
    runtime_dir: PathBuf,
    devices: DeviceBook,
    presence: Presence,
    courier: Option<Arc<dyn Courier>>,
    machine: Option<String>,
) -> watch::Watcher {
    watch::Watcher(tokio::spawn(notify(runtime_dir, devices, presence, courier, machine)))
}

async fn notify(
    runtime_dir: PathBuf,
    devices: DeviceBook,
    presence: Presence,
    mut courier: Option<Arc<dyn Courier>>,
    machine: Option<String>,
) {
    let (heard, mut notices) = mpsc::channel(NOTICE_BUFFER);
    let watching = watch::watch_sessions(runtime_dir, devices.clone(), heard);
    let _watching = watch::Watcher(tokio::spawn(watching));
    while let Some(notice) = notices.recv().await {
        let Some(courier) = courier_for(&mut courier, &devices).await else { continue };
        let notice = on_machine(notice, machine.as_deref());
        dispatch(&devices, &presence, &*courier, &notice).await;
    }
}

/// `notice` titled with the computer it came from as well as the project, so a phone paired with
/// two computers can tell which one wants it: `luxe · desk`.
fn on_machine(mut notice: Notice, machine: Option<&str>) -> Notice {
    if let Some(machine) = machine {
        notice.title = format!("{} \u{00b7} {machine}", notice.title);
    }
    notice
}

/// The courier, made on first use: the key is made when a device first subscribes, so it's there
/// by the time anything is heard.
async fn courier_for(
    kept: &mut Option<Arc<dyn Courier>>,
    devices: &DeviceBook,
) -> Option<Arc<dyn Courier>> {
    if let Some(courier) = kept {
        return Some(courier.clone());
    }
    match web_push(devices.clone()).await {
        Ok(courier) => Some(kept.insert(Arc::new(courier)).clone()),
        Err(error) => {
            tracing::error!(%error, "couldn't set up notifications");
            None
        }
    }
}

/// Web Push with the bridge's key, for logging when it can't be had.
async fn web_push(devices: DeviceBook) -> Result<WebPush, String> {
    match tokio::task::spawn_blocking(move || devices.push_key()).await {
        Ok(Ok(vapid)) => WebPush::new(vapid).map_err(|error| error.to_string()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(error) => Err(error.to_string()),
    }
}

/// Posts `notice` to every subscribed device that doesn't have its session open.
async fn dispatch(
    devices: &DeviceBook,
    presence: &Presence,
    courier: &dyn Courier,
    notice: &Notice,
) {
    let reader = devices.clone();
    let paired = tokio::task::spawn_blocking(move || reader.devices()).await.unwrap_or_default();
    for device in &paired {
        let Some(subscription) = device.subscription() else { continue };
        if presence.is_here(&device.id, &notice.session) {
            tracing::debug!(device = %device.name, "not notifying a device looking at the session");
            continue;
        }
        let delivered = courier.deliver(subscription, notice).await;
        settle(devices, device, &subscription.endpoint, delivered).await;
    }
}

/// Logs how a post went, and forgets a subscription its push service says is gone.
async fn settle(
    devices: &DeviceBook,
    device: &Device,
    endpoint: &str,
    delivered: Result<Delivery, web_push::PushError>,
) {
    match delivered {
        Ok(Delivery::Taken) => tracing::debug!(device = %device.name, "sent a notification"),
        Ok(Delivery::Gone) => {
            tracing::info!(device = %device.name, "a device's notifications ended at its push service");
            forget(devices.clone(), device.id.clone(), endpoint.to_string()).await;
        }
        Err(error) => tracing::warn!(%error, device = %device.name, "couldn't send a notification"),
    }
}

/// Drops the device's subscription, unless it has since subscribed somewhere new.
async fn forget(devices: DeviceBook, id: String, endpoint: String) {
    let forgot = tokio::task::spawn_blocking(move || devices.forget_endpoint(&id, &endpoint));
    if let Ok(Err(error)) = forgot.await {
        tracing::warn!(%error, "couldn't forget an ended subscription");
    }
}

#[cfg(test)]
pub(crate) mod tests;
