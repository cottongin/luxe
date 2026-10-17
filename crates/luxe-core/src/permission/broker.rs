//! Permission ask broker — bridges a blocked tool dispatch to the front-end's reply.
//!
//! When a tool call resolves to `ask` and a front-end can prompt, the dispatch path registers a
//! pending request here (getting a one-shot receiver) and emits `Event::PermissionRequested`. The
//! engine's action loop, on the matching `Action::RespondPermission`, calls [`PermissionBroker::resolve`]
//! to hand the answer back through the one-shot, unblocking the awaiting tool. Cancelling a turn
//! drops every pending sender via [`PermissionBroker::cancel_all`], so a waiting dispatch unblocks
//! promptly (a dropped sender surfaces as a rejected reply).

use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::oneshot;
use uuid::Uuid;

use super::PermissionReply;

/// Correlates an emitted `PermissionRequested` event with the `RespondPermission` action that answers it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct PermissionRequestId(String);

impl PermissionRequestId {
    /// Mint a fresh, unique request id.
    pub fn new() -> Self {
        Self(format!("perm:{}", Uuid::new_v4()))
    }

    /// The stable serialized identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for PermissionRequestId {
    fn default() -> Self {
        Self::new()
    }
}

/// A registry of in-flight permission prompts awaiting a user reply.
#[derive(Debug, Default)]
pub struct PermissionBroker {
    pending: Mutex<HashMap<PermissionRequestId, oneshot::Sender<PermissionReply>>>,
}

impl PermissionBroker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new pending prompt, returning its id and the receiver the dispatch awaits.
    pub fn register(&self) -> (PermissionRequestId, oneshot::Receiver<PermissionReply>) {
        let id = PermissionRequestId::new();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("permission broker lock").insert(id.clone(), tx);
        (id, rx)
    }

    /// Deliver a reply to the matching pending prompt (no-op if it's gone/stale).
    ///
    /// A stale reply is harmless and expected after a cancel, but it is also exactly what a lost
    /// answer looks like from here, so it is logged rather than dropped without a trace.
    pub fn resolve(&self, id: &PermissionRequestId, reply: PermissionReply) {
        match self.pending.lock().expect("permission broker lock").remove(id) {
            Some(tx) => {
                tracing::debug!(?id, ?reply, "permission answered");
                if tx.send(reply).is_err() {
                    tracing::warn!(
                        ?id,
                        "a permission answer arrived after its call stopped waiting"
                    );
                }
            }
            None => tracing::info!(?id, ?reply, "a permission answer matched no open prompt"),
        }
    }

    /// Forget a pending prompt without answering (the awaiting side resolves via its own fallback).
    pub fn forget(&self, id: &PermissionRequestId) {
        self.pending.lock().expect("permission broker lock").remove(id);
    }

    /// Drop every pending sender (turn cancelled/closed) so awaiting dispatches unblock.
    pub fn cancel_all(&self) {
        self.pending.lock().expect("permission broker lock").clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolve_delivers_the_reply() {
        let broker = PermissionBroker::new();
        let (id, rx) = broker.register();
        broker.resolve(&id, PermissionReply::Once);
        assert_eq!(rx.await.unwrap(), PermissionReply::Once);
    }

    #[tokio::test]
    async fn cancel_all_drops_pending_and_unblocks_receiver() {
        let broker = PermissionBroker::new();
        let (_id, rx) = broker.register();
        broker.cancel_all();
        assert!(rx.await.is_err(), "a dropped sender surfaces as a receive error");
    }

    #[tokio::test]
    async fn resolving_a_stale_id_is_a_noop() {
        let broker = PermissionBroker::new();
        broker.resolve(&PermissionRequestId::new(), PermissionReply::Reject);
    }
}
