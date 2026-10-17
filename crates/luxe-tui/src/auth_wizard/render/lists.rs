//! The wizard's lists — providers, then a provider's methods.

use super::*;

impl AuthWizardState {
    pub(super) fn render_providers(
        &self,
        frame: &mut Frame,
        inner: Rect,
        query: &str,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<HitRegions>,
    ) {
        let framing = self.framing();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(if framing.is_some() { 2 } else { 0 }),
                Constraint::Length(2),
                Constraint::Min(1),
            ])
            .split(inner);

        if let Some(framing) = framing {
            frame.render_widget(
                Paragraph::new(framing).style(theme.muted).wrap(Wrap { trim: true }),
                chunks[0],
            );
        }

        let search = Paragraph::new(Line::from(vec![
            Span::styled("Search: ", theme.accent.add_modifier(Modifier::BOLD)),
            Span::styled(query.to_string(), theme.text),
        ]))
        .block(Block::default().borders(Borders::BOTTOM).border_style(theme.border_muted));
        frame.render_widget(search, chunks[1]);
        let cursor_x = chunks[1]
            .x
            .saturating_add(8)
            .saturating_add(UnicodeWidthStr::width(query) as u16)
            .min(chunks[1].right().saturating_sub(1));
        frame.set_cursor_position((cursor_x, chunks[1].y));

        let visible = self.visible_providers();
        if visible.is_empty() {
            frame.render_widget(
                Paragraph::new("No provider matches that").style(theme.muted),
                chunks[2],
            );
        } else {
            let rows = chunks[2].height as usize;
            let selected = self.selected_row();
            let start = selected.saturating_sub(rows.saturating_sub(1));
            let row_width = usize::from(chunks[2].width);
            // One window drives both the rows drawn and the rows clickable
            let shown: Vec<(usize, &luxe_core::auth::ProviderOption)> = visible
                .iter()
                .enumerate()
                .skip(start)
                .take(rows)
                .map(|(row, (_, option))| (row, *option))
                .collect();
            let items: Vec<ListItem> = shown
                .iter()
                .map(|(row, option)| self.provider_row(option, *row == selected, row_width, theme))
                .collect();
            frame.render_widget(List::new(items), chunks[2]);

            let mut regions = hit.borrow_mut();
            for (offset, _) in &shown {
                let y = chunks[2].y.saturating_add((offset - start) as u16);
                if y < chunks[2].bottom() {
                    regions.push(
                        Rect::new(chunks[2].x, y, chunks[2].width, 1),
                        MouseTarget::AuthProviderRow(*offset),
                    );
                }
            }
        }
    }

    /// One provider's row: its name, given way to the badges that say what kind of way in it is.
    fn provider_row(
        &self,
        option: &luxe_core::auth::ProviderOption,
        focused: bool,
        row_width: usize,
        theme: &ThemeStyles,
    ) -> ListItem<'static> {
        let mut tail = Vec::new();
        if let Some(badge) = status_badge(&option.status) {
            tail.push(Span::styled(format!("  {badge}"), theme.success));
        }
        // Only worth marking in a list that also holds providers without one. A
        // subscription list saying "subscription" four times is a column of noise.
        if self.scope() == LoginScope::Any
            && option
                .methods
                .iter()
                .any(|m| m.kind == luxe_core::auth::AuthMethodKind::Subscription)
        {
            tail.push(Span::styled("  subscription", theme.muted));
        }
        // The badges say what kind of way in this is, so the name gives way to them
        // rather than pushing them off a narrow modal.
        let spent: usize =
            tail.iter().map(|span| UnicodeWidthStr::width(span.content.as_ref())).sum();
        let name = crate::widgets::truncate_end(
            &option.name,
            row_width.saturating_sub(crate::widgets::SELECTION_FRAME + 2 + spent),
        );
        let mut spans = vec![
            Span::styled(crate::glyphs::Mark::Cursor.gutter(focused), theme.accent_on(focused)),
            Span::styled(name, theme.label_on(focused)),
        ];
        spans.extend(tail);
        ListItem::new(crate::widgets::selection_row(spans, row_width, focused, theme))
    }

    pub(super) fn render_methods(
        &self,
        frame: &mut Frame,
        inner: Rect,
        theme: &ThemeStyles,
        hit: &std::cell::RefCell<HitRegions>,
    ) {
        let Some(option) = self.method_provider() else { return };
        // The composer behind the modal owns the cursor otherwise, and it would blink out there
        // while the user is reading a dialog.
        park_cursor(frame, inner);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(if option.note.is_some() { 3 } else { 0 }),
                Constraint::Min(2),
            ])
            .split(inner);

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                option.name.clone(),
                theme.text.add_modifier(Modifier::BOLD),
            ))),
            chunks[0],
        );
        if let Some(note) = &option.note {
            frame.render_widget(
                Paragraph::new(note.clone()).style(theme.muted).wrap(Wrap { trim: true }),
                chunks[1],
            );
        }

        // Under the label: the cursor gutter, then the `1. ` that numbers it.
        const DETAIL_INDENT: usize = 5;

        let selected = self.selected_row();
        let row_width = usize::from(chunks[2].width);
        let room = row_width.saturating_sub(crate::widgets::SELECTION_FRAME);
        let mut lines: Vec<Line> = Vec::new();
        for (index, method) in option.methods.iter().enumerate() {
            let focused = index == selected;
            // A method and what it does to you are one choice, so they are cut as one slab rather
            // than two rows that happen to sit next to each other.
            lines.extend(crate::widgets::selection_block(
                vec![
                    vec![
                        Span::styled(
                            crate::glyphs::Mark::Cursor.gutter(focused),
                            theme.accent_on(focused),
                        ),
                        Span::styled(format!("{}. ", index + 1), theme.dim),
                        Span::styled(method.label.clone(), theme.label_on(focused)),
                    ],
                    vec![
                        Span::raw(" ".repeat(DETAIL_INDENT)),
                        Span::styled(
                            crate::widgets::truncate_end(
                                method.detail.as_deref().unwrap_or_default(),
                                room.saturating_sub(DETAIL_INDENT),
                            ),
                            theme.muted,
                        ),
                    ],
                ],
                row_width,
                focused,
                theme,
            ));
        }
        frame.render_widget(Paragraph::new(lines), chunks[2]);

        let mut regions = hit.borrow_mut();
        for index in 0..option.methods.len() {
            let y = chunks[2].y.saturating_add(index as u16 * METHOD_ROWS);
            if y < chunks[2].bottom() {
                // Both rows, so the line explaining a method is as clickable as its name.
                regions.push(
                    Rect::new(
                        chunks[2].x,
                        y,
                        chunks[2].width,
                        METHOD_ROWS.min(chunks[2].bottom() - y),
                    ),
                    MouseTarget::AuthMethodRow(index),
                );
            }
        }
    }
}
