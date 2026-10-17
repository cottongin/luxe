//! command_detail — what the palette and the slash flyout say about the command under the cursor.
//!
//! A list row has room for a name and nothing else, so everything a reader might want before
//! running something lives here: its keys, what happens, the rest of the story, and the argument
//! forms. Both surfaces place these lines in a pane beside their list; neither builds its own.

use ratatui::{
    style::Modifier,
    text::{Line, Span},
};

use crate::commands::MenuEntry;
use crate::theme::styles::ThemeStyles;

/// The detail pane's lines for `entry`, unwrapped: the pane wraps them to its own width.
pub(crate) fn lines(entry: &MenuEntry, theme: &ThemeStyles) -> Vec<Line<'static>> {
    let command = &entry.command;
    let mut lines = vec![Line::from(Span::styled(
        command.name.to_string(),
        theme.text.add_modifier(Modifier::BOLD),
    ))];
    if let Some(keys) = keys_line(entry, theme) {
        lines.push(keys);
    }
    lines.push(Line::default());
    lines.push(Line::from(crate::widgets::code_spans(&command.summary, theme.text, theme)));
    if !command.detail.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(crate::widgets::code_spans(&command.detail, theme.muted, theme)));
    }
    lines.extend(usage_lines(entry, theme));
    lines
}

/// The slash flyout's shorter pane: what it does and its argument forms.
///
/// The row beside it already carries the name and the trigger, and the flyout is the quick door,
/// so the rest of the story is left to the palette.
pub(crate) fn brief_lines(entry: &MenuEntry, theme: &ThemeStyles) -> Vec<Line<'static>> {
    let mut lines =
        vec![Line::from(crate::widgets::code_spans(&entry.command.summary, theme.text, theme))];
    lines.extend(usage_lines(entry, theme));
    lines
}

/// A blank line, `Usage`, then each form in the code style with its gloss after it.
fn usage_lines(entry: &MenuEntry, theme: &ThemeStyles) -> Vec<Line<'static>> {
    let usage = &entry.command.usage;
    if usage.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![
        Line::default(),
        Line::from(Span::styled("Usage", theme.muted.add_modifier(Modifier::BOLD))),
    ];
    for (form, gloss) in usage {
        let mut spans = vec![Span::raw("  "), Span::styled(form.to_string(), theme.md_code)];
        if !gloss.is_empty() {
            spans.push(Span::styled(format!("  {gloss}"), theme.muted));
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// The one line a list too narrow for a pane keeps: what the command does.
pub(crate) fn summary_line(entry: &MenuEntry, theme: &ThemeStyles) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    spans.extend(crate::widgets::code_spans(&entry.command.summary, theme.muted, theme));
    Line::from(spans)
}

/// The other ways to run it: `/diff · Ctrl+G`, the key in the accent the way every hint draws one.
fn keys_line(entry: &MenuEntry, theme: &ThemeStyles) -> Option<Line<'static>> {
    let mut spans = Vec::new();
    if let Some(slash) = entry.command.slash_name() {
        spans.push(Span::styled(slash.to_owned(), theme.muted));
    }
    if let Some(chord) = entry.chord.as_deref() {
        if !spans.is_empty() {
            spans.push(Span::styled(" · ", theme.muted));
        }
        spans.push(Span::styled(chord.to_owned(), theme.accent));
    }
    (!spans.is_empty()).then(|| Line::from(spans))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{CommandContext, CommandRegistry};
    use luxe_core::keymap::KeyMap;

    fn theme() -> ThemeStyles {
        let resolved = luxe_core::theme::ThemeRegistry::new().resolve("nord").expect("nord");
        ThemeStyles::from_resolved(&resolved, "nord")
    }

    fn entry(name: &str) -> MenuEntry {
        CommandRegistry::all()
            .arrange(&CommandContext::default(), &KeyMap::defaults(), &[])
            .entries
            .into_iter()
            .find(|entry| entry.command.name == name)
            .unwrap_or_else(|| panic!("{name} is listed"))
    }

    fn text(lines: &[Line]) -> Vec<String> {
        lines.iter().map(ToString::to_string).collect()
    }

    /// Name, then the other ways in, then what it does, the rest of it, and every argument form.
    #[test]
    fn the_pane_reads_name_keys_summary_detail_usage() {
        let theme = theme();
        let lines = lines(&entry("Review changes"), &theme);
        let text = text(&lines);
        assert_eq!(text[0], "Review changes");
        assert_eq!(text[1], "/diff · ctrl+g");
        assert_eq!(text[2], "");
        assert!(text[3].starts_with("Opens a Guided Review"), "{text:?}");
        assert!(text.iter().any(|line| line == "Usage"));
        assert!(text.iter().any(|line| line.contains("/diff pr <number or URL>")));
        assert!(text.iter().any(|line| line == "  /diff turn  the last turn"), "{text:?}");
        assert!(lines[0].spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(lines[1].spans[2].style, theme.accent, "the key reads as a key");
    }

    /// A command with no trigger, no key, no detail and no arguments is its name and its sentence.
    #[test]
    fn a_plain_command_says_only_what_it_has() {
        let theme = theme();
        let text = text(&lines(&entry("Swap the sidebar's side"), &theme));
        assert_eq!(text.len(), 3, "{text:?}");
        assert_eq!(text[1], "");
    }

    /// Inline code in the prose is drawn as code, without its backticks.
    #[test]
    fn code_in_the_prose_is_drawn_as_code() {
        let theme = theme();
        let lines = lines(&entry("New session"), &theme);
        let detail =
            lines.iter().find(|line| line.to_string().contains("luxe -c")).expect("detail");
        assert!(!detail.to_string().contains('`'));
        assert!(detail.spans.iter().any(|span| span.style == theme.md_code));
        assert!(summary_line(&entry("Tools"), &theme).to_string().starts_with(" Every tool"));
    }

    /// The flyout's pane skips what its row already says and keeps what you need to type next.
    #[test]
    fn the_brief_pane_is_what_it_does_and_how_to_give_it_arguments() {
        let theme = theme();
        let text = text(&brief_lines(&entry("Context Dock"), &theme));
        assert_eq!(text[0], "Shows or hides the sidebar.");
        assert!(!text.iter().any(|line| line == "Context Dock"), "the row says the name");
        assert!(text.iter().any(|line| line == "  /dock left"), "{text:?}");
        assert_eq!(text_of_brief("Tools").len(), 1, "no arguments, no usage");
    }

    fn text_of_brief(name: &str) -> Vec<String> {
        text(&brief_lines(&entry(name), &theme()))
    }
}
