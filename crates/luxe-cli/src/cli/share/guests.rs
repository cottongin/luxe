//! Guests — who's joined the shared session right now, by name, for the terminal's dock.
//!
//! Each announced client takes a seat when it joins and gives it up when it leaves, so two of the
//! same name (a phone that reconnected before its old socket closed) are counted apart.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// The clients in the session now, in the order they joined.
#[derive(Clone, Default)]
pub(super) struct GuestList {
    seated: Arc<Mutex<Vec<(u64, String)>>>,
    next: Arc<AtomicU64>,
}

impl GuestList {
    /// Seats `name`, returning the seat to give up on leaving.
    pub(super) fn arrive(&self, name: String) -> u64 {
        let seat = self.next.fetch_add(1, Ordering::Relaxed);
        self.seated().push((seat, name));
        seat
    }

    /// Gives up `seat`.
    pub(super) fn leave(&self, seat: u64) {
        self.seated().retain(|(taken, _)| *taken != seat);
    }

    /// Everyone seated, by name.
    pub(super) fn names(&self) -> Vec<String> {
        self.seated().iter().map(|(_, name)| name.clone()).collect()
    }

    /// The list, even if a panic elsewhere poisoned the lock: a stale name is all it could hold.
    fn seated(&self) -> MutexGuard<'_, Vec<(u64, String)>> {
        self.seated.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A guest leaves by its seat, so one of two with the same name leaving leaves the other.
    ///
    /// The break this catches: a phone that reconnected before its old socket closed vanishing
    /// from the dock when the old one goes, while it's still joined.
    #[test]
    fn a_guest_leaves_by_its_seat_not_its_name() {
        let guests = GuestList::default();
        let old = guests.arrive("iPhone".into());
        let _new = guests.arrive("iPhone".into());
        let ipad = guests.arrive("iPad".into());
        guests.leave(old);
        assert_eq!(guests.names(), ["iPhone", "iPad"]);
        guests.leave(ipad);
        assert_eq!(guests.names(), ["iPhone"]);
    }
}
