use super::*;
use crate::lsp::{Diagnostic, Severity};

fn diag(severity: Severity, line: u32, message: &str) -> Diagnostic {
    Diagnostic { severity: Some(severity), line, character: 0, message: message.to_string() }
}

#[test]
fn format_orders_errors_first_and_labels_severity() {
    let diags = vec![
        diag(Severity::Warning, 10, "a warning"),
        diag(Severity::Error, 20, "an error"),
        diag(Severity::Hint, 5, "a hint"),
        diag(Severity::Information, 7, "an info"),
    ];
    let out = format_diagnostics("src/x.rs", &diags);
    let error_pos = out.find("an error").unwrap();
    let warn_pos = out.find("a warning").unwrap();
    let info_pos = out.find("an info").unwrap();
    let hint_pos = out.find("a hint").unwrap();
    assert!(
        error_pos < warn_pos && warn_pos < info_pos && info_pos < hint_pos,
        "errors first, then warning, info, hint"
    );
    assert!(out.contains("info 8:1: an info"), "{out}");
    assert!(out.contains("error 21:1: an error"));
    assert!(out.contains("1 error(s)"));
}

/// A server that does not say how serious something is gets a neutral label and the last place.
#[test]
fn a_diagnostic_with_no_severity_is_a_note_and_sorts_last() {
    let unranked =
        Diagnostic { severity: None, line: 0, character: 0, message: "unranked".to_string() };
    let diags = vec![unranked, diag(Severity::Hint, 5, "a hint")];

    let out = format_diagnostics("src/x.rs", &diags);

    assert!(out.find("a hint").unwrap() < out.find("unranked").unwrap(), "{out}");
    assert!(out.contains("note 1:1: unranked"), "{out}");
}

#[test]
fn format_caps_and_summarizes_overflow() {
    let diags: Vec<Diagnostic> =
        (0..30).map(|i| diag(Severity::Warning, i, &format!("w{i}"))).collect();
    let out = format_diagnostics("src/x.rs", &diags);
    let shown = out.lines().filter(|l| l.starts_with("- ")).count();
    assert_eq!(shown, MAX_INJECTED_DIAGNOSTICS, "caps the inline list");
    assert!(out.contains("and 10 more"), "summarizes the remainder: {out}");
}

mod baseline;
mod fetched;
mod size;
mod test_gap;
