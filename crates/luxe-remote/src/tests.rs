use std::net::SocketAddr;

use futures::{SinkExt, StreamExt};
use luxe_core::local_endpoint::{EndpointListener, LiveSession, Publication};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use super::*;

mod access;
mod app;
mod closing;
mod home_screen;
mod push;
mod render;

/// The device's end of a joined session socket.
type DeviceSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

const SESSION: &str = "5898d57b-0000-4000-8000-000000000001";
const OWNER: &str = "alex@github";

/// A session as a running Luxe shares it: an endpoint, and its record beside it.
struct FakeSession {
    listener: EndpointListener,
    _record: Publication,
}

fn fake_session(runtime: &std::path::Path) -> FakeSession {
    let listener = EndpointListener::bind(runtime).expect("bind the endpoint");
    let record = LiveSession {
        pid: std::process::id(),
        session_id: SESSION.into(),
        cwd: "/work/app".into(),
        address: listener.address().to_string(),
        luxe_version: "0.1.3".into(),
    };
    let _record = record.publish(runtime).expect("publish the record");
    FakeSession { listener, _record }
}

/// A stand-in for `luxe --rpc`: it publishes a record for itself, then runs until stdin closes.
#[cfg(unix)]
pub(crate) fn fake_luxe(runtime: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = runtime.join("fake-luxe");
    let record = r#"{"pid":%s,"session_id":"started","cwd":"%s","address":"","luxe_version":"0"}"#;
    let body = format!(
        "#!/bin/sh\nprintf '{record}' $$ \"$PWD\" > \"{dir}/$$.json\"\ncat > /dev/null\n",
        dir = runtime.display()
    );
    std::fs::write(&script, body).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

/// No stand-in on Windows, where only the tests that never start one run.
#[cfg(not(unix))]
pub(crate) fn fake_luxe(runtime: &std::path::Path) -> std::path::PathBuf {
    runtime.join("no-luxe-here")
}

/// A bridge on a free port, and the folders it reads.
struct Harness {
    address: SocketAddr,
    runtime: tempfile::TempDir,
    devices: DeviceBook,
    presence: crate::push::Presence,
    _config: tempfile::TempDir,
}

async fn harness() -> Harness {
    harness_with(|bridge| bridge).await
}

/// [`harness`], drawing through `renderer`.
async fn harness_rendering(renderer: std::sync::Arc<dyn Renderer>) -> Harness {
    harness_with(move |bridge| bridge.rendering_with(renderer)).await
}

/// [`harness`], with the bridge set up by `configure` before it serves.
async fn harness_with(configure: impl FnOnce(Bridge) -> Bridge) -> Harness {
    let runtime = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    let devices = DeviceBook::at(config.path().join("remote.json"));
    let access = Access { logins: vec![OWNER.into()], devices: devices.clone() };
    let bridge = Bridge::bind(([127, 0, 0, 1], 0).into(), runtime.path().into(), access);
    let book = folders::FolderBook::at(config.path().join("remote-folders.json"));
    let launcher = launch::Launcher::new(fake_luxe(runtime.path()), launch::Herdr::Off, book);
    let bridge = configure(bridge.await.unwrap().with_launcher(launcher));
    let (address, presence) = (bridge.address(), bridge.presence());
    tokio::spawn(bridge.serve(std::future::pending()));
    Harness { address, runtime, devices, presence, _config: config }
}

impl Harness {
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }

    /// A request as Tailscale Serve forwards one from the owner.
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new().get(self.url(path)).header("Tailscale-User-Login", OWNER)
    }

    /// A paired device asking for a session in `folder`.
    async fn start_in(&self, folder: &str) -> reqwest::Response {
        let cookie = self.pair().await;
        let request = reqwest::Client::new().post(self.url("/api/sessions"));
        let request = request.header("Tailscale-User-Login", OWNER).header("cookie", cookie);
        request.json(&serde_json::json!({ "folder": folder })).send().await.unwrap()
    }

    /// Pairs a device through the pairing endpoint, returning its cookie.
    async fn pair(&self) -> String {
        let invitation = self.devices.invite().unwrap();
        let response = reqwest::Client::new()
            .post(self.url("/api/pair"))
            .header("Tailscale-User-Login", OWNER)
            .json(&serde_json::json!({ "invitation": invitation, "name": "iPhone" }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "pairing failed: {:?}", response.text().await);
        let cookie = response.headers()["set-cookie"].to_str().unwrap();
        cookie.split(';').next().unwrap().to_string()
    }

    /// A socket request for `session`, as the paired device sends it.
    fn socket_request(
        &self,
        session: &str,
        cookie: &str,
    ) -> tokio_tungstenite::tungstenite::handshake::client::Request {
        let url = format!("ws://{}/api/sessions/{session}/socket", self.address);
        let mut request = url.into_client_request().unwrap();
        request.headers_mut().insert("tailscale-user-login", OWNER.parse().unwrap());
        request.headers_mut().insert("cookie", cookie.parse().unwrap());
        request
    }

    /// Pairs a device and joins `session` with it: the device's socket, and the session's end.
    async fn join(
        &self,
        session: &mut FakeSession,
    ) -> (DeviceSocket, luxe_core::local_endpoint::Joined) {
        let cookie = self.pair().await;
        let request = self.socket_request(SESSION, &cookie);
        let (device, _) = tokio_tungstenite::connect_async(request).await.expect("joined");
        let joined = session.listener.accept().await.expect("the bridge connected");
        (device, joined)
    }
}

/// The bridge will not listen beyond this machine, whatever it is asked.
///
/// The break this catches: binding what it is given, so `--port` with a wider address, or a
/// changed default, puts every session on the local network with only the device check in front.
#[tokio::test]
async fn the_bridge_listens_on_loopback_only() {
    let dir = tempfile::tempdir().unwrap();
    let access = || Access { logins: vec![], devices: DeviceBook::at(dir.path().join("r.json")) };
    let wide = Bridge::bind(([0, 0, 0, 0], 0).into(), dir.path().into(), access()).await;
    assert!(matches!(wide, Err(RemoteError::NotLoopback(_))));
    let narrow = Bridge::bind(([127, 0, 0, 1], 0).into(), dir.path().into(), access()).await;
    assert!(narrow.is_ok());
}

/// The list names running sessions and keeps their local address on this machine.
///
/// The break this catches: listing a record whose Luxe has died, which the app shows as a session
/// that fails to open; or sending the endpoint's path, which means nothing off this machine.
#[tokio::test]
async fn the_list_shows_running_sessions_without_their_address() {
    let bridge = harness().await;
    let _live = fake_session(bridge.runtime.path());
    let dead = LiveSession {
        pid: dead_pid(),
        session_id: "dead".into(),
        cwd: "/work/old".into(),
        address: "nowhere".into(),
        luxe_version: "0.1.2".into(),
    };
    let _dead = dead.publish(bridge.runtime.path()).unwrap();

    let cookie = bridge.pair().await;
    let listed = bridge.get("/api/sessions").header("cookie", cookie).send().await.unwrap();
    let listed: serde_json::Value = listed.json().await.unwrap();
    let pid = std::process::id();
    let expected = serde_json::json!([
        { "session_id": SESSION, "pid": pid, "cwd": "/work/app", "luxe_version": "0.1.3" }
    ]);
    assert_eq!(listed, expected);
}

/// The pid of a process that has exited: this test binary, asked only to list its tests.
fn dead_pid() -> u32 {
    let mut gone = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--list")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    gone.wait().unwrap();
    gone.id()
}

/// A device's socket and the session's endpoint carry each other's lines, after the bridge has
/// introduced the device by the name it paired with.
///
/// The break this catches: either direction dropped or reframed, so the app sees nothing or the
/// session reads a frame as several broken actions; or no introduction, so the terminal names
/// the bridge's process instead of the phone; or the owner's own phone not said to be theirs, so
/// its every reconnect is announced.
#[tokio::test]
async fn a_device_socket_relays_the_protocol_both_ways() {
    let bridge = harness().await;
    let mut session = fake_session(bridge.runtime.path());
    let cookie = bridge.pair().await;
    let request = bridge.socket_request(SESSION, &cookie);
    let (mut device, _) = tokio_tungstenite::connect_async(request).await.expect("joined");
    let joined = session.listener.accept().await.expect("the bridge connected");
    let (from_device, mut to_device) = tokio::io::split(joined.stream);

    to_device.write_all(b"{\"kind\":\"ready\"}\n").await.unwrap();
    let frame = device.next().await.expect("a frame").expect("readable");
    assert_eq!(frame, Message::Text("{\"kind\":\"ready\"}".into()));

    device.send(Message::Text("{\n\"kind\": \"cancel\"\n}".into())).await.unwrap();
    let mut lines = BufReader::new(from_device).lines();
    let introduction = r#"{"kind":"introduce","content":{"name":"iPhone","yours":true}}"#;
    assert_eq!(lines.next_line().await.unwrap().as_deref(), Some(introduction));
    assert_eq!(lines.next_line().await.unwrap().as_deref(), Some("{ \"kind\": \"cancel\" }"));

    device.close(None).await.unwrap();
    assert_eq!(lines.next_line().await.unwrap(), None, "the device left, so the bridge did");
}

/// A session older than `introduce` refuses it, and the device never sees that refusal, only what
/// follows.
///
/// The break this catches: joining a session started before an update shows "Refused: no such
/// action `introduce`" in the phone's transcript, for a frame the bridge sent, not the phone.
#[tokio::test]
async fn an_older_sessions_refusal_of_the_introduction_stays_off_the_device() {
    let bridge = harness().await;
    let mut session = fake_session(bridge.runtime.path());
    let cookie = bridge.pair().await;
    let request = bridge.socket_request(SESSION, &cookie);
    let (mut device, _) = tokio_tungstenite::connect_async(request).await.expect("joined");
    let joined = session.listener.accept().await.expect("the bridge connected");
    let (_, mut to_device) = tokio::io::split(joined.stream);
    let refusal = r#"{"kind":"action_rejected","content":{"kind":"introduce","reason":"no such action `introduce`"}}"#;
    to_device.write_all(format!("{refusal}\n{{\"kind\":\"ready\"}}\n").as_bytes()).await.unwrap();
    let frame = device.next().await.expect("a frame").expect("readable");
    assert_eq!(frame, Message::Text("{\"kind\":\"ready\"}".into()), "the refusal was held back");
}

/// A paired device starts a session in a folder, and is told which one it can join.
///
/// The break this catches: answering before the session is listed, or with another session, so
/// the app's join that follows finds nothing to join.
#[cfg(unix)]
#[tokio::test]
async fn a_device_starts_a_session_in_a_folder() {
    let bridge = harness().await;
    let folder = tempfile::tempdir().unwrap();
    let started = bridge.start_in(&folder.path().to_string_lossy()).await;
    assert_eq!(started.status(), 201);
    let started: serde_json::Value = started.json().await.unwrap();
    assert_eq!(started["session"]["session_id"], "started");
    assert_eq!(started["place"], "bridge");
}

/// A folder that isn't there is the device's mistake, said in words it can show.
///
/// The break this catches: launching anyway and waiting out the timeout, or answering with a
/// server error the app would offer to retry.
#[tokio::test]
async fn starting_in_a_folder_that_isnt_there_is_refused() {
    let bridge = harness().await;
    let missing = bridge.runtime.path().join("missing");
    let refused = bridge.start_in(&missing.to_string_lossy()).await;
    assert_eq!(refused.status(), 400);
    assert!(refused.text().await.unwrap().contains("isn't a folder"));
}

/// The folders a device can start in include the ones Luxe is running in, counted.
///
/// The break this catches: listing only remembered folders, so a project open at the desk right
/// now is missing from the phone's list until it's been seen once before.
#[tokio::test]
async fn the_folder_list_names_where_luxe_is_running() {
    let bridge = harness().await;
    let project = tempfile::tempdir().unwrap();
    let running = LiveSession {
        pid: std::process::id(),
        session_id: SESSION.into(),
        cwd: project.path().to_string_lossy().into_owned(),
        address: String::new(),
        luxe_version: "0.1.3".into(),
    };
    let _record = running.publish(bridge.runtime.path()).unwrap();
    let cookie = bridge.pair().await;
    let listed = bridge.get("/api/folders").header("cookie", cookie).send().await.unwrap();
    let listed: serde_json::Value = listed.json().await.unwrap();
    assert_eq!(listed["herdr"], false);
    assert_eq!(listed["folders"][0]["path"], running.cwd.as_str());
    assert_eq!(listed["folders"][0]["sessions"], 1);
}

/// A session that isn't running is a plain HTTP error, before any socket opens.
///
/// The break this catches: upgrading first and closing at once, which the app can't tell apart
/// from a dropped connection worth retrying.
#[tokio::test]
async fn joining_a_session_that_is_not_running_is_not_found() {
    let bridge = harness().await;
    let cookie = bridge.pair().await;
    let request = bridge.socket_request(SESSION, &cookie);
    let refused = tokio_tungstenite::connect_async(request).await.expect_err("nothing to join");
    let tokio_tungstenite::tungstenite::Error::Http(response) = refused else {
        panic!("expected an HTTP refusal, got {refused:?}");
    };
    assert_eq!(response.status(), 404);
}
