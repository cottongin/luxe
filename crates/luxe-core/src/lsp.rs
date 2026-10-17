//! Diagnostics — what the `file_changed` hook asks a language server about an edited file.
//!
//! Implementations of [`DiagnosticsProvider`] live in `luxe-lsp`. The [`DiagnosticsStore`] is the
//! shared cache their results flow into for the Context Dock.

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// How serious a diagnostic is, worst first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// The code does not work
    Error,
    /// The code works and is probably wrong
    Warning,
    /// Something worth knowing
    Information,
    /// A suggestion
    Hint,
}

impl Severity {
    /// The word a report uses for it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Information => "info",
            Self::Hint => "hint",
        }
    }
}

/// A diagnostic message from a language server.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// How serious it is; `None` when the server did not say.
    pub severity: Option<Severity>,
    /// Line number (0-indexed).
    pub line: u32,
    /// Character offset (0-indexed).
    pub character: u32,
    /// Diagnostic message.
    pub message: String,
}

/// Why a file's diagnostics could not be produced.
#[derive(Debug, thiserror::Error)]
pub enum DiagnosticsError {
    /// The file could not be read to be sent to the server
    #[error("read {path}: {error}")]
    Read {
        /// The file, as a reader would write it
        path: String,
        /// What the read said
        error: std::io::Error,
    },
}

/// Something that can run diagnostics on a file.
///
/// `&self`: a provider is shared by every caller at once, so one slow read must not hold the rest.
#[async_trait]
pub trait DiagnosticsProvider: Send + Sync {
    /// Runs diagnostics on a file.
    async fn run_diagnostics(&self, path: &Path) -> Result<Vec<Diagnostic>, DiagnosticsError>;
}

/// A clonable, thread-safe cache of the latest diagnostics per file.
///
/// The diagnostics-on-edit hook writes each changed file's results here; the workspace collector
/// reads a snapshot to build a [`crate::workspace::DiagnosticsSummary`] for the Context Dock. This
/// decouples the (core) hook from the (runtime) collector without threading a channel between
/// them. Setting a file to an empty result clears its entry, so fixed files drop out on the next
/// edit. Handles share one underlying map via [`Arc`].
#[derive(Clone, Default)]
pub struct DiagnosticsStore {
    inner: Arc<RwLock<HashMap<PathBuf, Vec<Diagnostic>>>>,
}

impl DiagnosticsStore {
    /// Creates an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces a file's diagnostics; an empty list clears the entry.
    ///
    /// A poisoned lock is recovered rather than propagated — a stale diagnostics cache must never
    /// take down the runtime.
    pub fn set(&self, path: PathBuf, diagnostics: Vec<Diagnostic>) {
        let mut map = self.inner.write().unwrap_or_else(|poison| poison.into_inner());
        if diagnostics.is_empty() {
            map.remove(&path);
        } else {
            map.insert(path, diagnostics);
        }
    }

    /// Snapshots every `(path, diagnostics)` pair for reduction into a summary.
    pub fn snapshot(&self) -> Vec<(PathBuf, Vec<Diagnostic>)> {
        let map = self.inner.read().unwrap_or_else(|poison| poison.into_inner());
        map.iter().map(|(path, diags)| (path.clone(), diags.clone())).collect()
    }

    /// Removes every tracked file, e.g. when switching projects.
    pub fn clear(&self) {
        self.inner.write().unwrap_or_else(|poison| poison.into_inner()).clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diag(severity: Severity) -> Diagnostic {
        Diagnostic { severity: Some(severity), line: 0, character: 0, message: "x".into() }
    }

    #[test]
    fn store_sets_clears_and_snapshots() {
        let store = DiagnosticsStore::new();
        let a = PathBuf::from("/w/a.rs");
        let b = PathBuf::from("/w/b.rs");
        store.set(a.clone(), vec![diag(Severity::Error), diag(Severity::Warning)]);
        store.set(b.clone(), vec![diag(Severity::Warning)]);

        let mut snap = store.snapshot();
        snap.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].0, a);
        assert_eq!(snap[0].1.len(), 2);

        // An empty set clears the file (fixed); a shared handle sees the same map.
        let handle = store.clone();
        handle.set(a, Vec::new());
        assert_eq!(store.snapshot().len(), 1);
        assert_eq!(store.snapshot()[0].0, b);

        store.clear();
        assert!(store.snapshot().is_empty());
    }
}
