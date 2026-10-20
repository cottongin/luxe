//! Luxe TUI — ratatui front-end for the Luxe agent.
//!
//! This crate wraps the headless core in an interactive terminal UI. It is a thin
//! subscriber to the core `Event` stream: it holds no business logic, only rendering
//! state and input handling.

pub mod app;
pub mod auth_wizard;
pub(crate) mod changes;
pub mod clipboard;
pub(crate) mod command_detail;
pub mod commands;
pub mod configuration;
pub mod context_files;
pub mod context_menu;
pub(crate) mod cost_report;
pub mod dashboard;
pub mod editor;
pub mod files;
#[cfg(any(test, feature = "fixture"))]
pub mod fixture;
mod frame_profile;
pub(crate) mod fuzzy;
pub mod github_detail;
pub mod github_panel;
pub mod glyphs;
pub mod graphics;
pub mod guided_review;
pub mod herdr;
pub mod highlight;
pub(crate) mod image_viewer;
pub(crate) mod keys_card;
pub mod launcher;
pub mod main_loop;
pub mod markdown;
pub mod marks;
pub mod mention_flyout;
pub mod mentions;
pub mod model_picker;
pub mod modes;
pub mod mouse;
pub mod notify;
pub(crate) mod option_layer;
pub mod palette;
pub mod permission_prompt;
pub mod picker;
pub(crate) mod pull_request;
pub mod question_prompt;
pub mod ref_provider;
pub mod restack_panel;
pub mod reveal;
pub mod scroll;
pub mod search;
pub mod selection;
pub mod session_picker;
pub(crate) mod slash_args;
pub mod slash_flyout;
pub mod statusline;
pub mod statusline_state;
// How wide a tab is, for every surface that measures a line before drawing it.
pub(crate) mod tabstop;
pub mod terminal;
#[cfg(test)]
pub(crate) mod test_env;
pub mod theme;
pub mod theme_picker;
pub mod thinking_picker;
pub(crate) mod tooling;
pub mod view;
pub mod widgets;
pub mod workspace_dock;
pub mod workspace_layout;
