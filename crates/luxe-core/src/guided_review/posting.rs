//! Posting — a pull request's review, as one GitHub review.
//!
//! The reviewer's comments become a single review with a verdict, the way they would submit one on
//! the site. Where each comment lands is decided here, against GitHub's own patches rather than
//! Luxe's diff: the two usually agree, and a comment that assumed so and was wrong would be refused
//! (`Line could not be resolved`) and take the whole review with it.
//!
//! Nothing is added to the reviewer's words. A comment that cannot sit on a line goes in the
//! review's body under a heading that says where it was written — which is placement, not prose.
//! Questions are for the agent and never leave the machine, and a comment already posted is not
//! posted again.

use super::{
    DiffSide, FindingAnchor, FindingId, FindingSeverity, GuidedReview, ReviewBase, ReviewFile,
    ReviewFinding, ReviewHunk,
};
use crate::github::{DraftComment, FilePatch, ReviewEvent, SubmitReview, ThreadSide};

/// One review, ready to post, and where each comment in it lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PostPlan {
    /// The request itself
    pub review: SubmitReview,
    /// Every comment it carries, in the order the reviewer wrote them
    pub placed: Vec<PlacedComment>,
}

/// Where one comment lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedComment {
    /// The finding it came from
    pub finding_id: FindingId,
    /// Where it lands
    pub place: Placement,
}

/// The places a comment can land on GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// On one line
    Line {
        /// File path
        path: String,
        /// Line number on `side`
        line: u32,
        /// Side the number counts on
        side: DiffSide,
    },
    /// On a run of lines
    Lines {
        /// File path
        path: String,
        /// First line
        start: u32,
        /// Last line
        end: u32,
        /// Side both count on
        side: DiffSide,
    },
    /// In the review's body, because it is about more than a line or GitHub's diff has no line for it
    Body(BodyReason),
}

/// Why a comment went in the review's body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyReason {
    /// It was written on the review as a whole
    WholeReview,
    /// It was written on a chapter, named here
    Chapter(String),
    /// Its lines are not in GitHub's diff of the file, so a line comment would be refused
    OffDiff {
        /// File path
        path: String,
        /// Where it was, as `12` or `12–18`
        lines: String,
    },
}

/// Why a review cannot be posted as it stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PostPlanError {
    /// Only a pull request's review can be posted.
    #[error("only a pull request's review can be posted to GitHub")]
    NotAPullRequest,
    /// There is nothing to say: no comment and no verdict that stands alone.
    #[error("there is nothing to post: write a comment first, or approve")]
    Nothing,
    /// Requesting changes needs words GitHub can show as the review, not only line comments.
    #[error(
        "requesting changes needs a comment on the review as a whole saying what should change"
    )]
    NeedsBody,
}

impl PostPlanError {
    /// Why, as a sentence for the reader: what the preview and a refused post both say.
    pub fn sentence(self) -> String {
        let reason = self.to_string();
        let mut chars = reason.chars();
        let opening: String = chars
            .next()
            .map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect());
        format!("{opening}.")
    }
}

/// Builds the review to post from the reviewer's comments.
///
/// `findings` may be every finding in the session: questions and ones already posted are left out
/// here, so a caller cannot send either by accident.
pub fn plan_post(
    review: &GuidedReview,
    findings: &[ReviewFinding],
    patches: &[FilePatch],
    event: ReviewEvent,
) -> Result<PostPlan, PostPlanError> {
    let ReviewBase::PullRequest { head_oid, .. } = &review.source.base else {
        return Err(PostPlanError::NotAPullRequest);
    };
    let mut comments = Vec::new();
    let mut placed = Vec::new();
    let (mut whole, mut chapters, mut off_diff) = (Vec::new(), Vec::new(), Vec::new());

    for finding in postable(findings) {
        let place = placement(review, finding, patches);
        match &place {
            Placement::Line { path, line, side } => comments.push(DraftComment {
                path: path.clone(),
                line: *line,
                side: thread_side(*side),
                start_line: None,
                body: finding.body.trim().to_owned(),
            }),
            Placement::Lines { path, start, end, side } => comments.push(DraftComment {
                path: path.clone(),
                line: *end,
                side: thread_side(*side),
                start_line: Some(*start),
                body: finding.body.trim().to_owned(),
            }),
            Placement::Body(BodyReason::WholeReview) => whole.push(finding.body.trim().to_owned()),
            Placement::Body(BodyReason::Chapter(title)) => {
                chapters.push(format!("**{title}**\n\n{}", finding.body.trim()));
            }
            Placement::Body(BodyReason::OffDiff { path, lines }) => {
                let quoted = quoted_source(review, finding);
                off_diff.push(format!("**`{path}` {lines}**\n\n{quoted}{}", finding.body.trim()));
            }
        }
        placed.push(PlacedComment { finding_id: finding.id.clone(), place });
    }

    let body = [whole, chapters, off_diff].concat().join("\n\n");
    if body.is_empty() && comments.is_empty() && event != ReviewEvent::Approve {
        return Err(PostPlanError::Nothing);
    }
    if body.is_empty() && event == ReviewEvent::RequestChanges {
        return Err(PostPlanError::NeedsBody);
    }
    Ok(PostPlan {
        review: SubmitReview { commit_id: head_oid.clone(), event, body, comments },
        placed,
    })
}

/// The findings a post may carry: not questions, not already posted.
pub fn postable(findings: &[ReviewFinding]) -> impl Iterator<Item = &ReviewFinding> {
    findings
        .iter()
        .filter(|finding| finding.severity != FindingSeverity::Question && finding.posted.is_none())
}

/// Where one finding lands, given what GitHub's patch of its file accepts.
fn placement(review: &GuidedReview, finding: &ReviewFinding, patches: &[FilePatch]) -> Placement {
    match &finding.anchor {
        FindingAnchor::Review => Placement::Body(BodyReason::WholeReview),
        FindingAnchor::Chapter { chapter_id } => Placement::Body(BodyReason::Chapter(
            review
                .chapters
                .iter()
                .find(|chapter| &chapter.id == chapter_id)
                .map_or_else(|| "A chapter".to_owned(), |chapter| chapter.title.clone()),
        )),
        FindingAnchor::Line { file_id, side, line, .. } => {
            let Some(file) = review.files.iter().find(|file| &file.id == file_id) else {
                return Placement::Body(BodyReason::WholeReview);
            };
            let path = file_path(file);
            match patch_for(patches, file) {
                Some(patch) if patch.commentable.accepts(thread_side(*side), *line) => {
                    Placement::Line { path, line: *line, side: *side }
                }
                _ => Placement::Body(BodyReason::OffDiff { path, lines: line.to_string() }),
            }
        }
        FindingAnchor::Hunk { file_id, hunk_id } => {
            let Some(file) = review.files.iter().find(|file| &file.id == file_id) else {
                return Placement::Body(BodyReason::WholeReview);
            };
            let path = file_path(file);
            let Some((side, start, end)) =
                file.hunks.iter().find(|hunk| &hunk.id == hunk_id).and_then(hunk_span)
            else {
                return Placement::Body(BodyReason::OffDiff { path, lines: "(a hunk)".into() });
            };
            // Every line of the run, not only its ends: GitHub splitting the region into two hunks
            // leaves a gap its API refuses, and the gap is invisible from the endpoints
            let whole_run = patch_for(patches, file).is_some_and(|patch| {
                (start..=end).all(|line| patch.commentable.accepts(thread_side(side), line))
            });
            match (whole_run, start == end) {
                (true, true) => Placement::Line { path, line: start, side },
                (true, false) => Placement::Lines { path, start, end, side },
                (false, _) if start == end => {
                    Placement::Body(BodyReason::OffDiff { path, lines: start.to_string() })
                }
                (false, _) => Placement::Body(BodyReason::OffDiff {
                    path,
                    lines: format!("{start}\u{2013}{end}"),
                }),
            }
        }
    }
}

/// The side a hunk's comment counts on and the lines it spans: the new side when the hunk has any
/// new lines, else the old side of a pure deletion.
fn hunk_span(hunk: &ReviewHunk) -> Option<(DiffSide, u32, u32)> {
    let new: Vec<u32> = hunk.lines.iter().filter_map(|line| line.new_line).collect();
    if let (Some(start), Some(end)) = (new.iter().min(), new.iter().max()) {
        return Some((DiffSide::New, *start, *end));
    }
    let old: Vec<u32> = hunk.lines.iter().filter_map(|line| line.old_line).collect();
    Some((DiffSide::Old, *old.iter().min()?, *old.iter().max()?))
}

/// GitHub's patch of `file`, matched on its path at the head or before a rename.
fn patch_for<'a>(patches: &'a [FilePatch], file: &ReviewFile) -> Option<&'a FilePatch> {
    let new = file.new_path.as_deref().map(crate::paths::display);
    let old = file.old_path.as_deref().map(crate::paths::display);
    patches.iter().find(|patch| {
        new.as_deref() == Some(patch.path.as_str())
            || (old.is_some() && old.as_deref() == patch.previous_path.as_deref())
    })
}

fn file_path(file: &ReviewFile) -> String {
    file.new_path
        .as_deref()
        .or(file.old_path.as_deref())
        .map(crate::paths::display)
        .unwrap_or_default()
}

fn thread_side(side: DiffSide) -> ThreadSide {
    match side {
        DiffSide::Old => ThreadSide::Left,
        DiffSide::New => ThreadSide::Right,
    }
}

/// The line a comment was written on, quoted, so a reader of the body can find it without a line
/// comment to point at it.
fn quoted_source(review: &GuidedReview, finding: &ReviewFinding) -> String {
    let FindingAnchor::Line { file_id, side, line, .. } = &finding.anchor else {
        return String::new();
    };
    review
        .files
        .iter()
        .filter(|file| &file.id == file_id)
        .flat_map(|file| &file.hunks)
        .flat_map(|hunk| &hunk.lines)
        .find(|candidate| match side {
            DiffSide::Old => candidate.old_line == Some(*line),
            DiffSide::New => candidate.new_line == Some(*line),
        })
        .map(|candidate| format!("> `{}`\n\n", candidate.text.trim()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
