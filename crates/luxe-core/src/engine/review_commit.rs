//! Review commit — turning reviewed chapters into commits.

use super::*;
use crate::guided_review::ReviewSessionSaveRequestId;

impl Engine {
    /// Turns reviewed chapters into commits off the runtime thread.
    ///
    /// Everything safety-relevant lives in `LocalGit::commit_review_chapters`; this only routes the
    /// outcome, so a refusal reaches the reviewer as an explanation rather than silence.
    pub(crate) async fn handle_commit_review_chapters(
        &self,
        request_id: ReviewSessionSaveRequestId,
        review_id: ReviewId,
        expected_revision: ReviewRevisionId,
        chapter_ids: Vec<crate::guided_review::ChapterId>,
        squash: bool,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let review = match self.open_review(&review_id, Some(&expected_revision)).await {
            Ok((review, _)) => review,
            Err(message) => {
                let _ = event_tx.send(Event::ReviewCommitFailed { request_id, message }).await;
                return;
            }
        };
        let Some(project_root) = self.config.guided_review_project_root.clone() else {
            let _ = event_tx
                .send(Event::ReviewCommitFailed {
                    request_id,
                    message: "Committing needs an opened project workspace.".into(),
                })
                .await;
            return;
        };
        let rendered = if squash {
            crate::guided_review::squashed_commit(&review, &chapter_ids).map(|commit| vec![commit])
        } else {
            crate::guided_review::chapter_commits(&review, &chapter_ids)
        };
        let commits = match rendered {
            Ok(commits) => commits,
            Err(error) => {
                let _ = event_tx
                    .send(Event::ReviewCommitFailed { request_id, message: error.to_string() })
                    .await;
                return;
            }
        };
        let skipped: Vec<String> =
            commits.iter().flat_map(|commit| commit.skipped.clone()).collect();
        // Every chapter in one commit, which a review that starts before HEAD can still make
        let whole = squash
            && (chapter_ids.is_empty()
                || review.chapters.iter().all(|chapter| chapter_ids.contains(&chapter.id)));
        match self.review_git(project_root).commit_review_chapters(&review, &commits, whole).await {
            Ok(created) => {
                let _ = event_tx
                    .send(Event::ReviewCommitted { request_id, commits: created, skipped })
                    .await;
            }
            Err(error) => {
                warn!(%error, "could not commit the reviewed chapters");
                let _ = event_tx
                    .send(Event::ReviewCommitFailed { request_id, message: error.to_string() })
                    .await;
            }
        }
    }
}
