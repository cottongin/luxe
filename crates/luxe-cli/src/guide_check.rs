//! Guide check — the user guide's config examples, flags and slash commands, held against the code.
//!
//! `docs/guide/` is what a user copies from and what an agent follows. luxe ignores a config key it
//! does not know, on purpose, so an old config loads on a new luxe; the cost is that a misspelled
//! or invented key in the docs loads too, and does nothing. These tests load every example the
//! guide names a file for as that file, and fail on anything luxe would ignore.
//!
//! A code block opts in by naming its file in the fence, ` ```json title="config.json" `, which is
//! also what the site shows above the block. `scripts/docs-check.py` covers links and voice.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use clap::CommandFactory;
use serde::de::DeserializeOwned;

use crate::cli::Args;

fn guide_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/guide")
}

/// Every page, as `(name, text)`.
fn pages() -> Vec<(String, String)> {
    let mut pages: Vec<(String, String)> = std::fs::read_dir(guide_dir())
        .expect("docs/guide exists beside the crates")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .map(|path| {
            let name = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&path).expect("a guide page reads as UTF-8");
            (name, text)
        })
        .collect();
    pages.sort();
    pages
}

/// A fenced code block.
struct Block {
    /// Where it starts, as `page.md:line`
    at: String,
    lang: String,
    /// The file it says it is, from `title="…"` in the fence
    title: Option<String>,
    body: String,
}

fn blocks(page: &str, text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut open: Option<(usize, String, Option<String>, Vec<&str>)> = None;
    for (number, line) in text.lines().enumerate() {
        match (&mut open, line.trim_start().strip_prefix("```")) {
            (None, Some(info)) => {
                let lang = info.split_whitespace().next().unwrap_or_default().to_owned();
                let title = info
                    .split_once("title=\"")
                    .and_then(|(_, rest)| rest.split_once('"'))
                    .map(|(title, _)| title.to_owned());
                open = Some((number + 1, lang, title, Vec::new()));
            }
            (Some(_), Some(_)) => {
                let (start, lang, title, lines) = open.take().expect("a block is open");
                blocks.push(Block {
                    at: format!("{page}.md:{start}"),
                    lang,
                    title,
                    body: lines.join("\n"),
                });
            }
            (Some((_, _, _, lines)), None) => lines.push(line),
            (None, None) => {}
        }
    }
    blocks
}

/// The keys luxe would ignore reading `text` as `T`, as paths like `dock.mode`.
fn ignored<T: DeserializeOwned>(text: &str) -> Result<Vec<String>, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let mut paths = Vec::new();
    serde_ignored::deserialize::<_, _, T>(value, |path| paths.push(path.to_string()))
        .map_err(|error| error.to_string())?;
    Ok(paths)
}

/// The keys luxe would ignore in an `auth.json`: every top-level key is a provider's credential,
/// except the configuration keys it shares with `models.json`.
fn auth_file(text: &str) -> Result<Vec<String>, String> {
    const CONFIGURATION: &[&str] =
        &["providers", "models", "default_provider", "default_model", "$schema"];
    let value: serde_json::Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let object = value.as_object().ok_or("auth.json is an object")?;
    let (configuration, credentials): (serde_json::Map<_, _>, serde_json::Map<_, _>) =
        object.clone().into_iter().partition(|(key, _)| CONFIGURATION.contains(&key.as_str()));
    let mut unknown = ignored::<luxe_providers::config::ModelsConfig>(
        &serde_json::Value::Object(configuration).to_string(),
    )?;
    unknown.extend(
        credentials
            .iter()
            .filter(|(_, entry)| luxe_providers::auth::Credential::from_json(entry).is_none())
            .map(|(provider, _)| format!("{provider} (not a credential)")),
    );
    Ok(unknown)
}

/// What is wrong with `text` as the file `title` names, if anything.
fn check_file(title: &str, text: &str) -> Result<(), String> {
    let file = title.rsplit('/').next().unwrap_or(title);
    let unknown = match file {
        "config.json" => ignored::<luxe_core::config::Config>(text)?,
        "models.json" => ignored::<luxe_providers::config::ModelsConfig>(text)?,
        "auth.json" => auth_file(text)?,
        "permissions.json" => ignored::<luxe_core::permission::PermissionsFile>(text)?,
        "hooks.json" => {
            let mut unknown = ignored::<luxe_core::command_hooks::HooksFile>(text)?;
            let file = luxe_core::command_hooks::HooksFile::from_json(text)
                .map_err(|error| error.to_string())?;
            unknown.extend(
                file.unknown_points.into_iter().map(|point| format!("{point} (hook point)")),
            );
            unknown
        }
        "mcp.json" | ".mcp.json" if text.contains("\"mcpServers\"") => {
            ignored::<luxe_core::mcp_config::StandardConfigFile>(text)?
        }
        "mcp.json" | ".mcp.json" => ignored::<luxe_core::mcp_config::McpConfigFile>(text)?,
        "keybindings.json" => {
            let map: HashMap<String, Vec<String>> =
                serde_json::from_str(text).map_err(|error| error.to_string())?;
            let mut unknown = Vec::new();
            for (action, chords) in map {
                if luxe_core::keymap::UiAction::from_id(&action).is_none() {
                    unknown.push(format!("{action} (action)"));
                }
                for chord in chords {
                    if luxe_core::keymap::KeyChord::parse(&chord).is_err() {
                        unknown.push(format!("{chord} (chord, for {action})"));
                    }
                }
            }
            unknown
        }
        "lsp-servers.json" => ignored::<HashMap<String, luxe_lsp::ServerEntryPatch>>(text)?,
        "debug-adapters.json" => ignored::<Vec<luxe_dap::CustomAdapter>>(text)?,
        _ if title.contains("themes/") && file.ends_with(".json") => {
            ignored::<luxe_core::theme::Theme>(text)?
        }
        _ => return Err(format!("no file type known for {title}; add it to `check_file`")),
    };
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(format!("luxe would ignore {}", unknown.join(", ")))
    }
}

/// Every config example the guide names a file for loads as that file, with nothing ignored.
#[test]
fn every_named_config_example_loads() {
    let mut failures = Vec::new();
    for (page, text) in pages() {
        for block in blocks(&page, &text) {
            let Some(title) = block.title.as_deref() else { continue };
            if block.lang != "json" {
                continue;
            }
            if let Err(problem) = check_file(title, &block.body) {
                failures.push(format!("{} ({title}): {problem}", block.at));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "config examples luxe would not read as written:\n{}",
        failures.join("\n")
    );
}

/// The command lines the guide shows: `luxe …` at the start of a shell line or an inline code span.
fn command_lines(page: &str, text: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for block in blocks(page, text) {
        if !matches!(block.lang.as_str(), "bash" | "sh" | "shell" | "console" | "") {
            continue;
        }
        for line in block.body.lines() {
            let line = line.trim().trim_start_matches("$ ");
            if line == "luxe" || line.starts_with("luxe ") {
                found.push((block.at.clone(), line.to_owned()));
            }
        }
    }
    for (number, line) in text.lines().enumerate() {
        for span in line.split('`').skip(1).step_by(2) {
            if span == "luxe" || span.starts_with("luxe ") {
                found.push((format!("{page}.md:{}", number + 1), span.to_owned()));
            }
        }
    }
    found
}

/// `line` split the way a shell would split it: a quoted argument is one word.
fn shell_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut in_word = false;
    for c in line.chars() {
        match (quote, c) {
            (Some(open), c) if c == open => quote = None,
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// What is wrong with a `luxe …` command line, if anything: a flag or subcommand luxe lacks.
fn check_command_line(root: &clap::Command, line: &str) -> Result<(), String> {
    let mut path: Vec<&clap::Command> = vec![root];
    let words = shell_words(line);
    let mut tokens = words.iter().map(String::as_str).skip(1).peekable();
    while let Some(token) = tokens.next() {
        if matches!(token, "#" | "|" | "||" | "&&" | ";" | ">" | "<" | "2>&1" | "\\") {
            break;
        }
        let here = *path.last().expect("the root is always on the path");
        let flag = |arg: &clap::Arg| {
            if let Some(long) = token.strip_prefix("--") {
                let long = long.split('=').next().unwrap_or(long);
                arg.get_long() == Some(long)
                    || arg.get_all_aliases().is_some_and(|aliases| aliases.contains(&long))
            } else {
                token.len() == 2 && arg.get_short() == token.chars().nth(1)
            }
        };
        if token.starts_with('-') && token.len() > 1 {
            // a global flag is declared once, on an ancestor
            let arg =
                path.iter().rev().find_map(|command| command.get_arguments().find(|arg| flag(arg)));
            let Some(arg) = arg else {
                let name =
                    path.iter().map(|command| command.get_name()).collect::<Vec<_>>().join(" ");
                return Err(format!("`{name}` has no {token}"));
            };
            if arg.get_action().takes_values() && !token.contains('=') {
                tokens.next();
            }
        } else if let Some(sub) = here.find_subcommand(token) {
            path.push(sub);
        }
    }
    Ok(())
}

/// Every `luxe` flag and subcommand the guide shows exists.
#[test]
fn every_command_line_in_the_guide_exists() {
    let mut root = Args::command();
    root.build();
    let mut failures = Vec::new();
    for (page, text) in pages() {
        for (at, line) in command_lines(&page, &text) {
            if let Err(problem) = check_command_line(&root, &line) {
                failures.push(format!("{at}: {problem}: {line}"));
            }
        }
    }
    assert!(failures.is_empty(), "command lines luxe would refuse:\n{}", failures.join("\n"));
}

/// Paths that look like a slash command in a code span.
const NOT_COMMANDS: &[&str] =
    &["/tmp", "/etc", "/usr", "/dev", "/home", "/var", "/opt", "/bin", "/proc"];

/// Every slash command the guide names is one luxe has.
#[test]
fn every_slash_command_in_the_guide_exists() {
    let registry = luxe_tui::commands::CommandRegistry::all();
    let mut failures = Vec::new();
    for (page, text) in pages() {
        let mut in_code = false;
        for (number, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("```") {
                in_code = !in_code;
                continue;
            }
            if in_code {
                continue;
            }
            for span in line.split('`').skip(1).step_by(2) {
                let name = span.split_whitespace().next().unwrap_or_default();
                let looks_like_one = name.len() > 1
                    && name.starts_with('/')
                    && name[1..]
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
                // an API version in a URL path, `/v1`
                let version = name.len() > 2
                    && name.starts_with("/v")
                    && name[2..].chars().all(|c| c.is_ascii_digit());
                if looks_like_one
                    && !version
                    && !NOT_COMMANDS.contains(&name)
                    && registry.parse_slash(span).is_none()
                {
                    failures.push(format!("{page}.md:{}: {span}", number + 1));
                }
            }
        }
    }
    assert!(failures.is_empty(), "slash commands luxe does not have:\n{}", failures.join("\n"));
}

/// The command reference lists every slash command luxe has, and every key it binds by default, so a
/// new command or binding cannot ship undocumented.
#[test]
fn the_command_reference_is_complete() {
    let page = std::fs::read_to_string(guide_dir().join("commands.md")).expect("commands.md");
    let mut missing: Vec<String> = luxe_tui::commands::CommandRegistry::all()
        .iter()
        .filter_map(|command| command.slash_name().map(str::to_owned))
        .filter(|slash| {
            !page.contains(&format!("`{slash}`")) && !page.contains(&format!("`{slash} "))
        })
        .collect();
    let defaults = luxe_core::keymap::KeyMap::defaults().to_config_map();
    missing.extend(defaults.into_keys().filter(|action| !page.contains(&format!("`{action}`"))));
    missing.sort();
    assert!(missing.is_empty(), "commands.md does not mention:\n{}", missing.join("\n"));
}

/// The configuration reference names every key `config.json` takes, from the schema luxe writes
/// beside it, so a new setting cannot ship undocumented.
#[test]
fn the_configuration_reference_is_complete() {
    fn keys(node: &serde_json::Value, prefix: &str, out: &mut Vec<String>) {
        let Some(properties) = node.get("properties").and_then(serde_json::Value::as_object) else {
            return;
        };
        for (name, child) in properties {
            if name == "$schema" {
                continue;
            }
            let path = format!("{prefix}{name}");
            // a block is documented by its keys, not by a row of its own
            if child.get("properties").is_some() {
                keys(child, &format!("{path}."), out);
            } else {
                out.push(path);
            }
        }
    }
    let schema: serde_json::Value =
        serde_json::from_str(luxe_core::config::Config::schema_json()).expect("the schema parses");
    let page =
        std::fs::read_to_string(guide_dir().join("configuration.md")).expect("configuration.md");
    let mut all = Vec::new();
    keys(&schema, "", &mut all);
    // siblings may be listed by their leaf: `plan.model`, `.thinking`
    let missing: Vec<String> = all
        .into_iter()
        .filter(|key| {
            let leaf = key.rsplit('.').next().unwrap_or(key);
            !page.contains(&format!("`{key}`")) && !page.contains(&format!("`.{leaf}`"))
        })
        .collect();
    assert!(missing.is_empty(), "configuration.md does not mention:\n{}", missing.join("\n"));
}

/// The RPC reference lists every action a client may send, from the protocol's own published list.
#[test]
fn the_rpc_reference_lists_every_action() {
    let page = std::fs::read_to_string(guide_dir().join("rpc.md")).expect("rpc.md");
    let missing: Vec<&str> = luxe_core::event::Action::KINDS
        .iter()
        .copied()
        .filter(|kind| !page.contains(&format!("| `{kind}` |")))
        .collect();
    assert!(missing.is_empty(), "rpc.md has no row for: {missing:?}");
    let version = format!("`protocol_version` is `{}`", luxe_core::event::PROTOCOL_VERSION);
    assert!(page.contains(&version), "rpc.md names a different protocol version");
}

/// The command-line reference names every subcommand and flag the CLI has, so neither can ship
/// undocumented.
#[test]
fn the_command_line_reference_is_complete() {
    fn walk(command: &clap::Command, path: &str, page: &str, missing: &mut Vec<String>) {
        for arg in command.get_arguments() {
            let Some(long) = arg.get_long() else { continue };
            if matches!(long, "help" | "version") || arg.is_hide_set() {
                continue;
            }
            if !page.contains(&format!("--{long}")) {
                missing.push(format!("{path} --{long}"));
            }
        }
        for sub in command.get_subcommands() {
            if sub.get_name() == "help" || sub.is_hide_set() {
                continue;
            }
            let path = format!("{path} {}", sub.get_name());
            if !page.contains(&format!("`{path}")) && !page.contains(&format!(", `{path}")) {
                missing.push(path.clone());
            }
            walk(sub, &path, page, missing);
        }
    }
    let page = std::fs::read_to_string(guide_dir().join("cli.md")).expect("cli.md");
    let mut root = Args::command();
    root.build();
    let mut missing = Vec::new();
    walk(&root, "luxe", &page, &mut missing);
    missing.dedup();
    assert!(missing.is_empty(), "cli.md does not mention:\n{}", missing.join("\n"));
}

/// The checks themselves can fail: an invented key, flag and command are each caught.
#[test]
fn the_checks_catch_what_they_exist_for() {
    assert!(check_file("~/.config/luxe/config.json", r#"{ "dock": { "mode": "auto" } }"#).is_ok());
    let problem = check_file("config.json", r#"{ "dock": { "moed": "auto" } }"#).unwrap_err();
    assert!(problem.contains("dock.moed"), "{problem}");
    assert!(
        check_file("hooks.json", r#"{ "pre_tool_usage": [] }"#).is_err(),
        "an unknown hook point"
    );
    assert!(check_file("keybindings.json", r#"{ "tui.nope": ["ctrl+x"] }"#).is_err());
    assert!(check_file("notes.json", "{}").is_err(), "a file nobody taught it");
    assert!(
        check_file("auth.json", r#"{ "openai": { "type": "api_key", "key": "sk-x" } }"#).is_ok()
    );
    assert!(check_file("auth.json", r#"{ "openai": { "kind": "nope" } }"#).is_err());
    assert!(check_file("auth.json", r#"{ "providers": { "x": { "base_urll": "y" } } }"#).is_err());

    let mut root = Args::command();
    root.build();
    assert!(check_command_line(&root, "luxe --headless -p \"hi\"").is_ok());
    assert!(check_command_line(&root, "luxe auth login anthropic").is_ok());
    assert!(check_command_line(&root, "luxe --model gpt-5 --no-such-flag").is_err());
    assert!(check_command_line(&root, "luxe auth login --stdin < key.txt").is_ok());
    assert!(
        check_command_line(&root, "luxe mcp add --id c --name \"C\" --command \"npx -y x\"")
            .is_ok(),
        "a quoted argument is one word"
    );
    assert!(check_command_line(&root, "luxe mcp add --id c --nope \"a b\"").is_err());

    let registry = luxe_tui::commands::CommandRegistry::all();
    assert!(registry.parse_slash("/model").is_some());
    assert!(registry.parse_slash("/no-such-command").is_none());
}
