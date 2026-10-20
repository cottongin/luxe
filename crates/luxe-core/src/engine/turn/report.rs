//! Turn report — reading a fix turn's remediation report, and asking once when there is none.
//!
//! A fix request ends with the report the agent is to fill in. The turn reads everything the agent
//! said in it, every message rather than only the last, as the turn is about to end. A report that
//! reads goes to the review. When there is none luxe can use, luxe appends one message saying what
//! was wrong, with the template again, and the turn goes on for the answer: once, so a model that
//! cannot comply costs a call rather than a loop. Whatever still has no word from the agent when
//! the turn ends, however it ended, goes to the review desk, which notes it on the comment.

use super::*;
use crate::briefs::REPORT_REQUEST;
use crate::guided_review::FindingId;

/// What a fix turn has said toward its report.
#[derive(Default)]
pub(super) struct ReportWatch {
    /// Every assistant text the turn committed, in order
    said: Vec<String>,
    /// Whether luxe has asked for the report this turn
    asked: bool,
    /// Why the last reading found no report luxe could use
    problem: Option<&'static str>,
    /// The comments the delivered report spoke for; `None` until one is delivered
    covered: Option<Vec<FindingId>>,
}

impl ReportWatch {
    /// Keeps `text` for the reading at the end of the turn.
    pub(super) fn heard(&mut self, text: &str) {
        self.said.push(text.to_string());
    }

    /// Says why the turn will end without the report, when it ends some way other than a reply.
    pub(super) fn cut_short(&mut self, why: &'static str) {
        self.problem = Some(why);
    }
}

/// What the turn does next once its report has been read.
pub(super) enum ReportNext {
    /// Delivered, or asked for already: the turn ends
    Done,
    /// Ask for it, for this reason, and carry on
    Ask(&'static str),
}

impl Engine {
    /// Reads the turn's report as the turn is about to end, and delivers it when it reads.
    pub(super) async fn read_report(
        &self,
        context: &RemediationTurnContext,
        entry_id: &EntryId,
        watch: &mut ReportWatch,
        event_tx: &mpsc::Sender<Event>,
    ) -> ReportNext {
        let said = watch.said.join("\n\n");
        let problem = match crate::guided_review::extract_remediation_report(
            &said,
            &context.review_id,
            &context.revision_id,
            context.finding_ids.iter().cloned(),
        ) {
            Ok(Some(report)) => {
                watch.covered =
                    Some(report.findings.iter().map(|r| r.finding_id.clone()).collect());
                self.review_desk
                    .send(crate::engine::review_desk::DeskCommand::Remediation(report.clone()));
                let _ = event_tx
                    .send(Event::RemediationReported { entry_id: entry_id.clone(), report })
                    .await;
                return ReportNext::Done;
            }
            Ok(None) => NO_REPORT,
            Err(error) => {
                warn!(%entry_id, %error, "the remediation report cannot be used");
                error.reason()
            }
        };
        watch.problem = Some(problem);
        match watch.asked {
            true => ReportNext::Done,
            false => ReportNext::Ask(problem),
        }
    }

    /// Appends the one message asking for the report, and returns the new leaf.
    pub(super) async fn ask_for_report(
        &self,
        leaf: EntryId,
        context: &RemediationTurnContext,
        reason: &str,
        watch: &mut ReportWatch,
        event_tx: &mpsc::Sender<Event>,
    ) -> Result<EntryId, EngineError> {
        watch.asked = true;
        debug!(reason, "asking for the remediation report");
        let message = Message::user(report_request(context, reason));
        let sid = self.current_session_id().await;
        let entry_id =
            self.sessions_mut().append(&sid, Some(leaf), EntryKind::Message(message.clone()))?;
        self.set_leaf(entry_id.clone()).await;
        let _ = event_tx
            .send(Event::MessageCommitted {
                session_id: sid.to_string(),
                entry_id: entry_id.clone(),
                message,
            })
            .await;
        Ok(entry_id)
    }

    /// Hands the desk every comment the turn ends with no word on, and why.
    pub(super) fn note_unreported(&self, context: &RemediationTurnContext, watch: &ReportWatch) {
        let Some(submission_id) = context.submission_id.clone().filter(|_| context.reports) else {
            return;
        };
        let covered = watch.covered.as_deref().unwrap_or_default();
        let finding_ids: Vec<FindingId> =
            context.finding_ids.iter().filter(|id| !covered.contains(id)).cloned().collect();
        if finding_ids.is_empty() {
            return;
        }
        let reason = match (&watch.covered, watch.problem) {
            (Some(_), _) => "its report left this comment out",
            (None, _) if self.is_cancelled() => "the turn was stopped before it reported",
            (None, problem) => problem.unwrap_or(NO_REPORT),
        };
        self.review_desk.send(crate::engine::review_desk::DeskCommand::Unreported {
            submission_id,
            finding_ids,
            reason,
        });
    }
}

/// Why there is no report, when the agent simply wrote none.
const NO_REPORT: &str = "it wrote no report";

/// The message asking for a missing report: what was wrong, and the template to answer with.
///
/// No `Submission:` line, which is how a retried submission finds the message it already sent.
fn report_request(context: &RemediationTurnContext, reason: &str) -> String {
    format!(
        "{REPORT_REQUEST}\nYour reply did not give the report luxe reads to update the review: \
         {reason}. Do not redo any work. Reply with just this block, one line per \
         comment in the order they were sent, each status set to fixed, not_fixed or \
         needs_clarification and each note saying what you did, in one sentence.\n\n{}",
        crate::guided_review::remediation_report_template(&context.finding_ids)
    )
}

impl Engine {
    /// Sends a question turn's answer to the questions it was asked about.
    ///
    /// A question carries no report; what it said is the answer, and it goes on the questions.
    pub(super) async fn answer_questions(
        &self,
        context: &RemediationTurnContext,
        text: &str,
        event_tx: &mpsc::Sender<Event>,
    ) {
        let Some(submission_id) = context.submission_id.clone() else {
            return;
        };
        let answer = text.trim().to_string();
        if answer.is_empty() {
            return;
        }
        self.review_desk.send(crate::engine::review_desk::DeskCommand::Answered {
            submission_id: submission_id.clone(),
            answer: answer.clone(),
        });
        let _ = event_tx.send(Event::ReviewQuestionsAnswered { submission_id, answer }).await;
    }
}
