//! Test environment — process-global environment variables, held and restored.
//!
//! A test that repoints `XDG_CONFIG_HOME` or fakes a terminal changes the whole process, so it
//! takes one lock and puts everything back when it is dropped, even on a failed assertion. Safe
//! either way under nextest's process-per-test; this is what keeps it right under `cargo test`.

use std::ffi::OsString;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Variables set for one test, and what they were before.
pub(crate) struct EnvGuard {
    saved: Vec<(String, Option<OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    /// Sets each `(name, value)`; `None` removes the variable.
    pub(crate) fn set(vars: &[(&str, Option<&str>)]) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let mut saved = Vec::new();
        for (name, value) in vars {
            saved.push(((*name).to_string(), std::env::var_os(name)));
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
        Self { saved, _lock: lock }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (name, value) in self.saved.drain(..).rev() {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
    }
}

/// The config home pointed at a directory of the test's own, until dropped.
///
/// `LUXE_CONFIG_DIR` outranks `XDG_CONFIG_HOME`, so it is cleared too: a developer who has it set
/// would otherwise have a test write over the real config.
pub(crate) struct TempConfigHome {
    _env: EnvGuard,
}

impl TempConfigHome {
    /// Points the config home at `dir`.
    pub(crate) fn at(dir: &Path) -> Self {
        let dir = dir.to_str().expect("a temp dir is UTF-8");
        Self { _env: EnvGuard::set(&[("XDG_CONFIG_HOME", Some(dir)), ("LUXE_CONFIG_DIR", None)]) }
    }
}
