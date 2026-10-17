//! Code graph — a structural map of the project for blast-radius impact (design-code-graph.md).
//!
//! A directed graph of files and symbols (`model`) built from tree-sitter (`extract`) and held in a
//! swappable store (`store`). It answers "if this changes, what else is affected?" so an agent
//! reads only what matters and Guided Review can ground its relevant files in real structure. Later
//! phases add review integration (G5) and LSP enrichment (G6). Call/test resolution (G2) lives in
//! `resolve`; `build` assembles a fully-resolved graph from files; the warm `service` +
//! content-addressed `cache` (G3) keep it fast and incremental; `blast` computes impact (G4).

mod blast;
mod build;
mod cache;
mod enrich;
mod extract;
mod model;
mod resolve;
mod review;
mod service;
mod store;
mod testing;

pub use blast::{blast_radius, BlastRadius, ImpactedFile, DEFAULT_MAX_DEPTH};
pub use build::build_graph;
pub use cache::{content_hash, GraphCache};
pub use enrich::{verify_callers, CallerVerification, LspReferenceSource, ReferenceSource};
pub use extract::{extract_file, FileGraph};
pub use model::{Edge, EdgeKind, EdgeTarget, Node, NodeKind, Precision, QualifiedName};
pub use review::{review_insight, ChangedRegion};
pub use service::{build_graph_cached, BuildStats, GraphError, GraphService};
pub use store::{GraphStore, InMemoryGraph};
