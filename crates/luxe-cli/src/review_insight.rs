//! Graph-backed Guided Review insight provider.
//!
//! Implements `luxe-core`'s `ReviewInsightProvider` over the warm code graph: maps a review's
//! changed files to project-relative paths **and the new-side lines their hunks touch**, then
//! computes advisory structural grounding (relevant callers/tests, plus which changed symbols a test
//! covers) via `luxe_lsp::graph::review_insight`. Lives in `luxe-cli` because `luxe-core` (where
//! the review is built) can't depend on `luxe-lsp` (where the graph lives).

use async_trait::async_trait;
use luxe_core::guided_review::{ReviewFile, ReviewInsight, ReviewInsightProvider};
use luxe_lsp::graph::{review_insight, ChangedRegion, GraphService};
use std::sync::Arc;

/// Computes advisory review insight from the shared warm code graph.
pub(crate) struct GraphReviewInsight {
    graph: Arc<GraphService>,
}

impl GraphReviewInsight {
    pub(crate) fn new(graph: Arc<GraphService>) -> Self {
        Self { graph }
    }
}

#[async_trait]
impl ReviewInsightProvider for GraphReviewInsight {
    async fn insight(&self, files: &[ReviewFile]) -> Option<ReviewInsight> {
        let changed: Vec<ChangedRegion> = files.iter().filter_map(changed_region).collect();
        if changed.is_empty() {
            return None;
        }
        // advisory: with no graph the review simply carries no insight, said in the log
        let graph = self
            .graph
            .graph(false)
            .await
            .inspect_err(|error| tracing::warn!(%error, "review insight skipped"))
            .ok()?;
        let insight = review_insight(&graph, &changed);
        (!insight.is_empty()).then_some(insight)
    }
}

/// A review file's path plus the new-side lines its hunks touch.
///
/// The spans scope coverage claims to the symbols the diff really changed. A file whose hunks carry
/// no new-side lines (a pure deletion) yields a whole-file region, which deliberately makes no
/// symbol-level claim at all.
fn changed_region(file: &ReviewFile) -> Option<ChangedRegion> {
    let path = file
        .new_path
        .as_ref()
        .or(file.old_path.as_ref())
        .map(|path| path.to_string_lossy().into_owned())?;
    let changed_lines = file
        .hunks
        .iter()
        .filter(|hunk| hunk.new_range.count > 0)
        .map(|hunk| {
            let start = hunk.new_range.start.max(1);
            (start, start.saturating_add(hunk.new_range.count.saturating_sub(1)))
        })
        .collect();
    Some(ChangedRegion { file: path, changed_lines })
}

/// Reports changed symbols with no covering test for the file just edited.
///
/// Same graph, same question as the review's insight — asked while the agent is still working, so a
/// gap becomes its decision rather than the reviewer's homework. The whole file's symbols are
/// considered here (an edit's exact line ranges are not on the `FileChanged` event), and test files
/// are excluded, so the advisory says "this file has untested functions" rather than pretending to
/// know which lines moved.
pub(crate) struct GraphTestGaps {
    graph: Arc<GraphService>,
}

impl GraphTestGaps {
    pub(crate) fn new(graph: Arc<GraphService>) -> Self {
        Self { graph }
    }
}

#[async_trait]
impl luxe_core::hooks::file_changed::TestGapAdvisor for GraphTestGaps {
    async fn untested_symbols(&self, path: &std::path::Path) -> Vec<String> {
        let Some(relative) = path.to_str() else {
            return Vec::new();
        };
        match self.graph.graph(false).await {
            Ok(graph) => {
                review_insight(&graph, &[ChangedRegion::whole_file(relative)]).untested_symbols
            }
            Err(error) => {
                tracing::warn!(%error, "test-gap advice skipped");
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luxe_core::guided_review::parse_unified_diff;

    /// The provider's job is to say *where* the change is, not just which file it is in.
    #[test]
    fn regions_carry_the_new_side_spans_of_every_hunk() {
        let files = parse_unified_diff(
            "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n\
             @@ -10,2 +10,3 @@ fn one\n context\n+added\n context\n\
             @@ -80,1 +81,1 @@ fn two\n-old\n+new\n",
        )
        .expect("fixture diff parses")
        .files;

        let region = changed_region(&files[0]).expect("a path");

        assert_eq!(region.file, "src/lib.rs");
        assert_eq!(region.changed_lines, vec![(10, 12), (81, 81)]);
    }
}
