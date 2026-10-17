//! Config trust — project commands require the user's explicit consent.

use std::path::Path;

use super::{load_project_config, Config};

impl Config {
    /// Load standard config layers, ignoring project-supplied gate commands.
    pub fn load(repo_root: &Path) -> Self {
        Self::load_trusted(repo_root, false)
    }
}

pub(super) fn project_config(repo_root: &Path, trust_project: bool) -> Option<Config> {
    let mut project = load_project_config(repo_root)?;
    if !trust_project && !project.gate.commands.is_empty() {
        tracing::warn!(
            path = %repo_root.join(".luxe/config.json").display(),
            "ignoring project gate.commands; use --trust-project to allow them"
        );
        project.gate.commands.clear();
    }
    Some(project)
}
