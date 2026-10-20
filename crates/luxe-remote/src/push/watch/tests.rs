use super::*;
use luxe_core::event::Action;
use luxe_core::local_endpoint::{EndpointListener, Joined, Publication};
use tokio::io::AsyncWriteExt;

const PERMISSION: &str = r#"{"kind":"permission_requested","content":{"id":"perm:1","session_id":"session-1","tool":"bash","permission":"bash","summary":"Run: rm -rf build","always_patterns":["rm *"]}}"#;
const QUESTION: &str = r#"{"kind":"question_asked","content":{"id":"ask:1","session_id":"session-1","prompt":"Which database?","options":[],"multi":false}}"#;
const CLOSED: &str = r#"{"kind":"prompt_closed","content":{"id":"perm:1"}}"#;

/// A session's endpoint, and the folder it's published in.
struct Session {
    listener: EndpointListener,
    live: LiveSession,
    runtime: tempfile::TempDir,
}

fn session() -> Session {
    let runtime = tempfile::tempdir().unwrap();
    let listener = EndpointListener::bind(runtime.path()).unwrap();
    let live = LiveSession {
        pid: std::process::id(),
        session_id: "session-1".into(),
        cwd: "/work/luxe-dev".into(),
        address: listener.address().to_string(),
        luxe_version: "0.1.3".into(),
    };
    Session { listener, live, runtime }
}

/// Starts a watcher on `session`, and takes its notices and the session's end of the stream.
async fn watched(session: &mut Session) -> (mpsc::Receiver<Notice>, Joined, Watcher) {
    let (notices, heard) = mpsc::channel(8);
    let watcher = Watcher(tokio::spawn(watch_session(session.live.clone(), notices)));
    let joined = session.listener.accept().await.unwrap();
    (heard, joined, watcher)
}

async fn say(joined: &mut Joined, lines: &[&str]) {
    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    joined.stream.write_all(text.as_bytes()).await.unwrap();
}

/// The watcher opens as watch-only, so the terminal doesn't announce it, and then says nothing.
///
/// The break this catches: a watcher that joins as an ordinary client (the terminal says
/// "Notifications joined this session" in every session) or that sends the session anything else,
/// an answer included.
#[tokio::test(start_paused = true)]
async fn the_watcher_introduces_itself_and_says_nothing_else() {
    let mut session = session();
    let (_heard, joined, _watcher) = watched(&mut session).await;
    let mut lines = BufReader::new(joined.stream).lines();
    let first = lines.next_line().await.unwrap().unwrap();
    let introduced: Action = serde_json::from_str(&first).unwrap();
    let watcher =
        Action::Introduce { name: "Notifications".into(), watch_only: true, yours: false };
    assert_eq!(introduced, watcher);
    let more = tokio::time::timeout(Duration::from_secs(60), lines.next_line()).await;
    assert!(more.is_err(), "the watcher sent {more:?}");
}

/// The break this catches: a prompt sent the moment it's asked, buzzing a phone while you answer
/// at the desk, or never sent at all.
#[tokio::test(start_paused = true)]
async fn a_prompt_is_sent_once_it_has_waited() {
    let mut session = session();
    let (mut heard, mut joined, _watcher) = watched(&mut session).await;
    let asked = Instant::now();
    say(&mut joined, &[PERMISSION]).await;
    let notice = heard.recv().await.expect("a notice");
    assert_eq!((notice.title.as_str(), notice.body.as_str()), ("luxe-dev", "Approve bash"));
    assert!(asked.elapsed() >= PROMPT_GRACE, "sent after {:?}", asked.elapsed());
}

/// The break this catches: a prompt answered at the desk still reaching the phone, or one answer
/// letting every held prompt go.
#[tokio::test(start_paused = true)]
async fn a_prompt_answered_in_time_is_never_sent() {
    let mut session = session();
    let (mut heard, mut joined, _watcher) = watched(&mut session).await;
    say(&mut joined, &[PERMISSION, QUESTION, CLOSED]).await;
    let notice = heard.recv().await.expect("a notice");
    assert_eq!(notice.body, "Luxe has a question");
    tokio::time::sleep(PROMPT_GRACE * 2).await;
    assert!(heard.try_recv().is_err(), "the answered prompt was sent");
}

/// The break this catches: a watcher that outlives its session, holding a task (and in time a
/// file handle) for every session that ever ran.
#[tokio::test(start_paused = true)]
async fn the_watcher_ends_with_its_session() {
    let mut session = session();
    let (_heard, joined, watcher) = watched(&mut session).await;
    // once it has introduced itself it's reading, rather than failing to join
    let mut lines = BufReader::new(joined.stream).lines();
    lines.next_line().await.unwrap().expect("an introduction");
    drop(lines);
    tokio::time::timeout(Duration::from_secs(5), async {
        while !watcher.0.is_finished() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the watcher stopped");
}

/// A book with one paired device, and that device's id.
fn paired() -> (tempfile::TempDir, DeviceBook, String) {
    let dir = tempfile::tempdir().unwrap();
    let book = DeviceBook::at(dir.path().join("remote.json"));
    let token = book.invite().unwrap();
    let (device, _) = book.pair(&token, "iPhone", "alex@github").unwrap();
    (dir, book, device.id)
}

fn subscribe(book: &DeviceBook, id: &str, on: bool) {
    let subscription = on.then(|| serde_json::from_str(crate::push::FROM_SAFARI).unwrap());
    book.subscribe(id, subscription).unwrap();
}

/// Whether the watcher joins `session` within a few checks.
async fn joins(session: &mut Session) -> Option<Joined> {
    tokio::time::timeout(WATCH_CHECK * 3, session.listener.accept()).await.ok()?.ok()
}

/// The break this catches: a bridge that sits in every session whether or not anyone wants
/// notifications (a "Notifications joined" line in every terminal), or that never joins.
#[tokio::test(start_paused = true)]
async fn sessions_are_watched_only_while_a_device_is_subscribed() {
    let mut session = session();
    let _record: Publication = session.live.publish(session.runtime.path()).unwrap();
    let (_dir, book, id) = paired();
    let (notices, _heard) = mpsc::channel(8);
    let runtime = session.runtime.path().to_path_buf();
    let _watching = Watcher(tokio::spawn(watch_sessions(runtime, book.clone(), notices)));
    assert!(joins(&mut session).await.is_none(), "joined with nobody subscribed");

    subscribe(&book, &id, true);
    let joined = joins(&mut session).await.expect("joined once a device subscribed");
    subscribe(&book, &id, false);
    let mut lines = BufReader::new(joined.stream).lines();
    let _introduction = lines.next_line().await.unwrap();
    let ended = tokio::time::timeout(WATCH_CHECK * 3, lines.next_line()).await;
    assert!(matches!(ended, Ok(Ok(None))), "still watching after the last device left: {ended:?}");
}
