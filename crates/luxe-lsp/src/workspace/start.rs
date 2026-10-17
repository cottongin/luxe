//! Starting a server — each way of launching a language, tried in order, and why none worked.

use super::*;

/// What launching one way of running a language's server came to.
pub(super) enum Tried {
    Started(Box<LspClient>),
    /// It ran and did not complete the handshake, with the reason
    Refused(String),
    /// The repository ships one, and this run may not launch it: where it is
    NotPermitted(String),
    /// Nothing by that name resolved
    Absent,
}

/// How long a (language, root) is skipped after its `attempts`th consecutive failure: the base,
/// doubling each time, up to the cap.
pub(super) fn next_cooldown(attempts: u32) -> Duration {
    RETRY_BASE_COOLDOWN
        .saturating_mul(1u32 << attempts.saturating_sub(1).min(5))
        .min(RETRY_MAX_COOLDOWN)
}

impl WorkspaceLspClient {
    /// One way of launching `language`'s server, resolved, started and handshaken.
    pub(super) async fn try_candidate(
        &self,
        language: &str,
        root: &Path,
        candidate: &crate::registry::ServerCommand,
    ) -> Tried {
        // Resolve the binary rather than trusting `PATH`: language servers arrive via
        // `cargo install`, `pipx`, `npm -g`, or an editor extension, and those bin directories
        // are routinely missing from the environment an app is launched with. Where project
        // servers are allowed the copy this repository installed for itself wins, so a project
        // can pin the version its own toolchain is built around.
        let Some(program) =
            luxe_core::program::resolve_in(root, &candidate.command, self.project_servers)
        else {
            // Present but not permitted is a different answer from absent, and the one a
            // reader can act on: "no server found" while the server sits in
            // `node_modules/.bin` is the least useful true statement available
            return match luxe_core::program::project_local(root, &candidate.command) {
                Some(local) => Tried::NotPermitted(local.to_string_lossy().into_owned()),
                None => Tried::Absent,
            };
        };
        // Worth an `info`: it is the one case where the program Luxe ran came from the tree
        // being edited rather than from the machine, and a reader should be able to find that
        // out from a normal run rather than by reasoning about it.
        if luxe_core::program::project_local(root, &candidate.command).as_deref()
            == Some(program.as_path())
        {
            tracing::info!(
                language,
                server = %luxe_core::paths::display(&program),
                "using the language server this project installed"
            );
        }
        let mut command = vec![program.to_string_lossy().into_owned()];
        command.extend(candidate.args.clone());
        let config = LspConfig {
            command,
            server_id: language.to_string(),
            root_path: root.to_path_buf(),
            init_timeout: self.init_timeout,
        };

        let mut client = LspClient::new(config);
        match client.initialize().await {
            Ok(()) => {
                debug!(language, command = %candidate.command, "language server started");
                Tried::Started(Box::new(client))
            }
            Err(error) => {
                debug!(
                    language,
                    command = %candidate.command,
                    %error,
                    "language server candidate refused; trying the next"
                );
                Tried::Refused(format!("`{}`: {error}", candidate.command))
            }
        }
    }

    /// Why no server could be started: what refused, what was found but not allowed, or what is
    /// simply not installed.
    pub(super) fn failure_reason(
        &self,
        language: &str,
        refusals: &[String],
        unpermitted: Option<&String>,
    ) -> String {
        // Nothing resolved at all is a different situation from everything refusing, and the
        // install hint only helps with the first.
        match (refusals.is_empty(), unpermitted) {
            (false, _) => {
                format!("{language} language server did not start — {}", refusals.join("; "))
            }
            // The one case where the install hint would be actively wrong: it is installed.
            (true, Some(local)) => format!(
                "this project supplies a {language} language server at {local}, and this run does \
                 not launch servers a repository brought with it — pass `--trust-project`, set \
                 `lsp.project_servers` to `always`, or install the server globally"
            ),
            (true, None) => format!(
                "no {language} language server found{}",
                self.registry
                    .install_hint(language)
                    .map(|hint| format!(" — install one (`{hint}`) or check it is on PATH"))
                    .unwrap_or_default()
            ),
        }
    }
}
