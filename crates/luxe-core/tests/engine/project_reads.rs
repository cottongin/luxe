//! Project reads — what the preview's FILE tab may read, as the engine answers it.

use std::sync::Arc;

use luxe_core::engine::{Action, Engine, EngineConfig};
use luxe_core::event::Event;
use luxe_core::external_work::ExternalWork;
use luxe_core::project_files::{FileReadRequestId, ProjectFileFailure};
use luxe_core::session::SessionStore;

use crate::fixtures::*;

/// A file this session changed outside the project reads back like a project file, and a file
/// beside it that the session never touched is still refused.
#[tokio::test]
async fn the_preview_reads_what_the_session_changed_outside_the_project() {
    let (project, elsewhere) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let changed = elsewhere.path().join("notes.md");
    let work = ExternalWork::new();
    work.observe(&changed);
    std::fs::write(&changed, "# notes\n").unwrap();
    std::fs::write(elsewhere.path().join("secret.md"), "hush\n").unwrap();
    let (actions, mut events) = Engine::spawn(engine_with(project.path(), work));

    let read = read_back(&actions, &mut events, &changed).await;
    assert_eq!(read.map(|file| file.text), Ok("# notes\n".to_owned()));
    let refused = read_back(&actions, &mut events, &elsewhere.path().join("secret.md")).await;
    assert_eq!(refused, Err(ProjectFileFailure::OutsideProject));
    let _ = actions.send(Action::Close).await;
}

/// An engine on `project` that knows the session's outside work is `work`.
fn engine_with(project: &std::path::Path, work: ExternalWork) -> EngineConfig {
    let mut store = project_store(project);
    let session_id = store.create(&luxe_core::paths::display(project)).unwrap();
    let provider = Arc::new(RemediationProvider { reply: String::new() });
    EngineConfig {
        guided_review_project_root: Some(project.to_path_buf()),
        external_work: Some(work),
        ..base_config(store, session_id, provider)
    }
}

/// What the engine answers a FILE-tab read of `path` with.
async fn read_back(
    actions: &tokio::sync::mpsc::Sender<Action>,
    events: &mut tokio::sync::mpsc::Receiver<Event>,
    path: &std::path::Path,
) -> Result<luxe_core::project_files::ProjectFile, ProjectFileFailure> {
    let request_id = FileReadRequestId::new();
    let path = luxe_core::paths::display(path);
    let asked = Action::ReadProjectFile { request_id: request_id.clone(), path };
    actions.send(asked).await.unwrap();
    first(events, "the file read", |event| match event {
        Event::ProjectFileRead { request_id: id, file } if id == request_id => Some(Ok(*file)),
        Event::ProjectFileReadFailed { request_id: id, error, .. } if id == request_id => {
            Some(Err(error))
        }
        _ => None,
    })
    .await
}
