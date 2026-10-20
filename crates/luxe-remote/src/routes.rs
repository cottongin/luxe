//! Routes — what the bridge answers, and the one place a request is turned into a session.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Path, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};

use crate::access::{require_device, require_login};
use crate::devices::{Device, DeviceBook};
use crate::folders::Folder;
use crate::launch::{self, LaunchError, Launcher, Place};
use crate::push::api as push;
use crate::sessions::{self, SessionSummary};
use crate::REVOCATION_CHECK;

/// What every route reads.
pub(crate) struct Bridged {
    /// Where running Luxes record their sessions
    pub(crate) runtime_dir: PathBuf,
    /// Tailnet logins allowed in
    pub(crate) logins: Vec<String>,
    /// The paired devices, read fresh on each request
    pub(crate) devices: DeviceBook,
    /// What starts sessions, when this bridge may
    pub(crate) launcher: Option<Launcher>,
    /// What turns messages into the app's HTML
    pub(crate) renderer: Arc<dyn crate::Renderer>,
    /// Which devices have which sessions open, so neither is notified about what it's showing
    pub(crate) presence: crate::push::Presence,
    /// The computer's tailnet name, when Tailscale gave one, which names the phone's app for it
    pub(crate) machine: Option<String>,
}

/// Where an invitation link lands.
const PAIR_PAGE: &str = include_str!("pair.html");

/// The most a render request may carry. A diff or a file comes back to be rendered, and core caps
/// each at 2 MiB of text, which as a diff's JSON (an object per line, with its numbers) is several
/// times that.
const RENDER_BODY_LIMIT: usize = 16 * 1024 * 1024;

pub(crate) fn router(bridged: Bridged) -> Router {
    let bridged = Arc::new(bridged);
    Router::new()
        .route("/", get(crate::app::index))
        .route("/app/{*path}", get(crate::app::file))
        .route("/service-worker.js", get(crate::app::service_worker))
        .route("/theme.css", get(crate::app::theme))
        .route("/manifest.webmanifest", get(crate::home_screen::manifest))
        .route("/icons/{file}", get(crate::home_screen::icon))
        .route("/pair", get(|| async { Html(PAIR_PAGE) }))
        .route("/api/pair", post(crate::pairing::pair))
        .merge(paired_only(&bridged))
        .layer(from_fn_with_state(bridged.clone(), require_login))
        .with_state(bridged)
}

/// What only a paired device reaches: its sessions and everything about them.
fn paired_only(bridged: &Arc<Bridged>) -> Router<Arc<Bridged>> {
    Router::new()
        .route("/api/sessions", get(list_sessions).post(start_session))
        .route("/api/folders", get(list_folders))
        .route("/api/render", post(render).layer(DefaultBodyLimit::max(RENDER_BODY_LIMIT)))
        .route("/api/sessions/{session_id}/socket", get(join_session))
        .route("/api/push", get(push::status).put(push::subscribe).delete(push::unsubscribe))
        .route_layer(from_fn_with_state(bridged.clone(), require_device))
}

async fn list_sessions(State(bridged): State<Arc<Bridged>>) -> Json<Vec<SessionSummary>> {
    let runtime_dir = bridged.runtime_dir.clone();
    // reads a folder and, on Windows, asks `tasklist` about each pid
    let running = tokio::task::spawn_blocking(move || sessions::running(&runtime_dir)).await;
    let running = running.unwrap_or_default();
    if let Some(launcher) = &bridged.launcher {
        launcher.saw(running.iter().map(|session| session.cwd.clone()).collect()).await;
    }
    Json(running)
}

/// Renders what the app asks for, off the async threads: highlighting a long block is real work.
async fn render(
    State(bridged): State<Arc<Bridged>>,
    Json(request): Json<crate::render::RenderRequest>,
) -> Response {
    let renderer = bridged.renderer.clone();
    let pieces = request.pieces;
    let html = tokio::task::spawn_blocking(move || {
        pieces.iter().map(|piece| crate::render::render(&*renderer, piece)).collect()
    });
    match html.await {
        Ok(html) => Json(crate::render::Rendered { html }).into_response(),
        Err(error) => {
            tracing::error!(%error, "rendering failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "rendering failed").into_response()
        }
    }
}

#[derive(serde::Serialize)]
struct Folders {
    folders: Vec<Folder>,
    /// Whether a session started now opens in herdr
    herdr: bool,
}

async fn list_folders(State(bridged): State<Arc<Bridged>>) -> Response {
    let Some(launcher) = &bridged.launcher else {
        return (StatusCode::NOT_IMPLEMENTED, "this bridge doesn't start sessions").into_response();
    };
    let (folders, herdr) = launcher.folders(&bridged.runtime_dir).await;
    Json(Folders { folders, herdr }).into_response()
}

#[derive(serde::Deserialize)]
struct StartRequest {
    folder: String,
}

#[derive(serde::Serialize)]
struct Started {
    session: SessionSummary,
    place: Place,
}

/// Starts a session in the folder the device names, answering once it can be joined.
async fn start_session(
    State(bridged): State<Arc<Bridged>>,
    Extension(device): Extension<Device>,
    Json(request): Json<StartRequest>,
) -> Response {
    let Some(launcher) = &bridged.launcher else {
        return (StatusCode::NOT_IMPLEMENTED, "this bridge doesn't start sessions").into_response();
    };
    match start_in(launcher, &request.folder, &bridged.runtime_dir).await {
        Ok((session, place)) => {
            tracing::info!(session = %session.session_id, ?place, device = %device.name, "a device started a session");
            let started = Started { session: SessionSummary::from(&session), place };
            (StatusCode::CREATED, Json(started)).into_response()
        }
        Err(error) => {
            tracing::warn!(%error, device = %device.name, "could not start a session");
            (start_failure_status(&error), error.to_string()).into_response()
        }
    }
}

async fn start_in(
    launcher: &Launcher,
    asked: &str,
    runtime_dir: &std::path::Path,
) -> Result<(luxe_core::local_endpoint::LiveSession, Place), LaunchError> {
    let folder = launch::resolve_folder(asked, luxe_core::user_dirs::home_dir().as_deref())?;
    launcher.start(&folder, runtime_dir).await
}

fn start_failure_status(error: &LaunchError) -> StatusCode {
    match error {
        LaunchError::NotAFolder(_) => StatusCode::BAD_REQUEST,
        LaunchError::Herdr(_) | LaunchError::Spawn(_) => StatusCode::BAD_GATEWAY,
        LaunchError::NeverAppeared(_) => StatusCode::GATEWAY_TIMEOUT,
    }
}

/// Joins the session for the device, connecting before the upgrade so a session that has gone is
/// an HTTP error the app can show, not a socket that opens and closes.
async fn join_session(
    State(bridged): State<Arc<Bridged>>,
    Extension(device): Extension<Device>,
    Path(session_id): Path<String>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let introduction = introduction(&bridged, &device);
    match connect(bridged.runtime_dir.clone(), &session_id, &introduction).await {
        Ok(stream) => {
            tracing::info!(session = %session_id, device = %device.name, "a device joined");
            let here = bridged.presence.arrive(&device.id, &session_id);
            let revoked = revoked(bridged.devices.clone(), device.id);
            upgrade.on_upgrade(move |socket| async move {
                crate::relay::relay(socket, stream, revoked).await;
                drop(here);
            })
        }
        Err(refusal) => refusal.into_response(),
    }
}

/// Resolves once device `id` is no longer paired, checking every [`REVOCATION_CHECK`].
///
/// Polled because the file is changed by another process (`luxe remote devices revoke`), and a
/// revoked device's open sockets have to close, not just its next request fail.
async fn revoked(devices: DeviceBook, id: String) {
    loop {
        tokio::time::sleep(REVOCATION_CHECK).await;
        let reader = devices.clone();
        let paired = tokio::task::spawn_blocking(move || reader.devices()).await;
        let Ok(paired) = paired else { continue };
        if !paired.iter().any(|device| device.id == id) {
            tracing::info!(device = %id, "closing a revoked device's socket");
            return;
        }
    }
}

/// Who the bridge joins as for `device`: its name, and whether the computer's owner paired it,
/// judged by the first allowed login, where `luxe remote` puts the one Tailscale names as the
/// machine's.
fn introduction<'a>(bridged: &Bridged, device: &'a Device) -> crate::relay::Introduction<'a> {
    let yours = owned_by(&bridged.logins, &device.login);
    crate::relay::Introduction { name: &device.name, watch_only: false, yours }
}

/// Whether `login` is the owner's, the first of the allowed `logins`.
fn owned_by(logins: &[String], login: &str) -> bool {
    logins.first().is_some_and(|owner| owner == login)
}

/// Connects to `session_id` and introduces the device to it as `device`.
///
/// A refusal is the status and reason to answer with.
async fn connect(
    runtime_dir: PathBuf,
    session_id: &str,
    device: &crate::relay::Introduction<'_>,
) -> Result<luxe_core::local_endpoint::LocalStream, (StatusCode, String)> {
    let wanted = session_id.to_string();
    let found = tokio::task::spawn_blocking(move || sessions::find(&runtime_dir, &wanted)).await;
    let Some(session) = found.ok().flatten() else {
        return Err((StatusCode::NOT_FOUND, format!("no running session {session_id}")));
    };
    open(&session.address, device).await.map_err(|error| {
        tracing::warn!(%error, session = %session_id, "could not join the session");
        let reason = format!("could not join session {session_id}: {error}");
        (StatusCode::BAD_GATEWAY, reason)
    })
}

/// Connects to the endpoint at `address` and introduces the bridge there as `introduction`.
pub(crate) async fn open(
    address: &str,
    introduction: &crate::relay::Introduction<'_>,
) -> Result<luxe_core::local_endpoint::LocalStream, String> {
    let connected = luxe_core::local_endpoint::connect(address).await;
    let mut stream = connected.map_err(|error| error.to_string())?;
    let introduced = crate::relay::introduce(&mut stream, introduction).await;
    introduced.map_err(|error| error.to_string())?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::owned_by;

    /// A device is the owner's when the login that paired it is the first allowed, and only then.
    ///
    /// The break this catches: a guest let in with `--allow` treated as the owner, so their phone
    /// joins the owner's session without a line, or the owner's own phone announced every time.
    #[test]
    fn only_the_first_allowed_login_is_the_owners() {
        let logins = ["alex@github".to_string(), "sam@github".to_string()];
        assert!(owned_by(&logins, "alex@github"));
        assert!(!owned_by(&logins, "sam@github"));
        assert!(!owned_by(&[], "alex@github"));
    }
}
