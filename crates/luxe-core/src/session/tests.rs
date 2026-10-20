#![allow(clippy::unwrap_used)]

use super::*;
use std::path::PathBuf;

fn temp_root() -> PathBuf {
    let dir = std::env::temp_dir().join("luxe-test-sessions");
    let hash = Uuid::new_v4().to_string();
    dir.join(hash)
}

#[test]
fn session_id_new_is_unique() {
    let a = SessionId::new();
    let b = SessionId::new();
    assert_ne!(a, b);
}

#[test]
fn entry_id_new_is_unique() {
    let a = EntryId::new();
    let b = EntryId::new();
    assert_ne!(a, b);
}

#[test]
fn create_session_writes_header() {
    let root = temp_root();
    let mut store = JsonlSessionStore::new(&root);
    store.set_project_hash("test-project");

    let session = store.create("/tmp/test").unwrap();

    // Verify the file exists.
    let path = store.session_dir(&session);
    assert!(path.exists());

    // Load and verify header.
    let entries = store.load(&session).unwrap();
    assert_eq!(entries.len(), 1);
    match &entries[0].kind {
        EntryKind::Header { cwd, created } => {
            assert_eq!(cwd, "/tmp/test");
            assert!(*created > 0);
        }
        _ => panic!("expected header entry"),
    }

    fs::remove_dir_all(root).ok();
}

#[test]
fn append_and_load() {
    let root = temp_root();
    let mut store = JsonlSessionStore::new(&root);
    store.set_project_hash("test-project");

    let session = store.create("/tmp/test").unwrap();

    // Append a user message.
    let msg = Message::user("hello");
    let parent = store.append(&session, None, EntryKind::Message(msg.clone())).unwrap();

    // Append an assistant reply.
    let reply = Message::assistant("hi there");
    store.append(&session, Some(parent), EntryKind::Message(reply)).unwrap();

    // Load and verify.
    let entries = store.load(&session).unwrap();
    assert_eq!(entries.len(), 3); // header + 2 messages

    match &entries[1].kind {
        EntryKind::Message(msg) => {
            assert_eq!(msg.role, crate::message::Role::User);
            assert_eq!(msg.content.len(), 1);
        }
        _ => panic!("expected message entry"),
    }

    fs::remove_dir_all(root).ok();
}

#[test]
fn active_path_reconstructs_conversation() {
    let root = temp_root();
    let mut store = JsonlSessionStore::new(&root);
    store.set_project_hash("test-project");

    let session = store.create("/tmp/test").unwrap();

    // Build a linear conversation: header -> user -> assistant
    let h_id = {
        let entries = store.load(&session).unwrap();
        entries[0].id.clone()
    };

    let u_id = store.append(&session, Some(h_id), EntryKind::Message(Message::user("q"))).unwrap();
    let a_id =
        store.append(&session, Some(u_id), EntryKind::Message(Message::assistant("a"))).unwrap();

    // Walk from the leaf.
    let path = store.active_path(&session, &a_id).unwrap();
    assert_eq!(path.len(), 3);

    // Verify order (root to leaf).
    match &path[0].kind {
        EntryKind::Header { .. } => {}
        _ => panic!("first should be header"),
    }
    match &path[1].kind {
        EntryKind::Message(msg) => assert_eq!(msg.role, crate::message::Role::User),
        _ => panic!("second should be user message"),
    }
    match &path[2].kind {
        EntryKind::Message(msg) => assert_eq!(msg.role, crate::message::Role::Assistant),
        _ => panic!("third should be assistant message"),
    }

    fs::remove_dir_all(root).ok();
}

/// The break this catches: lending the whole log, or the walk unreversed, rather than the branch
/// from the root to the leaf, which a request built from it would send as the conversation.
#[test]
fn with_active_path_lends_only_the_branch_to_the_leaf() {
    let root = temp_root();
    let mut store = JsonlSessionStore::new(&root);
    store.set_project_hash("test-project");
    let session = store.create("/tmp/test").unwrap();
    let header = store.load(&session).unwrap()[0].id.clone();
    let question = Message::user("q");
    let user = store.append(&session, Some(header.clone()), EntryKind::Message(question)).unwrap();
    let other = EntryKind::Message(Message::assistant("other"));
    store.append(&session, Some(user.clone()), other).unwrap();
    let kept = EntryKind::Message(Message::assistant("kept"));
    let leaf = store.append(&session, Some(user.clone()), kept).unwrap();

    let lent = with_active_path(&store, &session, &leaf, |path| {
        path.iter().map(|entry| entry.id.clone()).collect::<Vec<_>>()
    });

    assert_eq!(lent.unwrap(), vec![header, user, leaf]);
    fs::remove_dir_all(root).ok();
}

/// The break this catches: a leaf the log lacks lending a shortened path instead of failing, so a
/// request would go out without the conversation behind it.
#[test]
fn with_active_path_refuses_a_leaf_the_log_lacks() {
    let root = temp_root();
    let mut store = JsonlSessionStore::new(&root);
    store.set_project_hash("test-project");
    let session = store.create("/tmp/test").unwrap();
    let missing = EntryId::new();

    let lent = with_active_path(&store, &session, &missing, |path| path.len());

    assert!(matches!(&lent, Err(SessionError::EntryNotFound(id)) if *id == missing), "{lent:?}");
    fs::remove_dir_all(root).ok();
}

#[test]
fn branching_two_children() {
    let root = temp_root();
    let mut store = JsonlSessionStore::new(&root);
    store.set_project_hash("test-project");

    let session = store.create("/tmp/test").unwrap();

    // header -> user -> assistant (shared ancestor)
    let h_id = {
        let entries = store.load(&session).unwrap();
        entries[0].id.clone()
    };
    let u_id = store.append(&session, Some(h_id), EntryKind::Message(Message::user("q"))).unwrap();
    let a_id =
        store.append(&session, Some(u_id), EntryKind::Message(Message::assistant("a1"))).unwrap();

    // Two branches from the assistant message.
    let b1_id = store
        .append(&session, Some(a_id.clone()), EntryKind::Message(Message::assistant("b1")))
        .unwrap();
    let b2_id =
        store.append(&session, Some(a_id), EntryKind::Message(Message::assistant("b2"))).unwrap();

    // Path 1: header -> user -> assistant -> b1
    let path1 = store.active_path(&session, &b1_id).unwrap();
    assert_eq!(path1.len(), 4);
    match &path1[3].kind {
        EntryKind::Message(msg) => {
            assert_eq!(msg.content[0], crate::message::ContentBlock::Text("b1".into()))
        }
        _ => panic!("expected message"),
    }

    // Path 2: header -> user -> assistant -> b2
    let path2 = store.active_path(&session, &b2_id).unwrap();
    assert_eq!(path2.len(), 4);
    match &path2[3].kind {
        EntryKind::Message(msg) => {
            assert_eq!(msg.content[0], crate::message::ContentBlock::Text("b2".into()))
        }
        _ => panic!("expected message"),
    }

    // Paths share the first 3 entries.
    for i in 0..3 {
        assert_eq!(path1[i].id, path2[i].id);
    }

    fs::remove_dir_all(root).ok();
}

#[test]
fn resume_after_reopen() {
    let root = temp_root();
    let mut store = JsonlSessionStore::new(&root);
    store.set_project_hash("test-project");

    let session = store.create("/tmp/test").unwrap();

    // Build a conversation.
    let h_id = {
        let entries = store.load(&session).unwrap();
        entries[0].id.clone()
    };
    let u_id = store.append(&session, Some(h_id), EntryKind::Message(Message::user("q"))).unwrap();
    let a_id =
        store.append(&session, Some(u_id), EntryKind::Message(Message::assistant("a"))).unwrap();

    // Drop the store and reopen.
    drop(store);
    let mut store2 = JsonlSessionStore::new(&root);
    store2.set_project_hash("test-project");

    // Load should still work.
    let entries = store2.load(&session).unwrap();
    assert_eq!(entries.len(), 3);

    // Active path should reconstruct correctly.
    let path = store2.active_path(&session, &a_id).unwrap();
    assert_eq!(path.len(), 3);

    fs::remove_dir_all(root).ok();
}

#[test]
fn corrupt_jsonl_fails_loud() {
    let root = temp_root();
    let mut store = JsonlSessionStore::new(&root);
    store.set_project_hash("test-project");

    let session = store.create("/tmp/test").unwrap();

    // Append a valid entry.
    store.append(&session, None, EntryKind::Message(Message::user("ok"))).unwrap();

    // Corrupt the file by appending invalid JSON.
    let path = store.session_dir(&session);
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"not valid json\n")
        .unwrap();

    // Load should fail with a parse error.
    let result = store.load(&session);
    assert!(result.is_err());

    fs::remove_dir_all(root).ok();
}

/// The `.jsonl` on disk outlives the build that wrote it, so its shape is a compatibility
/// promise rather than an implementation detail.
///
/// Deliberately not a round-trip. Serializing a value and reading it straight back exercises
/// both halves of the *current* code, so a renamed field or a retagged variant passes
/// unnoticed — both sides moved together. What actually breaks is a session written by an
/// earlier build, and only a literal from that era can catch it.
#[test]
fn the_on_disk_entry_shape_is_frozen() {
    // These literals are copied from a real `.luxe/sessions/*.jsonl`, not written from what
    // the struct looks like it ought to produce. Guessing is how this test was first written
    // asserting a flat `kind`/`data` pair, which the code has never emitted.
    let stored = r#"{"id":"e1","parent_id":null,"ts":1234567890,"kind":{"kind":"header","data":{"cwd":"/tmp/project","created":1234567890}}}"#;
    let entry: Entry = serde_json::from_str(stored).expect("an older header still loads");
    assert_eq!(entry.id, EntryId("e1".into()));
    assert_eq!(entry.parent_id, None);
    assert_eq!(entry.ts, 1234567890);
    let EntryKind::Header { cwd, created } = entry.kind else { panic!("expected a header") };
    assert_eq!(cwd, "/tmp/project");
    assert_eq!(created, 1234567890);

    let stored = r#"{"id":"e2","parent_id":"e1","ts":1,"kind":{"kind":"message","data":{"role":"user","content":[{"type":"text","content":"hello"}]}}}"#;
    let entry: Entry = serde_json::from_str(stored).expect("an older message still loads");
    assert_eq!(entry.parent_id, Some(EntryId("e1".into())));
    let EntryKind::Message(message) = entry.kind else { panic!("expected a message") };
    assert_eq!(message.role, crate::message::Role::User);
    assert_eq!(message.text(), "hello");

    // And what we write today is what we just promised to keep reading.
    let written = serde_json::to_string(&Entry {
        id: EntryId("e3".into()),
        parent_id: Some(EntryId("e2".into())),
        ts: 7,
        kind: EntryKind::Header { cwd: "/w".into(), created: 7 },
    })
    .unwrap();
    assert_eq!(
        written,
        r#"{"id":"e3","parent_id":"e2","ts":7,"kind":{"kind":"header","data":{"cwd":"/w","created":7}}}"#
    );

    // A link to the pull request a session worked on, copied from a real round trip that closed
    // one: `luxe --from-pr` reads exactly this
    let stored = r#"{"id":"f25fd65c-ee99-4db8-9ed9-76aa08ed7fd9","parent_id":"035e7a31-2d6b-410a-b1d1-361920bf0615","ts":1790289080,"kind":{"kind":"pull_request_link","data":{"number":146,"url":"https://github.com/nora-quinn/luxe-dev/pull/146"}}}"#;
    let entry: Entry = serde_json::from_str(stored).unwrap();
    assert_eq!(
        entry.kind,
        EntryKind::PullRequestLink {
            number: 146,
            url: "https://github.com/nora-quinn/luxe-dev/pull/146".into()
        }
    );
    assert_eq!(serde_json::to_string(&entry).unwrap(), stored);

    // A session's worktree, copied from the real check that made, resumed and removed one:
    // resuming reads exactly this to know which branch to re-enter
    let stored = r#"{"id":"3508477e-67b5-48a6-880f-364e78b3a263","parent_id":"557b8a9d-0a0d-416d-9ba0-3570271e0b58","ts":1790294351,"kind":{"kind":"worktree","data":{"branch":"wt-check","path":".luxe/worktrees/wt-check"}}}"#;
    let entry: Entry = serde_json::from_str(stored).unwrap();
    assert_eq!(
        entry.kind,
        EntryKind::Worktree { branch: "wt-check".into(), path: ".luxe/worktrees/wt-check".into() }
    );
    assert_eq!(serde_json::to_string(&entry).unwrap(), stored);

    // The issue a session took up, copied from the real `luxe --issue 151`: a pull request
    // opened from the session reads exactly this to say what it closes
    let stored = r#"{"id":"e83c41d6-04c1-409a-82b6-4a23398220a1","parent_id":"76abaff5-9102-42ab-913b-65192df21a92","ts":1790310478,"kind":{"kind":"issue_link","data":{"number":151,"url":"https://github.com/nora-quinn/luxe-dev/issues/151"}}}"#;
    let entry: Entry = serde_json::from_str(stored).unwrap();
    assert_eq!(
        entry.kind,
        EntryKind::IssueLink {
            number: 151,
            url: "https://github.com/nora-quinn/luxe-dev/issues/151".into()
        }
    );
    assert_eq!(serde_json::to_string(&entry).unwrap(), stored);

    // A cache refresh's spend, copied from the live check that kept an Opus 4.8 turn's cache
    // alive through `sleep 300`: its own kind, so an older build skips it as `Unknown` where a
    // `usage` entry with a source it can't parse would fail the whole session load
    let stored = r#"{"id":"f9de6ec2-99c5-4c9f-bc72-2eb976e3507c","parent_id":"cc47a2e6-c9dc-4e8d-b506-5efc02e844f6","ts":1791214401,"kind":{"kind":"cache_warm","data":{"model":"claude-opus-4-8","totals":{"calls":1,"prompt_tokens":11512,"completion_tokens":1,"cache_read_tokens":11510,"cache_write_tokens":0,"reasoning_tokens":0,"cost_usd":0.00579,"unpriced_calls":0,"plan_usd":0.0,"plan_calls":0}}}}"#;
    let entry: Entry = serde_json::from_str(stored).unwrap();
    let EntryKind::CacheWarm { model, totals } = &entry.kind else {
        panic!("expected a cache refresh, got {:?}", entry.kind)
    };
    assert_eq!(model, "claude-opus-4-8");
    assert_eq!((totals.cache_read_tokens, totals.cache_write_tokens), (11_510, 0));
    assert_eq!(serde_json::to_string(&entry).unwrap(), stored);
}

/// Corruption inside a kind we *do* know must still fail loudly — the unknown-kind fallback
/// exists for tags from the future, not as a way to quietly accept a damaged transcript.
#[test]
fn a_known_kind_written_wrong_still_fails_loud() {
    let broken =
        r#"{"id":"e1","parent_id":null,"ts":1,"kind":{"kind":"header","data":{"cwd":42}}}"#;
    assert!(
        serde_json::from_str::<Entry>(broken).is_err(),
        "a malformed header must not degrade to Unknown"
    );
}

/// [`EntryKind::Unknown`] makes three promises about a log a newer luxe has written to: it
/// stays readable, the unrecognized entry is kept rather than dropped, and the parent chain
/// still walks through it. All three were documented; none were tested.
#[test]
fn an_entry_from_a_newer_luxe_does_not_break_the_transcript() {
    let root = temp_root();
    let mut store = JsonlSessionStore::new(&root);
    store.set_project_hash("forward-compat");
    let session = store.create("/tmp/test").unwrap();
    let header = store.load(&session).unwrap()[0].id.clone();
    let first =
        store.append(&session, Some(header), EntryKind::Message(Message::user("before"))).unwrap();

    // A kind this build has never heard of, appended the way a newer one would.
    let path = store.session_dir(&session);
    let future = format!(
        "{{\"id\":\"future-1\",\"parent_id\":\"{first}\",\"ts\":9,\"kind\":{{\"kind\":\"telemetry_v2\",\"data\":{{\"whatever\":true}}}}}}\n"
    );
    fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(future.as_bytes()).unwrap();

    let entries = store.load(&session).expect("an unknown kind must not fail the load");
    let unknown = entries
        .iter()
        .find(|entry| entry.id == EntryId("future-1".into()))
        .expect("the unrecognized entry is kept, not dropped");
    assert_eq!(unknown.kind, EntryKind::Unknown);
    assert_eq!(
        unknown.parent_id,
        Some(first.clone()),
        "its parent link survives, which is what keeps resume walking"
    );

    // Resume chains onto it, and the conversation either side is still reachable.
    let after = store
        .append(
            &session,
            Some(EntryId("future-1".into())),
            EntryKind::Message(Message::assistant("after")),
        )
        .unwrap();
    let said: Vec<String> = store
        .active_path(&session, &after)
        .unwrap()
        .iter()
        .filter_map(|entry| match &entry.kind {
            EntryKind::Message(message) => Some(message.text()),
            _ => None,
        })
        .collect();
    assert_eq!(said, vec!["before", "after"], "the transcript spans the unknown entry");

    fs::remove_dir_all(root).ok();
}

#[test]
fn most_recent_session_picks_newest() {
    let root = temp_root();
    let mut store = JsonlSessionStore::new(&root);
    store.set_project_hash("test-project");

    let first = store.create("/tmp/test").unwrap();
    // Ensure a distinct mtime for the second file.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let second = store.create("/tmp/test").unwrap();

    assert!(store.session_path(&first).unwrap().exists());
    assert_eq!(store.most_recent_session().as_ref(), Some(&second));

    // A fresh store rescans from disk and still finds the newest.
    let reopened = JsonlSessionStore::new(&root);
    let mut reopened = reopened;
    reopened.set_project_hash("test-project");
    assert_eq!(reopened.most_recent_session().as_ref(), Some(&second));

    fs::remove_dir_all(root).ok();
}

#[test]
fn most_recent_session_empty_is_none() {
    let root = temp_root();
    let mut store = JsonlSessionStore::new(&root);
    store.set_project_hash("empty-project");
    assert!(store.most_recent_session().is_none());
    assert!(store.session_path(&SessionId("nope".into())).is_none());
}

/// A compaction is appended *after* the conversation and keeps it reachable. Chaining it under
/// `first_kept` instead — which is what it used to do — forks the branch: the summary lands on
/// a twig nothing points at, and the entries after `first_kept` end up on the other side of the
/// fork, so the leaf that resume finds walks a path missing half the session.
#[test]
fn a_compaction_extends_the_conversation_rather_than_forking_it() {
    let root = temp_root();
    let mut store = JsonlSessionStore::new(root);

    let session_id = store.create("/tmp/test").unwrap();
    // Chained onto the header, the way a real turn chains onto whatever the leaf was.
    let mut parent = latest_leaf(&store.load(&session_id).unwrap());
    let mut ids = Vec::new();
    for index in 0..4 {
        let message = EntryKind::Message(Message::user(format!("m{index}")));
        let id = store.append(&session_id, parent.clone(), message).unwrap();
        parent = Some(id.clone());
        ids.push(id);
    }
    let leaf = ids.last().cloned();
    let first_kept = ids[2].clone();

    let compacted = store
        .compact(&session_id, leaf, "## Goal\nTest compaction.\n", &first_kept, 9_000)
        .unwrap();

    let entries = store.load(&session_id).unwrap();
    assert_eq!(entries.len(), 6, "header + four messages + compaction");
    match &entries[5].kind {
        EntryKind::Compaction { summary, first_kept: kept, tokens_before } => {
            assert_eq!(summary, "## Goal\nTest compaction.\n");
            assert_eq!(kept, &first_kept);
            assert_eq!(*tokens_before, 9_000, "what it replaced, not a placeholder zero");
        }
        other => panic!("expected a Compaction entry, got {other:?}"),
    }

    // It is the tip, and everything written before it is still reachable through it.
    assert_eq!(latest_leaf(&entries).as_ref(), Some(&compacted));
    let path = store.active_path(&session_id, &compacted).unwrap();
    assert_eq!(path.len(), 6, "nothing is stranded on a fork: {path:?}");
    assert_eq!(path.last().map(|entry| &entry.id), Some(&compacted));
}
