//! Theme system — color tokens, built-in themes, and ratatui Style derivation.
//!
//! This module bridges the theme data model (in `luxe-core::theme`) with the TUI
//! rendering layer. It provides:
//! - [`styles::ThemeStyles`]: pre-computed [`ratatui::style::Style`] for every token,
//!   derived from a resolved theme map via `ThemeStyles::from_resolved`.
//! - Per-token fallback to defaults when a token is missing or unparseable — never panics.

pub mod styles;
