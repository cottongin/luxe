use super::read_symbol::{enclosing_context, pick_symbol};
use super::symbol_search::{rank, tighten, MAX_LOOSE_SYMBOLS};
use super::*;

fn sym(name: &str) -> FlatSymbol {
    FlatSymbol {
        name: name.to_string(),
        kind: 12,
        uri: String::new(),
        line: 0,
        character: 0,
        start_line: 0,
        end_line: 0,
        container: None,
    }
}

/// A name that matched is the answer: its lookalikes stay out, and looser matches are returned,
/// capped, only when nothing matched the name.
#[test]
fn a_named_match_leaves_the_lookalikes_out() {
    let symbol = |name: &str| crate::symbol_index::IndexedSymbol {
        name: name.into(),
        kind: 6,
        path: "a.rs".into(),
        line: 1,
        container: None,
    };
    let pool: Vec<_> = ["set_cancelled", "set_cancelled_flag", "set", "Cancelled", "cancel_tx"]
        .into_iter()
        .map(symbol)
        .collect();
    let names = |hits: Vec<&crate::symbol_index::IndexedSymbol>| -> Vec<String> {
        hits.into_iter().map(|hit| hit.name.clone()).collect()
    };

    let (kept, dropped) = tighten(pool.iter().collect(), "set_cancelled");
    assert_eq!(names(kept), ["set_cancelled", "set_cancelled_flag"]);
    assert!(dropped);

    let (kept, dropped) = tighten(pool.iter().collect(), "SET_CANCELLED");
    assert_eq!(names(kept).len(), 2, "case does not matter");
    assert!(dropped);

    let (kept, dropped) = tighten(pool[..1].iter().collect(), "set_cancelled");
    assert_eq!(names(kept), ["set_cancelled"]);
    assert!(!dropped, "nothing left out, so nothing said");

    let many: Vec<_> = (0..30).map(|n| symbol(&format!("render_{n}"))).collect();
    let (kept, dropped) = tighten(many.iter().collect(), "footer");
    assert_eq!(kept.len(), MAX_LOOSE_SYMBOLS);
    assert!(dropped);
    let (kept, dropped) = tighten(many[..3].iter().collect(), "footer");
    assert_eq!(kept.len(), 3);
    assert!(!dropped);
}

#[test]
fn rank_orders_exact_prefix_substring() {
    assert_eq!(rank("foo", "foo"), 0);
    assert_eq!(rank("foobar", "foo"), 1);
    assert_eq!(rank("do_foo", "foo"), 2);
    assert_eq!(rank("bar", "foo"), 3);
}

#[test]
fn pick_prefers_exact_then_substring() {
    let symbols = [sym("compute_total"), sym("total"), sym("subtotal")];
    assert_eq!(pick_symbol(&symbols, "total").unwrap().name, "total");
    assert_eq!(pick_symbol(&symbols, "compute").unwrap().name, "compute_total");
    assert!(pick_symbol(&symbols, "missing").is_none());
}

/// A typo'd path must say "not found", not "no symbols" — the latter reads like an empty file.
#[tokio::test]
async fn a_missing_file_is_reported_as_missing() {
    let service = std::sync::Arc::new(crate::workspace::WorkspaceLspClient::new());
    let (event_tx, _event_rx) = tokio::sync::mpsc::channel(4);
    let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let cx = ToolContext {
        cwd: std::env::temp_dir(),
        session_id: "test-session".to_string(),
        cancelled: std::sync::Arc::new(cancel_rx),
        event_tx,
        hook_ctx: None,
        question_broker: None,
    };
    let Err(error) = build_outline(&service, &cx, "definitely/not/here.rs", false).await else {
        panic!("a missing path must be an error");
    };
    assert!(error.contains("file not found"), "{error}");

    let Err(directory) = build_outline(&service, &cx, ".", false).await else {
        panic!("a directory must be an error");
    };
    assert!(directory.contains("is a directory"), "{directory}");
}

fn scoped(name: &str, container: Option<&str>, start: u32, end: u32) -> FlatSymbol {
    FlatSymbol {
        name: name.to_string(),
        kind: 6,
        uri: String::new(),
        line: start,
        character: 0,
        start_line: start,
        end_line: end,
        container: container.map(str::to_string),
    }
}

/// The measured failure: a five-line method body that answers nothing on its own because the
/// field it returns from is assigned in `__init__`.
#[test]
fn a_method_body_carries_the_fields_it_depends_on() {
    let source = vec![
        "class GoogleAuthService:",
        "    def __init__(self, session):",
        "        self.connection_repo = GoogleConnectionRepository(session)",
        "        self.unused = Other()",
        "",
        "    async def get_connection(self, *, user_id: int):",
        "        return await self.connection_repo.get_by_user_id(user_id)",
    ];
    let symbols = vec![
        scoped("GoogleAuthService", None, 0, 6),
        scoped("get_connection", Some("GoogleAuthService"), 5, 6),
    ];
    let body = source[5..=6].join("\n");

    let context = enclosing_context(&symbols[1], &symbols, &source, &body);

    assert!(context.contains("GoogleAuthService"), "names the class: {context}");
    assert!(
        context.contains("GoogleConnectionRepository"),
        "and the field's real type, which is the actual answer: {context}"
    );
    assert!(!context.contains("self.unused"), "only fields the body uses: {context}");
}

#[test]
fn a_free_function_gets_no_class_context() {
    let source = vec!["def helper():", "    return 1"];
    let symbols = vec![scoped("helper", None, 0, 1)];
    assert_eq!(enclosing_context(&symbols[0], &symbols, &source, "return 1"), "");
}

#[test]
fn a_method_touching_no_fields_just_names_its_class() {
    let source = vec!["class C:", "    def f(self):", "        return 1"];
    let symbols = vec![scoped("C", None, 0, 2), scoped("f", Some("C"), 1, 2)];
    let context = enclosing_context(&symbols[1], &symbols, &source, "return 1");
    assert_eq!(context, "\n\n(defined on `C`)");
}
