//! Provider step — the list of providers the model picker opens on from the dock's provider row.
//!
//! Choosing a provider keeps the model in use when that provider serves it too, and otherwise moves
//! on to that provider's models. [`super::ModelPickerState`] makes that call; this holds the list
//! and its cursor.

use super::ModelEntry;
use crate::picker::PickerNav;

/// One provider row: its id and how many of the catalog's models it serves.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProviderRow {
    pub(super) id: String,
    pub(super) models: usize,
}

/// The provider list and its cursor.
#[derive(Debug, Clone)]
pub(super) struct ProviderStep {
    pub(super) rows: Vec<ProviderRow>,
    pub(super) nav: PickerNav,
}

impl ProviderStep {
    /// Every provider serving one of `models`, with the cursor on `current` when it is among them.
    pub(super) fn new(models: &[ModelEntry], current: Option<&str>) -> Self {
        let rows = provider_rows(models);
        let mut nav = PickerNav::new(rows.len());
        if let Some(row) = current.and_then(|id| rows.iter().position(|row| row.id == id)) {
            nav.select(row);
        }
        Self { rows, nav }
    }

    /// The provider under the cursor, if any row matches the query.
    pub(super) fn selected(&self) -> Option<&ProviderRow> {
        self.rows.get(self.nav.selected_item_index()?)
    }

    /// What a reader can type at a provider row: its id.
    pub(super) fn searchable(&self) -> Vec<String> {
        self.rows.iter().map(|row| row.id.clone()).collect()
    }
}

/// One row per provider, by id. The catalog lists providers in a hash set's order, which means
/// nothing, so a reader finds one where the alphabet says it is.
fn provider_rows(models: &[ModelEntry]) -> Vec<ProviderRow> {
    let mut counts = std::collections::BTreeMap::<&str, usize>::new();
    for entry in models {
        *counts.entry(entry.info.r#ref.provider.as_str()).or_default() += 1;
    }
    counts.into_iter().map(|(id, models)| ProviderRow { id: id.to_owned(), models }).collect()
}
