//! Surfaces — the start screen and the Configuration surface, which own the whole frame.

use super::input;
use crate::app::App;
use crate::workspace_dock::DockPresentation;
use crate::workspace_layout::gutter_inset;
use ratatui::{layout::Rect, text::Span, widgets::Paragraph, Frame};

/// Draw the login wizard, if one is open.
///
/// The chat's `/login` draws it as what it is there: a modal over the conversation. The
/// Configuration surface draws it itself, in the region its own panel would have occupied.
pub(super) fn render_auth_wizard(f: &mut Frame, size: ratatui::layout::Rect, app: &App) {
    if let Some(wizard) = app.overlays.auth_wizard.as_ref() {
        wizard.view(f, size, &app.theme_styles, &app.hit);
    }
}

/// Draw the Configuration surface: the logotype, and one panel under it.
///
/// Whatever the surface shows — its menu, the login wizard a row opened, or the model picker login
/// handed off to — gets the same region, so the reader never looks at two surfaces at once.
pub(super) fn render_configuration(f: &mut Frame, size: ratatui::layout::Rect, app: &App) {
    let Some(configuration) = app.overlays.configuration.as_ref() else { return };
    configuration.note_drawn(crate::configuration::fits(size));
    if !crate::configuration::fits(size) {
        let settings = configuration.settings();
        crate::configuration::render_too_small(f, size, &app.theme_styles, settings);
        return;
    }
    let theme = &app.theme_styles;
    if let Some(wizard) = app.overlays.auth_wizard.as_ref() {
        let surface = crate::configuration::frame(size, wizard.desired_height());
        crate::configuration::render_banner(f, surface, theme, app.banner);
        wizard.view_in(f, surface.panel, theme, &app.hit);
        return;
    }
    if let Some(picker) = app.overlays.model_picker.as_ref() {
        let surface = crate::configuration::frame(size, picker.desired_height(size));
        crate::configuration::render_banner(f, surface, theme, app.banner);
        picker.view_in(f, surface.panel, theme, &app.hit);
        return;
    }
    configuration.view(f, size, theme, &app.hit, app.banner);
    // The settings' theme row opens the picker over the surface, which repaints under it as the
    // cursor moves — the preview is the point of it
    if let Some(picker) = app.overlays.theme_picker.as_ref() {
        picker.view(f, size, theme, &app.hit);
    }
}

/// Draw the start screen: the wordmark, the session's own composer as its prompt, the list joined
/// under it, and the statusline the session will have.
///
/// The composer is the one a session draws — same widget, same wrapping, same title — so sending
/// from here does not change what the prompt is, only what is around it.
pub(super) fn render_dashboard(f: &mut Frame, size: Rect, app: &mut App) {
    if app.dashboard.is_none() {
        return;
    }
    // No dock here to carry the project and the model, so the bar does unless it was turned off
    let statusline = app.statusline_state.visible(DockPresentation::Hidden) && size.height >= 12;
    let body = Rect { height: size.height - u16::from(statusline), ..size };
    let width = crate::dashboard::DashboardState::input_width(body);
    let commands = crate::commands::CommandRegistry::shared();
    let marks = crate::slash_args::marks(app.editor.current_input(), commands);
    let layout = app.editor.layout_marked(input::text_width(width), &marks);
    let rows = input::height(&layout, body.height);
    let border = crate::modes::accent(&app.status.mode, &app.theme_styles);
    let flyout = app.overlays.slash_flyout.is_some() || app.overlays.mention.is_some();
    let mode_switch = app.mode_switch_key();
    if let Some(dashboard) = app.dashboard.as_mut() {
        dashboard.set_flyout(flyout);
        dashboard.set_mode_switch(mode_switch);
    }
    let Some(dashboard) = app.dashboard.as_ref() else { return };
    let composer = dashboard.view(f, body, &app.theme_styles, &app.hit, app.banner, rows, border);
    input::composer(f, composer, app, &layout);
    if let Some(dashboard) = app.dashboard.as_ref() {
        dashboard.finish(f, composer, &app.theme_styles, border);
        // An empty prompt says what it is for, past the caret, in the quietest ink there is
        if app.editor.current_input().is_empty() && composer.width > 4 && composer.height > 2 {
            let words = match dashboard.provider_configured() {
                true => "Ask anything, or type to find a session or a command",
                false => "Set up a provider, then ask anything",
            };
            let at = Rect::new(composer.x + 2, composer.y + 1, composer.width - 4, 1);
            f.render_widget(Paragraph::new(Span::styled(words, app.theme_styles.dim)), at);
        }
    }
    if statusline {
        let bar = Rect { y: size.bottom() - 1, height: 1, ..size };
        crate::statusline::render(f, gutter_inset(bar, size), app);
    }
    // The `/` and `@` flyouts the session grows up from its composer, grown down from this one:
    // here the prompt sits high, and the room is under it
    let below = Rect {
        y: composer.bottom(),
        height: body.bottom().saturating_sub(composer.bottom()),
        ..composer
    };
    let tinted = crate::modes::styles(&app.status.mode, &app.theme_styles);
    if let Some(ref flyout) = app.overlays.slash_flyout {
        crate::slash_flyout::render_slash_flyout(
            f,
            below,
            crate::picker::Edge::Below,
            flyout,
            &tinted,
            &app.hit,
        );
    } else if let Some(ref mention) = app.overlays.mention {
        crate::mention_flyout::render_mention_flyout(
            f,
            below,
            crate::picker::Edge::Below,
            mention,
            &tinted,
            &app.hit,
            app.status.spinner_frame,
        );
    }
}
