//! luxe-providers — LLM provider adapters.
//!
//! Hand-rolled thin adapters for OpenAI-compatible APIs, Anthropic, and Gemini.
//! Depends on `luxe-core` for the `Provider` trait. [`catalog`] resolves what luxe knows
//! about a model — layering the embedded models.dev snapshot, the refreshed cache, and the
//! user's config — so every entry point runs on the same metadata. [`auth`] owns credentials:
//! the curated provider catalog, the `0600` store, the OAuth flows, and the per-request auth the
//! adapters resolve on every call.

pub mod anthropic;
pub mod auth;
pub mod catalog;
mod chat_template;
pub mod config;
pub mod copilot;
mod http;
mod learned;
pub mod openai_compat;
pub mod openai_responses;

pub mod wire;

mod sampling;
mod sse;
mod wire_json;

#[cfg(test)]
mod provider_errors_tests;
#[cfg(test)]
mod test_server;
