//! Presence — which devices have which session open right now.
//!
//! The app closes its socket when it's hidden, so an open socket means the session is on that
//! device's screen, and a notification about it would only repeat what's in front of you.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// A device id and a session id.
type Visit = (String, String);

/// The sockets open now, counted per device and session.
#[derive(Debug, Clone, Default)]
pub(crate) struct Presence {
    open: Arc<Mutex<HashMap<Visit, usize>>>,
}

impl Presence {
    /// Counts `device` as looking at `session` until the returned guard drops.
    pub(crate) fn arrive(&self, device: &str, session: &str) -> Here {
        let visit = (device.to_string(), session.to_string());
        *self.lock().entry(visit.clone()).or_default() += 1;
        Here { presence: self.clone(), visit }
    }

    /// Whether `device` has `session` open.
    pub(crate) fn is_here(&self, device: &str, session: &str) -> bool {
        self.lock().contains_key(&(device.to_string(), session.to_string()))
    }

    // a panic elsewhere can't leave a count half-changed, so a poisoned lock is still right
    fn lock(&self) -> MutexGuard<'_, HashMap<Visit, usize>> {
        self.open.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One open socket, counted while it lives.
pub(crate) struct Here {
    presence: Presence,
    visit: Visit,
}

impl Drop for Here {
    fn drop(&mut self) {
        let mut open = self.presence.lock();
        let Some(count) = open.get_mut(&self.visit) else { return };
        *count -= 1;
        if *count == 0 {
            open.remove(&self.visit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The break this catches: the first socket to close clearing the device, though the session
    /// is still open in another tab or a second socket, or one device's socket covering another.
    #[test]
    fn a_device_is_here_while_any_of_its_sockets_is_open() {
        let presence = Presence::default();
        let first = presence.arrive("phone", "s1");
        let second = presence.arrive("phone", "s1");
        assert!(!presence.is_here("tablet", "s1"), "another device");
        assert!(!presence.is_here("phone", "s2"), "another session");
        drop(first);
        assert!(presence.is_here("phone", "s1"), "one socket is still open");
        drop(second);
        assert!(!presence.is_here("phone", "s1"));
    }
}
