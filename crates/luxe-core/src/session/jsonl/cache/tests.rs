#![allow(clippy::unwrap_used)]

use super::super::super::{
    EntryKind, JsonlSessionStore, Message, SessionError, SessionId, SessionStore,
};
use std::fs;
use std::path::Path;

fn store_at(root: &Path) -> JsonlSessionStore {
    let mut store = JsonlSessionStore::new(root);
    store.set_project_hash("project");
    store
}

fn say(store: &mut JsonlSessionStore, session: &SessionId, words: &str) {
    store.append(session, None, EntryKind::Message(Message::user(words))).unwrap();
}

/// Breaks the file's first line in place, keeping its length, so a store that parses the file
/// again fails and one reading on from where it stopped doesn't notice.
fn break_first_line(path: &Path) {
    let mut bytes = fs::read(path).unwrap();
    bytes[0] = b'x';
    fs::write(path, bytes).unwrap();
}

// The break this catches: a cache that never looks at the file again, so another luxe's entries
// (or a write this store didn't make) never show up
#[test]
fn entries_another_store_appends_are_read_on() {
    let root = tempfile::tempdir().unwrap();
    let mut mine = store_at(root.path());
    let session = mine.create("/tmp").unwrap();
    assert_eq!(mine.load(&session).unwrap().len(), 1);
    say(&mut store_at(root.path()), &session, "from elsewhere");
    assert_eq!(mine.load(&session).unwrap().len(), 2);
}

// The break this catches: the session being parsed whole on every read, ~60ms a model call on a
// long session, which is what the cache is for
#[test]
fn what_was_read_is_not_parsed_again() {
    let root = tempfile::tempdir().unwrap();
    let mut store = store_at(root.path());
    let session = store.create("/tmp").unwrap();
    store.load(&session).unwrap();
    break_first_line(&store.session_dir(&session));
    say(&mut store, &session, "after");
    assert_eq!(store.load(&session).unwrap().len(), 2, "read on from where it stopped");
    let fresh = store_at(root.path());
    assert!(matches!(fresh.load(&session), Err(SessionError::JsonParse { line: 1, .. })));
}

// The break this catches: a resumed session, which this store claims rather than creates, parsed
// whole on every read
#[test]
fn a_claimed_session_is_kept() {
    let root = tempfile::tempdir().unwrap();
    let session = store_at(root.path()).create("/tmp").unwrap();
    let store = store_at(root.path());
    let _claim = store.claim(&session).unwrap();
    store.load(&session).unwrap();
    break_first_line(&store.session_dir(&session));
    assert_eq!(store.load(&session).unwrap().len(), 1);
}

// The break this catches: a switch to another session keeping the first one cached, so the
// session now being written is parsed whole on every read
#[test]
fn switching_sessions_keeps_the_new_one() {
    let root = tempfile::tempdir().unwrap();
    let mut store = store_at(root.path());
    let first = store.create("/tmp").unwrap();
    store.load(&first).unwrap();
    let second = store.create("/tmp").unwrap();
    store.load(&second).unwrap();
    break_first_line(&store.session_dir(&second));
    assert_eq!(store.load(&second).unwrap().len(), 1);
}

// The break this catches: a file replaced by a shorter one served from the old entries, or read
// on from an offset past its end
#[test]
fn a_shorter_file_is_read_again_from_the_start() {
    let root = tempfile::tempdir().unwrap();
    let mut store = store_at(root.path());
    let session = store.create("/tmp").unwrap();
    let path = store.session_dir(&session);
    let header = fs::read_to_string(&path).unwrap();
    say(&mut store, &session, "dropped");
    assert_eq!(store.load(&session).unwrap().len(), 2);
    fs::write(&path, header).unwrap();
    assert_eq!(store.load(&session).unwrap().len(), 1);
}

// The break this catches: every session the picker reads kept in memory, or one served from
// the cache though this store doesn't write it
#[test]
fn a_session_this_store_does_not_write_is_read_fresh() {
    let root = tempfile::tempdir().unwrap();
    let other = store_at(root.path()).create("/tmp").unwrap();
    let mut store = store_at(root.path());
    store.create("/tmp").unwrap();
    store.load(&other).unwrap();
    break_first_line(&store.session_dir(&other));
    assert!(matches!(store.load(&other), Err(SessionError::JsonParse { line: 1, .. })));
}

// The break this catches: a bad line found while reading on reported by its place in the new
// part rather than in the file, pointing at the wrong line
#[test]
fn a_bad_line_read_later_names_its_line_in_the_file() {
    let root = tempfile::tempdir().unwrap();
    let mut store = store_at(root.path());
    let session = store.create("/tmp").unwrap();
    say(&mut store, &session, "fine");
    store.load(&session).unwrap();
    let path = store.session_dir(&session);
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str("not json\n");
    fs::write(&path, text).unwrap();
    assert!(matches!(store.load(&session), Err(SessionError::JsonParse { line: 3, .. })));
}

// The break this catches: the walk from a leaf served from the cache going stale, so a request
// built after an append is missing the newest entry
#[test]
fn the_active_path_includes_what_was_just_appended() {
    let root = tempfile::tempdir().unwrap();
    let mut store = store_at(root.path());
    let session = store.create("/tmp").unwrap();
    let header = store.load(&session).unwrap()[0].id.clone();
    let leaf =
        store.append(&session, Some(header), EntryKind::Message(Message::user("now"))).unwrap();
    let path = store.active_path(&session, &leaf).unwrap();
    assert_eq!(path.last().map(|entry| &entry.id), Some(&leaf));
    assert_eq!(path.len(), 2);
}
