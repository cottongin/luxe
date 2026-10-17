//! Merge — the confirmation a merge goes through, with the method chosen in it.
//!
//! Merging is the one thing in the detail that cannot be taken back, so it has no letter: Enter on
//! the Ready row opens this, and Enter here merges. The methods are the ones the repository allows,
//! in GitHub's order with the first of them chosen, moved between with Tab the way Post to GitHub
//! moves its verdict; what each one does to the history is said under it, because that is the part
//! a team cares about.
//!
//! The same confirmation turns on auto-merge, from the Auto-merge row: the method is chosen the
//! same way, and GitHub merges by itself once nothing stands in the way.

use crossterm::event::{KeyCode, KeyEvent};
use luxe_core::engine::PullRequestWrite;
use luxe_core::github::overview::{MergeMethod, MergeRequest, PullRequestOverview};
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Padding, Paragraph};
use ratatui::Frame;

use super::{GithubDetail, Subject};
use crate::glyphs::{Icon, Mark};
use crate::theme::styles::ThemeStyles;

/// A merge waiting on a yes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeDialog {
    /// Whether this turns on auto-merge rather than merging now
    pub auto: bool,
    /// The chosen method, among the ones the repository allows
    pub method: usize,
    /// Whether the head branch goes once it is merged
    pub delete_branch: bool,
    /// Whether it has been sent and GitHub has not answered yet
    pub merging: bool,
    /// Why GitHub refused it, as a sentence
    pub error: Option<String>,
}

impl MergeDialog {
    /// On the method this account merged with last here, as GitHub's own button opens, or the
    /// first the repository allows; deleting the branch after.
    pub fn new(pr: &PullRequestOverview) -> Self {
        let default = pr.default_merge_method();
        let method = pr.merge_options().iter().position(|method| *method == default).unwrap_or(0);
        // A fork's branch is not this repository's to delete
        Self {
            auto: false,
            method,
            delete_branch: !pr.cross_repository,
            merging: false,
            error: None,
        }
    }

    /// The same, turning on auto-merge: the branch is left to the repository, since nobody is
    /// here to delete it when GitHub merges.
    pub fn auto(pr: &PullRequestOverview) -> Self {
        Self { auto: true, delete_branch: false, ..Self::new(pr) }
    }
}

/// Whether the branch is the reader's to keep or delete: not a fork's, not one the repository
/// deletes by itself, where a choice to keep it would be a promise GitHub breaks, and not one
/// GitHub merges later, when nobody is here to delete it.
fn branch_is_a_choice(pr: &PullRequestOverview, dialog: &MergeDialog) -> bool {
    !dialog.auto && !pr.cross_repository && !pr.merge_settings.deletes_branch
}

/// What a key does while the confirmation is open: Tab moves the method, `d` keeps or deletes the
/// branch, Esc backs out, and Enter answers the merge to send.
pub(super) fn on_key(detail: &mut GithubDetail, key: &KeyEvent) -> Option<PullRequestWrite> {
    let Subject::PullRequest(pr) = &detail.subject else {
        return None;
    };
    let methods = pr.merge_options();
    let dialog = detail.merging.as_mut()?;
    if dialog.merging {
        return None;
    }
    match key.code {
        KeyCode::Esc => detail.merging = None,
        KeyCode::Tab | KeyCode::Right => dialog.method = (dialog.method + 1) % methods.len(),
        KeyCode::BackTab | KeyCode::Left => {
            dialog.method = dialog.method.checked_sub(1).unwrap_or(methods.len() - 1);
        }
        KeyCode::Char('d') if branch_is_a_choice(pr, dialog) => {
            dialog.delete_branch = !dialog.delete_branch;
        }
        KeyCode::Enter => {
            dialog.merging = true;
            dialog.error = None;
            let method = methods[dialog.method.min(methods.len() - 1)];
            if dialog.auto {
                return Some(PullRequestWrite::EnableAutoMerge {
                    node_id: pr.node_id.clone(),
                    method,
                    head_oid: pr.head_oid.clone(),
                });
            }
            return Some(PullRequestWrite::Merge(MergeRequest {
                method,
                head_oid: pr.head_oid.clone(),
                // The repository's own deletion needs no help
                delete_branch: (dialog.delete_branch && branch_is_a_choice(pr, dialog))
                    .then(|| pr.head_ref.clone()),
            }));
        }
        _ => {}
    }
    None
}

/// The confirmation's keys.
pub(super) fn hint(detail: &GithubDetail) -> String {
    let (Subject::PullRequest(pr), Some(dialog)) = (&detail.subject, &detail.merging) else {
        return String::new();
    };
    match (dialog.merging, dialog.auto) {
        (true, true) => return "turning on auto-merge…".into(),
        (true, false) => return "merging…".into(),
        _ => {}
    }
    let act = if dialog.auto { "enable auto-merge" } else { "merge" };
    let branch = if branch_is_a_choice(pr, dialog) { " · d delete branch" } else { "" };
    format!("enter {act} · tab method{branch} · esc cancel")
}

/// What the chosen method will do: the commit it makes, and what happens to the branch.
fn method_lines(
    pr: &PullRequestOverview,
    method: MergeMethod,
    commits: &str,
    delete_branch: bool,
    styles: &ThemeStyles,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let bold = styles.text.add_modifier(Modifier::BOLD);
    let (label, title, effect) = match method {
        MergeMethod::Squash => (
            "Commit",
            format!("{} (#{})", pr.title, pr.number),
            format!("{commits} become one on {}", pr.base_ref),
        ),
        MergeMethod::Merge => (
            "Commit",
            // GitHub's own default title: the head named with its owner
            format!("Merge pull request #{} from {}/{}", pr.number, owner(pr), pr.head_ref),
            format!("keeps all {commits} and adds a merge commit on {}", pr.base_ref),
        ),
        MergeMethod::Rebase => (
            "Commits",
            format!("{commits} replayed onto {}", pr.base_ref),
            "no merge commit; each keeps its own message".to_string(),
        ),
    };
    lines.push(Line::from(Span::styled(label, styles.muted)));
    lines.push(Line::from(Span::styled(title, bold)));
    lines.push(Line::from(Span::styled(effect, styles.muted)));
    lines.push(Line::default());
    let (mark, style, said) = match (pr.cross_repository, delete_branch) {
        (true, _) => (Mark::No, styles.muted, format!("{} is in a fork, and stays", pr.head_ref)),
        _ if pr.merge_settings.deletes_branch => (
            Mark::Yes,
            styles.success,
            format!("The repository deletes {} after merging", pr.head_ref),
        ),
        (false, true) => {
            (Mark::Yes, styles.success, format!("Delete {} after merging", pr.head_ref))
        }
        (false, false) => (Mark::No, styles.muted, format!("Keep {} after merging", pr.head_ref)),
    };
    lines.push(Line::from(vec![
        Span::styled(format!("{} ", mark.glyph()), style),
        Span::styled(said, styles.text),
    ]));
    lines
}

/// Draws the confirmation over `area`, the surface the detail is on.
pub(super) fn render(f: &mut Frame, area: Rect, detail: &GithubDetail, styles: &ThemeStyles) {
    let (Subject::PullRequest(pr), Some(dialog)) = (&detail.subject, &detail.merging) else {
        return;
    };
    let methods = pr.merge_options();
    let method = methods[dialog.method.min(methods.len() - 1)];
    let commits = match pr.commits {
        1 => "1 commit".to_string(),
        n => format!("{n} commits"),
    };

    let mut lines = vec![Line::from(crate::widgets::code_spans(
        &format!("Into `{}` from `{}` · {commits}", pr.base_ref, pr.head_ref),
        styles.accent,
        styles,
    ))];
    if dialog.auto {
        lines.push(Line::from(Span::styled(
            "Auto-merge · merges once nothing is in the way",
            styles.muted,
        )));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Method", styles.muted)));
    let labels: Vec<&str> = methods.iter().map(|method| method.label()).collect();
    lines.push(crate::picker::tab_line(&labels, dialog.method, styles));
    lines.push(Line::default());

    lines.extend(method_lines(pr, method, &commits, dialog.delete_branch, styles));
    if let Some(error) = &dialog.error {
        lines.push(Line::default());
        lines.push(Line::from(crate::widgets::code_spans(error, styles.error, styles)));
    }
    if dialog.merging {
        lines.push(Line::default());
        let said = if dialog.auto { "Turning on auto-merge…" } else { "Merging on GitHub…" };
        lines.push(Line::from(Span::styled(said, styles.muted)));
    }

    let width = area.width.saturating_sub(8).clamp(40, 84).min(area.width);
    let height = u16::try_from(lines.len() + 2 + usize::from(crate::widgets::FOOTER_ROWS))
        .unwrap_or(u16::MAX)
        .min(area.height);
    let popup = crate::widgets::centered_rect(width, height, area);
    let title = if dialog.auto { "Enable auto-merge" } else { "Merge pull request" };
    let block = crate::widgets::titled_panel(Icon::GitHub.glyph(), title, styles)
        .padding(Padding::horizontal(1));
    f.render_widget(Clear, popup);
    let body = crate::widgets::render_footed(f, block, popup, &hint(detail), None, styles);
    f.render_widget(Paragraph::new(lines), body);
}

/// The owner of the repository a pull request belongs to, from its `owner/name`.
fn owner(pr: &PullRequestOverview) -> &str {
    pr.repository.split('/').next().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::super::test_support::pr;
    use super::*;
    use crossterm::event::KeyModifiers;

    /// Tab walks the methods the repository allows in GitHub's order and comes round; `d` keeps or
    /// deletes the branch; Esc backs out without merging.
    #[test]
    fn the_confirmation_chooses_a_method_and_backs_out() {
        let overview = PullRequestOverview {
            merge_methods: vec![MergeMethod::Rebase, MergeMethod::Squash, MergeMethod::Merge],
            ..pr()
        };
        let mut detail = GithubDetail::new(Subject::PullRequest(Box::new(overview.clone())));
        detail.merging = Some(MergeDialog::new(&overview));
        let press = |detail: &mut GithubDetail, code| {
            on_key(detail, &KeyEvent::new(code, KeyModifiers::NONE))
        };
        assert_eq!(overview.merge_options()[0], MergeMethod::Merge, "a merge commit first");
        press(&mut detail, KeyCode::Tab);
        press(&mut detail, KeyCode::Tab);
        assert_eq!(detail.merging.as_ref().map(|dialog| dialog.method), Some(2));
        press(&mut detail, KeyCode::Tab);
        assert_eq!(detail.merging.as_ref().map(|dialog| dialog.method), Some(0), "round again");
        press(&mut detail, KeyCode::BackTab);
        assert_eq!(detail.merging.as_ref().map(|dialog| dialog.method), Some(2));
        press(&mut detail, KeyCode::Char('d'));
        assert_eq!(detail.merging.as_ref().map(|dialog| dialog.delete_branch), Some(false));
        press(&mut detail, KeyCode::Esc);
        assert!(detail.merging.is_none());
    }

    /// The confirmation opens on the method this account used last, as GitHub's button does,
    /// unless the repository has since turned it off; a branch the repository deletes by itself is
    /// said as such, and neither offered nor asked for.
    #[test]
    fn the_confirmation_opens_on_the_last_method_and_leaves_the_branch_to_the_repository() {
        use luxe_core::github::overview::MergeSettings;
        let remembering = |last, deletes_branch| PullRequestOverview {
            merge_methods: vec![MergeMethod::Merge, MergeMethod::Squash],
            merge_settings: MergeSettings {
                last_method: Some(last),
                deletes_branch,
                ..MergeSettings::default()
            },
            ..pr()
        };
        assert_eq!(MergeDialog::new(&remembering(MergeMethod::Squash, false)).method, 1);
        assert_eq!(
            MergeDialog::new(&remembering(MergeMethod::Rebase, false)).method,
            0,
            "a method turned off since falls back to the first allowed"
        );

        let overview = remembering(MergeMethod::Squash, true);
        let mut detail = GithubDetail::new(Subject::PullRequest(Box::new(overview.clone())));
        detail.merging = Some(MergeDialog::new(&overview));
        assert!(!hint(&detail).contains("d delete branch"));
        let press = |detail: &mut GithubDetail, code| {
            on_key(detail, &KeyEvent::new(code, KeyModifiers::NONE))
        };
        press(&mut detail, KeyCode::Char('d'));
        let Some(PullRequestWrite::Merge(request)) = press(&mut detail, KeyCode::Enter) else {
            panic!("a merge");
        };
        assert_eq!((request.method, request.delete_branch), (MergeMethod::Squash, None));
    }

    /// Enter answers the merge exactly as left — the method, the head the reader saw, the branch
    /// to delete — once; a fork's branch is never offered for deleting.
    #[test]
    fn enter_answers_the_merge_as_left() {
        let overview = PullRequestOverview {
            merge_methods: vec![MergeMethod::Squash, MergeMethod::Merge],
            ..pr()
        };
        let mut detail = GithubDetail::new(Subject::PullRequest(Box::new(overview.clone())));
        detail.merging = Some(MergeDialog::new(&overview));
        let press = |detail: &mut GithubDetail, code| {
            on_key(detail, &KeyEvent::new(code, KeyModifiers::NONE))
        };
        press(&mut detail, KeyCode::Tab);
        assert_eq!(
            press(&mut detail, KeyCode::Enter),
            Some(PullRequestWrite::Merge(MergeRequest {
                method: MergeMethod::Squash,
                head_oid: overview.head_oid.clone(),
                delete_branch: Some("digest-routing".into()),
            }))
        );
        assert_eq!(press(&mut detail, KeyCode::Enter), None, "once, until GitHub answers");
        assert_eq!(hint(&detail), "merging…");

        let fork = PullRequestOverview { cross_repository: true, ..overview };
        let mut detail = GithubDetail::new(Subject::PullRequest(Box::new(fork.clone())));
        detail.merging = Some(MergeDialog::new(&fork));
        press(&mut detail, KeyCode::Char('d'));
        let Some(PullRequestWrite::Merge(request)) = press(&mut detail, KeyCode::Enter) else {
            panic!("a merge");
        };
        assert_eq!(request.delete_branch, None, "a fork's branch stays");
        assert!(!hint(&detail).contains("delete branch"));
    }

    /// From the Auto-merge row the confirmation turns auto-merge on: the method chosen the same
    /// way, the head the reader saw, the pull request by GitHub's id, and no branch to delete —
    /// nobody is here when GitHub merges.
    #[test]
    fn the_auto_merge_confirmation_turns_it_on_as_left() {
        let overview = PullRequestOverview {
            node_id: "PR_kwDOU5qvE88AAAABGZIsWQ".into(),
            merge_methods: vec![MergeMethod::Merge, MergeMethod::Squash],
            ..pr()
        };
        let mut detail = GithubDetail::new(Subject::PullRequest(Box::new(overview.clone())));
        detail.merging = Some(MergeDialog::auto(&overview));
        assert_eq!(hint(&detail), "enter enable auto-merge · tab method · esc cancel");
        let press = |detail: &mut GithubDetail, code| {
            on_key(detail, &KeyEvent::new(code, KeyModifiers::NONE))
        };
        press(&mut detail, KeyCode::Char('d'));
        press(&mut detail, KeyCode::Tab);
        assert_eq!(
            press(&mut detail, KeyCode::Enter),
            Some(PullRequestWrite::EnableAutoMerge {
                node_id: "PR_kwDOU5qvE88AAAABGZIsWQ".into(),
                method: MergeMethod::Squash,
                head_oid: overview.head_oid.clone(),
            })
        );
        assert_eq!(hint(&detail), "turning on auto-merge…");
    }
}
