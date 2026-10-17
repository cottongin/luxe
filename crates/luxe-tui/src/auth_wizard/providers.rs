//! Provider and method selection — the keys on the first two steps, filtering and choosing.

use super::*;

impl AuthWizardState {
    pub(super) fn provider_key(&mut self, key: &KeyEvent) -> WizardOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => WizardOutcome::Close,
            KeyCode::Up | KeyCode::Char('p') if ctrl || key.code == KeyCode::Up => {
                self.move_selection(-1);
                WizardOutcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('n') if ctrl || key.code == KeyCode::Down => {
                self.move_selection(1);
                WizardOutcome::Consumed
            }
            KeyCode::Enter => {
                let Stage::Provider { filtered, selected, .. } = &self.stage else {
                    return WizardOutcome::Consumed;
                };
                match filtered.get(*selected).copied() {
                    Some(index) => self.enter_provider(index).unwrap_or(WizardOutcome::Consumed),
                    None => WizardOutcome::Consumed,
                }
            }
            KeyCode::Backspace => {
                if let Stage::Provider { query, .. } = &mut self.stage {
                    query.pop();
                }
                self.refilter();
                WizardOutcome::Consumed
            }
            KeyCode::Char('u') if ctrl => {
                if let Stage::Provider { query, .. } = &mut self.stage {
                    query.clear();
                }
                self.refilter();
                WizardOutcome::Consumed
            }
            KeyCode::Char(ch) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                if let Stage::Provider { query, .. } = &mut self.stage {
                    query.push(ch);
                }
                self.refilter();
                WizardOutcome::Consumed
            }
            _ => WizardOutcome::Consumed,
        }
    }

    pub(super) fn method_key(&mut self, key: &KeyEvent) -> WizardOutcome {
        let Stage::Method { provider, selected } = &mut self.stage else {
            return WizardOutcome::Consumed;
        };
        let provider_index = *provider;
        let count = self.providers.get(provider_index).map(|o| o.methods.len()).unwrap_or(0);
        match key.code {
            // Back to the list rather than out of the wizard: one step at a time.
            KeyCode::Esc | KeyCode::Left => {
                let filtered = (0..self.providers.len()).collect();
                self.stage =
                    Stage::Provider { query: String::new(), filtered, selected: provider_index };
                WizardOutcome::Consumed
            }
            KeyCode::Up => {
                *selected = selected.saturating_sub(1);
                WizardOutcome::Consumed
            }
            KeyCode::Down => {
                if count > 0 {
                    *selected = (*selected + 1).min(count - 1);
                }
                WizardOutcome::Consumed
            }
            KeyCode::Enter | KeyCode::Right => {
                let row = *selected;
                let Some(option) = self.providers.get(provider_index) else {
                    return WizardOutcome::Consumed;
                };
                let Some(method) = option.methods.get(row) else {
                    return WizardOutcome::Consumed;
                };
                let outcome =
                    WizardOutcome::Start { provider: option.id.clone(), method: method.id.clone() };
                self.stage = Stage::Running { provider: option.name.clone(), lines: Vec::new() };
                outcome
            }
            KeyCode::Char(ch) if ch.is_ascii_digit() => {
                let row = (ch as usize).wrapping_sub('1' as usize);
                if row < count {
                    *selected = row;
                }
                WizardOutcome::Consumed
            }
            _ => WizardOutcome::Consumed,
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if let Stage::Provider { filtered, selected, .. } = &mut self.stage {
            if filtered.is_empty() {
                *selected = 0;
                return;
            }
            let last = filtered.len() - 1;
            *selected = match delta {
                d if d < 0 => selected.saturating_sub(1),
                _ => (*selected + 1).min(last),
            };
        }
    }

    /// Re-rank the provider list against the filter.
    ///
    /// Ranked, not merely filtered: typing `cop` should put GitHub Copilot first, and the shared
    /// matcher is what every other picker uses.
    pub(super) fn refilter(&mut self) {
        let keys: Vec<String> = self.providers.iter().map(|option| option.search_key()).collect();
        let Stage::Provider { query, filtered, selected } = &mut self.stage else { return };
        let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();
        *filtered = crate::fuzzy::rank(query, &borrowed).into_iter().map(|hit| hit.idx).collect();
        *selected = (*selected).min(filtered.len().saturating_sub(1));
    }

    /// Move the provider cursor to a clicked row (a filtered position).
    pub fn select_provider(&mut self, row: usize) {
        if let Stage::Provider { filtered, selected, .. } = &mut self.stage {
            if !filtered.is_empty() {
                *selected = row.min(filtered.len() - 1);
            }
        }
    }

    /// Move the method cursor to a clicked row, or a select-prompt cursor when one is up.
    pub fn select_method(&mut self, row: usize) {
        if let Some(pending) = self.pending.as_mut() {
            let choices = pending.choices().len();
            if choices > 0 {
                pending.selected = row.min(choices - 1);
                return;
            }
        }
        if let Stage::Method { provider, selected } = &mut self.stage {
            let count = self.providers.get(*provider).map(|o| o.methods.len()).unwrap_or(0);
            if count > 0 {
                *selected = row.min(count - 1);
            }
        }
    }

    /// Rows currently visible in the provider list, as `(index, option)`.
    pub fn visible_providers(&self) -> Vec<(usize, &ProviderOption)> {
        match &self.stage {
            Stage::Provider { filtered, .. } => filtered
                .iter()
                .filter_map(|index| self.providers.get(*index).map(|o| (*index, o)))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The provider whose method list is showing.
    pub fn method_provider(&self) -> Option<&ProviderOption> {
        match &self.stage {
            Stage::Method { provider, .. } => self.providers.get(*provider),
            _ => None,
        }
    }
}
