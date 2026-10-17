//! Blast radius — the transitive impact of a change (design-code-graph.md §7).
//!
//! Given seed symbols (a changed file's definitions, or one symbol), a **reverse** BFS over the
//! resolved reverse index answers *"who is affected if this changes?"* (callers, subclasses,
//! covering tests), and a shallower **forward** BFS answers *"what does this rely on?"* (the context
//! a reviewer/agent needs). Results are bounded by depth and a node cap, rolled up to ranked files
//! with a one-line reason, plus **test-gap** signals (seed symbols no test covers).
//!
//! Only *resolved* edges are traversed, so this is as precise as resolution (G2) made it. Import
//! edges are still name-unresolved, so file-level importers don't yet appear — callers/tests/
//! subclasses (the review-critical relationships) do.

use super::model::{EdgeKind, EdgeTarget, QualifiedName};
use super::store::{GraphStore, InMemoryGraph};
use std::collections::{HashMap, HashSet};

/// Default maximum BFS depth (like CRG).
pub const DEFAULT_MAX_DEPTH: u32 = 2;
/// Safety cap on nodes visited per direction, so a hub symbol can't fan out unboundedly.
const NODE_CAP: usize = 400;
/// How many ranked files each direction lists.
const MAX_FILES: usize = 20;
/// How many files the read-ready list suggests.
const MAX_READY: usize = 15;

/// Which way to walk the graph.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// Reverse edges — who depends on the seed.
    Dependents,
    /// Forward edges — what the seed depends on.
    Dependencies,
}

/// A file reached by the walk, with why and how far.
#[derive(Debug, Clone, PartialEq)]
pub struct ImpactedFile {
    pub file: String,
    pub min_depth: u32,
    /// A one-line reason, e.g. `session_handler calls login`.
    pub reason: String,
    /// Ranking score (higher = more impacted).
    pub score: f64,
}

/// The computed blast radius for a target.
#[derive(Debug, Clone)]
pub struct BlastRadius {
    /// The seed symbols the walk started from.
    pub seeds: Vec<QualifiedName>,
    pub max_depth: u32,
    /// Files affected if the seeds change (reverse), ranked.
    pub dependents: Vec<ImpactedFile>,
    /// Files the seeds rely on (forward), ranked.
    pub dependencies: Vec<ImpactedFile>,
    /// Function/method seeds with no covering test (a test-gap signal).
    pub untested_seeds: Vec<QualifiedName>,
}

/// Resolve `target` to seed nodes: an exact qualified name, else a file's defined symbols, else all
/// definitions of a bare name.
fn seeds_for(graph: &InMemoryGraph, target: &str) -> Vec<QualifiedName> {
    let as_qn = QualifiedName::from_raw(target);
    // An exact symbol name (a file node isn't a useful seed — its reverse edges are unresolved
    // imports; a file target should seed the file's symbols instead).
    if !as_qn.is_file() && graph.node(&as_qn).is_some() {
        return vec![as_qn];
    }
    let file_symbols = graph.symbols_in_file(target);
    if !file_symbols.is_empty() {
        return file_symbols;
    }
    graph.defs_by_name(target).to_vec()
}

/// Compute the blast radius of `target` (a qualified name, file path, or bare symbol name).
pub fn blast_radius(graph: &InMemoryGraph, target: &str, max_depth: u32) -> BlastRadius {
    let seeds = seeds_for(graph, target);
    let dependents = rank_files(walk(graph, &seeds, Direction::Dependents, max_depth));
    // Forward context is intentionally shallow — one hop of dependencies is enough to understand a
    // change without dragging in the whole transitive world.
    let dependencies = rank_files(walk(graph, &seeds, Direction::Dependencies, 1));
    let untested_seeds = untested(graph, &seeds);
    BlastRadius { seeds, max_depth, dependents, dependencies, untested_seeds }
}

/// One reached node during a walk.
struct Reached {
    file: String,
    depth: u32,
    reason: String,
    score: f64,
}

/// BFS from `seeds` following reverse or forward resolved edges, bounded by depth and a node cap.
fn walk(
    graph: &InMemoryGraph,
    seeds: &[QualifiedName],
    direction: Direction,
    max_depth: u32,
) -> Vec<Reached> {
    let mut visited: HashSet<QualifiedName> = seeds.iter().cloned().collect();
    let mut frontier: Vec<QualifiedName> = seeds.to_vec();
    let mut reached: Vec<Reached> = Vec::new();

    let mut depth = 1;
    while depth <= max_depth && !frontier.is_empty() && reached.len() < NODE_CAP {
        let mut next = Vec::new();
        for node in &frontier {
            let edges = match direction {
                Direction::Dependents => graph.in_edges(node),
                Direction::Dependencies => graph.out_edges(node),
            };
            for edge in edges {
                let neighbor = match direction {
                    Direction::Dependents => edge.from.clone(),
                    Direction::Dependencies => match &edge.to {
                        EdgeTarget::Resolved(qn) => qn.clone(),
                        EdgeTarget::Unresolved(_) => continue,
                    },
                };
                if neighbor.is_file() || !visited.insert(neighbor.clone()) {
                    continue;
                }
                reached.push(Reached {
                    file: neighbor.file_path().to_string(),
                    depth,
                    reason: reason_text(edge),
                    score: weight(edge.kind) / depth as f64,
                });
                next.push(neighbor);
                if reached.len() >= NODE_CAP {
                    break;
                }
            }
        }
        frontier = next;
        depth += 1;
    }
    reached
}

/// Roll reached nodes up to ranked files: max score wins the reason, min depth kept.
fn rank_files(reached: Vec<Reached>) -> Vec<ImpactedFile> {
    let mut by_file: HashMap<String, ImpactedFile> = HashMap::new();
    for r in reached {
        let entry = by_file.entry(r.file.clone()).or_insert_with(|| ImpactedFile {
            file: r.file.clone(),
            min_depth: r.depth,
            reason: r.reason.clone(),
            score: r.score,
        });
        entry.min_depth = entry.min_depth.min(r.depth);
        if r.score > entry.score {
            entry.score = r.score;
            entry.reason = r.reason;
        }
    }
    let mut files: Vec<ImpactedFile> = by_file.into_values().collect();
    files.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.min_depth.cmp(&b.min_depth))
            .then(a.file.cmp(&b.file))
    });
    files.truncate(MAX_FILES);
    files
}

/// Function/method seeds with no incoming `TestCovers` edge.
fn untested(graph: &InMemoryGraph, seeds: &[QualifiedName]) -> Vec<QualifiedName> {
    use super::model::NodeKind;
    seeds
        .iter()
        .filter(|qn| {
            matches!(graph.node(qn).map(|n| n.kind), Some(NodeKind::Function | NodeKind::Method))
        })
        .filter(|qn| !graph.in_edges(qn).iter().any(|e| e.kind == EdgeKind::TestCovers))
        .cloned()
        .collect()
}

/// Impact weight of an edge kind — direct behavior (tests/calls) outweighs structure (imports).
fn weight(kind: EdgeKind) -> f64 {
    match kind {
        EdgeKind::TestCovers => 4.0,
        EdgeKind::Calls => 3.0,
        EdgeKind::Inherits | EdgeKind::Implements => 2.0,
        EdgeKind::Imports => 1.0,
        EdgeKind::Defines => 0.5,
    }
}

/// The verb for a reason phrase.
fn verb(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::Calls => "calls",
        EdgeKind::TestCovers => "tests",
        EdgeKind::Inherits => "inherits",
        EdgeKind::Implements => "implements",
        EdgeKind::Imports => "imports",
        EdgeKind::Defines => "defines",
    }
}

/// `subject verb object`, e.g. `session_handler calls login`.
fn reason_text(edge: &super::model::Edge) -> String {
    let object = match &edge.to {
        EdgeTarget::Resolved(qn) => qn.base_name().to_string(),
        EdgeTarget::Unresolved(name) => name.rsplit('.').next().unwrap_or(name).to_string(),
    };
    format!("{} {} {}", edge.from.base_name(), verb(edge.kind), object)
}

impl BlastRadius {
    /// Whether nothing was reached (unknown target or an isolated symbol).
    pub fn is_empty(&self) -> bool {
        self.dependents.is_empty() && self.dependencies.is_empty()
    }

    /// Deduped, ranked files worth reading — dependents first, then dependencies.
    pub fn read_ready(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.dependents
            .iter()
            .chain(self.dependencies.iter())
            .map(|f| f.file.clone())
            .filter(|f| seen.insert(f.clone()))
            .take(MAX_READY)
            .collect()
    }

    /// A terse one-liner for `module_report` (counts only).
    pub fn summary_line(&self) -> String {
        format!(
            "{} dependent file(s), {} dependency file(s), {} untested seed(s)",
            self.dependents.len(),
            self.dependencies.len(),
            self.untested_seeds.len()
        )
    }

    /// A full, token-compact rendering for the `impact` tool.
    pub fn render(&self, target: &str) -> String {
        if self.seeds.is_empty() {
            return format!(
                "No graph node found for '{target}'. Try a file path or a symbol name."
            );
        }
        let mut out = format!(
            "Blast radius of `{target}` (depth <= {}, {} seed symbol(s)):\n",
            self.max_depth,
            self.seeds.len()
        );

        out.push_str(&format!(
            "\nDependents — affected if this changes ({}):\n",
            self.dependents.len()
        ));
        render_files(&mut out, &self.dependents);

        out.push_str(&format!(
            "\nDependencies — what this relies on ({}):\n",
            self.dependencies.len()
        ));
        render_files(&mut out, &self.dependencies);

        out.push_str("\nTest coverage: ");
        if self.untested_seeds.is_empty() {
            out.push_str("every function/method seed has a covering test.\n");
        } else {
            let names: Vec<&str> = self.untested_seeds.iter().map(|s| s.base_name()).collect();
            out.push_str(&format!("no covering test for {}.\n", names.join(", ")));
        }

        let ready = self.read_ready();
        if !ready.is_empty() {
            out.push_str(&format!("\nRead next: {}\n", ready.join(", ")));
        }
        out
    }
}

fn render_files(out: &mut String, files: &[ImpactedFile]) {
    if files.is_empty() {
        out.push_str("  (none)\n");
        return;
    }
    for file in files {
        out.push_str(&format!("  {}  ·  {}  [depth {}]\n", file.file, file.reason, file.min_depth));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::build::build_graph;

    fn project() -> Vec<(&'static str, &'static str)> {
        vec![
            ("src/auth.rs", "pub fn login() -> bool { true }\npub fn logout() {}\n"),
            ("src/session.rs", "use crate::auth::login;\npub fn start() -> bool { login() }\n"),
            ("src/api.rs", "use crate::session::start;\npub fn handle() -> bool { start() }\n"),
            (
                "tests/auth_test.rs",
                "#[cfg(test)]\nmod tests {\n    fn test_login() { login(); }\n}\n",
            ),
        ]
    }

    #[test]
    fn reverse_bfs_finds_transitive_callers_and_tests() {
        let graph = build_graph(project());
        let br = blast_radius(&graph, "src/auth.rs::login", DEFAULT_MAX_DEPTH);

        let dep_files: Vec<&str> = br.dependents.iter().map(|f| f.file.as_str()).collect();
        // Direct caller (depth 1) and transitive caller (depth 2).
        assert!(dep_files.contains(&"src/session.rs"), "direct caller");
        assert!(dep_files.contains(&"src/api.rs"), "transitive caller");
        // The covering test shows up too.
        assert!(dep_files.contains(&"tests/auth_test.rs"), "covering test");

        // The direct caller/test outrank the transitive caller.
        let session = br.dependents.iter().find(|f| f.file == "src/session.rs").unwrap();
        let api = br.dependents.iter().find(|f| f.file == "src/api.rs").unwrap();
        assert_eq!(session.min_depth, 1);
        assert_eq!(api.min_depth, 2);
        assert!(session.score > api.score);
    }

    #[test]
    fn depth_bound_is_respected() {
        let graph = build_graph(project());
        let shallow = blast_radius(&graph, "src/auth.rs::login", 1);
        let files: Vec<&str> = shallow.dependents.iter().map(|f| f.file.as_str()).collect();
        assert!(files.contains(&"src/session.rs"));
        // api.rs is two hops away → excluded at depth 1.
        assert!(!files.contains(&"src/api.rs"));
    }

    #[test]
    fn untested_seed_is_flagged() {
        let graph = build_graph(project());
        // logout has no test; login does.
        let logout = blast_radius(&graph, "src/auth.rs::logout", DEFAULT_MAX_DEPTH);
        assert!(logout.untested_seeds.iter().any(|s| s.base_name() == "logout"));

        let login = blast_radius(&graph, "src/auth.rs::login", DEFAULT_MAX_DEPTH);
        assert!(login.untested_seeds.is_empty(), "login is covered by test_login");
    }

    #[test]
    fn forward_walk_lists_dependencies() {
        let graph = build_graph(project());
        let br = blast_radius(&graph, "src/session.rs::start", DEFAULT_MAX_DEPTH);
        // start() calls login() in auth.rs.
        assert!(br.dependencies.iter().any(|f| f.file == "src/auth.rs"));
    }

    #[test]
    fn file_target_seeds_all_its_symbols() {
        let graph = build_graph(project());
        let br = blast_radius(&graph, "src/auth.rs", DEFAULT_MAX_DEPTH);
        // Seeds are auth.rs's functions (login, logout), not the file node.
        assert!(br.seeds.iter().all(|s| !s.is_file()));
        assert!(br.seeds.iter().any(|s| s.base_name() == "login"));
        // Its callers are still found.
        assert!(br.dependents.iter().any(|f| f.file == "src/session.rs"));
    }

    #[test]
    fn unknown_target_renders_a_hint() {
        let graph = build_graph(project());
        let br = blast_radius(&graph, "does_not_exist", DEFAULT_MAX_DEPTH);
        assert!(br.seeds.is_empty());
        assert!(br.render("does_not_exist").contains("No graph node found"));
    }
}
