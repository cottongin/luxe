//! Review insight — advisory, graph-derived grounding for a Guided Review (design-code-graph.md
//! §8.1).
//!
//! Structural context computed from the code graph: for each changed file, the ranked callers /
//! dependents / covering tests that make it review-critical, plus changed symbols with **no**
//! covering test. This is **advisory metadata only** — it is *not* part of the chapter ↔
//! source-item bijection, so it can never add, drop, or move a source item. It
//! makes a review more trustworthy in two ways: it grounds the chaptering model's prompt (agents)
//! and it renders as test-gap badges + relevant-files in the review UI (humans).
//!
//! Computed in `luxe-lsp` (which owns the graph); the type lives here so `luxe-core`'s review
//! model can carry it without depending on `luxe-lsp`.

use super::ReviewFile;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// A relevant file reached from a changed file, with a one-line reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelevantFile {
    /// Project-relative path of the relevant file.
    pub file: String,
    /// Why it's relevant, e.g. `session::start calls login` or `tests::test_login tests login`.
    pub reason: String,
}

/// Structural relevance for one changed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileInsight {
    /// Project-relative path of the changed file.
    pub file: String,
    /// Ranked callers / dependents / covering tests affected if this file changes.
    pub relevant: Vec<RelevantFile>,
}

/// Advisory, graph-derived grounding for a whole review. Empty when no structural relationships were
/// found (e.g. a docs-only diff, or an unbuilt graph).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewInsight {
    /// Per changed-file structural relevance (only files with relationships appear).
    pub files: Vec<FileInsight>,
    /// Changed function/method symbols with no covering test, as `path::Symbol`.
    pub untested_symbols: Vec<String>,
    /// Changed function/method symbols that **do** have a covering test, as `path::Symbol`.
    ///
    /// The other half of the same question, and the evidence the review budget spends: a reviewer is
    /// only entitled to skip a hunk if something specific vouches for it.
    pub covered_symbols: Vec<String>,
}

/// `a, b, c +N more` — a bounded rendering of a symbol list.
fn bounded_list(symbols: &[String]) -> String {
    let shown = symbols.len().min(MAX_INSIGHT_SYMBOLS);
    let mut list = symbols[..shown].join(", ");
    let overflow = symbols.len() - shown;
    if overflow > 0 {
        list.push_str(&format!(" +{overflow} more"));
    }
    list
}

/// Supplies advisory graph-derived insight for a review's changed files.
///
/// Implemented outside `luxe-core` (where the code graph lives) and injected via `EngineConfig`
/// so the review build can ground itself in real structure without `luxe-core` depending on the
/// graph. A missing provider (or `None` result) simply leaves the review ungrounded.
#[async_trait]
pub trait ReviewInsightProvider: Send + Sync {
    /// Compute insight for the review's changed files, or `None` when unavailable.
    async fn insight(&self, files: &[ReviewFile]) -> Option<ReviewInsight>;
}

/// Longest symbol list carried in either direction; the rest are counted, never enumerated.
///
/// A real diff produced **236** untested symbols because the source flagged every symbol in a changed
/// file. The source is fixed, but the bound stays: a list nobody can read is not grounding.
const MAX_INSIGHT_SYMBOLS: usize = 24;

impl ReviewInsight {
    /// Whether nothing structural was found.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty() && self.untested_symbols.is_empty() && self.covered_symbols.is_empty()
    }

    /// Whether a changed symbol (`path::Symbol`) has a covering test.
    pub fn covers(&self, symbol: &str) -> bool {
        self.covered_symbols.iter().any(|covered| covered == symbol)
    }

    /// A compact prompt block to ground the chaptering model, or `None` when empty.
    ///
    /// It steers the model toward objective relevant-files, a core → callers → tests ordering, and
    /// test-gap callouts, while explicitly forbidding any change to the source-item set.
    pub fn render_prompt_block(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut out = String::from(
            "Structural context from the code graph (advisory — ground \"relevant files\" in it, \
             order chapters core -> callers -> tests, and flag missing tests; do NOT add, drop, or \
             move any source item):\n",
        );
        for file in &self.files {
            let parts: Vec<String> =
                file.relevant.iter().map(|r| format!("{} ({})", r.file, r.reason)).collect();
            out.push_str(&format!("- {} is relied on by: {}\n", file.file, parts.join(", ")));
        }
        if !self.untested_symbols.is_empty() {
            out.push_str(&format!(
                "- Changed symbols with no covering test: {}\n",
                bounded_list(&self.untested_symbols)
            ));
        }
        if !self.covered_symbols.is_empty() {
            out.push_str(&format!(
                "- Changed symbols a test already covers: {}\n",
                bounded_list(&self.covered_symbols)
            ));
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ReviewInsight {
        ReviewInsight {
            files: vec![FileInsight {
                file: "src/auth.rs".into(),
                relevant: vec![
                    RelevantFile {
                        file: "src/session.rs".into(),
                        reason: "start calls login".into(),
                    },
                    RelevantFile {
                        file: "tests/auth.rs".into(),
                        reason: "test_login tests login".into(),
                    },
                ],
            }],
            untested_symbols: vec!["src/auth.rs::logout".into()],
            covered_symbols: vec!["src/auth.rs::login".into()],
        }
    }

    #[test]
    fn empty_insight_has_no_prompt_block() {
        assert!(ReviewInsight::default().is_empty());
        assert!(ReviewInsight::default().render_prompt_block().is_none());
    }

    #[test]
    fn prompt_block_lists_relevant_files_and_test_gaps() {
        let block = sample().render_prompt_block().expect("non-empty");
        assert!(block.contains("src/auth.rs is relied on by:"));
        assert!(block.contains("src/session.rs (start calls login)"));
        assert!(block.contains("no covering test: src/auth.rs::logout"));
        assert!(block.contains("already covers: src/auth.rs::login"));
        // The bijection guard is spelled out so the model can't drop/duplicate items.
        assert!(block.contains("do NOT add, drop, or move any source item"));
    }

    /// A grounding block is a prompt, not a data dump: a long list is counted, not enumerated.
    #[test]
    fn long_symbol_lists_are_counted_rather_than_pasted() {
        let insight = ReviewInsight {
            untested_symbols: (0..MAX_INSIGHT_SYMBOLS + 6)
                .map(|index| format!("src/lib.rs::symbol_{index}"))
                .collect(),
            ..ReviewInsight::default()
        };

        let block = insight.render_prompt_block().expect("non-empty");

        assert!(block.contains("+6 more"), "the overflow is counted: {block}");
        assert!(!block.contains(&format!("symbol_{}", MAX_INSIGHT_SYMBOLS + 5)));
    }

    #[test]
    fn round_trips_through_serde_with_defaults() {
        let json = serde_json::to_string(&sample()).unwrap();
        assert_eq!(serde_json::from_str::<ReviewInsight>(&json).unwrap(), sample());
        // A missing field deserializes to an empty insight (backward-compatible).
        assert_eq!(serde_json::from_str::<ReviewInsight>("{}").unwrap(), ReviewInsight::default());
    }
}
