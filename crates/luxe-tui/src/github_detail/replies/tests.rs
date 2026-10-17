use super::super::test_support::{comment, pr};
use super::super::{DetailTab, Subject};
use super::*;
use crossterm::event::{KeyCode, KeyModifiers};

fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

fn thread() -> ConversationEntry {
    ConversationEntry::Thread(luxe_core::github::ReviewThread {
        id: "PRRT_1".into(),
        path: "src/dispatch/router.rs".into(),
        line: Some(88),
        start_line: None,
        side: luxe_core::github::ThreadSide::Right,
        outdated: false,
        resolved: false,
        file_level: false,
        comments: vec![luxe_core::github::ThreadComment {
            author: "jon".into(),
            body: "This decides the fallback again.".into(),
            created_at: String::new(),
            url: String::new(),
        }],
        total_comments: 1,
        anchor_text: None,
        can_reply: true,
        can_resolve: true,
    })
}

fn detail() -> GithubDetail {
    let mut detail = GithubDetail::new(Subject::PullRequest(Box::new(pr())));
    detail.tab = DetailTab::Conversation;
    detail.conversation_asked = true;
    detail.conversation = Some(vec![comment("mara", false, "Reads well."), thread()]);
    detail
}

/// The Conversation is read the first time its tab opens, and not again on the way back.
#[test]
fn the_conversation_is_read_when_first_looked_at() {
    let mut detail = GithubDetail::new(Subject::PullRequest(Box::new(pr())));
    assert_eq!(detail.on_key(&key(KeyCode::Tab, KeyModifiers::NONE)), DetailKey::ReadConversation);
    detail.on_key(&key(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(detail.on_key(&key(KeyCode::Tab, KeyModifiers::NONE)), DetailKey::Held);
    detail.land_conversation(None, Some("GitHub timed out.".into()));
    assert_eq!(detail.conversation_error.as_deref(), Some("GitHub timed out."));
    detail.land_conversation(Some(Vec::new()), None);
    assert!(detail.conversation_error.is_none());
}

/// `r` on a thread replies in it; on a comment, it answers it with a new comment; Ctrl+Enter
/// keeps the words and Enter asks for them to be posted, quoting what a comment answers.
#[test]
fn a_reply_goes_where_its_entry_is() {
    let mut detail = detail();
    detail.cursor = 1;
    detail.on_key(&key(KeyCode::Char('r'), KeyModifiers::NONE));
    let card = detail.composing.as_ref().expect("writing");
    assert_eq!(card.draft.target, "Reply to @jon on `src/dispatch/router.rs:88`");
    assert_eq!(card.draft.thread.as_deref(), Some("PRRT_1"));
    for character in "Done.".chars() {
        detail.on_key(&key(KeyCode::Char(character), KeyModifiers::NONE));
    }
    detail.on_key(&key(KeyCode::Enter, KeyModifiers::CONTROL));
    assert_eq!(
        detail.on_key(&key(KeyCode::Enter, KeyModifiers::NONE)),
        DetailKey::Post(PostRequest {
            thread: Some("PRRT_1".into()),
            answering: Some("This decides the fallback again.".into()),
            body: "Done.".into(),
        })
    );

    let mut detail = super::tests::detail();
    detail.on_key(&key(KeyCode::Char('r'), KeyModifiers::NONE));
    let card = detail.composing.as_ref().expect("writing");
    assert_eq!(card.draft.target, "Comment on #142, answering @mara");
    assert_eq!(card.draft.thread, None);
}

/// Esc keeps a written reply under what it answers; `r` on the same entry picks it up again; a
/// post that landed closes the card and asks for the conversation again, one that failed says
/// why and leaves the card open to try again.
#[test]
fn a_kept_reply_comes_back_and_a_post_lands_or_says_why() {
    let mut detail = detail();
    detail.cursor = 1;
    detail.on_key(&key(KeyCode::Char('r'), KeyModifiers::NONE));
    detail.on_key(&key(KeyCode::Char('x'), KeyModifiers::NONE));
    detail.on_key(&key(KeyCode::Enter, KeyModifiers::CONTROL));
    detail.on_key(&key(KeyCode::Esc, KeyModifiers::NONE));
    assert!(detail.composing.is_none());
    assert_eq!(detail.drafts.len(), 1, "kept");
    detail.on_key(&key(KeyCode::Char('r'), KeyModifiers::NONE));
    assert_eq!(detail.composing.as_ref().map(|card| card.draft.body.as_str()), Some("x"));
    assert!(detail.drafts.is_empty(), "taken back up, not copied");

    detail.on_key(&key(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!detail.posted(Some("PRRT_1"), Some("Thread is locked.".into())));
    let card = detail.composing.as_ref().expect("still open");
    assert_eq!((card.posting, card.error.as_deref()), (false, Some("Thread is locked.")));
    assert!(!detail.posted(Some("another"), None), "another thread's answer is not this one's");
    detail.on_key(&key(KeyCode::Enter, KeyModifiers::NONE));
    assert!(detail.posted(Some("PRRT_1"), None));
    assert!(detail.composing.is_none());
    assert!(!detail.conversation_asked, "read again");
}

/// `d` on a thread asks the agent for a reply to exactly that thread and opens the card on it,
/// drafting; what lands fills the card and says it was the agent's and what it came from.
#[test]
fn d_asks_the_agent_and_the_draft_lands_in_its_card() {
    let mut detail = detail();
    detail.cursor = 1;
    let DetailKey::Draft(ask) = detail.on_key(&key(KeyCode::Char('d'), KeyModifiers::NONE)) else {
        panic!("a draft asked for")
    };
    assert!(
        matches!(&ask.target, luxe_core::engine::DraftTarget::Thread(thread) if thread.id == "PRRT_1")
    );
    assert_eq!(ask.conversation.len(), 2, "the conversation goes with it, for the digest");
    let card = detail.composing.as_ref().expect("the card");
    assert_eq!(card.drafting.as_deref(), Some(ask.request_id.as_str()));
    assert!(!detail.drafted("another", Some("no".into()), Vec::new(), None, None));
    assert!(detail.drafted(
        &ask.request_id,
        Some("Fixed in 4c1e2a0.".into()),
        vec!["this session".into(), "the thread".into()],
        None,
        Some("I took the fix from the commit, not the thread.".into()),
    ));
    let card = detail.composing.as_ref().expect("the card");
    assert_eq!(card.draft.body, "Fixed in 4c1e2a0.");
    assert!(card.draft.by_agent && card.drafting.is_none());
    assert_eq!(card.draft.sources, ["this session", "the thread"]);
    assert_eq!(
        card.draft.note.as_deref(),
        Some("I took the fix from the commit, not the thread."),
        "what the agent said beside it rides with it"
    );
}

/// Put away while the agent writes it, a draft is kept under what it answers when it lands.
#[test]
fn a_draft_put_away_while_drafting_is_kept_when_it_lands() {
    let mut detail = detail();
    detail.cursor = 1;
    let DetailKey::Draft(ask) = detail.on_key(&key(KeyCode::Char('d'), KeyModifiers::NONE)) else {
        panic!("a draft asked for")
    };
    detail.on_key(&key(KeyCode::Esc, KeyModifiers::NONE));
    assert!(detail.composing.is_none() && detail.waiting.len() == 1);
    assert!(detail.drafted(&ask.request_id, Some("Later.".into()), Vec::new(), None, None));
    assert!(detail.waiting.is_empty());
    assert_eq!(detail.drafts.len(), 1);
    assert_eq!((detail.drafts[0].body.as_str(), detail.drafts[0].by_agent), ("Later.", true));
}

/// `d` on a draft with words asks what should change, then asks again carrying the reader's
/// text and the note.
#[test]
fn a_redraft_carries_what_should_change_and_the_readers_text() {
    let mut detail = detail();
    detail.cursor = 1;
    let DetailKey::Draft(ask) = detail.on_key(&key(KeyCode::Char('d'), KeyModifiers::NONE)) else {
        panic!("a draft asked for")
    };
    detail.drafted(&ask.request_id, Some("A long reply.".into()), Vec::new(), None, None);
    assert_eq!(detail.on_key(&key(KeyCode::Char('d'), KeyModifiers::NONE)), DetailKey::Held);
    for character in "Shorter.".chars() {
        detail.on_key(&key(KeyCode::Char(character), KeyModifiers::NONE));
    }
    let DetailKey::Draft(again) = detail.on_key(&key(KeyCode::Enter, KeyModifiers::NONE)) else {
        panic!("asked again")
    };
    assert_eq!(again.note.as_deref(), Some("Shorter."));
    assert_eq!(again.current.as_deref(), Some("A long reply."));
    assert_ne!(again.request_id, ask.request_id);
}
