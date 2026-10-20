//! Auth — credential storage, the provider catalog, OAuth flows, and per-request auth.
//!
//! `luxe_core::auth` defines what a front-end sees; this module is the machinery behind it. The
//! split exists because signing in needs the network and the filesystem, and neither the core nor
//! the UI crates may have those dependencies.
//!
//! - `store` — the `0600`, never-lossy `auth.json`.
//! - [`credential`] — one entry's typed contents.
//! - [`deployments`] — asking an Azure resource what it has actually deployed.
//! - [`config_file`] — the surgical `models.json` writes a login makes (endpoints, defaults).
//! - [`json_file`] — lossless, locked, atomic read-modify-write shared by both files (from
//!   `luxe_core`, which the MCP token store uses too).
//! - [`catalog`] — the curated providers a user can configure, and how each is reached.
//! - `source` — per-request auth: resolve a credential, refresh an expiring token once under a
//!   lock, and hand the adapter the headers (and sometimes the base URL) for this one call.
//! - [`oauth`] — PKCE, the loopback callback, device-code polling, and the subscription flows.
//! - `service` — the [`luxe_core::auth::AuthService`] the front-ends drive.

pub mod catalog;
pub mod config_file;
pub mod credential;
pub mod deployments;
pub use luxe_core::json_file;
pub mod oauth;
mod service;
mod source;
mod store;

pub use catalog::{
    canonical_id, provider_descriptor, provider_descriptors, wire_api_compat_floor,
    ProviderDescriptor, ServedModels,
};
pub use config_file::ConfigFile;
pub use credential::{ApiKeyCredential, Credential, OAuthCredential};
pub use service::ProviderAuth;
pub use source::{AuthSource, KeyStyle, RequestAuth, StaticAuth, StoreAuth};
pub use store::{AuthStore, StoreError};

use std::path::PathBuf;

/// luxe's config directory, where the credential store and `models.json` live. See
/// [`luxe_core::user_dirs::luxe_config_dir`], the one owner.
pub fn config_dir() -> PathBuf {
    luxe_core::user_dirs::luxe_config_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The store and the config loader must agree on where config lives; a mismatch would present as
    /// "I logged in and it didn't take".
    #[test]
    fn the_config_dir_follows_xdg() {
        let previous = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", "/tmp/luxe-xdg-test");
        assert_eq!(config_dir(), PathBuf::from("/tmp/luxe-xdg-test/luxe"));
        assert_eq!(
            AuthStore::default_path().path(),
            PathBuf::from("/tmp/luxe-xdg-test/luxe/auth.json")
        );
        match previous {
            Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
    }
}
