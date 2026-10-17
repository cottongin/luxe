//! Review budget — what needs a human's judgement, and what something else already vouches for.
//!
//! A review is not an inspection of every hunk. Agents find the bugs and check the logic; the part
//! that does not automate away is judgement: does this fit the system, does it solve the problem,
//! what did it cost elsewhere. So the reviewer's surface is the change **minus what the harness can
//! account for**, and every exclusion names the evidence behind it.
//!
//! Nothing here marks anything reviewed on a human's behalf, and nothing is dropped: accounted-for
//! items render as counted rows that expand in place, and `All Files` remains complete. The claim is
//! only ever "something specific vouches for this", never "this is fine".

use super::{
    AttentionSignal, ChapterId, GuidedReview, HunkId, NoiseKind, ReviewChapter, ReviewFile,
    ReviewHunk, ReviewItemRef,
};
use std::collections::HashMap;

/// Why an item does not need the reviewer's own judgement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Accounted {
    /// A changed symbol here has a covering test, and a test command ran after the last edit.
    Covered {
        /// The command that ran, so the claim is checkable.
        command: String,
    },
    /// Formatting, generated, lockfile, or mechanical churn.
    Noise {
        /// The classifier's reason.
        class: NoiseKind,
    },
    /// Byte-identical to something in the revision the reviewer last finished.
    SeenBefore,
}

impl Accounted {
    /// A short reviewer-facing reason, used in the counted rows.
    pub fn label(&self) -> String {
        match self {
            Self::Covered { command } => format!("covered by `{command}`"),
            Self::Noise { class } => match class {
                NoiseKind::FormattingOnly => "formatting only".into(),
                NoiseKind::Generated => "generated file".into(),
                NoiseKind::Lockfile => "lockfile".into(),
                NoiseKind::Mechanical => "mechanical move or rename".into(),
                NoiseKind::CommentOnly => "comments only".into(),
                NoiseKind::Semantic | NoiseKind::Unknown => "low signal".into(),
            },
            Self::SeenBefore => "unchanged since your last review".into(),
        }
    }

    /// Stable grouping key, so consecutive items with the same reason collapse into one row.
    pub fn group_key(&self) -> String {
        self.label()
    }
}

/// What one item asks of the reviewer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemVerdict {
    /// Nothing vouches for it: this is the reading list.
    NeedsReview,
    /// Something specific vouches for it, and says what.
    Accounted(Accounted),
}

impl ItemVerdict {
    /// Whether this item is on the reviewer's reading list.
    pub const fn needs_review(&self) -> bool {
        matches!(self, Self::NeedsReview)
    }
}

/// Per-chapter roll-up, so a chapter that asks nothing of the reviewer can present as one line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChapterBudget {
    /// Items still on the reviewer's list
    pub needs_review: usize,
    /// Items something else accounts for.
    pub accounted: usize,
}

impl ChapterBudget {
    /// Items in the chapter, whatever their verdict.
    pub const fn total(&self) -> usize {
        self.needs_review + self.accounted
    }
}

/// The reviewer's actual workload for one pinned review.
#[derive(Debug, Clone, Default)]
pub struct ReviewBudget {
    verdicts: HashMap<String, ItemVerdict>,
    chapters: HashMap<ChapterId, ChapterBudget>,
    /// Items still on the reviewer's list, across the whole review
    pub needs_review: usize,
    /// Items something else accounts for.
    pub accounted: usize,
    /// Counted reasons, most numerous first, for the accounted-for block.
    pub reasons: Vec<(String, usize)>,
}

impl ReviewBudget {
    /// The verdict for one item; anything unclassified is the reviewer's to read.
    pub fn verdict(&self, item: &ReviewItemRef) -> ItemVerdict {
        self.verdicts.get(item_key(item)).cloned().unwrap_or(ItemVerdict::NeedsReview)
    }

    /// The verdict for one hunk.
    pub fn hunk_verdict(&self, hunk: &HunkId) -> ItemVerdict {
        self.verdicts.get(hunk.as_str()).cloned().unwrap_or(ItemVerdict::NeedsReview)
    }

    /// Whether this hunk is on the reading list.
    pub fn hunk_needs_review(&self, hunk: &HunkId) -> bool {
        self.hunk_verdict(hunk).needs_review()
    }

    /// Roll-up for one chapter.
    pub fn chapter(&self, chapter: &ChapterId) -> ChapterBudget {
        self.chapters.get(chapter).cloned().unwrap_or_default()
    }

    /// Total items considered.
    pub const fn total(&self) -> usize {
        self.needs_review + self.accounted
    }

    /// Whether the whole change is accounted for and nothing is left to read.
    pub const fn is_fully_accounted(&self) -> bool {
        self.needs_review == 0 && self.accounted > 0
    }
}

/// Computes what the reviewer actually has to look at.
///
/// Order of precedence matters: a *risk* signal always wins over an excuse. A hunk that adds a
/// credential-shaped line, weakens a gate, or lives in a file the agent edited without reading stays
/// on the reading list even if a test covers it, because those are exactly the cases where "a test
/// passed" is not the question being asked.
pub fn review_budget(review: &GuidedReview, attention: &[AttentionSignal]) -> ReviewBudget {
    let flagged = flagged_hunks(attention);
    let flagged_paths = flagged_paths(attention);
    let covering_command = covering_command(review);

    let flagged_paths_ref = &flagged_paths;
    let mut budget = ReviewBudget::default();
    let mut reasons: HashMap<String, usize> = HashMap::new();

    for file in &review.files {
        let path = file_path(file);
        for hunk in &file.hunks {
            let verdict = hunk_verdict(
                hunk,
                file,
                path.as_deref(),
                &flagged,
                &flagged_paths,
                covering_command.as_deref(),
                review,
            );
            record(&mut budget, &mut reasons, hunk.id.as_str(), verdict);
        }
        if file.non_text.is_some() {
            // A binary, mode-only, or rename-only change carries no lines to read; it is a fact to
            // notice, not a judgement call, unless something flagged it.
            let verdict = if path.as_deref().is_some_and(|path| is_flagged(flagged_paths_ref, path))
            {
                ItemVerdict::NeedsReview
            } else {
                ItemVerdict::Accounted(Accounted::Noise { class: NoiseKind::Mechanical })
            };
            record(&mut budget, &mut reasons, file.id.as_str(), verdict);
        }
    }

    for chapter in &review.chapters {
        let mut roll = ChapterBudget::default();
        for item in &chapter.items {
            if budget.verdict(item).needs_review() {
                roll.needs_review += 1;
            } else {
                roll.accounted += 1;
            }
        }
        budget.chapters.insert(chapter.id.clone(), roll);
    }

    let mut reasons: Vec<(String, usize)> = reasons.into_iter().collect();
    reasons.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    budget.reasons = reasons;
    budget
}

/// Whether a chapter asks anything of the reviewer.
pub fn chapter_needs_review(budget: &ReviewBudget, chapter: &ReviewChapter) -> bool {
    budget.chapter(&chapter.id).needs_review > 0
}

fn hunk_verdict(
    hunk: &ReviewHunk,
    file: &ReviewFile,
    path: Option<&str>,
    flagged: &[HunkId],
    flagged_paths: &[String],
    covering_command: Option<&str>,
    review: &GuidedReview,
) -> ItemVerdict {
    // Risk first: an attention signal is the reason a reviewer is here at all.
    if flagged.contains(&hunk.id) || path.is_some_and(|path| is_flagged(flagged_paths, path)) {
        return ItemVerdict::NeedsReview;
    }
    // Already read, and not touched since: the reviewer's own past attention is the evidence.
    if review.seen.as_ref().is_some_and(|seen| seen.covers_item(hunk.id.as_str())) {
        return ItemVerdict::Accounted(Accounted::SeenBefore);
    }
    if hunk.noise.is_foldable() {
        return ItemVerdict::Accounted(Accounted::Noise { class: hunk.noise });
    }
    // Coverage is only evidence when a test actually ran after the last edit *and* the graph says
    // this symbol is covered. Either half alone proves nothing about what shipped.
    if let (Some(command), Some(symbol)) = (covering_command, hunk_symbol(hunk, path)) {
        if review.insight.as_ref().is_some_and(|insight| insight.covers(&symbol)) {
            return ItemVerdict::Accounted(Accounted::Covered { command: command.to_owned() });
        }
    }
    let _ = file;
    ItemVerdict::NeedsReview
}

fn record(
    budget: &mut ReviewBudget,
    reasons: &mut HashMap<String, usize>,
    key: &str,
    verdict: ItemVerdict,
) {
    match &verdict {
        ItemVerdict::NeedsReview => budget.needs_review += 1,
        ItemVerdict::Accounted(accounted) => {
            budget.accounted += 1;
            *reasons.entry(accounted.label()).or_default() += 1;
        }
    }
    budget.verdicts.insert(key.to_owned(), verdict);
}

/// The `path::Symbol` key for a hunk, matching how graph insight names changed symbols.
fn hunk_symbol(hunk: &ReviewHunk, path: Option<&str>) -> Option<String> {
    let name = hunk.symbol.as_ref().and_then(|symbol| symbol.name.clone())?;
    Some(format!("{}::{name}", path?))
}

/// The test command whose result can vouch for covered symbols, if one qualifies.
///
/// Requires a passing command that looks like a test run *and* that nothing was edited after it:
/// a green run before the final change is evidence about a different program.
fn covering_command(review: &GuidedReview) -> Option<String> {
    review
        .provenance
        .as_ref()?
        .commands
        .iter()
        .rev()
        .find(|run| run.after_last_edit && !run.failed && run.is_test_run())
        .map(|run| run.command.clone())
}

/// Whether one of the flagged paths names this file.
fn is_flagged(flagged_paths: &[String], path: &str) -> bool {
    flagged_paths.iter().any(|flagged| flagged == path)
}

fn flagged_hunks(attention: &[AttentionSignal]) -> Vec<HunkId> {
    attention.iter().filter_map(|signal| signal.hunk_id.clone()).collect()
}

fn flagged_paths(attention: &[AttentionSignal]) -> Vec<String> {
    attention
        .iter()
        .filter(|signal| !signal.aggregate)
        .filter_map(|signal| signal.path.clone())
        .collect()
}

fn file_path(file: &ReviewFile) -> Option<String> {
    file.new_path
        .as_deref()
        .or(file.old_path.as_deref())
        .map(|path| path.to_string_lossy().into_owned())
}

fn item_key(item: &ReviewItemRef) -> &str {
    match item {
        ReviewItemRef::Hunk(id) => id.as_str(),
        ReviewItemRef::File(id) => id.as_str(),
    }
}

#[cfg(test)]
mod tests;
