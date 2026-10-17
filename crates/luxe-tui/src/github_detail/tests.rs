//! The detail's keys: where the cursor stops and what each key asks.

use super::test_support::{conversation, pr};
use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn press(detail: &mut GithubDetail, code: KeyCode) -> DetailKey {
    detail.on_key(&KeyEvent::new(code, KeyModifiers::NONE))
}

/// Tab moves between the tabs and starts each at its top; the cursor stays among the rows that
/// act; Enter opens an entry and closes it again; `b` takes bots out of the stops.
#[test]
fn keys_move_among_the_rows_that_act() {
    let mut detail = GithubDetail::new(Subject::PullRequest(Box::new(pr())));
    assert!(detail.overview_stops().is_empty(), "nothing acts off the branch");
    detail.on_branch = true;
    assert_eq!(
        detail.overview_stops(),
        [Some(CheckAction::FixChecks), Some(CheckAction::ResolveThreads)]
    );
    press(&mut detail, KeyCode::Down);
    press(&mut detail, KeyCode::Down);
    assert_eq!(detail.cursor, 1, "the last stop stays the last");
    assert_eq!(press(&mut detail, KeyCode::Enter), DetailKey::Act(CheckAction::ResolveThreads));
    assert_eq!(
        press(&mut detail, KeyCode::Char('f')),
        DetailKey::Act(CheckAction::FixChecks),
        "a row's letter does the same from anywhere in the Overview"
    );
    press(&mut detail, KeyCode::Tab);
    assert_eq!((detail.tab, detail.cursor), (DetailTab::Conversation, 0));

    detail.conversation = Some(conversation());
    assert_eq!(detail.conversation_stops().len(), 2);
    press(&mut detail, KeyCode::Down);
    press(&mut detail, KeyCode::Enter);
    assert!(detail.open.contains(&1));
    press(&mut detail, KeyCode::Enter);
    assert!(detail.open.is_empty(), "Enter closes what it opened");
    press(&mut detail, KeyCode::Char('b'));
    assert_eq!((detail.conversation_stops().len(), detail.cursor), (1, 0));

    assert_eq!(
        press(&mut detail, KeyCode::Char('o')),
        DetailKey::Open("https://github.com/acme/notify/pull/142".into())
    );
    assert_eq!(press(&mut detail, KeyCode::Left), DetailKey::Leave);
    assert_eq!(press(&mut detail, KeyCode::Esc), DetailKey::Leave);
}
