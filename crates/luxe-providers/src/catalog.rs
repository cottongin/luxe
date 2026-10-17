//! Model catalog — what Luxe knows about a model, and where it learned it.
//!
//! A model released after the binary was built still has to work properly: the right context
//! window, output cap, reasoning levels, and pricing, not silent fallbacks. Metadata therefore
//! comes from four layers — build-time [`snapshot`](mod@snapshot), refreshed [`fetch`](mod@fetch)
//! cache, and the user's config on top of provider-aware defaults — merged by [`resolve`] into the
//! single [`ModelInfo`](luxe_core::provider::ModelInfo) the engine runs on.
//!
//! Everything here is offline-safe: resolution never touches the network, and the refresh is a
//! best-effort background call that a missing network simply skips.

pub mod alias;
pub mod fetch;
mod resolve;
pub mod schema;
pub mod snapshot;
pub mod store;

pub use fetch::{fetch, Fetched, MODELS_DEV_API_URL};
pub use resolve::{resolve, Layer, Provenance, Sources, DEFAULT_CONTEXT_WINDOW};
pub use schema::{parse, Catalog, Model};
pub use snapshot::snapshot;
