use super::*;

/// A variable wins, arguments and all; without one, the first terminal file manager
/// installed; without any, nothing, which sends the caller to the desktop.
#[test]
fn a_file_manager_is_named_found_or_left_to_the_desktop() {
    let named = |value: &'static str| {
        move |variable: &str| (variable == "FILE_MANAGER").then(|| value.to_string())
    };
    assert_eq!(
        super::file_manager_from(named("lf -single"), |_| true),
        Some(("lf".into(), vec!["-single".into()]))
    );
    let unset = |_: &str| None;
    assert_eq!(
        super::file_manager_from(unset, |program| program == "ranger"),
        Some(("ranger".into(), Vec::new())),
        "yazi and lf are tried first and are not here"
    );
    assert_eq!(super::file_manager_from(unset, |_| false), None);
    assert_eq!(
        super::file_manager_from(named(""), |program| program == "nnn"),
        Some(("nnn".into(), Vec::new())),
        "an empty variable is unset"
    );
}

/// Each family of editor gets the line the way it reads one, and an editor nobody knows gets
/// the file alone rather than an argument it might open as a second file.
#[test]
fn a_file_opens_at_its_line_in_the_editors_own_syntax() {
    let args = |program: &str, line: Option<u32>| -> Vec<String> {
        editor_file_args(program, Path::new("src/main.rs"), line)
            .into_iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    };
    assert_eq!(args("nvim", Some(42)), ["+42", "src/main.rs"]);
    assert_eq!(args("/usr/bin/vim", Some(42)), ["+42", "src/main.rs"]);
    assert_eq!(args("nano", Some(7)), ["+7", "src/main.rs"]);
    assert_eq!(args("hx", Some(42)), ["src/main.rs:42"]);
    assert_eq!(args("zed", Some(42)), ["src/main.rs:42"]);
    assert_eq!(args("code", Some(42)), ["--goto", "src/main.rs:42"]);
    assert_eq!(args("Cursor.exe", Some(3)), ["--goto", "src/main.rs:3"]);
    assert_eq!(args("notepad", Some(42)), ["src/main.rs"], "unknown: the file alone");
    assert_eq!(args("nvim", None), ["src/main.rs"]);
    assert_eq!(args("nvim", Some(0)), ["src/main.rs"], "there is no line zero");
}

#[test]
fn an_editor_command_carries_its_arguments_and_quoted_values() {
    assert_eq!(
        parse_editor("nvim --cmd 'set number'"),
        Some(("nvim".to_string(), vec!["--cmd".to_string(), "set number".to_string()]))
    );
    assert_eq!(parse_editor("code --wait"), Some(("code".into(), vec!["--wait".into()])));
    assert_eq!(parse_editor("nvim '"), None, "unbalanced quotes");
    assert_eq!(parse_editor("   "), None);
}
