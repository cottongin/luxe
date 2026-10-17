//! Review insight — graph-derived grounding for Guided Review.
//!
//! Turns a set of changed regions into advisory structural context: the ranked callers/dependents/
//! covering-tests per file, plus the changed symbols a test does and does not cover. Returns the
//! `luxe-core` review type directly so the engine can attach it to a review without `luxe-core`
//! depending on the graph. Pure and offline — fixture-testable in isolation.
//!
//! Coverage claims are scoped to the symbols the **diff actually touches**. Seeding from every symbol
//! in a changed file buries the reader in "no covering test" call-outs, test functions in a changed
//! test file included, and a signal meant to focus attention stops doing it.

use super::blast::{blast_radius, DEFAULT_MAX_DEPTH};
use super::model::{EdgeKind, NodeKind};
use super::store::{GraphStore, InMemoryGraph};
use super::testing::is_test_path;
use luxe_core::guided_review::{FileInsight, RelevantFile, ReviewInsight};

/// Max relevant files listed per changed file (keeps the review prompt/UI compact).
const MAX_RELEVANT_PER_FILE: usize = 6;

/// One changed file and the new-side line spans the diff touched.
///
/// Spans are 1-based and inclusive, matching unified-diff line numbers. An empty span list means
/// "somewhere in this file" — no symbol-level claim is made for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedRegion {
    /// Project-relative path of the changed file.
    pub file: String,
    /// New-side line spans the diff touched, 1-based inclusive.
    pub changed_lines: Vec<(u32, u32)>,
}

impl ChangedRegion {
    /// A whole-file region, for a change with no usable new-side line information.
    pub fn whole_file(file: impl Into<String>) -> Self {
        Self { file: file.into(), changed_lines: Vec::new() }
    }
}

/// Compute advisory structural grounding for a set of changed regions.
///
/// Files with no known structural relationships are skipped; the result is empty for a docs-only
/// diff or an unbuilt graph.
pub fn review_insight(graph: &InMemoryGraph, changed: &[ChangedRegion]) -> ReviewInsight {
    let mut files = Vec::new();
    let mut untested: Vec<String> = Vec::new();
    let mut covered: Vec<String> = Vec::new();

    for region in changed {
        let br = blast_radius(graph, &region.file, DEFAULT_MAX_DEPTH);
        if br.seeds.is_empty() {
            continue;
        }
        let relevant: Vec<RelevantFile> = br
            .dependents
            .iter()
            .take(MAX_RELEVANT_PER_FILE)
            .map(|impacted| RelevantFile {
                file: impacted.file.clone(),
                reason: impacted.reason.clone(),
            })
            .collect();
        if !relevant.is_empty() {
            files.push(FileInsight { file: region.file.clone(), relevant });
        }
        // A test file's own functions need no covering test; saying otherwise is noise a reviewer
        // learns to ignore, which costs every other signal its credibility.
        if is_test_path(&region.file) {
            continue;
        }
        for symbol in br.seeds.iter().filter(|seed| touches_change(graph, seed, region)) {
            let Some(node) = graph.node(symbol) else { continue };
            if node.is_test || !matches!(node.kind, NodeKind::Function | NodeKind::Method) {
                continue;
            }
            let has_test =
                graph.in_edges(symbol).iter().any(|edge| edge.kind == EdgeKind::TestCovers);
            if has_test {
                covered.push(symbol.to_string());
            } else {
                untested.push(symbol.to_string());
            }
        }
    }

    untested.sort();
    untested.dedup();
    covered.sort();
    covered.dedup();
    ReviewInsight { files, untested_symbols: untested, covered_symbols: covered }
}

/// Whether a symbol's extent overlaps one of the region's changed line spans.
///
/// A region with no spans matches every symbol in its file: without new-side line numbers (a pure
/// deletion, an unparsed hunk) the honest answer is "we don't know which symbol", and the file-wide
/// claim is the one we can still stand behind.
fn touches_change(
    graph: &InMemoryGraph,
    symbol: &super::model::QualifiedName,
    region: &ChangedRegion,
) -> bool {
    if region.changed_lines.is_empty() {
        return true;
    }
    let Some(node) = graph.node(symbol) else { return false };
    // Graph extents are 0-based; diff line numbers are 1-based.
    let start = node.start_line.saturating_add(1);
    let end = node.end_line.saturating_add(1).max(start);
    region.changed_lines.iter().any(|(from, to)| start <= *to && *from <= end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::build::build_graph;

    fn project() -> Vec<(&'static str, &'static str)> {
        vec![
            ("src/auth.rs", "pub fn login() {}\npub fn logout() {}\n"),
            ("src/session.rs", "use crate::auth::login;\npub fn start() { login() }\n"),
            (
                "tests/auth_test.rs",
                "#[cfg(test)]\nmod tests {\n    fn test_login() { login(); }\n}\n",
            ),
            ("docs/notes.md", "# just docs\n"),
        ]
    }

    #[test]
    fn insight_grounds_relevant_files_and_splits_covered_from_untested() {
        let graph = build_graph(project());
        let insight = review_insight(&graph, &[ChangedRegion::whole_file("src/auth.rs")]);

        let auth = insight.files.iter().find(|f| f.file == "src/auth.rs").expect("auth insight");
        let relevant: Vec<&str> = auth.relevant.iter().map(|r| r.file.as_str()).collect();
        assert!(relevant.contains(&"src/session.rs"), "caller grounded");
        assert!(relevant.contains(&"tests/auth_test.rs"), "covering test grounded");

        // logout has no test; login does — and both facts are now reported.
        assert!(insight.untested_symbols.iter().any(|s| s.ends_with("::logout")));
        assert!(!insight.untested_symbols.iter().any(|s| s.ends_with("::login")));
        assert!(insight.covers("src/auth.rs::login"), "coverage is evidence the budget can spend");
    }

    /// The defect that produced 236 call-outs: a one-line change flagged every symbol in the file.
    #[test]
    fn only_the_symbols_the_diff_touches_are_judged() {
        let graph = build_graph(project());
        // `login` occupies line 1, `logout` line 2; the diff touches only line 2.
        let insight = review_insight(
            &graph,
            &[ChangedRegion { file: "src/auth.rs".into(), changed_lines: vec![(2, 2)] }],
        );

        assert!(insight.untested_symbols.iter().any(|s| s.ends_with("::logout")));
        assert!(
            !insight.covered_symbols.iter().any(|s| s.ends_with("::login")),
            "an untouched symbol is not part of this review: {:?}",
            insight.covered_symbols
        );
    }

    /// A changed test file must not have every one of its test functions flagged as untested.
    #[test]
    fn a_changed_test_file_is_never_asked_for_a_covering_test() {
        let graph = build_graph(project());
        let insight = review_insight(&graph, &[ChangedRegion::whole_file("tests/auth_test.rs")]);

        assert!(
            insight.untested_symbols.is_empty(),
            "tests need no tests: {:?}",
            insight.untested_symbols
        );
    }

    /// Rust's other test layout: `mod tests;` pointing at a sibling file, with plain `#[test]`
    /// names. Neither a container nor a name says "test" there, so the covering test was invisible
    /// and both halves of the badge were wrong at once — the changed code read as untested, and the
    /// test file was asked for a test of its own.
    #[test]
    fn a_sibling_tests_module_covers_the_code_it_calls() {
        let graph = build_graph(vec![
            (
                "src/app/toasts.rs",
                "pub fn spinner_char() -> char { '|' }\npub fn tick_spinner() {}\n",
            ),
            (
                "src/app/tests.rs",
                "fn spinner_char_cycles_through_frames() {\n    spinner_char();\n    \
                 tick_spinner();\n}\n",
            ),
        ]);

        let insight = review_insight(&graph, &[ChangedRegion::whole_file("src/app/toasts.rs")]);
        assert!(
            insight.untested_symbols.is_empty(),
            "the sibling test module covers both: {:?}",
            insight.untested_symbols
        );
        assert!(insight.covers("src/app/toasts.rs::spinner_char"));
        assert!(insight.covers("src/app/toasts.rs::tick_spinner"));

        let on_the_test_file =
            review_insight(&graph, &[ChangedRegion::whole_file("src/app/tests.rs")]);
        assert!(
            on_the_test_file.untested_symbols.is_empty(),
            "a test file is never asked for a covering test: {:?}",
            on_the_test_file.untested_symbols
        );
    }

    #[test]
    fn docs_only_change_yields_empty_insight() {
        let graph = build_graph(project());
        let insight = review_insight(&graph, &[ChangedRegion::whole_file("docs/notes.md")]);
        assert!(insight.is_empty());
        assert!(insight.render_prompt_block().is_none());
    }

    #[test]
    fn multiple_changed_files_aggregate_and_dedup_test_gaps() {
        let graph = build_graph(project());
        let insight = review_insight(
            &graph,
            &[
                ChangedRegion::whole_file("src/auth.rs"),
                ChangedRegion::whole_file("src/session.rs"),
            ],
        );

        let mut sorted = insight.untested_symbols.clone();
        sorted.sort();
        assert_eq!(insight.untested_symbols, sorted);
    }
}
