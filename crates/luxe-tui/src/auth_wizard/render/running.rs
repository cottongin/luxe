//! The running step — the transcript of a sign-in in progress, and the prompt it is waiting on.

use super::*;

impl AuthWizardState {
    /// Rows the pending question needs, or zero when nothing is being asked.
    pub(super) fn prompt_height(&self) -> u16 {
        match self.pending_prompt() {
            // A blank line, the question, and what each choice takes.
            Some((prompt, _)) => match &prompt.kind {
                AuthPromptKind::Select(choices) => choices.iter().map(choice_rows).sum::<u16>() + 2,
                // A blank line, the question, and the input row.
                _ => 3,
            },
            None => 0,
        }
    }

    pub(super) fn render_running(
        &self,
        frame: &mut Frame,
        inner: Rect,
        provider: &str,
        lines: &[WizardLine],
        theme: &ThemeStyles,
    ) {
        let prompt = self.pending_prompt();
        let prompt_height = self.prompt_height();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(2),
                Constraint::Length(prompt_height),
            ])
            .split(inner);

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("Signing in to {provider}…"),
                theme.text.add_modifier(Modifier::BOLD),
            ))),
            chunks[0],
        );
        let body = transcript_lines(lines, theme);
        frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: true }), chunks[1]);

        let Some((prompt, input)) = prompt else {
            park_cursor(frame, inner);
            return;
        };
        let mut rows: Vec<Line> = vec![
            Line::from(""),
            Line::from(Span::styled(
                prompt.message.clone(),
                theme.text.add_modifier(Modifier::BOLD),
            )),
        ];
        match &prompt.kind {
            AuthPromptKind::Select(choices) => {
                rows.extend(self.prompt_choice_lines(choices, chunks[2], theme));
            }
            kind => rows.push(prompt_input_line(prompt, kind, input, theme)),
        }
        place_prompt_cursor(frame, chunks[2], &prompt.kind, input);
        frame.render_widget(Paragraph::new(rows), chunks[2]);
    }

    /// A select prompt's choices, the one under the cursor framed, each with the detail the flow
    /// wrote for it.
    fn prompt_choice_lines(
        &self,
        choices: &[luxe_core::auth::AuthChoice],
        area: Rect,
        theme: &ThemeStyles,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        // Under the label, and clear of the cursor column.
        const DETAIL_INDENT: usize = 3;

        let selected = self.selected_row();
        let row_width = usize::from(area.width);
        let room = row_width.saturating_sub(crate::widgets::SELECTION_FRAME);
        for (index, choice) in choices.iter().enumerate() {
            let focused = index == selected;
            let mut content = vec![vec![
                Span::styled(crate::glyphs::Mark::Cursor.gutter(focused), theme.accent_on(focused)),
                Span::styled(choice.label.clone(), theme.label_on(focused)),
            ]];
            // The flow wrote this to be read at the moment of choosing — the headless
            // prompt has always printed it, and dropping it here left the reader
            // picking between three bare names.
            if let Some(detail) = &choice.detail {
                content.push(vec![
                    Span::raw(" ".repeat(DETAIL_INDENT)),
                    Span::styled(
                        crate::widgets::truncate_end(detail, room.saturating_sub(DETAIL_INDENT)),
                        theme.muted,
                    ),
                ]);
            }
            lines.extend(crate::widgets::selection_block(content, row_width, focused, theme));
        }
        lines
    }

    pub(super) fn render_failed(
        &self,
        frame: &mut Frame,
        inner: Rect,
        message: &str,
        theme: &ThemeStyles,
    ) {
        park_cursor(frame, inner);
        let lines = vec![
            Line::from(Span::styled(
                "Sign-in didn't finish",
                theme.error.add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(message.to_string(), theme.text)),
        ];
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
    }
}

/// The sign-in's transcript so far, its last [`MAX_LINES`] lines.
fn transcript_lines(lines: &[WizardLine], theme: &ThemeStyles) -> Vec<Line<'static>> {
    let mut body: Vec<Line<'static>> = Vec::new();
    for line in lines.iter().rev().take(MAX_LINES).rev() {
        match line {
            WizardLine::Info(message) => {
                body.push(Line::from(Span::styled(message.clone(), theme.muted)));
            }
            WizardLine::Progress(message) => {
                body.push(Line::from(vec![
                    Span::styled("· ", theme.accent),
                    Span::styled(message.clone(), theme.text),
                ]));
            }
            WizardLine::Url { url, instructions } => {
                body.push(Line::from(Span::styled(url.clone(), theme.accent)));
                if let Some(instructions) = instructions {
                    body.push(Line::from(Span::styled(instructions.clone(), theme.muted)));
                }
            }
            WizardLine::DeviceCode { code, url, expires_in_minutes } => {
                body.push(Line::from(vec![
                    Span::styled("1. Open ", theme.text),
                    Span::styled(url.clone(), theme.accent),
                ]));
                body.push(Line::from(vec![
                    Span::styled("2. Enter the code ", theme.text),
                    Span::styled(code.clone(), theme.accent.add_modifier(Modifier::BOLD)),
                    Span::styled(format!("  ({expires_in_minutes} min)"), theme.muted),
                ]));
            }
        }
    }
    body
}

/// A text or secret prompt's input row, a secret echoed as dots.
fn prompt_input_line(
    prompt: &luxe_core::auth::AuthPrompt,
    kind: &AuthPromptKind,
    input: &str,
    theme: &ThemeStyles,
) -> Line<'static> {
    // A secret is echoed as dots — long enough to see it registered, never the value.
    let shown = match kind {
        AuthPromptKind::Secret => "•".repeat(input.chars().count()),
        _ => input.to_string(),
    };
    let placeholder =
        prompt.placeholder.as_deref().filter(|_| input.is_empty()).unwrap_or_default();
    Line::from(vec![
        Span::styled("> ", theme.accent),
        Span::styled(shown, theme.text),
        Span::styled(placeholder.to_string(), theme.dim),
        Span::styled("▏", theme.accent),
    ])
}

/// Puts the terminal cursor at the end of what was typed, or parks it for a select.
fn place_prompt_cursor(frame: &mut Frame, area: Rect, kind: &AuthPromptKind, input: &str) {
    let cursor_row = area.y.saturating_add(2);
    match kind {
        AuthPromptKind::Select(_) => park_cursor(frame, area),
        AuthPromptKind::Secret => frame.set_cursor_position((
            area.x
                .saturating_add(2 + input.chars().count() as u16)
                .min(area.right().saturating_sub(1)),
            cursor_row,
        )),
        AuthPromptKind::Text => frame.set_cursor_position((
            area.x
                .saturating_add(2 + UnicodeWidthStr::width(input) as u16)
                .min(area.right().saturating_sub(1)),
            cursor_row,
        )),
    }
}
