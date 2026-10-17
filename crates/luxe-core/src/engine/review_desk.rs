//! Review desk — the one owner of the open review and its feedback session.
//!
//! Every change to a review's comments goes through here, whoever asks for it: a front end
//! (`Action::EditReview`), the turn that answers the feedback, the task that posts it to GitHub.
//! The desk runs as a task of its own fed by one channel, so changes apply and save one at a time,
//! in the order they arrived, and a front end can keep editing while a turn runs. It also publishes
//! reviews, which is what lets it promise that `guided_review_loaded` is always followed by the
//! session that goes with it.
//!
//! [`ReviewDesk::handle`] is the whole state machine: a command in, [`Effect`]s out.
//! [`spawn`](task::spawn) is the task around it and only forwards what `handle` returns. The
//! durable session changes only by way of a save that landed, so a refused or failed change leaves
//! nothing half-applied.
//!
//! `Action → engine → DeskCommand → desk → Event` (and, for feedback, `→ Action` back to the engine).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::Action;
use crate::event::Event;
use crate::guided_review::{
    remap_finding_anchor, thread_findings, DiffSide, FindingAnchor, FindingAnchorRefresh,
    FindingId, FindingSeverity, FindingState, GuideProvenance, GuidedReview, PostedComment,
    RemediationReport, ReviewBrief, ReviewChapter, ReviewFeedbackMode, ReviewFinding, ReviewId,
    ReviewLoadRequestId, ReviewOutcome, ReviewProgress, ReviewRevisionId, ReviewSession,
    ReviewSessionMutationError, ReviewSessionSaveRequestId, ReviewSessionStore,
    ReviewSessionStoreError, ReviewSessionValidationError, ReviewTarget, SeenReview,
    SeenReviewStore, SubmissionDispatch, SubmissionId, ThreadAnswerResult,
};

mod edits;
mod publishing;
mod storage;
mod task;

use edits::{apply, submit};
pub use edits::{CommentDraft, ReviewEdit};
use storage::SaveError;
pub(crate) use task::{DeskInbox, ReviewDeskHandle};

/// What reaches the desk: a front end's edit, or something that happened to the review.
#[derive(Debug)]
pub(crate) enum DeskCommand {
    /// A review was loaded, refreshed or restored, and is the one on screen now.
    Publish { request_id: ReviewLoadRequestId, review: Box<GuidedReview> },
    /// The Guide replaced the structural chapters of `revision_id`.
    GuideUpdated {
        request_id: ReviewLoadRequestId,
        review_id: ReviewId,
        revision_id: ReviewRevisionId,
        chapters: Vec<ReviewChapter>,
        provenance: GuideProvenance,
        brief: Option<ReviewBrief>,
    },
    /// A front end's edit, answered under its `request_id`.
    Edit { request_id: ReviewSessionSaveRequestId, review_id: ReviewId, edit: ReviewEdit },
    /// Which chapters have been walked. Saved when it changed, never answered.
    RecordProgress { review_id: ReviewId, progress: ReviewProgress },
    /// Remember what the walked chapters covered, so the next review leads with what is new.
    RecordSeen { review_id: ReviewId },
    /// A submission crossed into its turn.
    Dispatched(SubmissionId),
    /// A question's turn answered. Held for the completion it always precedes.
    Answered { submission_id: SubmissionId, answer: String },
    /// A submission's turn ended.
    Completed(SubmissionId),
    /// A submission's turn failed, or the engine refused it.
    Failed(SubmissionId),
    /// The agent reported what it did about the comments it was sent.
    Remediation(RemediationReport),
    /// A fix turn ended with no word from the agent on these comments.
    ///
    /// Args: `(submission_id, finding_ids, reason)` — `reason` as
    /// [`ReviewSession::note_unreported`] takes it.
    Unreported { submission_id: SubmissionId, finding_ids: Vec<FindingId>, reason: &'static str },
    /// A review went to GitHub, carrying these comments.
    Posted { finding_ids: Vec<FindingId>, comment: PostedComment, outcome: ReviewOutcome },
    /// Threads on the reader's own pull request were replied to or resolved.
    ThreadsAnswered(Vec<ThreadAnswerResult>),
    /// Answers once everything sent before it is handled, so the engine can stop without losing
    /// the last edit.
    Flush(tokio::sync::oneshot::Sender<()>),
    /// Answers with the open review and its session as they stand after everything sent before.
    Snapshot(tokio::sync::oneshot::Sender<Option<(GuidedReview, ReviewSession)>>),
}

/// What handling a command does outside the desk.
#[derive(Debug)]
pub(crate) enum Effect {
    /// Tell the front end
    Event(Event),
    /// Ask the engine: queue a durable submission's turn, or drop a cancelled one. Boxed, since
    /// `Action` dwarfs every event this carries.
    Engine(Box<Action>),
}

/// What a saved edit asks of the engine once it is durable.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FollowUp {
    /// Queue this submission's turn
    Submit(SubmissionId),
    /// Drop this submission from the engine's queue
    Cancel(SubmissionId),
}

/// The open review and its feedback session, and the rules for changing them.
pub(crate) struct ReviewDesk {
    /// Where sessions and the seen record are kept; `None` keeps nothing
    project_root: Option<PathBuf>,
    /// The review on screen
    review: Option<GuidedReview>,
    /// Its feedback session, as last stored (or new, until its first save)
    session: Option<ReviewSession>,
    /// Whether `session` has a stored revision to guard the next write with
    persisted: bool,
    /// Answers to questions, held for the save that completes their submission
    answers: Vec<(SubmissionId, String)>,
}

/// Marks a submission's turn dispatched; a retry may have done so already, before its turn began.
fn dispatch_submission(
    session: &mut ReviewSession,
    submission_id: &SubmissionId,
) -> Result<(), ReviewSessionMutationError> {
    match session.dispatch_submission(submission_id) {
        Err(ReviewSessionMutationError::SubmissionNotDispatchable {
            state: SubmissionDispatch::Dispatched,
        }) => Ok(()),
        other => other,
    }
}

/// Fails a submission's turn, leaving it retryable.
///
/// One refused before it ever dispatched (a fix asked of a pull request that is not checked out)
/// would stay `Queued` for good and never be retried, so it is failed from dispatch instead.
fn fail_submission(
    session: &mut ReviewSession,
    submission_id: &SubmissionId,
) -> Result<(), ReviewSessionMutationError> {
    match session.fail_submission(submission_id) {
        Err(ReviewSessionMutationError::SubmissionNotDispatched {
            state: SubmissionDispatch::Queued,
        }) => {
            session.dispatch_submission(submission_id)?;
            session.fail_submission(submission_id)
        }
        other => other,
    }
}

/// Marks the comments a review carried to GitHub as posted, with the verdict it went with.
fn mark_posted(
    session: &mut ReviewSession,
    finding_ids: &[FindingId],
    comment: &PostedComment,
    outcome: ReviewOutcome,
) -> Result<(), ReviewSessionMutationError> {
    session.mark_posted(finding_ids, comment);
    session.set_outcome(outcome);
    Ok(())
}

/// Records what landed on GitHub for each answered thread: a reply, a resolve, or both.
fn record_thread_answers(
    session: &mut ReviewSession,
    results: &[ThreadAnswerResult],
) -> Result<(), ReviewSessionMutationError> {
    for result in results.iter().filter(|r| r.reply_url.is_some() || r.resolved) {
        session.mark_thread_answered(&result.finding_id, result.reply_url.clone(), result.resolved);
    }
    Ok(())
}

fn saved(request_id: ReviewSessionSaveRequestId, session: ReviewSession) -> Effect {
    Effect::Event(Event::ReviewSessionSaved { request_id, session: Box::new(session) })
}

fn refused(request_id: ReviewSessionSaveRequestId, review_id: ReviewId, message: &str) -> Effect {
    Effect::Event(Event::ReviewSessionSaveFailed { request_id, review_id, message: message.into() })
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
}

impl ReviewDesk {
    /// A desk with nothing open, keeping its sessions under `project_root`.
    pub(crate) fn new(project_root: Option<PathBuf>) -> Self {
        Self { project_root, review: None, session: None, persisted: false, answers: Vec::new() }
    }

    /// Applies one command and says what has to happen because of it.
    pub(crate) async fn handle(&mut self, command: DeskCommand) -> Vec<Effect> {
        match command {
            DeskCommand::Publish { request_id, review } => self.publish(request_id, *review).await,
            DeskCommand::GuideUpdated {
                request_id,
                review_id,
                revision_id,
                chapters,
                provenance,
                brief,
            } => {
                self.apply_guide_upgrade(&review_id, &revision_id, &chapters, brief.as_ref());
                vec![Effect::Event(Event::GuidedReviewGuideUpdated {
                    request_id,
                    review_id,
                    revision_id,
                    chapters,
                    provenance,
                    brief,
                })]
            }
            DeskCommand::Edit { request_id, review_id, edit } => {
                self.edit(request_id, review_id, edit).await
            }
            DeskCommand::RecordProgress { review_id, progress } => {
                self.record_progress(&review_id, progress).await
            }
            DeskCommand::RecordSeen { review_id } => {
                self.record_seen(&review_id).await;
                Vec::new()
            }
            DeskCommand::Dispatched(submission_id) => {
                self.lifecycle("dispatch", |session| dispatch_submission(session, &submission_id))
                    .await
            }
            DeskCommand::Answered { submission_id, answer } => {
                self.answers.push((submission_id, answer));
                Vec::new()
            }
            DeskCommand::Completed(submission_id) => {
                self.complete_with_answers(submission_id).await
            }
            DeskCommand::Failed(submission_id) => {
                self.lifecycle("fail", |session| fail_submission(session, &submission_id)).await
            }
            DeskCommand::Remediation(report) => {
                let now = now_ms();
                self.lifecycle("record remediation", |session| {
                    session.record_remediation_report(&report, now)
                })
                .await
            }
            DeskCommand::Unreported { submission_id, finding_ids, reason } => {
                let now = now_ms();
                self.lifecycle("note unreported comments", |session| {
                    session.note_unreported(&submission_id, &finding_ids, reason, now)
                })
                .await
            }
            DeskCommand::Posted { finding_ids, comment, outcome } => {
                let posted =
                    |session: &mut _| mark_posted(session, &finding_ids, &comment, outcome);
                self.lifecycle("mark posted", posted).await
            }
            DeskCommand::Snapshot(reply) => {
                let open = self.review.clone().zip(self.bound());
                let _ = reply.send(open);
                Vec::new()
            }
            DeskCommand::Flush(done) => {
                // The channel is in order, so everything before this has been handled
                let _ = done.send(());
                Vec::new()
            }
            DeskCommand::ThreadsAnswered(results) => {
                let answered = |session: &mut _| record_thread_answers(session, &results);
                self.lifecycle("mark threads answered", answered).await
            }
        }
    }

    /// Puts the Guide's chapters on the held review, when it is still the revision they describe.
    ///
    /// Only ever an upgrade: a pass that wrote no brief must not take one away.
    fn apply_guide_upgrade(
        &mut self,
        review_id: &ReviewId,
        revision_id: &ReviewRevisionId,
        chapters: &[ReviewChapter],
        brief: Option<&ReviewBrief>,
    ) {
        let Some(review) = self
            .review
            .as_mut()
            .filter(|review| &review.id == review_id && &review.revision_id == revision_id)
        else {
            return;
        };
        review.chapters = chapters.to_vec();
        if let Some(brief) = brief {
            review.brief = Some(brief.clone());
        }
    }

    /// Completes a submission's turn, with the answers held for it recorded in the same save.
    async fn complete_with_answers(&mut self, submission_id: SubmissionId) -> Vec<Effect> {
        let (answers, others): (Vec<_>, Vec<_>) = std::mem::take(&mut self.answers)
            .into_iter()
            .partition(|(answered, _)| answered == &submission_id);
        self.answers = others;
        let now = now_ms();
        self.lifecycle("complete", |session| {
            for (_, answer) in &answers {
                session.record_answer(&submission_id, answer, now)?;
            }
            session.complete_submission(&submission_id)
        })
        .await
    }

    // === Edits ===

    async fn edit(
        &mut self,
        request_id: ReviewSessionSaveRequestId,
        review_id: ReviewId,
        edit: ReviewEdit,
    ) -> Vec<Effect> {
        let (Some(review), Some(mut candidate)) = (self.review.clone(), self.bound()) else {
            return vec![refused(request_id, review_id, "No review is open.")];
        };
        if review.id != review_id {
            return vec![refused(
                request_id,
                review_id,
                "That review is no longer open, so nothing was changed.",
            )];
        }
        match apply(edit, &mut candidate, &review) {
            Ok(follow_up) => self.commit(request_id, candidate, follow_up).await,
            Err(reason) => vec![refused(request_id, review_id, &reason)],
        }
    }

    /// Stores `candidate` and answers `request_id`, then starts whatever the edit was for.
    async fn commit(
        &mut self,
        request_id: ReviewSessionSaveRequestId,
        candidate: ReviewSession,
        follow_up: Option<FollowUp>,
    ) -> Vec<Effect> {
        if self.persisted && self.session.as_ref() == Some(&candidate) {
            return vec![saved(request_id, candidate)];
        }
        let stored = match self.save(&candidate).await {
            Ok(stored) => stored,
            Err(error) => {
                let mut effects = vec![self.save_failed(request_id, &error).await];
                if matches!(
                    error,
                    SaveError::Store(ReviewSessionStoreError::RevisionConflict { .. })
                ) {
                    if let Some(session) = self.session.clone() {
                        effects.push(Effect::Event(Event::ReviewSessionRestored {
                            session: Box::new(session),
                        }));
                    }
                }
                return effects;
            }
        };
        self.session = Some(stored.clone());
        let mut effects = vec![saved(request_id, stored.clone())];
        match follow_up {
            Some(FollowUp::Submit(submission_id)) => {
                effects.extend(submit(&stored, &submission_id))
            }
            Some(FollowUp::Cancel(submission_id)) => {
                effects.push(Effect::Engine(Box::new(Action::CancelQueuedReviewFeedback {
                    submission_id,
                })));
            }
            None => {}
        }
        effects
    }

    // === Lifecycle ===

    /// Applies something that happened to the review's feedback, and stores it.
    ///
    /// A command about a submission this session never had (a review since replaced) changes
    /// nothing, quietly; any other refusal is logged. Neither is anyone's mistake to report.
    async fn lifecycle(
        &mut self,
        what: &'static str,
        change: impl FnOnce(&mut ReviewSession) -> Result<(), ReviewSessionMutationError>,
    ) -> Vec<Effect> {
        let Some(mut candidate) = self.bound() else {
            debug!(what, "no review is open to apply this to");
            return Vec::new();
        };
        match change(&mut candidate) {
            Ok(()) => {}
            Err(
                ReviewSessionMutationError::SubmissionNotFound
                | ReviewSessionMutationError::RemediationReportWrongReview,
            ) => {
                debug!(what, "not about the open review");
                return Vec::new();
            }
            Err(error) => {
                warn!(%error, what, "the review session refused this");
                return Vec::new();
            }
        }
        if self.persisted && self.session.as_ref() == Some(&candidate) {
            return Vec::new();
        }
        self.commit(ReviewSessionSaveRequestId::new(), candidate, None).await
    }
}

#[cfg(test)]
mod tests;
