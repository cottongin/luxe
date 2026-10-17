//! Restack panel — what `/stack` would do to the stack's branches, before it does it.
//!
//! Drawn upward from the composer like a question, and modal the same way: Enter pushes exactly the
//! plan on screen, Esc leaves everything as it was. The plan was made by the engine, commits and
//! all, so nothing here recomputes it.

use crossterm::event::{KeyCode, KeyEvent};
use luxe_core::restack::{MergedBelow, RestackId, RestackPlan};
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Clear, Padding, Paragraph, Wrap};
use ratatui::Frame;

use crate::theme::styles::ThemeStyles;

/// Where `/stack` has got to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestackPanel {
    /// Fetching the stack and making the plan
    Planning,
    /// The plan, waiting for Enter
    Planned {
        /// Names the plan, which is what Enter confirms
        request_id: RestackId,
        /// What restacking does
        plan: RestackPlan,
        /// What merged out from under the bottom, when something did
        merged: Option<MergedBelow>,
    },
    /// Pushing the plan
    Pushing,
    /// Why nothing can be done, or what went wrong; nothing changed
    Said(String),
}

/// What a key does to the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelKey {
    /// Close it, changing nothing
    Close,
    /// Push the plan this names
    Push(RestackId),
    /// Nothing, but the panel took the key
    Held,
}

impl RestackPanel {
    /// What `key` does. Every key is the panel's while it is open, so none reaches the composer.
    pub fn on_key(&self, key: &KeyEvent) -> PanelKey {
        match (self, key.code) {
            (Self::Pushing, _) => PanelKey::Held,
            (Self::Planned { request_id, plan, .. }, KeyCode::Enter) if !plan.steps.is_empty() => {
                PanelKey::Push(request_id.clone())
            }
            (_, KeyCode::Esc | KeyCode::Enter) => PanelKey::Close,
            _ => PanelKey::Held,
        }
    }

    /// The panel's lines, without its frame.
    ///
    /// Args: `(styles, frame)` — the spinner's frame, turning while git and GitHub work.
    pub fn lines(&self, styles: &ThemeStyles, frame: usize) -> Vec<Line<'static>> {
        let spinner = crate::statusline::spinner(frame);
        // Branch names are quoted in the text and drawn as inline code, as everywhere else
        let say =
            |text: String, style| Line::from(crate::widgets::code_spans(&text, style, styles));
        match self {
            Self::Planning => {
                vec![say(
                    format!("{spinner} Fetching the stack and working out what moves…"),
                    styles.muted,
                )]
            }
            Self::Pushing => vec![say(format!("{spinner} Pushing the stack…"), styles.muted)],
            Self::Said(text) => vec![say(text.clone(), styles.text)],
            Self::Planned { plan, .. } if plan.steps.is_empty() => vec![say(
                "Every branch sits on the one below it: nothing to restack.".into(),
                styles.text,
            )],
            Self::Planned { plan, merged, .. } => {
                let mut lines = Vec::new();
                if let Some(merged) = merged {
                    lines.push(say(
                        format!(
                            "#{} merged into `{}`, so what sat on it moves there, without its commits.",
                            merged.number, merged.base_ref
                        ),
                        styles.muted,
                    ));
                }
                for step in &plan.steps {
                    lines.push(say(step_line(step), styles.text));
                    if let Some(place) = &step.checked_out {
                        lines.push(say(
                            format!(
                                "    checked out at {}, which moves with it",
                                luxe_core::paths::display(place)
                            ),
                            styles.muted,
                        ));
                    }
                }
                let pushed = plan.steps.iter().filter(|step| step.new != step.old).count();
                if pushed > 0 {
                    lines.push(say(
                        format!(
                            "Force-pushes {pushed} {} to {} in one go, each only if GitHub still has \
                             what was read.",
                            if pushed == 1 { "branch" } else { "branches" },
                            plan.remote
                        ),
                        styles.warning,
                    ));
                }
                lines
            }
        }
    }

    /// The hint along the bottom edge.
    pub fn hint(&self) -> &'static str {
        match self {
            Self::Planned { plan, .. } if !plan.steps.is_empty() => " enter restack · esc cancel ",
            Self::Pushing => " pushing… ",
            _ => " esc close ",
        }
    }
}

/// One branch's move, in words: `#140 feat/threads — 2 commits onto feat/review · retarget to main`.
fn step_line(step: &luxe_core::restack::RestackStep) -> String {
    let mut line = format!("#{} `{}` — ", step.number, step.head_ref);
    if step.new == step.old {
        line.push_str("stays as it is");
    } else {
        let commits =
            if step.commits == 1 { "1 commit".into() } else { format!("{} commits", step.commits) };
        line.push_str(&format!("{commits} onto `{}`", step.onto));
    }
    if let Some(base) = &step.retarget {
        line.push_str(&format!(" · its pull request moves to `{base}`"));
    }
    line
}

/// Rows the panel takes, frame included, at `width`, capped to what is above the composer.
pub fn height(panel: &RestackPanel, styles: &ThemeStyles, width: u16, above: u16) -> u16 {
    let inner = usize::from(width.saturating_sub(4)).max(1);
    let rows: usize = panel
        .lines(styles, 0)
        .iter()
        .map(|line| line.width().max(1).div_ceil(inner))
        .sum::<usize>()
        + 2
        + usize::from(crate::widgets::FOOTER_ROWS);
    u16::try_from(rows).unwrap_or(u16::MAX).min(above)
}

/// Draws the panel upward from the composer at `bottom`.
pub fn render(
    f: &mut Frame,
    region: Rect,
    bottom: u16,
    panel: &RestackPanel,
    styles: &ThemeStyles,
    frame: usize,
) {
    let rows = height(panel, styles, region.width, bottom.saturating_sub(region.y));
    let area = Rect::new(region.x, bottom.saturating_sub(rows), region.width, rows);
    let block =
        crate::widgets::titled_panel(crate::glyphs::Icon::Branch.glyph(), "Restack", styles)
            .padding(Padding::horizontal(1));
    f.render_widget(Clear, area);
    let body = crate::widgets::render_footed(f, block, area, panel.hint(), None, styles);
    f.render_widget(Paragraph::new(panel.lines(styles, frame)).wrap(Wrap { trim: false }), body);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use luxe_core::restack::RestackStep;

    fn step(number: u64, moved: bool, retarget: Option<&str>) -> RestackStep {
        RestackStep {
            number,
            head_ref: format!("b{number}"),
            old: "a".into(),
            new: if moved { "b".into() } else { "a".into() },
            commits: if moved { 2 } else { 0 },
            onto: "below".into(),
            retarget: retarget.map(str::to_string),
            checked_out: None,
            local: false,
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn each_move_is_said_and_only_a_plan_with_moves_pushes() {
        assert_eq!(step_line(&step(140, true, None)), "#140 `b140` — 2 commits onto `below`");
        assert_eq!(
            step_line(&step(141, false, Some("main"))),
            "#141 `b141` — stays as it is · its pull request moves to `main`"
        );
        let plan = RestackPlan { remote: "origin".into(), steps: vec![step(140, true, None)] };
        let request_id = RestackId::new();
        let planned = RestackPanel::Planned { request_id: request_id.clone(), plan, merged: None };
        assert_eq!(planned.on_key(&key(KeyCode::Enter)), PanelKey::Push(request_id));
        assert_eq!(
            planned.on_key(&key(KeyCode::Char('x'))),
            PanelKey::Held,
            "the composer gets nothing"
        );
        assert_eq!(planned.on_key(&key(KeyCode::Esc)), PanelKey::Close);
        let settled = RestackPanel::Planned {
            request_id: RestackId::new(),
            plan: RestackPlan { remote: "origin".into(), steps: Vec::new() },
            merged: None,
        };
        assert_eq!(settled.on_key(&key(KeyCode::Enter)), PanelKey::Close);
        assert_eq!(
            RestackPanel::Pushing.on_key(&key(KeyCode::Esc)),
            PanelKey::Held,
            "not mid-push"
        );
        assert_eq!(settled.hint(), " esc close ");
        assert_eq!(planned.hint(), " enter restack · esc cancel ");
    }

    #[test]
    fn the_plan_says_what_merged_and_what_is_force_pushed() {
        let registry = luxe_core::theme::ThemeRegistry::new();
        let resolved = registry.resolve("dark").expect("dark theme resolves");
        let styles = ThemeStyles::from_resolved(&resolved, "dark");
        let plan = RestackPlan {
            remote: "origin".into(),
            steps: vec![
                step(140, true, Some("main")),
                step(141, true, None),
                step(142, false, Some("x")),
            ],
        };
        let merged = MergedBelow { number: 139, head_oid: "c".into(), base_ref: "main".into() };
        let panel =
            RestackPanel::Planned { request_id: RestackId::new(), plan, merged: Some(merged) };
        let text: Vec<String> =
            panel.lines(&styles, 0).iter().map(|line| line.to_string()).collect();
        assert!(text[0].starts_with("#139 merged into main,"), "drawn as code: {text:?}");
        let first = &panel.lines(&styles, 0)[0];
        let main = first.spans.iter().find(|span| span.content == "main").expect("its own span");
        assert_eq!(main.style, styles.md_code);
        assert!(
            text.last().unwrap().starts_with("Force-pushes 2 branches to origin in one go"),
            "{text:?}"
        );
        let settled = RestackPanel::Planned {
            request_id: RestackId::new(),
            plan: RestackPlan { remote: "origin".into(), steps: Vec::new() },
            merged: None,
        };
        assert!(settled.lines(&styles, 0)[0].to_string().contains("nothing to restack"));
    }

    fn drawn(panel: &RestackPanel) -> Vec<String> {
        let registry = luxe_core::theme::ThemeRegistry::new();
        let resolved = registry.resolve("dark").expect("dark theme resolves");
        let styles = ThemeStyles::from_resolved(&resolved, "dark");
        let backend = ratatui::backend::TestBackend::new(90, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|f| render(f, Rect::new(0, 0, 90, 20), 18, panel, &styles, 0)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..20)
            .map(|y| (0..90).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>())
            .collect()
    }

    /// Drawn upward from the composer, exactly as tall as what it says: the frame, the title, a
    /// row a step, and the footer. A plan that only moves bases pushes nothing, so
    /// it does not say it will.
    #[test]
    fn the_panel_sits_on_the_composer_and_fits_what_it_says() {
        let plan = RestackPlan {
            remote: "origin".into(),
            steps: vec![step(140, true, None), step(141, true, None)],
        };
        let rows =
            drawn(&RestackPanel::Planned { request_id: RestackId::new(), plan, merged: None });
        let top = rows.iter().position(|row| row.contains("Restack")).expect("title");
        let bottom = rows.iter().position(|row| row.contains("enter restack")).expect("hint");
        assert_eq!(bottom, 16, "its last inner row is just above the composer's");
        assert_eq!(
            bottom - top,
            6,
            "two steps and the push line, wrapped, and the frame: {rows:#?}"
        );
        assert!(rows[top + 1].contains(" #140 b140 — 2 commits onto below"), "padded: {rows:#?}");

        let only_bases =
            RestackPlan { remote: "origin".into(), steps: vec![step(142, false, Some("main"))] };
        let rows = drawn(&RestackPanel::Planned {
            request_id: RestackId::new(),
            plan: only_bases,
            merged: None,
        });
        assert!(!rows.iter().any(|row| row.contains("Force-pushes")), "{rows:#?}");
        assert!(!rows.iter().any(|row| row.contains("pushing…")));
        let rows = drawn(&RestackPanel::Pushing);
        assert!(rows.iter().any(|row| row.contains("pushing…")), "{rows:#?}");
        assert!(rows.iter().any(|row| row.contains("Pushing the stack")), "{rows:#?}");
    }
}
