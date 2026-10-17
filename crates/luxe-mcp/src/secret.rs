//! Secrets — what a client secret or bearer token written in an MCP config resolves to.
//!
//! Three forms, the ones a provider key in `auth.json` accepts: the value itself, `${VAR}` (or
//! `$env:VAR`) for an environment variable, and `!command` for a program that prints it — a
//! password manager, usually, so the secret is on disk in plain text nowhere and in no process's
//! environment. Resolved when it is needed, at sign-in and on a refresh, never when the config is
//! read: the config is read on every workspace snapshot, and a password manager asked for a secret
//! that often would ask the reader for a passphrase that often.

use luxe_core::secret_command::SECRET_COMMAND_TIMEOUT;

/// Why a secret written in a config did not resolve to anything.
///
/// Each carries the sentence that says what to do about it, with the secret's own name in it.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// Nothing there: it is empty, or a `${VAR}` that is not set where Luxe runs
    #[error("{0}")]
    Unset(String),
    /// A `!command` in a config that may not run one
    #[error("{0}")]
    CommandRefused(String),
    /// The command ran, and did not print a secret
    #[error("{0}")]
    Command(String),
}

/// Resolves a secret as written in the config.
///
/// Args: `(written, commands_allowed, what)` — whether this server was declared in the reader's own
/// config, and what the secret is, for the error (`"the client secret for 'asana'"`).
///
/// A `!command` from a project's config is refused rather than run. A secret has no business in a
/// file a repository ships, and running one would make `luxe mcp login` — which reads as "open a
/// browser" — run whatever a cloned repository named. The error names the file it belongs in.
/// What the command printed never appears in an error, since a command that fails halfway may
/// still have printed the secret.
pub(crate) async fn resolve_secret(
    written: &str,
    commands_allowed: bool,
    what: &str,
) -> Result<String, SecretError> {
    resolve(written, commands_allowed, what).await.map(|secret| remembered(&secret))
}

/// `secret`, once remembered for redaction: a resolved secret or a token about to ride a request
/// has no shape a pattern could know.
pub(crate) fn remembered(secret: &str) -> String {
    luxe_core::redact::process_secrets().add(secret);
    secret.to_string()
}

async fn resolve(written: &str, commands_allowed: bool, what: &str) -> Result<String, SecretError> {
    let Some(command) = written.trim().strip_prefix('!') else {
        let expanded = luxe_core::mcp_client::expand_env(written);
        if expanded.trim().is_empty() {
            return Err(SecretError::Unset(match named_variable(written) {
                Some(name) => format!(
                    "{what} is `${{{name}}}`, and {name} is not set where Luxe is running — export \
                     it, or add the export to your shell profile so every terminal has it"
                ),
                None => format!("{what} is empty"),
            }));
        }
        return Ok(expanded);
    };
    if !commands_allowed {
        return Err(SecretError::CommandRefused(format!(
            "{what} is a command, and only your own config runs one — a project's config may \
             not. Move the server's auth to {}",
            luxe_core::user_dirs::config_path_display("mcp.json")
        )));
    }
    luxe_core::secret_command::run(command.trim(), what, SECRET_COMMAND_TIMEOUT)
        .await
        .map_err(SecretError::Command)
}

/// The variable `written` names, as `${VAR}` or `$env:VAR`, when it names one.
///
/// Asked only once the secret has expanded to nothing, so whatever it names is unset or blank, and
/// the first is the one to name: a secret that expands to nothing is nearly always a variable
/// exported in one terminal and not the one Luxe runs in, and saying which is the whole of the fix.
/// Whichever marker comes first, the order [`luxe_core::mcp_client::expand_env`] reads them in.
fn named_variable(written: &str) -> Option<&str> {
    let braced = written.find("${");
    let env = written.find("$env:");
    let name = match (braced, env) {
        (Some(at), env) if env.is_none_or(|env| at < env) => written[at + 2..].split_once('}')?.0,
        (_, Some(at)) => {
            let body = &written[at + 5..];
            body.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').next()?
        }
        _ => return None,
    };
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHAT: &str = "the client secret for 'asana'";

    /// A resolved secret is remembered, so a tool that prints it is scrubbed whatever its shape.
    #[tokio::test]
    async fn a_resolved_secret_is_remembered_for_redaction() {
        resolve_secret("mcp-plain-secret-5521", false, WHAT).await.unwrap();
        let seen = luxe_core::redact::process_secrets().redact("seen mcp-plain-secret-5521");
        assert_eq!(seen, "seen [REDACTED]");
    }

    #[tokio::test]
    async fn a_value_or_a_variable_resolves_as_before() {
        std::env::set_var("LUXE_MCP_SECRET_TEST", "from-env");
        assert_eq!(resolve_secret("literal", false, WHAT).await.unwrap(), "literal");
        assert_eq!(
            resolve_secret("${LUXE_MCP_SECRET_TEST}", false, WHAT).await.unwrap(),
            "from-env"
        );
        let unset = resolve_secret("${LUXE_MCP_SECRET_UNSET}", true, WHAT).await.unwrap_err();
        assert!(matches!(unset, SecretError::Unset(_)), "{unset:?}");
        assert_eq!(
            unset.to_string(),
            format!(
                "{WHAT} is `${{LUXE_MCP_SECRET_UNSET}}`, and LUXE_MCP_SECRET_UNSET is not set where \
                 Luxe is running — export it, or add the export to your shell profile so every \
                 terminal has it"
            )
        );
        let second =
            resolve_secret("$env:LUXE_MCP_SECRET_TEST$env:LUXE_MCP_SECRET_GONE", true, WHAT).await;
        assert_eq!(second.unwrap(), "from-env", "one variable set is a secret");
        let blank = resolve_secret("   ", true, WHAT).await.unwrap_err().to_string();
        assert_eq!(blank, format!("{WHAT} is empty"), "no variable to name");
        let nameless = resolve_secret("${}", true, WHAT).await.unwrap_err().to_string();
        assert_eq!(nameless, format!("{WHAT} is empty"), "nor here");
        let windows =
            resolve_secret("$env:LUXE_MCP_SECRET_GONE", true, WHAT).await.unwrap_err().to_string();
        assert!(windows.contains("and LUXE_MCP_SECRET_GONE is not set"), "{windows}");
        // the first marker is the one named, as the expander reads them left to right
        let mixed = "$env:LUXE_MCP_SECRET_FIRST${LUXE_MCP_SECRET_SECOND}";
        let first = resolve_secret(mixed, true, WHAT).await.unwrap_err().to_string();
        assert!(first.contains("and LUXE_MCP_SECRET_FIRST is not set"), "{first}");
        std::env::remove_var("LUXE_MCP_SECRET_TEST");
    }

    /// The first line of what the command prints is the secret — `pass show` puts the rest of
    /// the entry after it — and a project's config may not run one at all.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_prints_the_secret_and_only_your_config_runs_one() {
        let secret = resolve_secret("!printf 's3cret\\nurl: x\\n'", true, WHAT).await;
        assert_eq!(secret.unwrap(), "s3cret");

        let marker = tempfile::tempdir().unwrap();
        let path = marker.path().join("ran");
        let command = format!("!touch {} && echo s3cret", path.display());
        let refused = resolve_secret(&command, false, WHAT).await.unwrap_err();
        assert!(matches!(refused, SecretError::CommandRefused(_)), "{refused:?}");
        let refused = refused.to_string();
        assert!(refused.starts_with(&format!("{WHAT} is a command")), "{refused}");
        assert!(refused.contains("mcp.json"), "and says where it belongs: {refused}");
        assert!(!path.exists(), "a refused command never runs");
    }

    /// A failure says why in the command's own words and never repeats what it printed, which
    /// may be the secret.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_failing_command_says_why_without_what_it_printed() {
        let failed = resolve_secret("!echo s3cret; echo 'no such entry' >&2; exit 3", true, WHAT)
            .await
            .unwrap_err()
            .to_string();
        assert!(failed.contains("failed") && failed.ends_with(": no such entry"), "{failed}");
        assert!(!failed.contains("s3cret"), "{failed}");

        let silent = resolve_secret("!true", true, WHAT).await.unwrap_err().to_string();
        assert!(silent.ends_with("printed nothing"), "{silent}");
    }
}
