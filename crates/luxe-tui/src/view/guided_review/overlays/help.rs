//! Review help — the key reference, and the source picker.

use super::overlay_block;
use crate::app::App;
use crate::glyphs::Icon;
use crate::guided_review::ReviewSourcePickerSelection;
use crate::keys_card::HelpGroup;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

/// Panel label, shared by the rendered title and the popup-width computation.
const HELP_TITLE: &str = "Help · Guided Review";

/// The review's key card, drawn by the same renderer as the chat's.
pub(in crate::view::guided_review) fn render_help(frame: &mut Frame, area: Rect, app: &App) {
    crate::keys_card::render(
        frame,
        area,
        HELP_TITLE,
        &help_groups(app),
        " ? close ",
        None,
        &app.theme_styles,
    );
}

/// Every key the review answers to, grouped for finding: moving, reading, feedback, and the
/// session. A key someone may rebind is named from the keymap, so the card says what is bound
/// rather than what shipped.
fn help_groups(app: &App) -> Vec<HelpGroup> {
    use luxe_core::keymap::UiAction;
    let chord = |action, fallback| super::super::chord(app, action, fallback);
    // A shifted letter reads as the capital the review's other keys are written in: `E`, like `N`
    let letter = |action, fallback| {
        let bound = chord(action, fallback);
        match bound.strip_prefix("shift+").filter(|key| key.chars().count() == 1) {
            Some(key) => key.to_uppercase(),
            None => bound,
        }
    };
    let row = |keys: &str, words: &str| (keys.to_owned(), words.to_owned());
    // A pull request's comments go to GitHub, so its review finishes there rather than with the
    // agent
    let pull_request = crate::app::github_post_is_pull_request(app);
    let mut feedback = vec![
        row(
            "c",
            if pull_request {
                "comment, posted with the review"
            } else {
                "comment, for the agent to act on"
            },
        ),
        row(
            "a",
            if pull_request {
                "ask the agent, which stays here"
            } else {
                "ask; the agent answers, never edits"
            },
        ),
        (
            chord(UiAction::ReviewFindings, "alt+c"),
            if pull_request {
                "all comments: reply, verify".to_owned()
            } else {
                "all comments: send, reply, verify".to_owned()
            },
        ),
        row("e  x", "edit, delete a draft"),
        row("r", "resolve or verify"),
        (chord(UiAction::ReviewReanchorFinding, "alt+r"), "re-anchor a stale comment".to_owned()),
    ];
    if pull_request {
        feedback.push(row("o", "open on GitHub"));
    }
    vec![
        (
            "Move",
            vec![
                row("space  n", "next stop; past the last, finish"),
                row("N", "previous stop"),
                row("[  ]", "previous, next chapter"),
                row("ctrl+↑/↓", "previous, next file"),
                row("↑/↓", "line"),
                row("pgup/pgdn", "page"),
                row("ctrl+u/d", "half a page"),
                row("←/→", "scroll sideways"),
                row("home", "scroll back to the left"),
                row("/", "find"),
            ],
        ),
        (
            "Read",
            vec![
                (chord(UiAction::ReviewToggleGuide, "g"), "show or hide the guide".to_owned()),
                row("z", "show or hide the prose"),
                (
                    chord(UiAction::ReviewToggleSplit, "ctrl+b"),
                    "side-by-side or unified".to_owned(),
                ),
                row("w", "wrap long lines"),
                row("f  d", "open collapsed churn, say why"),
                (
                    letter(UiAction::ReviewOpenInEditor, "E"),
                    "open the file in your editor".to_owned(),
                ),
            ],
        ),
        ("Feedback", feedback),
        (
            "Session",
            vec![
                row("s", "change the source"),
                (chord(UiAction::ReviewRefresh, "ctrl+r"), "refresh".to_owned()),
                (
                    chord(UiAction::ReviewFinish, "alt+enter"),
                    if pull_request { "post the review to GitHub" } else { "finish" }.to_owned(),
                ),
                row("esc", "back to chat"),
            ],
        ),
    ]
}

pub(in crate::view::guided_review) fn render_source_picker(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    selection: ReviewSourcePickerSelection,
) {
    let width = area.width.clamp(28, 58);
    let height = area.height.clamp(7, 11);
    let popup = Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y.saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    );
    let last_turn = if app.review.last_turn.is_some() {
        "compare the latest completed turn"
    } else {
        "unavailable — no completed turn"
    };
    let rows = [
        (ReviewSourcePickerSelection::WorkingTree, "Working Tree", "capture current changes"),
        (ReviewSourcePickerSelection::LastTurn, "Last Turn", last_turn),
        (ReviewSourcePickerSelection::Base, "Base", "enter a local reference"),
        (ReviewSourcePickerSelection::Commit, "Commit", "enter an object identifier"),
    ];
    let mut lines = rows
        .into_iter()
        .map(|(kind, label, detail)| {
            let unavailable =
                kind == ReviewSourcePickerSelection::LastTurn && app.review.last_turn.is_none();
            let marker = if kind == selection {
                if unavailable {
                    "×"
                } else {
                    "›"
                }
            } else {
                " "
            };
            let label_style =
                if unavailable { app.theme_styles.muted } else { app.theme_styles.accent };
            Line::from(vec![
                Span::styled(format!("{marker} {label:<13}"), label_style),
                Span::styled(detail, app.theme_styles.muted),
            ])
        })
        .collect::<Vec<_>>();
    if let Some(entry) = app.guided_review_workspace.source_reference_entry.as_ref() {
        let label = match entry.kind {
            crate::guided_review::ReviewSourceReferenceKind::Base => "Base reference",
            crate::guided_review::ReviewSourceReferenceKind::Commit => "Commit object ID",
        };
        let value = if entry.value.is_empty() { "_" } else { entry.value.as_str() };
        lines.push(Line::from(vec![
            Span::styled(format!("{label}: "), app.theme_styles.text),
            Span::styled(value, app.theme_styles.accent),
        ]));
        if let Some(error) = entry.error {
            lines.push(Line::from(Span::styled(error, app.theme_styles.error)));
        }
    }
    let hint = match app.guided_review_workspace.source_reference_entry.as_ref() {
        Some(entry) => match entry.kind {
            crate::guided_review::ReviewSourceReferenceKind::Base => " enter load base · esc back ",
            crate::guided_review::ReviewSourceReferenceKind::Commit => {
                " enter load commit · esc back "
            }
        },
        None => " enter load · ↑/↓ choose · esc close ",
    };
    let block = overlay_block(app, Icon::Source.glyph(), "Change review source");
    frame.render_widget(Clear, popup);
    let body = crate::widgets::render_footed(frame, block, popup, hint, None, &app.theme_styles);
    frame.render_widget(Paragraph::new(lines), body);
}
