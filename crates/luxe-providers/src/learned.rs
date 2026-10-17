//! Learned — what a rejection taught an adapter about a model, kept for the session.
//!
//! A 400 can say which thinking encoding, output cap or tool-loading mode a model really accepts.
//! Remembering it keeps the correction to one wasted round trip per model per session instead of
//! one per turn. Keyed by model id, since one adapter serves every model behind its endpoint.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

/// A per-model memory, shared by every request the adapter sends.
///
/// A poisoned lock still answers: a panic elsewhere must not make the adapter forget, and a
/// forgotten correction costs a round trip on every turn.
#[derive(Debug)]
pub(crate) struct Learned<T> {
    by_model: Mutex<HashMap<String, T>>,
}

impl<T> Default for Learned<T> {
    fn default() -> Self {
        Self { by_model: Mutex::new(HashMap::new()) }
    }
}

impl<T: Clone> Learned<T> {
    /// What was learned about `model`, if anything.
    pub(crate) fn get(&self, model: &str) -> Option<T> {
        self.by_model.lock().unwrap_or_else(PoisonError::into_inner).get(model).cloned()
    }

    /// Keep `value` for `model`, replacing what was there.
    pub(crate) fn remember(&self, model: &str, value: T) {
        self.by_model
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(model.to_string(), value);
    }

    /// Whether anything was learned about `model`, for a lesson that carries no value.
    pub(crate) fn knows(&self, model: &str) -> bool {
        self.by_model.lock().unwrap_or_else(PoisonError::into_inner).contains_key(model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn a_lesson_is_kept_for_its_model_alone() {
        let learned = Learned::<u32>::default();
        assert_eq!(learned.get("a"), None);
        learned.remember("a", 64_000);
        learned.remember("a", 32_000);
        assert_eq!(learned.get("a"), Some(32_000), "the later lesson replaces the earlier");
        assert_eq!(learned.get("b"), None);
        assert!(learned.knows("a") && !learned.knows("b"));
    }

    /// A thread that panics holding the lock poisons it; reading that as having learned nothing
    /// would send the corrected request's mistake again on every turn.
    #[test]
    fn a_poisoned_lock_still_remembers_and_still_learns() {
        let learned = Arc::new(Learned::<u32>::default());
        learned.remember("a", 1);
        let holder = Arc::clone(&learned);
        let _ = std::thread::spawn(move || {
            let _held = holder.by_model.lock().unwrap();
            panic!("poison the lock");
        })
        .join();
        assert!(learned.by_model.is_poisoned(), "the setup poisoned it");
        assert_eq!(learned.get("a"), Some(1));
        assert!(learned.knows("a") && !learned.knows("b"));
        learned.remember("b", 2);
        assert_eq!(learned.get("b"), Some(2));
    }
}
