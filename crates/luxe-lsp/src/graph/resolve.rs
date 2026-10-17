//! Resolution — turn a file's raw name references into edges pointing at real nodes.
//!
//! Extraction leaves cross-file `Calls`/`Inherits`/`Implements` targets as
//! [`EdgeTarget::Unresolved`] simple names. Here we map each name to a definition through the
//! funnel of design-code-graph.md §6: **same-file symbol → project-wide unique → (ambiguous)**.
//! Ambiguity is never guessed away — an unresolvable-or-ambiguous project reference stays
//! `Unresolved` (a candidate for LSP enrichment in G6). External `Calls` (a name the project never
//! defines — stdlib/deps) are dropped as inert blast-radius noise; external heritage is kept as
//! context. Finally we derive `TestCovers` from resolved calls out of test nodes.

use super::extract::FileGraph;
use super::model::{Edge, EdgeKind, EdgeTarget, Node, Precision, QualifiedName};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Resolution {
    /// Exactly one definition — a confident target.
    Resolved(QualifiedName),
    /// Several candidate definitions — kept unresolved rather than guessed.
    Ambiguous(Vec<QualifiedName>),
    /// No project definition of this name (external/library reference).
    NotFound,
}

/// A project-wide map from a symbol's simple name to the qualified names that define it. Built once
/// over every file's nodes, then used to resolve each file's references (mirrors how the BM25
/// index is built once and queried many times).
pub(super) struct NameIndex {
    by_name: HashMap<String, Vec<QualifiedName>>,
}

impl NameIndex {
    /// Index every symbol node (file nodes aren't resolution targets).
    pub(super) fn from_nodes<'a>(nodes: impl Iterator<Item = &'a Node>) -> Self {
        let mut by_name: HashMap<String, Vec<QualifiedName>> = HashMap::new();
        for node in nodes {
            if node.name.is_file() {
                continue;
            }
            by_name.entry(node.name.base_name().to_string()).or_default().push(node.name.clone());
        }
        for defs in by_name.values_mut() {
            defs.sort();
            defs.dedup();
        }
        Self { by_name }
    }

    /// Resolve `name` as referenced from `caller_file`, honoring the funnel: a unique same-file
    /// definition wins; otherwise a unique project-wide definition; otherwise ambiguous/not-found.
    fn resolve(&self, caller_file: &str, name: &str) -> Resolution {
        let Some(candidates) = self.by_name.get(name) else { return Resolution::NotFound };

        let same_file: Vec<&QualifiedName> =
            candidates.iter().filter(|qn| qn.file_path() == caller_file).collect();
        match same_file.len() {
            1 => return Resolution::Resolved(same_file[0].clone()),
            n if n > 1 => return Resolution::Ambiguous(same_file.into_iter().cloned().collect()),
            _ => {}
        }

        match candidates.len() {
            1 => Resolution::Resolved(candidates[0].clone()),
            _ => Resolution::Ambiguous(candidates.clone()),
        }
    }
}

/// Resolve a file's edges against the project name index, in place, and derive `TestCovers`.
///
/// Upgrades `Calls`/`Inherits`/`Implements` to resolved targets; drops external `Calls`; keeps
/// ambiguous and external-heritage references unresolved; drops self-edges. Returns the file graph
/// with its edges rewritten (nodes untouched).
pub(super) fn resolve_file(mut fg: FileGraph, index: &NameIndex) -> FileGraph {
    let test_nodes: HashSet<QualifiedName> =
        fg.nodes.iter().filter(|n| n.is_test).map(|n| n.name.clone()).collect();

    let mut resolved: Vec<Edge> = Vec::with_capacity(fg.edges.len());
    let mut test_covers: Vec<Edge> = Vec::new();

    for mut edge in fg.edges.drain(..) {
        if is_name_ref(edge.kind) {
            if let EdgeTarget::Unresolved(name) = &edge.to {
                match index.resolve(edge.from.file_path(), name) {
                    Resolution::Resolved(qn) => {
                        if qn == edge.from {
                            continue; // self-edge (recursion / self-impl) — inert
                        }
                        edge.to = EdgeTarget::Resolved(qn);
                    }
                    Resolution::Ambiguous(_) => {} // keep unresolved candidate — never guess
                    Resolution::NotFound => {
                        if edge.kind == EdgeKind::Calls {
                            continue; // external/library call — inert blast-radius noise
                        }
                        // external base type — keep as unresolved context
                    }
                }
            }
        }

        if edge.kind == EdgeKind::Calls && test_nodes.contains(&edge.from) {
            if let EdgeTarget::Resolved(target) = &edge.to {
                test_covers.push(Edge {
                    from: edge.from.clone(),
                    to: EdgeTarget::Resolved(target.clone()),
                    kind: EdgeKind::TestCovers,
                    precision: Precision::Heuristic,
                });
            }
        }
        resolved.push(edge);
    }

    resolved.extend(test_covers);
    resolved.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    resolved.dedup();
    fg.edges = resolved;
    fg
}

/// Whether an edge kind carries a symbol-name reference the funnel should resolve.
fn is_name_ref(kind: EdgeKind) -> bool {
    matches!(kind, EdgeKind::Calls | EdgeKind::Inherits | EdgeKind::Implements)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::extract::extract_file;
    use crate::graph::model::NodeKind;

    fn node(qn: &str, kind: NodeKind, is_test: bool) -> Node {
        Node { name: QualifiedName::from_raw(qn), kind, start_line: 0, end_line: 0, is_test }
    }

    #[test]
    fn resolve_prefers_same_file_then_project_unique() {
        let nodes = [
            node("a.rs::foo", NodeKind::Function, false),
            node("b.rs::foo", NodeKind::Function, false),
            node("a.rs::only", NodeKind::Function, false),
        ];
        let index = NameIndex::from_nodes(nodes.iter());

        // `foo` exists in two files → ambiguous from a third file...
        assert_eq!(
            index.resolve("c.rs", "foo"),
            Resolution::Ambiguous(vec![
                QualifiedName::from_raw("a.rs::foo"),
                QualifiedName::from_raw("b.rs::foo"),
            ])
        );
        // ...but a same-file definition wins.
        assert_eq!(
            index.resolve("a.rs", "foo"),
            Resolution::Resolved(QualifiedName::from_raw("a.rs::foo"))
        );
        // A project-unique name resolves from anywhere.
        assert_eq!(
            index.resolve("z.rs", "only"),
            Resolution::Resolved(QualifiedName::from_raw("a.rs::only"))
        );
        // An unknown name is not found.
        assert_eq!(index.resolve("z.rs", "nope"), Resolution::NotFound);
    }

    /// End-to-end: two files, a cross-file call resolves, an external call is dropped, and a test
    /// that calls the covered symbol yields a `TestCovers` edge.
    #[test]
    fn resolve_file_wires_calls_and_test_covers() {
        let lib = extract_file("src/lib.rs", "pub fn compute() -> i32 { helperx() }\n").unwrap();
        let caller = extract_file(
            "src/app.rs",
            "fn run() { compute(); println(); }\n\
             #[cfg(test)]\nmod tests {\n    fn test_run() { compute(); }\n}\n",
        )
        .unwrap();

        let index = NameIndex::from_nodes(lib.nodes.iter().chain(caller.nodes.iter()));
        let caller = resolve_file(caller, &index);

        let calls: Vec<(String, String, bool)> = caller
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .map(|e| (e.from.as_str().into(), e.to.as_str().into(), e.to.resolved().is_some()))
            .collect();
        // `compute` resolves cross-file; the external `println` call is dropped as noise.
        assert!(calls.contains(&("src/app.rs::run".into(), "src/lib.rs::compute".into(), true)));
        assert!(!calls.iter().any(|(_, to, _)| to.contains("println")));

        // The test's call to compute() becomes a TestCovers edge.
        let covers: Vec<(String, String)> = caller
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::TestCovers)
            .map(|e| (e.from.as_str().into(), e.to.as_str().into()))
            .collect();
        assert_eq!(
            covers,
            vec![("src/app.rs::tests.test_run".to_string(), "src/lib.rs::compute".to_string())]
        );
    }

    #[test]
    fn recursion_self_edge_is_dropped() {
        let fg = extract_file("r.rs", "fn fact(n: u64) -> u64 { fact(n - 1) }\n").unwrap();
        let index = NameIndex::from_nodes(fg.nodes.iter());
        let fg = resolve_file(fg, &index);
        assert!(fg.edges.iter().all(|e| e.kind != EdgeKind::Calls), "self-recursion dropped");
    }

    #[test]
    fn ambiguous_call_stays_unresolved() {
        // Two `render` methods across files → a call to `render` can't be disambiguated.
        let a = extract_file("a.rs", "struct A;\nimpl A { fn render(&self) {} }\n").unwrap();
        let b = extract_file("b.rs", "struct B;\nimpl B { fn render(&self) {} }\n").unwrap();
        let c = extract_file("c.rs", "fn draw(x: A) { x.render(); }\n").unwrap();
        let index =
            NameIndex::from_nodes(a.nodes.iter().chain(b.nodes.iter()).chain(c.nodes.iter()));
        let c = resolve_file(c, &index);
        let render_call =
            c.edges.iter().find(|e| e.kind == EdgeKind::Calls && e.to.as_str() == "render");
        assert!(render_call.is_some(), "kept as an ambiguous candidate");
        assert!(render_call.unwrap().to.resolved().is_none(), "not guessed to a single target");
    }
}
