//! The read-only turn's rule, and what a refusal says.

use super::ReadOnlyTurn::{Answering, Drafting};
use super::*;
use serde_json::json;

/// Reading is allowed and changing is not, whatever the tool: a write, a shell, a commit, a
/// post, a rename applied — and a tool nobody has decided about yet.
#[test]
fn a_read_only_turn_reads_and_changes_nothing() {
    for (name, args) in [
        ("read", json!({ "path": "a" })),
        ("grep", json!({ "pattern": "x" })),
        ("git", json!({ "action": "log" })),
        ("git", json!({ "action": "diff" })),
        ("git", json!({ "action": "show", "args": ["4c1e2a0:src/router.rs"] })),
        ("git", json!({ "action": "pr", "args": ["view", "142"] })),
        ("lsp_navigation", json!({ "operation": "definition" })),
        ("ast_grep", json!({ "pattern": "x" })),
    ] {
        assert!(allows(name, &args), "{name} {args}");
    }
    for (name, args) in [
        ("write", json!({ "path": "a" })),
        ("edit", json!({})),
        ("bash", json!({ "command": "ls" })),
        ("test", json!({})),
        ("todo", json!({})),
        ("git", json!({ "action": "commit" })),
        ("git", json!({ "action": "diff", "args": ["--output=notes.txt"] })),
        ("git", json!({ "action": "log", "args": ["-p", "--output", "notes.txt"] })),
        ("git", json!({ "action": "show", "args": ["--output=x", "HEAD"] })),
        ("git", json!({ "action": "push" })),
        ("git", json!({ "action": "gh", "args": ["pr", "comment"] })),
        ("git", json!({ "action": "pr", "args": ["comment", "142"] })),
        ("git", json!({ "action": "pr", "args": ["merge", "142"] })),
        ("lsp_navigation", json!({ "operation": "rename", "apply": true })),
        ("ast_grep", json!({ "rewrite": "y", "apply": true })),
        ("mcp", json!({})),
        ("a_tool_added_later", json!({})),
    ] {
        assert!(!allows(name, &args), "{name} {args}");
    }
}

/// A refused shell command is told the tool that reads the way it tried to, past a `cd` prefix and
/// an environment assignment; anything else is told what the turn allows. Each turn says what it is
/// for and how to finish.
#[test]
fn a_refused_shell_command_is_told_which_tool_reads_that_way() {
    let refused = |command: &str| refusal(Drafting, "bash", &json!({ "command": command }));
    assert!(refused("cat src/router.rs").contains("use `read`"));
    assert!(refused("cd /repo && sed -n '1,40p' src/router.rs").contains("use `read`"));
    assert!(refused("ls -la src").contains("use `glob`"));
    assert!(refused("LC_ALL=C rg fallback").contains("use `grep`"));
    assert!(refused("git log --oneline -5").contains("`git` tool's `log`"));
    assert!(refused("gh pr view 142").contains("`git` tool's `pr view`"));
    let other = refused("cargo test");
    assert!(other.contains("Read with `read`, `grep` and `glob`"), "{other}");
    assert!(
        other.starts_with("This turn drafts a reply and changes nothing, so `bash` did not run.")
    );
    assert!(other.ends_with("Then answer with the draft."));
    assert!(refusal(Drafting, "write", &json!({})).contains("so `write` did not run. Read with"));

    let answering = refusal(Answering, "edit", &json!({}));
    assert!(answering.starts_with("This turn answers the reviewer's questions and changes nothing"));
    assert!(answering.contains("the reviewer decides whether to ask for it"), "{answering}");
}

/// A plan file that is a link carries no write past the mode.
///
/// The break this catches: matching the *spelling* `PLAN.md` against the plan globs, which would
/// let a repository whose `PLAN.md` links to `src/main.rs` have plan mode rewrite its source.
#[cfg(unix)]
#[test]
fn plan_mode_will_not_write_through_a_link() {
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join("src")).unwrap();
    std::fs::write(project.path().join("src/main.rs"), "fn main() {}").unwrap();
    std::os::unix::fs::symlink("src/main.rs", project.path().join("PLAN.md")).unwrap();
    let plan = crate::mode::Mode::plan(Vec::new());

    let verdict = judge_for_mode(&plan, "write", &json!({ "path": "PLAN.md" }), project.path());
    assert!(matches!(verdict, ModeVerdict::Refused(_)), "{verdict:?}");
}

/// Plan mode reads, keeps its own list, and writes the plan; it runs nothing.
#[test]
fn plan_mode_reads_and_writes_only_its_plan() {
    let project = tempfile::tempdir().unwrap();
    let plan = crate::mode::Mode::plan(vec!["docs/design/*.md".into()]);
    let judge =
        |name: &str, args: serde_json::Value| judge_for_mode(&plan, name, &args, project.path());

    assert_eq!(judge("read", json!({ "path": "src/a.rs" })), ModeVerdict::Runs);
    assert_eq!(judge("todo", json!({ "action": "add" })), ModeVerdict::Runs);
    assert_eq!(
        judge("write", json!({ "path": "docs/design/rates.md" })),
        ModeVerdict::WritesPlan("docs/design/rates.md".into())
    );
    for (name, args) in [
        ("write", json!({ "path": "docs/design/deeper/rates.md" })),
        ("write", json!({ "path": "PLAN.md" })),
        ("edit", json!({ "filePath": "src/a.rs" })),
        ("edit", json!({ "content": "no path at all" })),
        ("bash", json!({ "command": "ls" })),
        ("test", json!({})),
        ("git", json!({ "action": "commit" })),
    ] {
        assert!(
            matches!(judge(name, args.clone()), ModeVerdict::Refused(_)),
            "{name} {args} should be refused"
        );
    }
}

/// A review engine's mode writes nothing at all, plan files included.
#[test]
fn a_mode_that_writes_nothing_refuses_every_write() {
    let project = tempfile::tempdir().unwrap();
    let review = crate::mode::Mode::read_only("review");
    let verdict = judge_for_mode(&review, "write", &json!({ "path": "PLAN.md" }), project.path());
    assert!(
        matches!(&verdict, ModeVerdict::Refused(text) if text.contains("changes nothing")),
        "{verdict:?}"
    );
    assert_eq!(judge_for_mode(&review, "grep", &json!({}), project.path()), ModeVerdict::Runs);
}
