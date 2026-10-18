//! `luxe doctor` — is the tooling on this machine actually going to work?
//!
//! Luxe launches language servers and debug adapters; it does not install them, and it does not
//! bundle them: installation would mean owning each language's package manager and update policy.
//! A broken or absent one can otherwise stay invisible until a tool call comes back empty.
//!
//! This is the answer to "is my setup working", asked from a terminal rather than inferred from a
//! disappointing result. It reports what resolved, which candidate won where a language has more
//! than one, and — for a server that is present and still will not run — the reason.

use crate::output::{out, outln};
use std::fmt::Write as _;

use luxe_lsp::registry::{ServerEntry, ServerRegistry};

/// Run the doctor, printing each section as it is established.
///
/// Streamed rather than assembled: the language-server section actually starts each server, so it
/// is the slow part, and a checkup that prints nothing until it is finished looks exactly like a
/// checkup that has hung — which is the failure mode this command is for.
pub(crate) async fn run_doctor_command(
    settings: crate::engine_setup::AgentSettings,
) -> anyhow::Result<()> {
    let mut header = String::new();
    let _ = writeln!(header, "luxe {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(header, "  project   {}", settings.state_root.display());
    let _ = match &settings.model {
        Some(model) => writeln!(header, "  model     {}", model.r#ref),
        None => writeln!(header, "  model     none selected"),
    };
    out!("{header}");

    for section in [external_programs(), debug_adapters()] {
        outln!();
        out!("{section}");
    }
    outln!();
    language_servers().await;
    Ok(())
}

/// Every language in the registry, and which of its candidates is usable.
///
/// Each is actually **started**, not merely resolved. Resolution is the check that produced the
/// bug this command exists to catch: `typescript-language-server` is on `PATH` in a TypeScript 7
/// project and fails its handshake every time, so a report of "ok" would repeat the lie rather
/// than expose it. Cheap now that a server which refuses says so immediately.
async fn language_servers() {
    /// Print a line and get it on screen now: each language costs a real handshake, so a report
    /// that arrives all at once looks like one that has stalled.
    fn say(line: &str) {
        outln!("{line}");
    }
    say("Language servers");

    // The registry a session builds, not the shipped defaults alone: a server added in
    // `lsp-servers.json` is exactly the one somebody runs the checkup to confirm.
    let mut registry = ServerRegistry::new();
    let user = crate::engine_setup::read_lsp_servers().unwrap_or_else(|error| {
        say(&format!("  FAIL  lsp-servers.json is not valid, so none of it applies: {error}"));
        Default::default()
    });
    let configured: std::collections::HashSet<String> = user.keys().cloned().collect();
    for language in registry.merge_patches(user) {
        say(&format!(
            "  FAIL  {language:<12} lsp-servers.json gives no `command`, and Luxe ships none"
        ));
    }
    let mut languages: Vec<(&String, &ServerEntry)> = registry.iter().collect();
    languages.sort_by_key(|(language, _)| language.as_str());

    for (language, entry) in languages {
        // Each language costs a real handshake, so a reader who has stopped — `luxe doctor | head`
        // — should not be waited on by a report nobody will see.
        if !crate::output::is_open() {
            return;
        }
        let (working, refusals, absent) = probe_language(language, entry).await;
        let origin = if configured.contains(language) { "  (lsp-servers.json)" } else { "" };

        match working {
            Some(invocation) => {
                say(&format!("  ok    {language:<12} {invocation}{origin}"));
                // A candidate that refused before the working one is still worth naming: it is
                // usually the one the user thinks they are running.
                for refusal in &refusals {
                    say(&format!("        (skipped {refusal})"));
                }
            }
            None if refusals.is_empty() => {
                // Naming every candidate matters where a language has more than one: "install
                // typescript-language-server" is wrong advice for a TypeScript 7 project.
                say(&format!("  none  {language:<12} tried: {}{origin}", absent.join(", ")));
                if let Some(hint) = &entry.install_hint {
                    say(&format!("        {hint}"));
                }
            }
            None => {
                say(&format!("  FAIL  {language:<12} installed, but will not start{origin}"));
                for refusal in &refusals {
                    say(&format!("        {refusal}"));
                }
                if let Some(hint) = &entry.install_hint {
                    say(&format!("        {hint}"));
                }
            }
        }
    }
}

/// Try each of a language's candidates, returning the first that works plus what the others said.
///
/// Args: `(language, entry)`; returns `(working_invocation, refusals, absent)`.
async fn probe_language(
    language: &str,
    entry: &ServerEntry,
) -> (Option<String>, Vec<String>, Vec<String>) {
    let mut refusals = Vec::new();
    let mut absent = Vec::new();

    for candidate in entry.candidates() {
        let Some(program) = luxe_core::program::resolve(&candidate.command) else {
            absent.push(candidate.command.clone());
            continue;
        };
        let invocation = std::iter::once(candidate.command.clone())
            .chain(candidate.args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ");
        let mut command = vec![program.to_string_lossy().into_owned()];
        command.extend(candidate.args.clone());
        let mut client = luxe_lsp::LspClient::new(luxe_lsp::client::LspConfig {
            command,
            server_id: language.to_string(),
            root_path: std::env::current_dir().unwrap_or_else(|_| ".".into()),
            init_timeout: HANDSHAKE_BUDGET,
        });
        match client.initialize().await {
            Ok(()) => return (Some(invocation), refusals, absent),
            Err(error) => refusals.push(format!("{invocation}: {error}")),
        }
    }
    (None, refusals, absent)
}

/// How long a server gets to answer `initialize` during a checkup.
///
/// Shorter than a real session's budget on purpose: this is a report, and a server too slow to
/// greet in five seconds is itself worth reporting.
const HANDSHAKE_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

/// Every debug adapter a session can use — built-in and configured — and whether it is here.
fn debug_adapters() -> String {
    let mut out = String::from("Debug adapters\n");
    let user = crate::engine_setup::read_debug_adapters().unwrap_or_else(|error| {
        let _ = writeln!(
            out,
            "  FAIL  debug-adapters.json is not valid, so none of it applies: {error}"
        );
        Vec::new()
    });
    for (adapter, from_config) in crate::engine_setup::merged_debug_adapters(&user) {
        let (state, advice) = adapter_state(&adapter.spec.command);
        let origin = if from_config { "  (debug-adapters.json)" } else { "" };
        let _ = writeln!(
            out,
            "  {state} {:<12} {}{origin}{advice}",
            adapter.spec.id,
            adapter.spec.command.join(" ")
        );
    }
    let _ = writeln!(out, "  (`debug` is offered to the model only where an adapter resolves)");
    out
}

/// Whether an adapter's `command` will start, and, when it is there but will not, what to do: the
/// same judgment that decides whether `debug` is offered.
fn adapter_state(command: &[String]) -> (&'static str, String) {
    use luxe_dap::adapter::Unready;
    match luxe_dap::adapter::unready(command) {
        None => ("ok   ", String::new()),
        Some(why @ Unready::NoModule(..)) => ("none ", format!("  ({why})")),
        Some(Unready::Missing(_)) => ("none ", String::new()),
    }
}

/// The external programs the tools prefer, and what happens without each.
fn external_programs() -> String {
    /// Program, what wants it, and what absence costs.
    const PROGRAMS: &[(&str, &str, &str)] = &[
        ("ast-grep", "ast_grep", "the tool is unavailable; grep is the substitute"),
        ("git", "git", "the tool is unavailable"),
        ("gh", "git (pull requests)", "PR actions are unavailable"),
    ];

    let mut out = String::from("External programs\n");
    for (program, wanted_by, without) in PROGRAMS {
        match luxe_core::program::resolve(program).is_some() {
            true => {
                let _ = writeln!(out, "  ok    {program:<10} {wanted_by}");
            }
            false => {
                let _ = writeln!(out, "  none  {program:<10} {wanted_by} — {without}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The checkup finishes.
    ///
    /// It starts real language servers, which is the point and also the risk: a checkup that prints
    /// every line of its report and then does not come down was seen once and never reproduced
    /// after the probe moved into its own function, so the cause is **not known**. The property is
    /// asserted directly rather than assumed, and this is what will catch it coming back.
    ///
    /// Bounded well above the ~0.4s a six-language checkup actually costs on a developer box, so a
    /// slow CI runner cannot make it flaky; a hang is unbounded and fails it regardless.
    #[tokio::test]
    async fn the_checkup_terminates() {
        let registry = ServerRegistry::new();
        let started = std::time::Instant::now();
        for (language, entry) in registry.iter() {
            let (_working, _refusals, _absent) = probe_language(language, entry).await;
        }
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(90),
            "probing every language took {elapsed:?}; a checkup that does not finish is worse \
             than no checkup"
        );
    }

    /// Every language reports one of the three states, and a language with alternatives names
    /// them all when none is present — "install typescript-language-server" is wrong advice for a
    /// TypeScript 7 project.
    #[test]
    fn a_language_with_alternatives_names_all_of_them() {
        let registry = ServerRegistry::new();
        let typescript = registry.get("typescript").expect("typescript is a built-in");
        let candidates: Vec<String> =
            typescript.candidates().into_iter().map(|candidate| candidate.command).collect();
        assert!(candidates.len() > 1, "typescript needs a fallback: {candidates:?}");
        assert_eq!(candidates.first().map(String::as_str), Some("typescript-language-server"));
        assert!(
            candidates.iter().any(|command| command == "tsgo" || command == "tsc"),
            "the native TypeScript 7 server has to be reachable: {candidates:?}"
        );
    }
}
