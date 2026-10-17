//! Graph storage — the `GraphStore` trait and an in-memory implementation.
//!
//! The load-bearing storage decision (design-code-graph.md §5): keep the whole graph in RAM behind
//! a trait so a `SqliteGraphStore` can drop in later, but don't take the C/SQL dependency now.
//! [`InMemoryGraph`] holds forward adjacency, a reverse index (resolved targets only — the
//! blast-radius hot path), and a name→definitions map for `resolve.rs` (G2). All mutation is
//! per-file (`upsert_file`/`remove_file`) so the `FileChanged` hook can re-index one file (G3).

use super::model::{Edge, EdgeTarget, Node, QualifiedName};
use std::collections::HashMap;

/// A structural code graph. Trait-first so the backing store is swappable (§5).
pub trait GraphStore {
    /// Replace all nodes/edges for `file` (add on first sight, update on re-index).
    fn upsert_file(&mut self, file: &str, nodes: Vec<Node>, edges: Vec<Edge>);

    /// Drop every node/edge owned by `file`.
    fn remove_file(&mut self, file: &str);

    /// The node at `name`, if present.
    fn node(&self, name: &QualifiedName) -> Option<&Node>;

    /// Outgoing edges from `name` (what it depends on / defines).
    fn out_edges(&self, name: &QualifiedName) -> &[Edge];

    /// Incoming resolved edges to `name` (who depends on it) — the reverse BFS frontier.
    fn in_edges(&self, name: &QualifiedName) -> &[Edge];

    /// Qualified names defining the simple identifier `base` (for name resolution).
    fn defs_by_name(&self, base: &str) -> &[QualifiedName];

    /// Qualified names of the symbols `file` defines (excludes the file node itself) — the seed set
    /// for a file-level blast radius.
    fn symbols_in_file(&self, file: &str) -> Vec<QualifiedName>;

    fn node_count(&self) -> usize;

    fn edge_count(&self) -> usize;

    /// Edges touching `name` (out + in) — a cheap degree-centrality proxy for ranking.
    fn degree(&self, name: &QualifiedName) -> usize {
        self.out_edges(name).len() + self.in_edges(name).len()
    }
}

const NO_EDGES: &[Edge] = &[];
const NO_NAMES: &[QualifiedName] = &[];

/// A whole-graph, in-RAM store. Fast native adjacency for the blast-radius hot path.
#[derive(Default)]
pub struct InMemoryGraph {
    nodes: HashMap<QualifiedName, Node>,
    out_edges: HashMap<QualifiedName, Vec<Edge>>,
    in_edges: HashMap<QualifiedName, Vec<Edge>>,
    defs_by_name: HashMap<String, Vec<QualifiedName>>,
    /// Which qualified names each file owns — the unit of incremental replacement.
    file_nodes: HashMap<String, Vec<QualifiedName>>,
}

impl InMemoryGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every node currently in the graph, order unspecified (tests should sort).
    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    /// Total edge records (forward adjacency), for diagnostics/tests.
    fn total_forward_edges(&self) -> usize {
        self.out_edges.values().map(Vec::len).sum()
    }
}

impl GraphStore for InMemoryGraph {
    fn upsert_file(&mut self, file: &str, nodes: Vec<Node>, edges: Vec<Edge>) {
        self.remove_file(file);

        let mut owned = Vec::with_capacity(nodes.len());
        for node in nodes {
            let qn = node.name.clone();
            self.defs_by_name.entry(qn.base_name().to_string()).or_default().push(qn.clone());
            owned.push(qn.clone());
            self.nodes.insert(qn, node);
        }
        self.file_nodes.insert(file.to_string(), owned);

        for edge in edges {
            if let EdgeTarget::Resolved(target) = &edge.to {
                self.in_edges.entry(target.clone()).or_default().push(edge.clone());
            }
            self.out_edges.entry(edge.from.clone()).or_default().push(edge);
        }
    }

    fn remove_file(&mut self, file: &str) {
        let Some(owned) = self.file_nodes.remove(file) else { return };
        for qn in &owned {
            self.nodes.remove(qn);

            // Drop this node's outgoing edges, unhooking each from its target's reverse index.
            if let Some(outs) = self.out_edges.remove(qn) {
                for edge in outs {
                    if let EdgeTarget::Resolved(target) = &edge.to {
                        if let Some(ins) = self.in_edges.get_mut(target) {
                            ins.retain(|e| e.from != *qn);
                            if ins.is_empty() {
                                self.in_edges.remove(target);
                            }
                        }
                    }
                }
            }

            // Drop reverse edges pointing at this node (their sources live in other files).
            self.in_edges.remove(qn);

            // Drop this node from the name index.
            if let Some(defs) = self.defs_by_name.get_mut(qn.base_name()) {
                defs.retain(|d| d != qn);
                if defs.is_empty() {
                    self.defs_by_name.remove(qn.base_name());
                }
            }
        }
    }

    fn node(&self, name: &QualifiedName) -> Option<&Node> {
        self.nodes.get(name)
    }

    fn out_edges(&self, name: &QualifiedName) -> &[Edge] {
        self.out_edges.get(name).map(Vec::as_slice).unwrap_or(NO_EDGES)
    }

    fn in_edges(&self, name: &QualifiedName) -> &[Edge] {
        self.in_edges.get(name).map(Vec::as_slice).unwrap_or(NO_EDGES)
    }

    fn defs_by_name(&self, base: &str) -> &[QualifiedName] {
        self.defs_by_name.get(base).map(Vec::as_slice).unwrap_or(NO_NAMES)
    }

    fn symbols_in_file(&self, file: &str) -> Vec<QualifiedName> {
        self.file_nodes
            .get(file)
            .map(|owned| owned.iter().filter(|qn| !qn.is_file()).cloned().collect())
            .unwrap_or_default()
    }

    fn node_count(&self) -> usize {
        self.nodes.len()
    }

    fn edge_count(&self) -> usize {
        self.total_forward_edges()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::model::{EdgeKind, NodeKind, Precision};

    fn func(file: &str, name: &str) -> Node {
        Node {
            name: QualifiedName::symbol(file, None, name),
            kind: NodeKind::Function,
            start_line: 0,
            end_line: 1,
            is_test: false,
        }
    }

    fn resolved_edge(from: QualifiedName, to: QualifiedName, kind: EdgeKind) -> Edge {
        Edge { from, to: EdgeTarget::Resolved(to), kind, precision: Precision::Exact }
    }

    #[test]
    fn upsert_populates_forward_reverse_and_name_index() {
        let mut g = InMemoryGraph::new();
        let file = "src/a.rs";
        let caller = func(file, "caller");
        let callee = func(file, "callee");
        let edge = resolved_edge(caller.name.clone(), callee.name.clone(), EdgeKind::Calls);

        g.upsert_file(file, vec![caller.clone(), callee.clone()], vec![edge.clone()]);

        assert_eq!(g.node_count(), 2);
        assert_eq!(g.edge_count(), 1);
        assert!(g.node(&caller.name).is_some());
        assert_eq!(g.out_edges(&caller.name), std::slice::from_ref(&edge));
        assert_eq!(g.in_edges(&callee.name), std::slice::from_ref(&edge));
        assert_eq!(g.defs_by_name("callee"), std::slice::from_ref(&callee.name));
        // Nothing points at the caller yet.
        assert!(g.in_edges(&caller.name).is_empty());
    }

    #[test]
    fn remove_file_clears_nodes_edges_and_reverse_index() {
        let mut g = InMemoryGraph::new();
        let file = "src/a.rs";
        let caller = func(file, "caller");
        let callee = func(file, "callee");
        let edge = resolved_edge(caller.name.clone(), callee.name.clone(), EdgeKind::Calls);
        g.upsert_file(file, vec![caller.clone(), callee.clone()], vec![edge]);

        g.remove_file(file);

        assert_eq!(g.node_count(), 0);
        assert_eq!(g.edge_count(), 0);
        assert!(g.in_edges(&callee.name).is_empty());
        assert!(g.defs_by_name("callee").is_empty());
    }

    #[test]
    fn upsert_replaces_prior_version_of_same_file() {
        let mut g = InMemoryGraph::new();
        let file = "src/a.rs";
        g.upsert_file(file, vec![func(file, "old")], vec![]);
        assert_eq!(g.defs_by_name("old").len(), 1);

        // Re-index the file with a renamed symbol — the old one must disappear.
        g.upsert_file(file, vec![func(file, "new")], vec![]);
        assert!(g.defs_by_name("old").is_empty());
        assert_eq!(g.defs_by_name("new").len(), 1);
        assert_eq!(g.node_count(), 1);
    }
}
