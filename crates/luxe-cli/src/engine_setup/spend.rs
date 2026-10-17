//! Run-scoped spend accounting — one figure for a loop, across every engine it uses.
//!
//! A session ledger is reset and rebuilt whenever a session is loaded, and a loop run loads several:
//! it starts fresh sessions as it goes, and every phase review runs in an engine and a session of its
//! own. So the run's total is kept beside the phase counter in `.luxe/loop_state.json`, and both
//! engines report into it through [`luxe_core::usage::SpendSink`].

use std::sync::Arc;

use luxe_core::loop_controller::LoopController;
use luxe_core::usage::{SpendSink, UsageSource, UsageTotals};

/// Adds every engine's provider calls to the loop run's total.
#[derive(Debug)]
pub(crate) struct LoopSpendSink {
    controller: Arc<LoopController>,
}

impl LoopSpendSink {
    /// A sink writing into `controller`'s state.
    pub(crate) fn new(controller: Arc<LoopController>) -> Self {
        Self { controller }
    }
}

#[async_trait::async_trait]
impl SpendSink for LoopSpendSink {
    async fn record(&self, model_id: &str, _source: UsageSource, totals: &UsageTotals) {
        self.controller.record_spend(model_id, totals).await;
    }
}
