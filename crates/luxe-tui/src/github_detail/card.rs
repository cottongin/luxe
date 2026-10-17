//! Card — `/pr`: the branch's pull request in detail, on its own over the conversation.
//!
//! It opens at once on what the workspace already knows and reads the overview behind it. The
//! workspace runtime keeps the branch's pull request status current, so when that status moves —
//! a push, a review, a check finishing — the card reads the overview again rather than polling on
//! a clock of its own.

use luxe_core::github::overview::Overview;
use luxe_core::github::PullRequestStatus;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Padding, Paragraph};
use ratatui::Frame;

use super::{render, DetailKey, GithubDetail};
use crate::theme::styles::ThemeStyles;

/// `/pr`'s card, and the status that last made it read.
#[derive(Debug, Clone)]
pub struct PullRequestCard {
    /// The pull request's number
    pub number: u64,
    /// What there is to show
    pub state: CardState,
    /// The workspace's status when the overview was last asked for; a different one asks again
    pub seen: Option<PullRequestStatus>,
    /// Whether it is the checked-out branch's pull request, rather than one opened from it — the
    /// one below in a stack
    pub branch: bool,
}

/// What `/pr` has to show.
#[derive(Debug, Clone)]
pub enum CardState {
    Reading,
    /// Why it could not be read, as a sentence
    Failed(String),
    Ready(Box<GithubDetail>),
}

impl PullRequestCard {
    /// A card reading pull request `number`, opened on `status`.
    pub fn reading(number: u64, status: Option<PullRequestStatus>) -> Self {
        Self { number, state: CardState::Reading, seen: status, branch: true }
    }

    /// A card reading pull request `number`, which is not the branch's own.
    pub fn other(number: u64) -> Self {
        Self { branch: false, ..Self::reading(number, None) }
    }

    /// Lands an answer. A card already showing keeps its tab and cursor through a re-read, so a
    /// refresh never throws the reader back to the top.
    ///
    /// The overview query does not read stacks; the workspace's branch status does, so a pull
    /// request in one carries its place in the stack from the status the card was opened on.
    pub fn land(&mut self, mut overview: Option<Overview>, error: Option<String>) {
        if let (Some(Overview::PullRequest(pr)), Some(seen)) = (overview.as_mut(), &self.seen) {
            if pr.stack.is_none() && seen.number == pr.number {
                pr.stack = seen.stack.clone();
            }
        }
        match (overview, &mut self.state) {
            (Some(overview), CardState::Ready(detail)) => detail.subject = overview,
            (Some(overview), _) => {
                let mut detail = GithubDetail::new(overview);
                detail.on_branch = self.branch;
                self.state = CardState::Ready(Box::new(detail));
            }
            // A failed re-read leaves what was read on screen; only a first read shows the failure
            (None, CardState::Ready(_)) => {}
            (None, _) => {
                let reason = error.unwrap_or_else(|| "No answer from GitHub".into());
                self.state = CardState::Failed(reason);
            }
        }
    }

    /// Whether `status` is news since the last read: a push, a review, a check finishing.
    pub fn stale(&self, status: &PullRequestStatus) -> bool {
        status.number == self.number && self.seen.as_ref() != Some(status)
    }

    /// What `key` does. Esc closes from any state; a ready card gives the rest to its detail.
    pub fn on_key(&mut self, key: &crossterm::event::KeyEvent) -> DetailKey {
        match &mut self.state {
            CardState::Ready(detail) => detail.on_key(key),
            _ if key.code == crossterm::event::KeyCode::Esc => DetailKey::Leave,
            _ => DetailKey::Held,
        }
    }

    fn hint(&self) -> String {
        match &self.state {
            CardState::Ready(detail) => detail.hint("esc close"),
            _ => "esc close".into(),
        }
    }
}

/// Draws the card centered over `area`, most of the screen, the conversation dimmed behind it.
///
/// Args: `(frame, area, card, spinner frame, styles)`.
pub fn render_card(
    f: &mut Frame,
    area: Rect,
    card: &PullRequestCard,
    frame: usize,
    styles: &ThemeStyles,
) {
    let width = area.width.saturating_sub(8).clamp(40, 120).min(area.width);
    let height = area.height.saturating_sub(4).max(12).min(area.height);
    let popup = crate::widgets::centered_rect(width, height, area);
    crate::widgets::dim_backdrop(f, area, styles);
    f.render_widget(Clear, popup);
    let block = crate::widgets::titled_panel(
        crate::glyphs::Icon::PullRequest.glyph(),
        "Pull request",
        styles,
    )
    .padding(Padding::horizontal(1));
    let body = crate::widgets::render_footed(f, block, popup, &card.hint(), None, styles);
    match &card.state {
        CardState::Reading => {
            let spinner = crate::statusline::spinner(frame);
            let said = format!("{spinner} Reading #{} from GitHub…", card.number);
            f.render_widget(Paragraph::new(Line::from(Span::styled(said, styles.muted))), body);
        }
        CardState::Failed(reason) => {
            let line = Line::from(crate::widgets::code_spans(reason, styles.error, styles));
            f.render_widget(
                Paragraph::new(line).wrap(ratatui::widgets::Wrap { trim: false }),
                body,
            );
        }
        CardState::Ready(detail) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_secs());
            render(f, body, detail, true, detail.clock.unwrap_or(now), styles);
            super::render_overlays(f, popup, detail, frame, styles);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::pr;
    use super::*;
    use luxe_core::github::{CheckRollup, PullRequestState};

    fn status(passed: usize) -> PullRequestStatus {
        PullRequestStatus {
            number: 142,
            title: "Route digests by category".into(),
            url: "https://github.com/acme/notify/pull/142".into(),
            state: PullRequestState::Open,
            draft: false,
            review_decision: None,
            checks: CheckRollup { passed, ..CheckRollup::default() },
            unresolved_threads: 0,
            head_oid: "abc".into(),
            stack: None,
        }
    }

    /// The first answer opens the detail, ready to work the branch's threads; a re-read keeps the
    /// reader's place; a failed re-read keeps what was read; a failed first read says why.
    #[test]
    fn a_card_lands_its_answers_and_keeps_its_place() {
        let mut card = PullRequestCard::reading(142, Some(status(1)));
        card.land(Some(Overview::PullRequest(Box::new(pr()))), None);
        let CardState::Ready(detail) = &mut card.state else { panic!("{card:?}") };
        assert!(detail.on_branch, "the branch's own pull request");
        detail.cursor = 1;
        let mut moved = pr();
        moved.title = "Route digests by category, again".into();
        card.land(Some(Overview::PullRequest(Box::new(moved))), None);
        card.land(None, Some("GitHub timed out.".into()));
        let CardState::Ready(detail) = &card.state else { panic!("{card:?}") };
        assert_eq!(detail.cursor, 1);
        assert!(
            matches!(&detail.subject, Overview::PullRequest(pr) if pr.title.ends_with("again"))
        );

        // Its place in a stack comes from the branch status, which the overview does not read
        let stack = luxe_core::github::Stack {
            trunk: "main".into(),
            trunk_is_default: true,
            below: Vec::new(),
            above: Vec::new(),
        };
        let mut stacked = PullRequestCard::reading(
            142,
            Some(PullRequestStatus { stack: Some(stack.clone()), ..status(1) }),
        );
        stacked.land(Some(Overview::PullRequest(Box::new(pr()))), None);
        let CardState::Ready(detail) = &stacked.state else { panic!("{stacked:?}") };
        assert!(matches!(&detail.subject, Overview::PullRequest(pr) if pr.stack == Some(stack)));

        let mut failed = PullRequestCard::reading(142, None);
        failed.land(None, Some("The GitHub CLI is not signed in.".into()));
        assert!(
            matches!(&failed.state, CardState::Failed(said) if said == "The GitHub CLI is not signed in.")
        );
    }

    /// News is a different status for the same pull request; another branch's is not news here.
    #[test]
    fn a_moved_status_is_news_and_another_pull_request_is_not() {
        let card = PullRequestCard::reading(142, Some(status(1)));
        assert!(!card.stale(&status(1)));
        assert!(card.stale(&status(2)), "a check finished");
        assert!(!card.stale(&PullRequestStatus { number: 7, ..status(2) }));
    }
}
