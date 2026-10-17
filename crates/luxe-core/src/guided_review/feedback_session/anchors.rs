//! Anchors — where a comment points, the evidence it carries, and following it across a refresh.

use super::*;

impl ReviewSession {
    /// Reconciles finding anchors against a newly pinned source without inferring resolution.
    ///
    /// Submitted snapshots remain bound to the revision that captured them.
    pub fn reconcile_refreshed_source(
        &mut self,
        source: ReviewSource,
        mut remap: impl FnMut(&FindingAnchor) -> FindingAnchorRefresh,
    ) {
        self.source = source;
        for finding in &mut self.findings {
            match remap(&finding.anchor) {
                FindingAnchorRefresh::Exact => {}
                FindingAnchorRefresh::NeedsVerification(anchor) => {
                    if finding.state != FindingState::Resolved {
                        finding.anchor = anchor;
                        finding.state = FindingState::NeedsVerification;
                    }
                }
                FindingAnchorRefresh::Stale if finding.state != FindingState::Resolved => {
                    finding.state = FindingState::Stale;
                }
                FindingAnchorRefresh::Stale => {}
            }
        }
    }
}

/// Reviewer-facing and agent-facing evidence for one anchor, captured at submission time.
///
/// The submitted snapshot has to be self-describing: resolving `file:<sha256>` against a review
/// that has since been refreshed is impossible, and an agent handed only hashes cannot find the
/// code. Every field is optional because a chapter- or review-level comment has no line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FindingEvidence {
    /// Project-relative path at submission time.
    pub path: Option<String>,
    /// Which side of the diff owns `line`.
    pub side: Option<DiffSide>,
    /// One-based source line on `side`.
    pub line: Option<u32>,
    /// Enclosing symbol, when the hunk attributed one.
    pub symbol: Option<String>,
    /// The anchored source text, so the agent can still find it after the file drifts.
    pub source: Option<String>,
}

impl FindingEvidence {
    /// Resolves an anchor against the pinned review files it was created against.
    pub fn from_anchor(anchor: &FindingAnchor, files: &[ReviewFile]) -> Self {
        let file_id = match anchor {
            FindingAnchor::Line { file_id, .. } | FindingAnchor::Hunk { file_id, .. } => {
                Some(file_id)
            }
            FindingAnchor::Chapter { .. } | FindingAnchor::Review => None,
        };
        let file = file_id.and_then(|id| files.iter().find(|file| &file.id == id));
        let path = file
            .and_then(|file| file.new_path.as_deref().or(file.old_path.as_deref()))
            .map(|path| path.to_string_lossy().into_owned());
        let hunk = match anchor {
            FindingAnchor::Line { hunk_id, .. } | FindingAnchor::Hunk { hunk_id, .. } => {
                file.and_then(|file| file.hunks.iter().find(|hunk| &hunk.id == hunk_id))
            }
            FindingAnchor::Chapter { .. } | FindingAnchor::Review => None,
        };
        // Prefer an attributed symbol name, else the hunk heading; both beat a bare line number.
        let symbol = hunk.and_then(|hunk| {
            hunk.symbol
                .as_ref()
                .and_then(|symbol| symbol.name.clone())
                .or_else(|| hunk.heading.clone())
        });
        let (side, line, source) = match anchor {
            FindingAnchor::Line { side, line, .. } => {
                let text = hunk
                    .and_then(|hunk| {
                        hunk.lines.iter().find(|source| match side {
                            DiffSide::Old => source.old_line == Some(*line),
                            DiffSide::New => source.new_line == Some(*line),
                        })
                    })
                    .map(|source| source.text.clone());
                (Some(*side), Some(*line), text)
            }
            _ => (None, None, None),
        };
        Self { path, side, line, symbol, source }
    }

    /// A one-line `path:line (side) · symbol` location, or `None` for a whole-review comment.
    pub fn location(&self) -> Option<String> {
        let path = self.path.as_deref()?;
        let mut location = match (self.line, self.side) {
            (Some(line), Some(DiffSide::Old)) => format!("{path}:{line} (old)"),
            (Some(line), Some(DiffSide::New)) => format!("{path}:{line} (new)"),
            (Some(line), None) => format!("{path}:{line}"),
            (None, _) => path.to_owned(),
        };
        if let Some(symbol) = self.symbol.as_deref() {
            location.push_str(&format!(" · {symbol}"));
        }
        Some(location)
    }
}

/// Exact anchor for a review finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FindingAnchor {
    /// One source side of a rendered text-diff line.
    Line {
        /// File lineage containing the hunk.
        file_id: FileId,
        /// Immutable hunk containing the line.
        hunk_id: HunkId,
        /// Source side that owns the line number.
        side: DiffSide,
        /// One-based source line number on `side`.
        line: u32,
        /// Context digest used only for conservative remapping.
        ///
        /// Optional when a comment is made over RPC: a client names the line and the review desk
        /// fills this in from the pinned review. Never stored empty.
        #[serde(default)]
        context_hash: String,
    },
    /// A text hunk when no exact source line is available.
    Hunk {
        /// File lineage containing the hunk.
        file_id: FileId,
        /// Immutable hunk identity.
        hunk_id: HunkId,
    },
    /// A Guide chapter when a finding spans several source items.
    Chapter {
        /// Stable semantic chapter identity.
        chapter_id: ChapterId,
    },
    /// The complete review when no narrower source anchor applies.
    Review,
}

/// Maps one persisted anchor against immutable review content without guessing.
pub fn remap_finding_anchor(
    anchor: &FindingAnchor,
    files: &[ReviewFile],
    chapters: &[ReviewChapter],
) -> FindingAnchorRefresh {
    match anchor {
        FindingAnchor::Review => FindingAnchorRefresh::Exact,
        FindingAnchor::Chapter { chapter_id } => {
            if chapters.iter().any(|chapter| &chapter.id == chapter_id) {
                FindingAnchorRefresh::Exact
            } else {
                FindingAnchorRefresh::Stale
            }
        }
        FindingAnchor::Hunk { file_id, hunk_id } => {
            if files.iter().any(|file| {
                &file.id == file_id && file.hunks.iter().any(|hunk| &hunk.id == hunk_id)
            }) {
                FindingAnchorRefresh::Exact
            } else {
                FindingAnchorRefresh::Stale
            }
        }
        FindingAnchor::Line { file_id, hunk_id, side, line, context_hash } => {
            let Some(file) = files.iter().find(|file| &file.id == file_id) else {
                return FindingAnchorRefresh::Stale;
            };
            let line_matches = |candidate: &crate::guided_review::ReviewLine,
                                candidate_side: DiffSide| {
                candidate_side == *side
                    && match side {
                        DiffSide::Old => candidate.old_line,
                        DiffSide::New => candidate.new_line,
                    }
                    .is_some_and(|candidate_line| candidate_line == *line)
                    && candidate.context_hash() == *context_hash
            };
            if file.hunks.iter().any(|candidate_hunk| {
                &candidate_hunk.id == hunk_id
                    && candidate_hunk.lines.iter().any(|candidate| line_matches(candidate, *side))
            }) {
                return FindingAnchorRefresh::Exact;
            }
            let candidates = file
                .hunks
                .iter()
                .flat_map(|candidate_hunk| {
                    candidate_hunk.lines.iter().filter_map(move |candidate| {
                        let candidate_line = match side {
                            DiffSide::Old => candidate.old_line,
                            DiffSide::New => candidate.new_line,
                        }?;
                        (candidate.context_hash() == *context_hash).then(|| FindingAnchor::Line {
                            file_id: file_id.clone(),
                            hunk_id: candidate_hunk.id.clone(),
                            side: *side,
                            line: candidate_line,
                            context_hash: context_hash.clone(),
                        })
                    })
                })
                .collect::<Vec<_>>();
            match candidates.as_slice() {
                [candidate] => FindingAnchorRefresh::NeedsVerification(candidate.clone()),
                _ => FindingAnchorRefresh::Stale,
            }
        }
    }
}

impl FindingAnchor {
    pub(super) fn validate(&self) -> Result<(), ReviewSessionValidationError> {
        if let Self::Line { line, context_hash, .. } = self {
            if *line == 0 {
                return Err(ReviewSessionValidationError::InvalidLineAnchor);
            }
            if context_hash.trim().is_empty() {
                return Err(ReviewSessionValidationError::MissingContextHash);
            }
        }
        Ok(())
    }
}

/// Side of a unified or split diff line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffSide {
    /// Pre-change source line.
    Old,
    /// Post-change source line.
    New,
}

/// Conservative anchor remap result for a newly pinned review source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindingAnchorRefresh {
    /// The exact immutable anchor still exists, so its state remains unchanged.
    Exact,
    /// A unique local context match needs reviewer confirmation at its new anchor.
    NeedsVerification(FindingAnchor),
    /// No trustworthy current-source anchor exists; original evidence remains intact.
    Stale,
}
