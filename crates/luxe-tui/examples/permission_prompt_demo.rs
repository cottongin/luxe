//! Visual demo for the permission-ask prompt (dev-only; not shipped).
//!
//! Renders the yellow permission picker above a mock composer so the styling/layout can be
//! eyeballed or screenshotted. Run: `cargo run -p luxe-tui --example permission_prompt_demo`.

use std::cell::RefCell;
use std::io;

use crossterm::{execute, terminal};
use ratatui::{backend::CrosstermBackend, layout::Rect, widgets::Paragraph, Terminal};

use luxe_core::permission::PermissionRequestId;
use luxe_tui::mouse::HitRegions;
use luxe_tui::permission_prompt::{render_permission_prompt, PermissionPromptState};
use luxe_tui::theme::styles::ThemeStyles;

fn main() -> io::Result<()> {
    let registry = luxe_core::theme::ThemeRegistry::new();
    let resolved = registry.resolve("dark").expect("dark theme");
    let theme = ThemeStyles::from_resolved(&resolved, "dark");
    let state = PermissionPromptState::new(
        PermissionRequestId::new(),
        "bash".to_string(),
        "Run: rm -rf /etc".to_string(),
        vec!["bash rm *".to_string()],
    );
    let hit = RefCell::new(HitRegions::default());

    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, terminal::EnterAlternateScreen)?;
    let mut term = Terminal::new(CrosstermBackend::new(stdout))?;
    term.draw(|f| {
        let size = f.area();
        f.render_widget(ratatui::widgets::Block::default().style(theme.background), size);
        // Mock composer (rounded) at the bottom, matching the real chat layout's inset.
        let composer = Rect::new(1, size.height.saturating_sub(4), size.width.saturating_sub(2), 3);
        f.render_widget(
            Paragraph::new("  ask luxe to do something…")
                .style(theme.muted)
                .block(luxe_tui::widgets::panel_block().border_style(theme.border_accent)),
            composer,
        );
        let region = Rect::new(composer.x, 1, composer.width, composer.y.saturating_sub(1));
        render_permission_prompt(f, region, composer.y, &state, &theme, &hit);
    })?;
    // Hold the frame so a screenshot can capture it, then restore the terminal.
    std::thread::sleep(std::time::Duration::from_secs(6));
    terminal::disable_raw_mode()?;
    execute!(term.backend_mut(), terminal::LeaveAlternateScreen)?;
    Ok(())
}
