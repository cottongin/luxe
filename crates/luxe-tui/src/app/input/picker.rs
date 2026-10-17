//! Picker overlays — the modal lists drawn over the conversation.
//!
//! The command palette and the model, theme and thinking pickers. Each returns an outcome rather
//! than mutating the app itself, so applying it stays in one place.

use crossterm::event::KeyEvent;

use super::super::{commands, App};

/// Answers whether this layer took the key.
pub(super) fn picker_took(app: &mut App, key: KeyEvent) -> bool {
    if app.overlays.palette.is_some() {
        palette_key(app, key);
        return true;
    }
    if app.overlays.context_files.is_some() {
        context_files_key(app, key);
        return true;
    }
    if app.overlays.model_picker.is_some() {
        model_picker_key(app, key);
        return true;
    }
    if app.overlays.theme_picker.is_some() {
        theme_picker_key(app, key);
        return true;
    }
    if app.overlays.thinking_picker.is_some() {
        thinking_picker_key(app, key);
        return true;
    }
    false
}

/// The palette's answer to a key: run the command chosen, or close.
fn palette_key(app: &mut App, key: KeyEvent) {
    let Some(palette) = app.overlays.palette.as_mut() else {
        return;
    };
    use crate::palette::PaletteOutcome;
    match palette.on_key(&key) {
        PaletteOutcome::Dispatch(command) => {
            app.overlays.palette = None;
            commands::dispatch(app, *command);
        }
        PaletteOutcome::Close => {
            app.overlays.palette = None;
        }
        PaletteOutcome::Consumed => {}
    }
}

/// The context files list's answer to a key: open a file to edit, or close.
fn context_files_key(app: &mut App, key: KeyEvent) {
    let Some(files) = app.overlays.context_files.as_mut() else {
        return;
    };
    use crate::context_files::ContextFilesOutcome;
    match files.on_key(&key) {
        // Opening means leaving: `apply_route_action` turns this into a `TuiExit` and the CLI
        // runs the editor with the terminal back in its own hands.
        ContextFilesOutcome::EditFile(path) => {
            app.overlays.context_files = None;
            app.outbox.editor_file = Some(path);
        }
        ContextFilesOutcome::Close => {
            app.overlays.context_files = None;
        }
        ContextFilesOutcome::Consumed => {}
    }
}

/// The model picker's answer to a key: switch model, or close.
fn model_picker_key(app: &mut App, key: KeyEvent) {
    let Some(picker) = app.overlays.model_picker.as_mut() else {
        return;
    };
    use crate::model_picker::ModelPickerOutcome;
    match picker.on_key(&key) {
        ModelPickerOutcome::Select(info) => {
            app.overlays.model_picker = None;
            app.apply_model_selection(*info);
            // Over the Configuration surface this was the last step of setting a provider up,
            // so choosing finishes it rather than returning to the menu that started it.
            if app.current_view == crate::app::ViewMode::Configuration {
                app.close_configuration();
            }
        }
        ModelPickerOutcome::Close => {
            app.overlays.model_picker = None;
        }
        ModelPickerOutcome::Consumed => {}
    }
}

/// The theme picker's answer to a key: preview, choose, or cancel back to the theme it opened on.
fn theme_picker_key(app: &mut App, key: KeyEvent) {
    let Some(picker) = app.overlays.theme_picker.as_mut() else {
        return;
    };
    use crate::theme_picker::ThemePickerOutcome;
    match picker.on_key(&key) {
        ThemePickerOutcome::Preview(theme) => {
            app.preview_theme(&theme);
        }
        ThemePickerOutcome::Select(theme) => {
            app.overlays.theme_picker = None;
            app.apply_theme_selection(&theme);
            app.refresh_settings();
        }
        ThemePickerOutcome::Cancel(theme) => {
            app.overlays.theme_picker = None;
            app.preview_theme(&theme);
        }
        ThemePickerOutcome::Consumed => {}
    }
}

/// The thinking picker's answer to a key: choose a level, or close.
fn thinking_picker_key(app: &mut App, key: KeyEvent) {
    let Some(picker) = app.overlays.thinking_picker.as_mut() else {
        return;
    };
    use crate::thinking_picker::ThinkingPickerOutcome;
    match picker.on_key(&key) {
        ThinkingPickerOutcome::Select(level) => {
            app.overlays.thinking_picker = None;
            app.apply_thinking_selection(level);
        }
        ThinkingPickerOutcome::Close => {
            app.overlays.thinking_picker = None;
        }
        ThinkingPickerOutcome::Consumed => {}
    }
}
