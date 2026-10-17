//! Guide run — what a detached guide pass runs against, resolved once before it is spawned.
//!
//! A guide pass outlives the action that started it: it runs beside later turns, model switches
//! and session switches. So the model it runs on, the provider that serves it and the ledger it
//! bills are settled when it starts — the session's current model, routed to its own provider, and
//! the ledger of the session that asked, which keeps its late spend even after a switch.

use super::review_state::GuideCancel;
use super::*;

/// Everything a guide pass needs from the session that started it.
pub(crate) struct GuideRunContext {
    pub(crate) provider: Arc<dyn Provider + Send + Sync>,
    pub(crate) model: crate::provider::ModelInfo,
    pub(crate) cancel: GuideCancel,
    /// The asking session's ledger, which settles to that session whatever is open later
    pub(crate) ledger: Arc<crate::usage::UsageLedger>,
}

impl Engine {
    /// A guide pass's context, on the model the session is on now.
    pub(crate) async fn guide_run(&self, cancel: GuideCancel) -> GuideRunContext {
        let model = self.effective_model().await;
        GuideRunContext {
            provider: self.provider_for(&model),
            model,
            cancel,
            ledger: self.usage_ledger(),
        }
    }
}
