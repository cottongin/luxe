//! Project reads — one file's diff, one folder's listing and one file's text, each off the loop.

use super::*;

impl Engine {
    /// Starts whichever project read `action` asks for. Each is synchronous and spawns, so the
    /// loop can take it mid-turn too.
    pub(crate) fn handle_project_read(&self, action: Action, event_tx: &mpsc::Sender<Event>) {
        match action {
            Action::LoadFileDiff { request_id, path } => {
                self.handle_load_file_diff(request_id, path, event_tx);
            }
            Action::ListDirectory { request_id, path, include_ignored } => {
                self.handle_list_directory(request_id, path, include_ignored, event_tx);
            }
            Action::ReadProjectFile { request_id, path } => {
                self.handle_read_project_file(request_id, path, event_tx);
            }
            other => debug!(?other, "not a project read"),
        }
    }

    /// Captures one file's change and reports it, without touching review state.
    ///
    /// Detached rather than awaited: a glance must never make the runtime wait on git, and the
    /// front-end already correlates the answer by request id, so a slow capture arriving after the
    /// reader has moved on is discarded there rather than blocking anything here.
    pub(crate) fn handle_load_file_diff(
        &self,
        request_id: crate::file_diff::FileDiffRequestId,
        path: String,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let event_tx = event_tx.clone();
        // An absolute path is a file the session reached outside the project, and the project root
        // has nothing to say about it. Its baseline comes from the external-work record, which is
        // the only copy of what a file in no repository held before this session wrote to it.
        let external = crate::file_diff::is_external_path(&path);
        let baseline = external
            .then(|| self.config.external_work.as_ref().and_then(|work| work.baseline(&path)))
            .flatten();
        let project_root = self.config.guided_review_project_root.clone();
        if !external && project_root.is_none() {
            tokio::spawn(async move {
                let _ = event_tx
                    .send(Event::FileDiffFailed {
                        request_id,
                        path,
                        error: crate::file_diff::FileDiffFailure::NotAWorktree,
                    })
                    .await;
            });
            return;
        }
        tokio::spawn(async move {
            let captured = match &project_root {
                Some(root) if !external => crate::file_diff::capture(root, &path).await,
                _ => crate::file_diff::capture_external(&path, baseline.as_ref()).await,
            };
            let event = match captured {
                Ok(diff) => Event::FileDiffLoaded { request_id, diff: Box::new(diff) },
                Err(error) => {
                    debug!(%path, %error, "could not capture a file diff");
                    Event::FileDiffFailed { request_id, path, error }
                }
            };
            let _ = event_tx.send(event).await;
        });
    }

    /// Lists one project folder for the FILES tree, off the loop. Synchronous and spawning, so a
    /// turn in flight answers it too.
    pub(crate) fn handle_list_directory(
        &self,
        request_id: crate::project_files::ListingRequestId,
        path: String,
        include_ignored: bool,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let event_tx = event_tx.clone();
        let root = self.config.guided_review_project_root.clone();
        tokio::spawn(async move {
            let asked = path.clone();
            let listed = match root {
                Some(root) => tokio::task::spawn_blocking(move || {
                    crate::project_files::list_directory(&root, &asked, include_ignored)
                })
                .await
                .unwrap_or_else(|error| {
                    Err(crate::project_files::ProjectFileFailure::Unreadable(error.to_string()))
                }),
                None => Err(crate::project_files::ProjectFileFailure::OutsideProject),
            };
            let event = match listed {
                Ok(listing) => Event::DirectoryListed { request_id, listing },
                Err(error) => {
                    debug!(%path, %error, "could not list a project folder");
                    Event::DirectoryListFailed { request_id, path, error }
                }
            };
            let _ = event_tx.send(event).await;
        });
    }

    /// Reads one file for the preview, off the loop: a project file, or one this session changed
    /// outside the project.
    pub(crate) fn handle_read_project_file(
        &self,
        request_id: crate::project_files::FileReadRequestId,
        path: String,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let event_tx = event_tx.clone();
        let reader = self.file_reader(path.clone());
        tokio::spawn(async move {
            let read = tokio::task::spawn_blocking(reader).await.unwrap_or_else(|error| {
                Err(crate::project_files::ProjectFileFailure::Unreadable(error.to_string()))
            });
            let event = match read {
                Ok(file) => Event::ProjectFileRead { request_id, file: Box::new(file) },
                Err(error) => {
                    debug!(%path, %error, "could not read a project file");
                    Event::ProjectFileReadFailed { request_id, path, error }
                }
            };
            let _ = event_tx.send(event).await;
        });
    }

    /// What reading `path` for the preview takes. An absolute path is answered only by the
    /// external-work record, which reads nothing the session did not change.
    fn file_reader(
        &self,
        path: String,
    ) -> impl FnOnce() -> Result<
        crate::project_files::ProjectFile,
        crate::project_files::ProjectFileFailure,
    > + Send
           + 'static {
        let external = crate::file_diff::is_external_path(&path);
        let work = self.config.external_work.clone().filter(|_| external);
        let root = self.config.guided_review_project_root.clone().filter(|_| !external);
        move || match (work, root) {
            (Some(work), _) => work.read_file(&path),
            (_, Some(root)) => crate::project_files::read_file(&root, &path),
            _ => Err(crate::project_files::ProjectFileFailure::OutsideProject),
        }
    }
}
