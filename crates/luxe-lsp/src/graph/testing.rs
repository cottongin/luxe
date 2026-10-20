//! Testing — the one answer to "is this test code?".
//!
//! Two callers need it and they must not disagree: [`extract`](super::extract) marks a node
//! `is_test` (which is what derives `TestCovers`, and so what "covered" means), and
//! [`review`](super::review) refuses to ask a test file for a covering test. If they drift, a
//! layout like Rust's `mod tests;` in a sibling file reads every real test as production code: the
//! changed code is badged untested while its test sits one file away.
//!
//! The predicates stay deliberately conservative: an outline carries no attributes, so `#[test]`
//! is invisible here and layout is all there is to go on.

/// Whether a path is test code by convention, across the languages the graph extracts.
///
/// Paths are compared with forward slashes, so a Windows-shaped path answers the same.
pub(super) fn is_test_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let name = normalized.rsplit('/').next().unwrap_or(&normalized);
    normalized.starts_with("tests/")
        || normalized.starts_with("test/")
        || normalized.contains("/tests/")
        || normalized.contains("/test/")
        || normalized.contains("/__tests__/")
        || normalized.contains("/spec/")
        // Rust's `mod tests;` in a sibling file: the module *is* the file, so there is no `tests`
        // container for a symbol to sit in and no `test_` prefix on any of its functions.
        || name == "tests.rs"
        || name == "test.rs"
        || name.starts_with("test_")
        || name.ends_with("_test.rs")
        || name.ends_with("_tests.rs")
        || name.ends_with("_test.go")
        || name.ends_with("_test.py")
        || [".test.ts", ".test.tsx", ".test.js", ".spec.ts", ".spec.tsx", ".spec.js"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

/// Whether a function/method is test code, from its file, its container and its name.
///
/// `kind` is an LSP `SymbolKind`; anything that is not a function or method is never a test. The
/// file is consulted first because it is the only signal that survives a layout where neither the
/// container nor the name carries the word "test".
pub(super) fn is_test_symbol(
    kind: u32,
    name: &str,
    container: Option<&str>,
    rel_path: &str,
) -> bool {
    use crate::protocol::symbol_kinds as k;
    if kind != k::FUNCTION && kind != k::METHOD {
        return false;
    }
    if is_test_path(rel_path) {
        return true;
    }
    let container_is_test = container
        .is_some_and(|c| c.eq_ignore_ascii_case("tests") || c.eq_ignore_ascii_case("test"));
    let lower = name.to_ascii_lowercase();
    container_is_test
        || name.starts_with("test_")
        || name.starts_with("Test")
        || lower.ends_with("_test")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::symbol_kinds as k;

    #[test]
    fn test_paths_are_recognized_across_conventions() {
        for path in [
            "tests/auth_test.rs",
            // Each of these is carried by exactly one clause, so none of them can lean on
            // another to pass.
            "tests/integration.rs",
            "test/helpers.rs",
            "crates/x/tests/integration.rs",
            "src/parser_test.rs",
            "src/__tests__/button.tsx",
            "web/src/button.test.tsx",
            "api/handlers_test.go",
            "app/spec/models/user.rb",
        ] {
            assert!(is_test_path(path), "{path} is test code");
        }
        for path in ["src/auth.rs", "crates/luxe-core/src/latest.rs", "src/contest.rs"] {
            assert!(!is_test_path(path), "{path} is production code");
        }
    }

    /// The layout that broke it: `mod tests;` pointing at a sibling file. luxe lays five of its own
    /// modules out this way, so the miss was never exotic.
    #[test]
    fn a_sibling_tests_module_is_test_code() {
        for path in [
            "src/app/tests.rs",
            "crates/luxe-tui/src/guided_review/tests.rs",
            "src/test.rs",
            "src\\app\\tests.rs",
        ] {
            assert!(is_test_path(path), "{path} is test code");
        }
        // A file whose name merely ends in those letters is not.
        for path in ["src/latests.rs", "src/protest.rs"] {
            assert!(!is_test_path(path), "{path} is production code");
        }
    }

    /// A `#[test]` function in a sibling test module has no `tests` container and no `test_`
    /// prefix, so its file is the only thing left that can identify it.
    #[test]
    fn a_test_functions_file_identifies_it_when_nothing_else_does() {
        assert!(is_test_symbol(k::FUNCTION, "spinner_char_cycles", None, "src/app/tests.rs"));
        assert!(is_test_symbol(k::FUNCTION, "works", Some("tests"), "src/auth.rs"));
        assert!(is_test_symbol(k::FUNCTION, "test_login", None, "src/auth.rs"));
        assert!(is_test_symbol(k::FUNCTION, "TestLogin", None, "src/auth.rs"));
        // Go and Python spell it the other way round, and this is the only clause that catches it.
        assert!(is_test_symbol(k::FUNCTION, "login_test", None, "src/auth.rs"));
        assert!(!is_test_symbol(k::FUNCTION, "login", None, "src/auth.rs"));
        // Only functions and methods are ever tests, wherever they live.
        assert!(!is_test_symbol(k::STRUCT, "Fixture", None, "src/app/tests.rs"));
    }
}
