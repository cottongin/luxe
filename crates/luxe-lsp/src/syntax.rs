//! Tree-sitter offline outlines.
//!
//! A fast, server-free path to a file's symbol outline (functions, types, methods, fields, …) for
//! the discovery tools and the BM25 symbol index. Works without a running language server and
//! produces the same normalized [`FlatSymbol`] shape as the LSP `documentSymbol` path, so
//! `module_report`/`read_symbol` prefer it and fall back to LSP only for unsupported languages.

use crate::protocol::{symbol_kinds as kinds, FlatSymbol};
use std::path::Path;
use std::sync::OnceLock;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, Tree};

/// A language this module can outline offline.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lang {
    Rust,
    Python,
    TypeScript,
    Tsx,
    JavaScript,
    Go,
}

/// Definition node kinds to capture, per language (`@def` = full extent, `@name` = the name).
const RUST_QUERY: &str = r#"
(function_item name: (identifier) @name) @def
(struct_item name: (type_identifier) @name) @def
(enum_item name: (type_identifier) @name) @def
(union_item name: (type_identifier) @name) @def
(trait_item name: (type_identifier) @name) @def
(mod_item name: (identifier) @name) @def
(type_item name: (type_identifier) @name) @def
(const_item name: (identifier) @name) @def
(static_item name: (identifier) @name) @def
(macro_definition name: (identifier) @name) @def
(impl_item type: (type_identifier) @name) @def
(field_declaration name: (field_identifier) @name) @def
(enum_variant name: (identifier) @name) @def
"#;

const PYTHON_QUERY: &str = r#"
(function_definition name: (identifier) @name) @def
(class_definition name: (identifier) @name) @def
"#;

const TS_QUERY: &str = r#"
(function_declaration name: (identifier) @name) @def
(class_declaration name: (type_identifier) @name) @def
(method_definition name: (property_identifier) @name) @def
(public_field_definition name: (property_identifier) @name) @def
(interface_declaration name: (type_identifier) @name) @def
(type_alias_declaration name: (type_identifier) @name) @def
(enum_declaration name: (identifier) @name) @def
"#;

const JS_QUERY: &str = r#"
(function_declaration name: (identifier) @name) @def
(class_declaration name: (identifier) @name) @def
(method_definition name: (property_identifier) @name) @def
"#;

const GO_QUERY: &str = r#"
(function_declaration name: (identifier) @name) @def
(method_declaration name: (field_identifier) @name) @def
(type_spec name: (type_identifier) @name) @def
(field_declaration name: (field_identifier) @name) @def
"#;

/// Whether tree-sitter can outline this path's language.
pub fn supported(path: &Path) -> bool {
    detect(path).is_some()
}

pub(crate) fn detect(path: &Path) -> Option<Lang> {
    match path.extension()?.to_str()? {
        "rs" => Some(Lang::Rust),
        "py" | "pyi" => Some(Lang::Python),
        "ts" | "mts" | "cts" => Some(Lang::TypeScript),
        "tsx" => Some(Lang::Tsx),
        "js" | "jsx" | "mjs" | "cjs" => Some(Lang::JavaScript),
        "go" => Some(Lang::Go),
        _ => None,
    }
}

/// The tree-sitter `Language` for a detected `Lang` — for callers (e.g. the graph extractor) that
/// run their own queries against the same grammars.
pub(crate) fn ts_language(lang: Lang) -> Language {
    language_and_query(lang).0
}

fn language_and_query(lang: Lang) -> (Language, &'static str) {
    match lang {
        Lang::Rust => (tree_sitter_rust::LANGUAGE.into(), RUST_QUERY),
        Lang::Python => (tree_sitter_python::LANGUAGE.into(), PYTHON_QUERY),
        Lang::TypeScript => (tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), TS_QUERY),
        Lang::Tsx => (tree_sitter_typescript::LANGUAGE_TSX.into(), TS_QUERY),
        Lang::JavaScript => (tree_sitter_javascript::LANGUAGE.into(), JS_QUERY),
        Lang::Go => (tree_sitter_go::LANGUAGE.into(), GO_QUERY),
    }
}

/// Cached compiled outline query for `lang`. Query compilation dominates a whole-project walk, so
/// it's built once per language and reused for every file (the same trick the graph extractor uses).
fn outline_query(lang: Lang) -> Option<&'static Query> {
    fn compile(lang: Lang) -> Option<Query> {
        let (language, src) = language_and_query(lang);
        Query::new(&language, src).ok()
    }
    match lang {
        Lang::Rust => {
            static Q: OnceLock<Option<Query>> = OnceLock::new();
            Q.get_or_init(|| compile(Lang::Rust)).as_ref()
        }
        Lang::Python => {
            static Q: OnceLock<Option<Query>> = OnceLock::new();
            Q.get_or_init(|| compile(Lang::Python)).as_ref()
        }
        Lang::TypeScript => {
            static Q: OnceLock<Option<Query>> = OnceLock::new();
            Q.get_or_init(|| compile(Lang::TypeScript)).as_ref()
        }
        Lang::Tsx => {
            static Q: OnceLock<Option<Query>> = OnceLock::new();
            Q.get_or_init(|| compile(Lang::Tsx)).as_ref()
        }
        Lang::JavaScript => {
            static Q: OnceLock<Option<Query>> = OnceLock::new();
            Q.get_or_init(|| compile(Lang::JavaScript)).as_ref()
        }
        Lang::Go => {
            static Q: OnceLock<Option<Query>> = OnceLock::new();
            Q.get_or_init(|| compile(Lang::Go)).as_ref()
        }
    }
}

/// Produce a symbol outline for `source`, or `None` for an unsupported language / parse failure.
pub fn outline(path: &Path, source: &str) -> Option<Vec<FlatSymbol>> {
    let lang = detect(path)?;
    let language = ts_language(lang);
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(source, None)?;
    Some(outline_from_tree(lang, &tree, source.as_bytes()))
}

/// Build the symbol outline from an already-parsed tree, so a caller that also runs its own queries
/// (the graph extractor) can reuse a single parse. Uses the cached outline query.
pub(crate) fn outline_from_tree(lang: Lang, tree: &Tree, bytes: &[u8]) -> Vec<FlatSymbol> {
    let Some(query) = outline_query(lang) else { return Vec::new() };
    let Some(def_index) = query.capture_index_for_name("def") else { return Vec::new() };
    let Some(name_index) = query.capture_index_for_name("name") else { return Vec::new() };

    let mut defs: Vec<(Node, Node)> = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        let mut def = None;
        let mut name = None;
        for capture in m.captures {
            if capture.index == def_index {
                def = Some(capture.node);
            } else if capture.index == name_index {
                name = Some(capture.node);
            }
        }
        if let (Some(def), Some(name)) = (def, name) {
            defs.push((def, name));
        }
    }

    let mut symbols: Vec<FlatSymbol> = defs
        .iter()
        .filter_map(|(def, name)| {
            let name_text = name.utf8_text(bytes).ok()?.to_string();
            let container_def = innermost_container(&defs, def);
            let container = container_def
                .and_then(|(_, cname)| cname.utf8_text(bytes).ok())
                .map(str::to_string);
            let kind = symbol_kind(def.kind(), container_def.map(|(d, _)| d.kind()));
            let name_pos = name.start_position();
            Some(FlatSymbol {
                name: name_text,
                kind,
                uri: String::new(),
                line: name_pos.row as u32,
                character: name_pos.column as u32,
                start_line: def.start_position().row as u32,
                end_line: def.end_position().row as u32,
                container,
            })
        })
        .collect();
    symbols.sort_by_key(|s| (s.start_line, s.line));
    symbols
}

/// The innermost other definition that strictly encloses `def` (its container/parent).
fn innermost_container<'a>(
    defs: &'a [(Node, Node)],
    def: &Node,
) -> Option<&'a (Node<'a>, Node<'a>)> {
    defs.iter()
        .filter(|(other, _)| other.id() != def.id())
        .filter(|(other, _)| {
            other.start_byte() <= def.start_byte()
                && other.end_byte() >= def.end_byte()
                && (other.start_byte() < def.start_byte() || other.end_byte() > def.end_byte())
        })
        .min_by_key(|(other, _)| other.end_byte() - other.start_byte())
}

/// Map a tree-sitter node kind (and its container's kind) to an LSP `SymbolKind` integer.
fn symbol_kind(def_kind: &str, container_kind: Option<&str>) -> u32 {
    let nested_in_type = matches!(
        container_kind,
        Some(
            "impl_item"
                | "trait_item"
                | "class_definition"
                | "class_declaration"
                | "interface_declaration"
        )
    );
    match def_kind {
        "method_definition" | "method_declaration" => kinds::METHOD,
        "function_item" | "function_definition" | "function_declaration" => {
            if nested_in_type {
                kinds::METHOD
            } else {
                kinds::FUNCTION
            }
        }
        "struct_item" | "union_item" => kinds::STRUCT,
        "enum_item" | "enum_declaration" => kinds::ENUM,
        "trait_item" | "interface_declaration" => kinds::INTERFACE,
        "impl_item" | "class_definition" | "class_declaration" => kinds::CLASS,
        "mod_item" => kinds::MODULE,
        "type_item" | "type_alias_declaration" | "type_spec" => kinds::CLASS,
        "const_item" | "static_item" => kinds::CONSTANT,
        "macro_definition" => kinds::FUNCTION,
        "field_declaration" | "public_field_definition" => kinds::FIELD,
        "enum_variant" => kinds::ENUM_MEMBER,
        _ => 0,
    }
}

/// Function lengths for the size standards the edit hook holds, from the same outline
/// `module_report` draws.
pub struct SyntaxOutline;

impl luxe_core::hooks::file_changed::OutlineSource for SyntaxOutline {
    fn functions(
        &self,
        path: &Path,
        source: &str,
    ) -> Option<Vec<luxe_core::hooks::file_changed::FunctionSize>> {
        let symbols = outline(path, source)?;
        Some(
            symbols
                .into_iter()
                .filter(|symbol| matches!(symbol.kind, kinds::FUNCTION | kinds::METHOD))
                .map(|symbol| luxe_core::hooks::file_changed::FunctionSize {
                    name: match &symbol.container {
                        Some(container) => format!("{container}.{}", symbol.name),
                        None => symbol.name.clone(),
                    },
                    lines: (symbol.end_line - symbol.start_line + 1) as usize,
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(out: &[FlatSymbol]) -> Vec<&str> {
        out.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn rust_outline_captures_defs_fields_methods_and_nesting() {
        let src = "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\npub struct Point {\n    x: i32,\n}\n\nimpl Point {\n    pub fn origin() -> Self {\n        Point { x: 0 }\n    }\n}\n";
        let out = outline(Path::new("lib.rs"), src).expect("rust outline");
        assert!(names(&out).contains(&"add"));
        assert!(names(&out).contains(&"Point"));
        assert!(names(&out).contains(&"origin"));

        let add = out.iter().find(|s| s.name == "add").unwrap();
        assert_eq!(add.kind, kinds::FUNCTION);
        assert_eq!((add.start_line, add.end_line), (0, 2));

        let field = out.iter().find(|s| s.name == "x").unwrap();
        assert_eq!(field.kind, kinds::FIELD);
        assert_eq!(field.container.as_deref(), Some("Point"));

        let origin = out.iter().find(|s| s.name == "origin").unwrap();
        assert_eq!(origin.kind, kinds::METHOD);
        assert_eq!(origin.container.as_deref(), Some("Point"));
    }

    #[test]
    fn python_outline_marks_methods_inside_classes() {
        let src =
            "def top():\n    pass\n\nclass Widget:\n    def render(self):\n        return 1\n";
        let out = outline(Path::new("app.py"), src).expect("python outline");
        assert_eq!(out.iter().find(|s| s.name == "top").unwrap().kind, kinds::FUNCTION);
        assert_eq!(out.iter().find(|s| s.name == "Widget").unwrap().kind, kinds::CLASS);
        let render = out.iter().find(|s| s.name == "render").unwrap();
        assert_eq!(render.kind, kinds::METHOD);
        assert_eq!(render.container.as_deref(), Some("Widget"));
    }

    #[test]
    fn typescript_outline_captures_classes_interfaces_and_methods() {
        let src = "export function greet(n: string): string { return n; }\n\ninterface Named { name: string; }\n\nexport class Service {\n  url: string = \"\";\n  fetch(): number { return 1; }\n}\n";
        let out = outline(Path::new("svc.ts"), src).expect("ts outline");
        assert_eq!(out.iter().find(|s| s.name == "greet").unwrap().kind, kinds::FUNCTION);
        assert_eq!(out.iter().find(|s| s.name == "Named").unwrap().kind, kinds::INTERFACE);
        assert_eq!(out.iter().find(|s| s.name == "Service").unwrap().kind, kinds::CLASS);
        let method = out.iter().find(|s| s.name == "fetch").unwrap();
        assert_eq!(method.kind, kinds::METHOD);
        assert_eq!(method.container.as_deref(), Some("Service"));
    }

    #[test]
    fn go_outline_captures_functions_types_and_methods() {
        let src = "package main\n\nfunc Add(a, b int) int { return a + b }\n\ntype Point struct {\n\tX int\n}\n\nfunc (p Point) Norm() int { return p.X }\n";
        let out = outline(Path::new("main.go"), src).expect("go outline");
        assert_eq!(out.iter().find(|s| s.name == "Add").unwrap().kind, kinds::FUNCTION);
        assert!(names(&out).contains(&"Point"));
        let method = out.iter().find(|s| s.name == "Norm").unwrap();
        assert_eq!(method.kind, kinds::METHOD);
    }

    #[test]
    fn unsupported_language_returns_none() {
        assert!(outline(Path::new("notes.md"), "# hi").is_none());
        assert!(!supported(Path::new("notes.md")));
        assert!(supported(Path::new("x.rs")));
        assert!(supported(Path::new("x.ts")));
        assert!(supported(Path::new("x.go")));
    }

    /// A method is named by its class, so two `__init__`s are two functions, and its length counts
    /// its first and last lines.
    #[test]
    fn the_size_outline_names_methods_by_their_class() {
        use luxe_core::hooks::file_changed::OutlineSource;
        let source = "class A:\n    def __init__(self):\n        x = 1\n        y = 2\n\ndef top():\n    pass\n";
        let functions = SyntaxOutline.functions(Path::new("m.py"), source).expect("python parses");
        let named: Vec<(&str, usize)> =
            functions.iter().map(|function| (function.name.as_str(), function.lines)).collect();
        assert_eq!(named, [("A.__init__", 3), ("top", 2)]);
        assert!(SyntaxOutline.functions(Path::new("notes.txt"), "x").is_none());
    }
}
