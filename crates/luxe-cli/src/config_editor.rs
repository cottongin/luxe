//! Config editor handoff — opens one of luxe's files outside the alternate screen.
//!
//! The TUI returns control before this runs, so terminal editors inherit a normal terminal.
//! When the editor exits, the CLI rebuilds the TUI and reloads the updated configuration.

use anyhow::{bail, Context};
use luxe_core::config::Config;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
struct EditorCommand {
    program: String,
    args: Vec<String>,
}

/// Creates `path` when it is missing, then opens it in the user's editor.
///
/// Creating it first is the point: an editor pointed at a file that does not exist gives a new user
/// an empty buffer and no idea what belongs in it, and — for the credential file — whatever mode
/// their umask happens to grant.
pub(crate) async fn open_config_file(path: PathBuf) -> anyhow::Result<PathBuf> {
    let path = ensure_file(path)?;
    let editor = resolve_editor()?;
    let command_path = path.clone();
    let status = tokio::task::spawn_blocking(move || {
        std::process::Command::new(&editor.program)
            .args(&editor.args)
            .arg(&command_path)
            .status()
            .with_context(|| format!("launching config editor '{}'", editor.program))
    })
    .await
    .context("joining config editor process")??;

    if !status.success() {
        bail!("config editor exited with {status}");
    }
    Ok(path)
}

/// Make sure the chosen file exists, with the right skeleton and the right mode.
///
/// `config.json` goes through [`Config::ensure_global_file`], which also refreshes the JSON
/// schema beside it so an editor offers completion for settings this build added. A `.md` file is
/// an instruction file and gets [`AGENTS_TEMPLATE`]; everything else gets an empty object — valid
/// JSON that every loader here already understands — and `auth.json` gets it `0600`, because a
/// credential file created world-readable is a credential file leaked.
fn ensure_file(path: PathBuf) -> anyhow::Result<PathBuf> {
    if path == Config::global_path() {
        return Config::ensure_global_file().context("creating luxe user config");
    }
    if path.exists() {
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let markdown = path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("md"));
    if markdown {
        std::fs::write(&path, AGENTS_TEMPLATE)
            .with_context(|| format!("creating {}", path.display()))?;
        return Ok(path);
    }
    let secret = path.file_name().is_some_and(|name| name == "auth.json");
    let write = match secret {
        true => luxe_core::fs_atomic::write_private,
        false => |path: &Path, bytes: &[u8]| std::fs::write(path, bytes),
    };
    write(&path, b"{}\n").with_context(|| format!("creating {}", path.display()))?;
    Ok(path)
}

/// What a brand-new `AGENTS.md` opens on.
///
/// Headings only, and headings chosen to be the questions a reader can actually answer about their
/// own project. An empty buffer is the real failure mode here — the file's whole value is the
/// things a model cannot infer from the code, and nobody writes those from a blank page. It says
/// what *not* to put in as plainly as what to, because the common first draft is a summary of the
/// repository, which the model can already read.
const AGENTS_TEMPLATE: &str = "\
# AGENTS.md

Rules for working in this repository. This file rides in every request, so keep it to what a
capable engineer would get wrong without being told. Anything discoverable by reading the code
does not belong here.

## Commands

<!-- The canonical build, test, lint and typecheck commands. -->

## Architecture

<!-- Boundaries and invariants: what may depend on what, and what must stay true. -->

## Conventions

<!-- Style, error handling, testing habits — the ones specific to this project. -->
";

fn resolve_editor() -> anyhow::Result<EditorCommand> {
    let (program, args) = luxe_core::program::editor()?;
    Ok(EditorCommand { program, args })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file the reader picked has to exist by the time the editor opens it, and a credential
    /// file created with whatever the umask allowed is a credential file leaked.
    #[test]
    fn a_missing_file_is_created_before_the_editor_sees_it() {
        let directory = tempfile::tempdir().expect("a temp dir");
        let models = directory.path().join("nested").join("models.json");
        assert_eq!(ensure_file(models.clone()).expect("created"), models);
        assert_eq!(std::fs::read_to_string(&models).expect("readable"), "{}\n");

        let auth = directory.path().join("auth.json");
        ensure_file(auth.clone()).expect("created");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&auth).expect("stat").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the credential file is owner-only");
        }

        // An existing file is left exactly as it is — this only ever creates.
        std::fs::write(&models, "{\"default_model\": \"gpt-5.6\"}").expect("written");
        ensure_file(models.clone()).expect("kept");
        assert_eq!(
            std::fs::read_to_string(&models).expect("readable"),
            "{\"default_model\": \"gpt-5.6\"}"
        );
    }

    /// A new instruction file opens on prose, not on `{}`.
    ///
    /// The context-file surface offers to start an `AGENTS.md`, and the skeleton every other file
    /// here gets is an empty JSON object — which in a markdown file is just a wrong character on
    /// line one.
    #[test]
    fn a_new_instruction_file_opens_on_a_template_rather_than_json() {
        let directory = tempfile::tempdir().expect("a temp dir");
        let agents = directory.path().join("AGENTS.md");
        ensure_file(agents.clone()).expect("created");

        let written = std::fs::read_to_string(&agents).expect("readable");
        assert!(written.starts_with("# AGENTS.md"), "{written}");
        assert!(!written.contains("{}"), "no JSON skeleton in a markdown file: {written}");
        assert!(written.contains("## Commands"), "the headings are the prompt: {written}");

        // And it only ever creates: a file with content is handed over untouched.
        std::fs::write(&agents, "mine").expect("written");
        ensure_file(agents.clone()).expect("kept");
        assert_eq!(std::fs::read_to_string(&agents).expect("readable"), "mine");
    }
}
