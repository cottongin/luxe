//! Extraction — one file's source becomes nodes + edges (design-code-graph.md §4).
//!
//! Nodes and `Defines` edges are lifted from the tested [`crate::syntax::outline`] symbol layer;
//! `Imports`, `Inherits`/`Implements`, and `Calls` come from small per-language tree-sitter queries
//! here. This is the exact-to-heuristic structural layer: `Defines`/`Imports`/heritage are exact,
//! `Calls` is a name-based heuristic (§6). Same-file references resolve eagerly; everything
//! cross-file stays [`EdgeTarget::Unresolved`] for `resolve.rs` (G2) — we never invent an edge.

use super::model::{Edge, EdgeKind, EdgeTarget, Node, NodeKind, Precision, QualifiedName};
use crate::protocol::symbol_kinds as k;
use crate::syntax::{self, Lang};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Parser, Query, QueryCursor, Tree};

/// One file's extracted graph fragment. Serializable so it can be cached as a per-file shard (G3).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

/// Extract nodes + edges for a single file.
///
/// `rel_path` is the project-relative path — it seeds every qualified name and selects the
/// language. Returns `None` for an unsupported language. Output is sorted for determinism. Parses
/// once: the node layer (`syntax::outline_from_tree`) and the reference queries share the tree.
pub fn extract_file(rel_path: &str, source: &str) -> Option<FileGraph> {
    let lang = syntax::detect(Path::new(rel_path))?;
    let language = syntax::ts_language(lang);
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(source, None)?;
    let bytes = source.as_bytes();
    let symbols = syntax::outline_from_tree(lang, &tree, bytes);

    let file_qn = QualifiedName::file(rel_path);
    let mut nodes = Vec::with_capacity(symbols.len() + 1);
    nodes.push(Node::file(rel_path));

    // Simple name → qualified name for top-level (unnested) symbols; used to attach members to
    // their container and to resolve same-file heritage. Function/method ranges map a call site's
    // line back to its enclosing definition.
    let mut top_level: HashMap<String, QualifiedName> = HashMap::new();
    let mut func_ranges: Vec<(u32, u32, QualifiedName)> = Vec::new();
    let mut edges: Vec<Edge> = Vec::new();

    for sym in &symbols {
        let qn = QualifiedName::symbol(rel_path, sym.container.as_deref(), &sym.name);
        let is_test =
            super::testing::is_test_symbol(sym.kind, &sym.name, sym.container.as_deref(), rel_path);
        nodes.push(Node {
            name: qn.clone(),
            kind: NodeKind::from_symbol_kind(sym.kind),
            start_line: sym.start_line,
            end_line: sym.end_line,
            is_test,
        });
        if sym.container.is_none() {
            top_level.insert(sym.name.clone(), qn.clone());
        }
        if sym.kind == k::FUNCTION || sym.kind == k::METHOD {
            func_ranges.push((sym.start_line, sym.end_line, qn));
        }
    }

    // Defines edges: File → top-level symbol; container → member (falling back to File when the
    // container symbol itself wasn't captured).
    for sym in &symbols {
        let qn = QualifiedName::symbol(rel_path, sym.container.as_deref(), &sym.name);
        let parent = match &sym.container {
            None => file_qn.clone(),
            Some(c) => top_level.get(c).cloned().unwrap_or_else(|| file_qn.clone()),
        };
        edges.push(Edge::defines(parent, qn));
    }

    // The reference queries (imports / heritage / calls) run against the same parse.
    let mut seen_imports: HashSet<String> = HashSet::new();
    for module in extract_imports(lang, &tree, bytes) {
        if seen_imports.insert(module.clone()) {
            edges.push(Edge {
                from: file_qn.clone(),
                to: EdgeTarget::Unresolved(module),
                kind: EdgeKind::Imports,
                precision: Precision::Exact,
            });
        }
    }

    for (type_name, base_name, kind) in extract_heritage(lang, &tree, bytes) {
        let from = top_level
            .get(&type_name)
            .cloned()
            .unwrap_or_else(|| QualifiedName::symbol(rel_path, None, &type_name));
        let to = match top_level.get(&base_name) {
            Some(qn) => EdgeTarget::Resolved(qn.clone()),
            None => EdgeTarget::Unresolved(base_name),
        };
        edges.push(Edge { from, to, kind, precision: Precision::Exact });
    }

    let aliases = alias_map(lang, &tree, bytes);
    for (callee, row) in call_sites(lang, &tree, bytes) {
        // Resolve a locally-renamed import back to the name the rest of the graph knows.
        let callee = aliases.get(&callee).cloned().unwrap_or(callee);
        if let Some(from) = enclosing_function(row, &func_ranges) {
            edges.push(Edge {
                from,
                to: EdgeTarget::Unresolved(callee),
                kind: EdgeKind::Calls,
                precision: Precision::Heuristic,
            });
        }
    }

    nodes.sort_by(|a, b| (a.start_line, a.name.as_str()).cmp(&(b.start_line, b.name.as_str())));
    edges.sort_by(|a, b| a.sort_key().cmp(&b.sort_key()));
    edges.dedup();

    Some(FileGraph { nodes, edges })
}

/// Innermost function/method whose line range contains `row` — a call site's enclosing definition.
fn enclosing_function(row: u32, ranges: &[(u32, u32, QualifiedName)]) -> Option<QualifiedName> {
    ranges
        .iter()
        .filter(|(start, end, _)| *start <= row && row <= *end)
        .min_by_key(|(start, end, _)| end - start)
        .map(|(_, _, qn)| qn.clone())
}

// === per-language queries ===

/// Import query per language; the captured node is named `mod`.
fn import_query(lang: Lang) -> &'static str {
    match lang {
        Lang::Rust => "(use_declaration argument: (_) @mod)",
        Lang::Python => {
            "(import_statement name: (dotted_name) @mod)
             (import_statement name: (aliased_import name: (dotted_name) @mod))
             (import_from_statement module_name: (dotted_name) @mod)"
        }
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => {
            "(import_statement source: (string (string_fragment) @mod))"
        }
        Lang::Go => {
            "(import_spec path: (interpreted_string_literal (interpreted_string_literal_content) @mod))"
        }
    }
}

/// Heritage query per language, plus which base-capture maps to which edge kind.
fn heritage_spec(lang: Lang) -> Option<(&'static str, &'static [(&'static str, EdgeKind)])> {
    match lang {
        Lang::Rust => Some((
            "(impl_item trait: (type_identifier) @implements type: (type_identifier) @type)",
            &[("implements", EdgeKind::Implements)],
        )),
        Lang::Python => Some((
            "(class_definition name: (identifier) @type superclasses: (argument_list (identifier) @inherits))",
            &[("inherits", EdgeKind::Inherits)],
        )),
        Lang::TypeScript | Lang::Tsx => Some((
            "(class_declaration name: (type_identifier) @type (class_heritage (extends_clause value: (identifier) @inherits)))
             (class_declaration name: (type_identifier) @type (class_heritage (implements_clause (type_identifier) @implements)))",
            &[("inherits", EdgeKind::Inherits), ("implements", EdgeKind::Implements)],
        )),
        Lang::JavaScript => Some((
            "(class_declaration name: (identifier) @type (class_heritage (identifier) @inherits))",
            &[("inherits", EdgeKind::Inherits)],
        )),
        Lang::Go => None,
    }
}

/// Call-site query per language; the callee identifier is named `callee`. Covers bare calls,
/// method/attribute calls, and (Rust/Go) scoped/selector calls.
///
/// Rust also reaches inside macros. A macro's arguments parse as an unstructured `token_tree`, so
/// none of the `call_expression` patterns can see them — and in Rust the calls that prove a test
/// exercises something live almost entirely inside `assert!`/`assert_eq!`. Skipping them would
/// derive no `TestCovers` edge for the one call that matters and label a covered function untested.
/// Inside a token tree an identifier immediately followed by another token tree *is* the call
/// syntax; anything it over-matches (`vec![..]`, an index, a struct literal) resolves to nothing in
/// the project index and is dropped as external.
fn call_query(lang: Lang) -> &'static str {
    match lang {
        Lang::Rust => {
            "(call_expression function: (identifier) @callee)
             (call_expression function: (scoped_identifier name: (identifier) @callee))
             (call_expression function: (field_expression field: (field_identifier) @callee))
             (token_tree (identifier) @callee . (token_tree))"
        }
        Lang::Python => {
            "(call function: (identifier) @callee)
             (call function: (attribute attribute: (identifier) @callee))"
        }
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => {
            "(call_expression function: (identifier) @callee)
             (call_expression function: (member_expression property: (property_identifier) @callee))"
        }
        Lang::Go => {
            "(call_expression function: (identifier) @callee)
             (call_expression function: (selector_expression field: (field_identifier) @callee))"
        }
    }
}

// === query runner ===

/// One captured node: its text and 0-based start row.
struct Cap {
    text: String,
    row: u32,
}

/// A language's compiled reference queries, built once and reused for every file.
///
/// Compiling a tree-sitter `Query` is the dominant cost of a cold build (a state-machine
/// compilation, far pricier than parsing a file). Recompiling per file made a whole-repo build
/// seconds slower; caching per language keeps it well under the budget.
struct LangQueries {
    import: Option<Query>,
    heritage: Option<Query>,
    calls: Option<Query>,
    aliases: Option<Query>,
}

fn compile_queries(lang: Lang) -> LangQueries {
    let language = syntax::ts_language(lang);
    LangQueries {
        import: Query::new(&language, import_query(lang)).ok(),
        heritage: heritage_spec(lang).and_then(|(src, _)| Query::new(&language, src).ok()),
        calls: Query::new(&language, call_query(lang)).ok(),
        aliases: alias_query(lang).and_then(|src| Query::new(&language, src).ok()),
    }
}

/// Cached compiled queries for `lang` — compiled on first use, then shared process-wide.
fn lang_queries(lang: Lang) -> &'static LangQueries {
    match lang {
        Lang::Rust => {
            static Q: OnceLock<LangQueries> = OnceLock::new();
            Q.get_or_init(|| compile_queries(Lang::Rust))
        }
        Lang::Python => {
            static Q: OnceLock<LangQueries> = OnceLock::new();
            Q.get_or_init(|| compile_queries(Lang::Python))
        }
        Lang::TypeScript => {
            static Q: OnceLock<LangQueries> = OnceLock::new();
            Q.get_or_init(|| compile_queries(Lang::TypeScript))
        }
        Lang::Tsx => {
            static Q: OnceLock<LangQueries> = OnceLock::new();
            Q.get_or_init(|| compile_queries(Lang::Tsx))
        }
        Lang::JavaScript => {
            static Q: OnceLock<LangQueries> = OnceLock::new();
            Q.get_or_init(|| compile_queries(Lang::JavaScript))
        }
        Lang::Go => {
            static Q: OnceLock<LangQueries> = OnceLock::new();
            Q.get_or_init(|| compile_queries(Lang::Go))
        }
    }
}

/// Run a compiled query over the parsed tree; return, per match, a map from wanted capture name →
/// capture.
fn collect_matches(
    query: &Query,
    tree: &Tree,
    bytes: &[u8],
    wanted: &[&str],
) -> Vec<HashMap<String, Cap>> {
    let indices: Vec<(String, u32)> = wanted
        .iter()
        .filter_map(|name| query.capture_index_for_name(name).map(|i| ((*name).to_string(), i)))
        .collect();

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), bytes);
    let mut out = Vec::new();
    while let Some(m) = matches.next() {
        let mut fields = HashMap::new();
        for (name, index) in &indices {
            if let Some(cap) = m.captures.iter().find(|c| c.index == *index) {
                if let Ok(text) = cap.node.utf8_text(bytes) {
                    fields.insert(
                        name.clone(),
                        Cap { text: text.to_string(), row: cap.node.start_position().row as u32 },
                    );
                }
            }
        }
        if !fields.is_empty() {
            out.push(fields);
        }
    }
    out
}

/// Normalized, deduplicatable module reference strings.
fn extract_imports(lang: Lang, tree: &Tree, bytes: &[u8]) -> Vec<String> {
    let Some(query) = &lang_queries(lang).import else { return Vec::new() };
    collect_matches(query, tree, bytes, &["mod"])
        .into_iter()
        .filter_map(|mut m| m.remove("mod"))
        .map(|c| normalize_ws(&c.text))
        .filter(|s| !s.is_empty())
        .collect()
}

/// `(type_name, base_name, kind)` triples for each inheritance/implementation relationship.
fn extract_heritage(lang: Lang, tree: &Tree, bytes: &[u8]) -> Vec<(String, String, EdgeKind)> {
    let Some((_, bases)) = heritage_spec(lang) else { return Vec::new() };
    let Some(query) = &lang_queries(lang).heritage else { return Vec::new() };
    let wanted: Vec<&str> = std::iter::once("type").chain(bases.iter().map(|(n, _)| *n)).collect();

    let mut out = Vec::new();
    for m in collect_matches(query, tree, bytes, &wanted) {
        let Some(type_cap) = m.get("type") else { continue };
        for (cap, kind) in bases {
            if let Some(base) = m.get(*cap) {
                out.push((type_cap.text.clone(), base.text.clone(), *kind));
            }
        }
    }
    out
}

/// `(callee_name, row)` for each call site.
fn call_sites(lang: Lang, tree: &Tree, bytes: &[u8]) -> Vec<(String, u32)> {
    let Some(query) = &lang_queries(lang).calls else { return Vec::new() };
    collect_matches(query, tree, bytes, &["callee"])
        .into_iter()
        .filter_map(|mut m| m.remove("callee"))
        .map(|c| (c.text, c.row))
        .collect()
}

/// Locally-renamed imports: `alias -> original name`.
///
/// Call edges are matched by name, so a renamed import silently breaks the link — a file doing
/// `from x import calculate_credits as _calc` and then calling `_calc(...)` recorded a call to
/// `_calc`, which resolves to nothing, and the symbol's blast radius came back one caller short.
/// Under-reporting a blast radius is the one failure this tool must not have.
fn alias_map(lang: Lang, tree: &Tree, bytes: &[u8]) -> std::collections::HashMap<String, String> {
    let Some(query) = &lang_queries(lang).aliases else { return Default::default() };
    collect_matches(query, tree, bytes, &["orig", "alias"])
        .into_iter()
        .filter_map(|mut m| {
            let orig = m.remove("orig")?.text;
            let alias = m.remove("alias")?.text;
            // Only the final segment matters: calls are matched on the bare name.
            let orig = orig.rsplit('.').next()?.rsplit("::").next()?.to_string();
            (orig != alias).then_some((alias, orig))
        })
        .collect()
}

/// Renamed-import query per language, capturing `@orig` (the real name) and `@alias`.
fn alias_query(lang: Lang) -> Option<&'static str> {
    match lang {
        Lang::Python => Some(
            "(import_from_statement name: (aliased_import name: (dotted_name) @orig alias: (identifier) @alias))
             (import_statement name: (aliased_import name: (dotted_name) @orig alias: (identifier) @alias))",
        ),
        Lang::Rust => Some("(use_as_clause path: (_) @orig alias: (identifier) @alias)"),
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => Some(
            "(import_specifier name: (identifier) @orig alias: (identifier) @alias)",
        ),
        Lang::Go => None,
    }
}

/// Collapse internal whitespace runs to single spaces and trim — so an import reference is stable
/// regardless of the source's formatting.
fn normalize_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests;
