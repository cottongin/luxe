use super::*;

fn extract(path: &str, source: &str) -> FileGraph {
    extract_file(path, source).expect("supported language")
}

/// Edges of `kind`, as `(from, target-string, resolved?)` — stable, comparable tuples.
fn edges_of(fg: &FileGraph, kind: EdgeKind) -> Vec<(String, String, bool)> {
    fg.edges
        .iter()
        .filter(|e| e.kind == kind)
        .map(|e| {
            (e.from.as_str().to_string(), e.to.as_str().to_string(), e.to.resolved().is_some())
        })
        .collect()
}

fn node_names(fg: &FileGraph) -> Vec<String> {
    fg.nodes.iter().map(|n| n.name.as_str().to_string()).collect()
}

#[test]
fn rust_nodes_defines_imports_and_impls() {
    let src = "use crate::auth::AuthService;\n\
                   pub struct Point { x: i32 }\n\
                   impl Point { pub fn origin() -> Self { Point { x: 0 } } }\n\
                   impl Display for Point { fn fmt(&self) {} }\n";
    let fg = extract("src/geo.rs", src);

    // File node + symbols present.
    assert!(node_names(&fg).contains(&"src/geo.rs".to_string()));
    assert!(node_names(&fg).contains(&"src/geo.rs::Point".to_string()));
    assert!(node_names(&fg).contains(&"src/geo.rs::Point.origin".to_string()));
    assert!(node_names(&fg).contains(&"src/geo.rs::Point.x".to_string()));

    // Defines: File → Point (top-level), Point → origin / x (members).
    let defines = edges_of(&fg, EdgeKind::Defines);
    assert!(defines.contains(&("src/geo.rs".into(), "src/geo.rs::Point".into(), true)));
    assert!(defines.contains(&(
        "src/geo.rs::Point".into(),
        "src/geo.rs::Point.origin".into(),
        true
    )));
    assert!(defines.contains(&("src/geo.rs::Point".into(), "src/geo.rs::Point.x".into(), true)));

    // Imports: unresolved raw module reference.
    let imports = edges_of(&fg, EdgeKind::Imports);
    assert_eq!(imports, vec![("src/geo.rs".into(), "crate::auth::AuthService".into(), false)]);

    // Implements: Point implements Display — Display isn't defined here, so unresolved.
    let implements = edges_of(&fg, EdgeKind::Implements);
    assert_eq!(implements, vec![("src/geo.rs::Point".into(), "Display".into(), false)]);
}

#[test]
fn rust_same_file_impl_target_resolves() {
    let src = "trait Greet {}\nstruct Bot;\nimpl Greet for Bot {}\n";
    let fg = extract("src/a.rs", src);
    let implements = edges_of(&fg, EdgeKind::Implements);
    // Greet is defined in this file → the edge resolves to its node.
    assert_eq!(implements, vec![("src/a.rs::Bot".into(), "src/a.rs::Greet".into(), true)]);
}

/// In Rust the calls that prove a test exercises something are almost all inside an assertion
/// macro, whose arguments parse as an opaque token tree. Missing them meant a tested function
/// had no `TestCovers` edge and the review told the reviewer it was untested.
#[test]
fn rust_calls_inside_macros_are_attributed_like_any_other() {
    let src = "fn t() {\n    app.tick_spinner();\n    assert_eq!(app.spinner_char(), 'x');\n    \
                   assert!(helper(&build_line(&app, 80)).contains(\"y\"));\n}\n";
    let fg = extract("src/c.rs", src);
    let calls = edges_of(&fg, EdgeKind::Calls);
    for callee in ["tick_spinner", "spinner_char", "helper", "build_line", "contains"] {
        assert!(
            calls.contains(&("src/c.rs::t".into(), callee.into(), false)),
            "{callee} is a call site: {calls:?}"
        );
    }
}

#[test]
fn rust_calls_attributed_to_enclosing_function_unresolved() {
    let src = "fn helper() {}\nfn caller() { helper(); missing(); }\n";
    let fg = extract("src/c.rs", src);
    let calls = edges_of(&fg, EdgeKind::Calls);
    // Both calls attributed to `caller`; unresolved at extract time (resolve.rs resolves).
    assert!(calls.contains(&("src/c.rs::caller".into(), "helper".into(), false)));
    assert!(calls.contains(&("src/c.rs::caller".into(), "missing".into(), false)));
    // A call site with no enclosing function is dropped (nothing to attribute it to).
    assert!(calls.iter().all(|(from, _, _)| from == "src/c.rs::caller"));
}

#[test]
fn python_inherits_and_imports() {
    let src = "import os\n\
                   from a.b import c\n\
                   class Base: pass\n\
                   class Widget(Base, Mixin):\n    def render(self):\n        return 1\n";
    let fg = extract("app.py", src);

    let imports: Vec<String> = edges_of(&fg, EdgeKind::Imports).into_iter().map(|e| e.1).collect();
    assert!(imports.contains(&"os".to_string()));
    assert!(imports.contains(&"a.b".to_string()));

    let inherits = edges_of(&fg, EdgeKind::Inherits);
    // Base is defined here (resolved); Mixin is not (unresolved).
    assert!(inherits.contains(&("app.py::Widget".into(), "app.py::Base".into(), true)));
    assert!(inherits.contains(&("app.py::Widget".into(), "Mixin".into(), false)));

    // A method inside a class is defined by that class, and Defines wires File → class.
    let defines = edges_of(&fg, EdgeKind::Defines);
    assert!(defines.contains(&("app.py".into(), "app.py::Widget".into(), true)));
    assert!(defines.contains(&("app.py::Widget".into(), "app.py::Widget.render".into(), true)));
}

#[test]
fn typescript_extends_implements_and_imports() {
    let src = "import { A } from \"./mod\";\n\
                   interface Named { name: string; }\n\
                   class Service extends Base implements Named, Other {\n  fetch(): number { return 1; }\n}\n";
    let fg = extract("svc.ts", src);

    let imports: Vec<String> = edges_of(&fg, EdgeKind::Imports).into_iter().map(|e| e.1).collect();
    assert_eq!(imports, vec!["./mod".to_string()]);

    let inherits = edges_of(&fg, EdgeKind::Inherits);
    assert_eq!(inherits, vec![("svc.ts::Service".into(), "Base".into(), false)]);

    let implements = edges_of(&fg, EdgeKind::Implements);
    // Named is defined here (interface) → resolved; Other is not.
    assert!(implements.contains(&("svc.ts::Service".into(), "svc.ts::Named".into(), true)));
    assert!(implements.contains(&("svc.ts::Service".into(), "Other".into(), false)));
}

#[test]
fn javascript_extends_and_imports() {
    let src =
        "import { A } from \"./mod\";\nclass Service extends Base { fetch() { return 1; } }\n";
    let fg = extract("svc.js", src);
    assert_eq!(edges_of(&fg, EdgeKind::Imports), vec![("svc.js".into(), "./mod".into(), false)]);
    assert_eq!(
        edges_of(&fg, EdgeKind::Inherits),
        vec![("svc.js::Service".into(), "Base".into(), false)]
    );
}

#[test]
fn go_imports_and_calls_extracted() {
    let src = "package main\nimport (\n  \"fmt\"\n  m \"example.com/mod\"\n)\n\
                   func Add(a, b int) int { return a + b }\n\
                   func Run() { Add(1, 2) }\n";
    let fg = extract("main.go", src);
    let imports: Vec<String> = edges_of(&fg, EdgeKind::Imports).into_iter().map(|e| e.1).collect();
    assert!(imports.contains(&"fmt".to_string()));
    assert!(imports.contains(&"example.com/mod".to_string()));

    let calls = edges_of(&fg, EdgeKind::Calls);
    assert!(calls.contains(&("main.go::Run".into(), "Add".into(), false)));
}

#[test]
fn test_functions_are_flagged() {
    let src = "fn helper() {}\n#[cfg(test)]\nmod tests {\n    fn works() {}\n    fn test_direct() {}\n}\n";
    let fg = extract("src/x.rs", src);
    let is_test = |name: &str| fg.nodes.iter().find(|n| n.name.as_str() == name).unwrap().is_test;
    assert!(!is_test("src/x.rs::helper"));
    // `works` lives in `mod tests` → flagged by container.
    assert!(is_test("src/x.rs::tests.works"));
    // `test_direct` flagged by name too.
    assert!(is_test("src/x.rs::tests.test_direct"));
}

#[test]
fn extraction_is_deterministic() {
    let src = "use a::b;\nstruct S;\nimpl T for S {}\nfn f() { g(); }\nfn g() {}\n";
    let a = extract("d.rs", src);
    let b = extract("d.rs", src);
    assert_eq!(node_names(&a), node_names(&b));
    assert_eq!(a.edges, b.edges);
}

#[test]
fn unsupported_language_is_none() {
    assert!(extract_file("notes.md", "# hi").is_none());
}

/// The measured miss: a renamed import broke the call edge, and the symbol's blast radius
/// came back one caller short without saying so.
#[test]
fn a_renamed_import_still_links_the_call() {
    let fg = extract(
        "services/agent_service.py",
        "from utils.credit_helpers import calculate_credits as _calc_credits\n\
             def charge(usage):\n\
             \x20   return _calc_credits(usage)\n",
    );
    let calls = edges_of(&fg, EdgeKind::Calls);
    assert!(
        calls.iter().any(|c| c.1.contains("calculate_credits")),
        "the alias resolves to the real name: {calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c.1.contains("_calc_credits")),
        "and not to the local alias: {calls:?}"
    );
}

#[test]
fn an_unrenamed_call_is_unaffected() {
    let fg = extract(
        "services/plain.py",
        "from utils.credit_helpers import calculate_credits\n\
             def charge(usage):\n\
             \x20   return calculate_credits(usage)\n",
    );
    assert!(edges_of(&fg, EdgeKind::Calls).iter().any(|c| c.1.contains("calculate_credits")));
}
