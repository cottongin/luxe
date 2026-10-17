//! Fixture mentions — the `@` picker seeded on a tab, with project files and MCP servers.

use crate::app::App;
use luxe_core::WorkspaceSnapshot;

/// A snapshot naming four MCP servers: signed in, signed out, never discovered, and a local one.
fn seed_mcp_servers(app: &mut App) {
    use luxe_core::workspace::{ProjectIdentity, WorkspaceScopeId};
    let server = |id: &str, transport: &str, tools: Option<usize>, signed_in: Option<bool>| {
        luxe_core::McpServerStatus {
            id: id.into(),
            transport: transport.into(),
            auth: if signed_in.is_some() { "oauth" } else { "none" }.into(),
            tool_count: tools,
            enabled: true,
            tools: (0..tools.unwrap_or(0)).map(|n| format!("tool_{n}")).collect(),
            signed_in,
        }
    };
    let mut snapshot = WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("fixture"),
        ProjectIdentity::non_git("/project".into(), "project", "/project"),
    );
    snapshot.tooling.mcp_servers = vec![
        server("asana", "http", Some(22), Some(true)),
        server("linear", "http", Some(31), Some(false)),
        server("sentry", "http", None, None),
        server("chrome-devtools", "stdio", Some(26), None),
    ];
    app.workspace = Some(snapshot);
}

/// A fixed project for the Files tab: luxe-shaped paths, a few dotfiles, and three changed files.
fn seed_project_files(app: &mut App) {
    use luxe_core::workspace::{
        FileChangeStatus, FileChangeSummary, ProjectIdentity, WorkspaceScopeId,
    };
    let files = [
        ".cargo/mutants.toml",
        ".config/nextest.toml",
        ".github/workflows/ci.yml",
        ".gitignore",
        "CONTRIBUTING.md",
        "Cargo.toml",
        "README.md",
        "crates/luxe-core/src/mcp_client.rs",
        "crates/luxe-core/src/output_limit.rs",
        "crates/luxe-mcp/src/oauth.rs",
        "crates/luxe-tui/src/mention_flyout.rs",
        "crates/luxe-tui/src/picker.rs",
        "crates/luxe-tui/src/ref_provider.rs",
        "crates/luxe-tui/src/slash_flyout.rs",
        "crates/luxe-tui/tests/mention_flyout_render.rs",
        "docs/guide/interface.md",
        "docs/guide/mcp.md",
    ];
    app.file_provider =
        crate::ref_provider::FileRefProvider::with_files(files.iter().map(|f| (*f).to_string()));
    let changed = |path: &str, status| FileChangeSummary {
        path: path.into(),
        status,
        additions: None,
        deletions: None,
        binary: false,
    };
    let mut snapshot = WorkspaceSnapshot::empty(
        WorkspaceScopeId::new("fixture"),
        ProjectIdentity::non_git("/project".into(), "project", "/project"),
    );
    snapshot.changes.files = vec![
        changed("crates/luxe-tui/src/picker.rs", FileChangeStatus::Modified),
        changed("crates/luxe-tui/src/ref_provider.rs", FileChangeStatus::Modified),
        changed("crates/luxe-tui/tests/mention_flyout_render.rs", FileChangeStatus::Untracked),
    ];
    snapshot.changes.total_files = 3;
    app.workspace = Some(snapshot);
}

/// Opens the `@` picker for screenshot review: `files`, `skills`, `github` with a few rows,
/// `github-signed-out` as `gh` answers before `gh auth login`, `mcp` with a server in each state
/// its row can report, or `jira`, a source of the reader's own.
///
/// `files-working`, `files-status` and `files-query` are the Files tab over a fixed project with
/// three files in progress: as it opens, the same with each changed file's Git status beside it,
/// and a search for `flyout` lighting what matched and trimming long paths from the folder.
pub fn seed_mention_picker(app: &mut App, tab: &str) {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use luxe_core::github::{ListItem, PullRequestState};
    let key = |code| crate::app::Msg::Key(KeyEvent::new(code, KeyModifiers::NONE));
    // A fixture app has no project indexed; the working directory stands in for one
    app.file_provider =
        crate::ref_provider::FileRefProvider::new(&std::env::current_dir().unwrap_or_default());
    // A source of the reader's own, so the tabs show a third
    app.config.mentions = vec![luxe_core::config::MentionSource {
        name: "Jira".into(),
        kind: None,
        command: "jira-mentions".into(),
        args: Vec::new(),
        icon: Some("\u{f0303}".into()),
    }];
    if tab == "mcp" {
        seed_mcp_servers(app);
    }
    let fixed_files = tab.starts_with("files-");
    if fixed_files {
        seed_project_files(app);
    }
    app.update(key(KeyCode::Char('@')));
    if tab == "files-status" {
        if let Some(mention) = app.overlays.mention.as_mut() {
            mention.show_status();
        }
    }
    if tab == "files-query" {
        for character in "flyout".chars() {
            app.update(key(KeyCode::Char(character)));
        }
    }
    if tab == "files" || fixed_files {
        return;
    }
    if tab == "mcp" {
        // Back past Jira, which asks for its rows, to MCP, which never does
        app.update(key(KeyCode::BackTab));
        app.outbox.mention_source = None;
        app.update(key(KeyCode::BackTab));
        return;
    }
    if tab == "jira" {
        app.update(key(KeyCode::BackTab));
        app.outbox.mention_source = None;
        let row = |id: &str, label: &str, status: &str| luxe_core::mention_source::MentionRow {
            id: id.into(),
            label: format!("{id} {label}"),
            detail: Some(status.into()),
            text: format!("Jira issue {id} \"{label}\" ({status})"),
        };
        if let Some(mention) = app.overlays.mention.as_mut() {
            mention.set_source(
                "jira",
                Ok(vec![
                    row("NOTIFY-41", "Webhook categories arrive as free text", "In progress"),
                    row("NOTIFY-38", "Digest drops the deploy category", "To do"),
                    row(
                        "NOTIFY-35",
                        "Document the category enum for the Swift client",
                        "In review",
                    ),
                ]),
            );
        }
        return;
    }
    if tab == "skills" {
        // Skills fixed for review, rather than whatever this machine has installed
        let skill = |name: &str, description: &str| luxe_core::skills::SkillDoc {
            name: name.into(),
            description: description.into(),
            path: std::path::PathBuf::from(format!("~/.config/luxe/skills/{name}/SKILL.md")),
        };
        let cwd = std::env::current_dir().unwrap_or_default();
        app.overlays.mention = Some(crate::mention_flyout::MentionFlyoutState::new(
            app.file_provider.clone(),
            cwd,
            &app.config.mentions,
            vec![
                skill("gh-stack", "Manage stacked branches and pull requests with gh stack"),
                skill("webapp-testing", "Test a local web app with Playwright: screenshots, logs"),
                skill("release-notes", "Write the release notes for a tag from its merged PRs"),
            ],
            &[],
        ));
        app.update(key(KeyCode::Tab));
        return;
    }
    // Past Skills to GitHub
    app.update(key(KeyCode::Tab));
    app.update(key(KeyCode::Tab));
    let row = |pull_request: bool, number: u64, title: &str, state| ListItem {
        pull_request,
        number,
        title: title.into(),
        url: format!("https://github.com/acme/notify/pull/{number}"),
        repository: "acme/notify".into(),
        author: "mara-okafor".into(),
        draft: number == 181,
        updated_at: String::new(),
        state,
        facts: None,
    };
    let (rows, error) = match tab {
        "github-signed-out" => (
            Vec::new(),
            Some(
                "The GitHub CLI is not signed in. Run `gh auth login` in a terminal, then try \
                 again."
                    .to_string(),
            ),
        ),
        _ => (
            vec![
                row(
                    true,
                    182,
                    "Parse notification categories into an enum",
                    PullRequestState::Open,
                ),
                row(true, 181, "Route digests by category", PullRequestState::Open),
                row(true, 176, "Drop the legacy formatter", PullRequestState::Merged),
                row(
                    false,
                    318,
                    "A typo in a webhook creates a phantom category",
                    PullRequestState::Open,
                ),
            ],
            None,
        ),
    };
    if let Some(mention) = app.overlays.mention.as_mut() {
        mention.set_github(rows, error);
    }
}
