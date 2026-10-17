//! Build — assemble a resolved in-memory graph from a set of files.
//!
//! The cold-build path (design-code-graph.md §4): extract every file, index every symbol name,
//! resolve each file's cross-file references against that index, then load the store so its reverse
//! index reflects resolved edges. G3's `GraphService` drives this on `spawn_blocking`; keeping it a
//! free function here means the same code powers both the warm service and headless tests.

use super::extract::{extract_file, FileGraph};
use super::resolve::{resolve_file, NameIndex};
use super::store::{GraphStore, InMemoryGraph};

/// Build a fully-resolved [`InMemoryGraph`] from `(project_relative_path, source)` pairs.
/// Unsupported-language files are skipped. Deterministic for a given input set.
pub fn build_graph<I, P, S>(files: I) -> InMemoryGraph
where
    I: IntoIterator<Item = (P, S)>,
    P: AsRef<str>,
    S: AsRef<str>,
{
    let fragments: Vec<(String, FileGraph)> = files
        .into_iter()
        .filter_map(|(path, src)| {
            let path = path.as_ref().to_string();
            extract_file(&path, src.as_ref()).map(|fg| (path, fg))
        })
        .collect();
    assemble(fragments)
}

/// Resolve + load pre-extracted `(path, fragment)` pairs into a graph.
///
/// The shared back half of every build path: one project-wide [`NameIndex`] over all symbol nodes,
/// then per-file resolution and upsert so the reverse index reflects resolved cross-file edges.
/// G3's cached build feeds this fragments loaded from disk instead of freshly extracted ones.
pub(super) fn assemble(fragments: Vec<(String, FileGraph)>) -> InMemoryGraph {
    let index = NameIndex::from_nodes(fragments.iter().flat_map(|(_, fg)| fg.nodes.iter()));
    let mut graph = InMemoryGraph::new();
    for (path, fg) in fragments {
        let resolved = resolve_file(fg, &index);
        graph.upsert_file(&path, resolved.nodes, resolved.edges);
    }
    graph
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::model::{EdgeKind, QualifiedName};

    fn files() -> Vec<(&'static str, &'static str)> {
        vec![
            ("src/lib.rs", "pub fn compute() -> i32 { 1 }\n"),
            (
                "src/app.rs",
                "use crate::lib::compute;\n\
                 fn run() { compute(); }\n\
                 #[cfg(test)]\nmod tests {\n    fn test_run() { compute(); }\n}\n",
            ),
        ]
    }

    #[test]
    fn build_wires_cross_file_calls_into_the_reverse_index() {
        let graph = build_graph(files());
        let compute = QualifiedName::from_raw("src/lib.rs::compute");

        // Someone depends on compute: run() calls it, and the reverse index knows.
        let callers: Vec<&str> = graph
            .in_edges(&compute)
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .map(|e| e.from.as_str())
            .collect();
        assert!(callers.contains(&"src/app.rs::run"));

        // And a test covers it.
        assert!(graph
            .in_edges(&compute)
            .iter()
            .any(|e| e.kind == EdgeKind::TestCovers
                && e.from.as_str() == "src/app.rs::tests.test_run"));
    }

    #[test]
    fn build_is_deterministic_regardless_of_file_order() {
        let mut reversed = files();
        reversed.reverse();
        let a = build_graph(files());
        let b = build_graph(reversed);
        assert_eq!(a.node_count(), b.node_count());
        assert_eq!(a.edge_count(), b.edge_count());

        let compute = QualifiedName::from_raw("src/lib.rs::compute");
        let mut ca: Vec<String> = a.in_edges(&compute).iter().map(|e| e.from.to_string()).collect();
        let mut cb: Vec<String> = b.in_edges(&compute).iter().map(|e| e.from.to_string()).collect();
        ca.sort();
        cb.sort();
        assert_eq!(ca, cb);
    }
}
