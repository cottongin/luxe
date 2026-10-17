//! Graph model — nodes, edges, and the qualified names that address them.
//!
//! The structural layer under blast-radius (design-code-graph.md §3). A [`Node`] is a file or a
//! symbol; an [`Edge`] is a directed relationship (`Defines`/`Imports`/`Calls`/…). Two independent
//! axes describe an edge: [`Precision`] — *is this edge real?* (tree-sitter `Defines` is exact,
//! name-based `Calls` is a heuristic) — and [`EdgeTarget`] — *do we know which node it points at?*
//! (same-file targets resolve immediately; cross-file resolution is `resolve.rs`, phase G2).

use serde::{Deserialize, Serialize};

/// A node's stable, human-readable address within a project.
///
/// File nodes are the project-relative path (`src/auth.rs`); symbol nodes append `::` and a dotted
/// member path — top-level `src/auth.rs::authenticate`, member `src/auth.rs::AuthService.login`.
/// This mirrors the scheme `read_symbol` already uses implicitly.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct QualifiedName(String);

impl QualifiedName {
    /// A file node's qualified name (the project-relative path as-is).
    pub fn file(path: &str) -> Self {
        Self(path.to_string())
    }

    /// A symbol node's qualified name: `file::[Container.]name`.
    pub fn symbol(file: &str, container: Option<&str>, name: &str) -> Self {
        match container {
            Some(c) => Self(format!("{file}::{c}.{name}")),
            None => Self(format!("{file}::{name}")),
        }
    }

    /// Wrap a pre-built qualified-name string (e.g. loaded from cache).
    pub fn from_raw(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// The full qualified string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The project-relative file path (everything before the first `::`).
    pub fn file_path(&self) -> &str {
        self.0.split_once("::").map(|(f, _)| f).unwrap_or(&self.0)
    }

    /// The member path within the file (everything after `::`), or `None` for a file node.
    pub fn symbol_path(&self) -> Option<&str> {
        self.0.split_once("::").map(|(_, s)| s)
    }

    /// The simple, unqualified name — the last dotted segment of the symbol path, or the file path
    /// for a file node. `src/a.rs::Auth.login` → `login`; `src/a.rs` → `src/a.rs`.
    pub fn base_name(&self) -> &str {
        match self.symbol_path() {
            Some(sym) => sym.rsplit('.').next().unwrap_or(sym),
            None => &self.0,
        }
    }

    /// Whether this addresses a file (has no `::` symbol path).
    pub fn is_file(&self) -> bool {
        !self.0.contains("::")
    }
}

impl std::fmt::Display for QualifiedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a node is. Mirrors the LSP `SymbolKind` set we already emit, plus `File`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    File,
    Function,
    Method,
    Struct,
    Enum,
    /// A trait (Rust) or interface (TS/Go) — a nominal contract other types implement.
    Trait,
    /// A class, impl block, or type alias — anything `syntax.rs` maps to `CLASS`.
    Class,
    Field,
    Constant,
    Module,
    EnumMember,
    /// Any kind we don't model explicitly.
    Other,
}

impl NodeKind {
    /// Map an LSP `SymbolKind` integer (as produced by `syntax::outline`) to a `NodeKind`.
    pub fn from_symbol_kind(kind: u32) -> Self {
        use crate::protocol::symbol_kinds as k;
        match kind {
            k::FUNCTION => Self::Function,
            k::METHOD => Self::Method,
            k::STRUCT => Self::Struct,
            k::ENUM => Self::Enum,
            k::INTERFACE => Self::Trait,
            k::CLASS => Self::Class,
            k::FIELD | k::PROPERTY => Self::Field,
            k::CONSTANT => Self::Constant,
            k::MODULE | k::NAMESPACE | k::PACKAGE => Self::Module,
            k::ENUM_MEMBER => Self::EnumMember,
            _ => Self::Other,
        }
    }

    /// A short, stable label for output.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Function => "function",
            Self::Method => "method",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Trait => "trait",
            Self::Class => "class",
            Self::Field => "field",
            Self::Constant => "constant",
            Self::Module => "module",
            Self::EnumMember => "enum-member",
            Self::Other => "symbol",
        }
    }
}

/// A file or symbol in the graph. Line numbers are 0-based, matching `FlatSymbol`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    /// Stable address (the graph key).
    pub name: QualifiedName,
    pub kind: NodeKind,
    /// First line of the node's full extent (0-based); 0 for a file node.
    pub start_line: u32,
    /// Last line of the node's full extent (0-based); 0 for a file node.
    pub end_line: u32,
    /// Whether this looks like a test (heuristic — name/container based; see `extract`).
    pub is_test: bool,
}

impl Node {
    /// A file node for `path` (project-relative).
    pub fn file(path: &str) -> Self {
        Self {
            name: QualifiedName::file(path),
            kind: NodeKind::File,
            start_line: 0,
            end_line: 0,
            is_test: false,
        }
    }
}

/// Directed relationship kinds. `Defines`/`Imports`/`Inherits`/`Implements` land in G1;
/// `Calls`/`TestCovers` in G2.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeKind {
    /// File → symbol, or container → member.
    Defines,
    /// File → an imported module/file.
    Imports,
    /// Function → the function it calls (name-based heuristic).
    Calls,
    /// Type → a base class it extends.
    Inherits,
    /// Type → a trait/interface it implements.
    Implements,
    /// Test → the symbol it exercises (heuristic).
    TestCovers,
}

impl EdgeKind {
    /// A short, stable label for output.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Defines => "defines",
            Self::Imports => "imports",
            Self::Calls => "calls",
            Self::Inherits => "inherits",
            Self::Implements => "implements",
            Self::TestCovers => "tests",
        }
    }
}

/// How much to trust that an edge reflects real program structure.
///
/// Independent of resolution: an `Exact` edge can still be `Unresolved` (we know a `use` statement
/// exists but haven't mapped the module to a file yet), and a `Heuristic` edge can be `Resolved`
/// (a name-based call site that happens to have exactly one candidate).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Precision {
    /// Read straight from syntax — the relationship provably exists (`Defines`, `Imports` stmt).
    Exact,
    /// Name-based inference that may be wrong (`Calls`, `TestCovers`).
    Heuristic,
}

/// The far end of an edge: either a known node, or a raw name still awaiting resolution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeTarget {
    /// Resolved to a specific node in the graph.
    Resolved(QualifiedName),
    /// An unresolved raw reference — a module path, base-type name, or callee to resolve later.
    Unresolved(String),
}

impl EdgeTarget {
    /// The resolved node, if this target has one.
    pub fn resolved(&self) -> Option<&QualifiedName> {
        match self {
            Self::Resolved(qn) => Some(qn),
            Self::Unresolved(_) => None,
        }
    }

    /// The raw display string either way (qualified name or the raw reference).
    pub fn as_str(&self) -> &str {
        match self {
            Self::Resolved(qn) => qn.as_str(),
            Self::Unresolved(name) => name,
        }
    }
}

/// A directed edge between a known `from` node and a (possibly unresolved) `to` target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Edge {
    pub from: QualifiedName,
    pub to: EdgeTarget,
    pub kind: EdgeKind,
    pub precision: Precision,
}

impl Edge {
    /// A `Defines` edge (exact, resolved) from `from` to `to`.
    pub fn defines(from: QualifiedName, to: QualifiedName) -> Self {
        Self {
            from,
            to: EdgeTarget::Resolved(to),
            kind: EdgeKind::Defines,
            precision: Precision::Exact,
        }
    }

    /// Sort key for deterministic output: `(from, kind, target-string)`.
    pub fn sort_key(&self) -> (&str, u8, &str) {
        (self.from.as_str(), self.kind as u8, self.to.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_name_parts() {
        let f = QualifiedName::file("src/auth.rs");
        assert!(f.is_file());
        assert_eq!(f.file_path(), "src/auth.rs");
        assert_eq!(f.symbol_path(), None);
        assert_eq!(f.base_name(), "src/auth.rs");

        let top = QualifiedName::symbol("src/auth.rs", None, "authenticate");
        assert_eq!(top.as_str(), "src/auth.rs::authenticate");
        assert!(!top.is_file());
        assert_eq!(top.file_path(), "src/auth.rs");
        assert_eq!(top.symbol_path(), Some("authenticate"));
        assert_eq!(top.base_name(), "authenticate");

        let member = QualifiedName::symbol("src/auth.rs", Some("AuthService"), "login");
        assert_eq!(member.as_str(), "src/auth.rs::AuthService.login");
        assert_eq!(member.file_path(), "src/auth.rs");
        assert_eq!(member.symbol_path(), Some("AuthService.login"));
        assert_eq!(member.base_name(), "login");
    }

    #[test]
    fn node_kind_maps_from_symbol_kind() {
        use crate::protocol::symbol_kinds as k;
        assert_eq!(NodeKind::from_symbol_kind(k::FUNCTION), NodeKind::Function);
        assert_eq!(NodeKind::from_symbol_kind(k::METHOD), NodeKind::Method);
        assert_eq!(NodeKind::from_symbol_kind(k::INTERFACE), NodeKind::Trait);
        assert_eq!(NodeKind::from_symbol_kind(k::CLASS), NodeKind::Class);
        assert_eq!(NodeKind::from_symbol_kind(9999), NodeKind::Other);
    }

    #[test]
    fn edge_target_accessors() {
        let r = EdgeTarget::Resolved(QualifiedName::file("a.rs"));
        assert_eq!(r.resolved().map(QualifiedName::as_str), Some("a.rs"));
        assert_eq!(r.as_str(), "a.rs");

        let u = EdgeTarget::Unresolved("std::collections".to_string());
        assert!(u.resolved().is_none());
        assert_eq!(u.as_str(), "std::collections");
    }
}
