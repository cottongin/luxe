//! `luxe auth` — log in, list, inspect, and log out, without opening the TUI.
//!
//! The CLI half of onboarding. Same flow engine the `/login` wizard drives, so anything that works
//! here works there; this surface exists for headless boxes, SSH sessions, and scripts.

use crate::output::{out, outln};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use luxe_core::auth::{
    AuthError, AuthMethodKind, AuthService, CredentialStatus, LoginOutcome, ProviderOption,
};
use luxe_providers::auth::ProviderAuth;
use secrecy::SecretString;

use super::args::AuthCommand;
use crate::auth_terminal::{style, TerminalAuth};

/// Run an `auth` subcommand.
pub(crate) async fn run_auth_command(command: AuthCommand) -> Result<()> {
    match command {
        AuthCommand::Login { provider, method, api_key, stdin, no_verify } => {
            login(provider, method, api_key, stdin, no_verify).await
        }
        AuthCommand::List => list(),
        AuthCommand::Status { provider } => status(provider),
        AuthCommand::Logout { provider } => logout(provider),
    }
}

/// `luxe auth login [provider]`.
async fn login(
    provider: Option<String>,
    method: Option<String>,
    api_key: Option<String>,
    read_stdin: bool,
    no_verify: bool,
) -> Result<()> {
    let service = match no_verify {
        true => ProviderAuth::new().without_verification(),
        false => ProviderAuth::new(),
    };

    let key = match (api_key, read_stdin) {
        (Some(_), true) => bail!("pass either --api-key or --stdin, not both"),
        (Some(key), false) => Some(SecretString::from(key)),
        (None, true) => Some(read_key_from_stdin()?),
        (None, false) => None,
    };
    let interaction = TerminalAuth::new().with_api_key(key.clone());

    // Choosing a provider is a question, so a non-interactive run has to be told which one.
    let provider = match provider {
        Some(provider) => provider,
        None if interaction.is_interactive() => choose_provider(&service)?,
        None => bail!(
            "which provider? pass one (e.g. `luxe auth login openai`); run `luxe auth list` to \
             see them all"
        ),
    };

    let option = service.provider(&provider).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown provider '{provider}' — run `luxe auth list` to see the supported ones"
        )
    })?;
    let method_id = resolve_method(&option, method.as_deref(), key.is_some(), &interaction)?;

    eprintln!("\n{}", style::bold(&format!("Signing in to {}", option.name)));
    let interaction: Arc<dyn luxe_core::auth::AuthInteraction> = Arc::new(interaction);
    match service.login(&option.id, &method_id, interaction).await {
        Ok(outcome) => {
            report_success(&service, &outcome);
            Ok(())
        }
        Err(AuthError::Cancelled) => {
            eprintln!("\nCancelled. Nothing was saved.");
            std::process::exit(1);
        }
        Err(error) => Err(anyhow::anyhow!(error)).context(format!("signing in to {}", option.name)),
    }
}

/// Print the confirmation, and say what to do next.
///
/// Aligned labels rather than prose: after a multi-step login the useful thing is a scannable
/// summary of what now exists — which account, which models, which file.
fn report_success(service: &ProviderAuth, outcome: &LoginOutcome) {
    // A local server stores no credential, so it was connected rather than signed in to, and
    // there is no kind to name.
    let (verb, kind) = match outcome.kind {
        Some(kind) => ("Signed in to", format!(" {}", style::dim(&format!("({})", kind.label())))),
        None => ("Connected to", String::new()),
    };
    outln!(
        "\n{} {}{kind}",
        style::success("✓"),
        style::bold(&format!("{verb} {}", outcome.provider_name)),
    );
    let field = |label: &str, value: &str| outln!("  {:<9} {value}", style::dim(label));
    if let Some(account) = &outcome.account {
        field("account", account);
    }
    for note in &outcome.notes {
        field("models", note);
    }
    field("saved to", &luxe_core::paths::home_relative(&service.store_path()));
    if let Some(model) = &outcome.suggested_model {
        field("model", &format!("{model}  {}", style::dim("(now the default)")));
    }
    // Advice sits apart from the summary: it's about what to do later, not what just happened.
    for advice in &outcome.advice {
        outln!("\n  {}", style::dim(advice));
    }
    match outcome.suggested_model {
        Some(_) => outln!("\nRun `luxe` to start, or `/model` to pick another."),
        None => outln!("\nRun `luxe` to start, then `/model` to pick a model."),
    }
}

/// `luxe auth list` — every provider and how it's configured.
fn list() -> Result<()> {
    let service = ProviderAuth::new();
    let options = service.providers();
    let width = options.iter().map(|option| option.id.len()).max().unwrap_or(8);

    outln!("Providers ({}):\n", service.store_path().display());
    for option in &options {
        let methods: Vec<&str> = option
            .methods
            .iter()
            .map(|method| match method.kind {
                AuthMethodKind::Subscription => "subscription",
                AuthMethodKind::ApiKey if option.keyless => "local",
                AuthMethodKind::ApiKey => "api key",
            })
            .collect();
        outln!(
            "  {mark} {id:<width$}  {status:<26}  {name} [{methods}]",
            mark = if option.status.is_configured() { "●" } else { "○" },
            id = option.id,
            status = describe_option(option),
            name = option.name,
            methods = methods.join(", "),
        );
    }
    outln!("\n  ● configured   ○ not configured");
    outln!("  Sign in with `luxe auth login <provider>`.");
    Ok(())
}

/// `luxe auth status [provider]` — the detail, including OAuth expiry.
fn status(provider: Option<String>) -> Result<()> {
    let service = ProviderAuth::new();
    let options: Vec<ProviderOption> = match &provider {
        Some(id) => vec![service
            .provider(id)
            .ok_or_else(|| anyhow::anyhow!("unknown provider '{id}' — run `luxe auth list`"))?],
        None => service.providers().into_iter().filter(|o| o.status.is_configured()).collect(),
    };

    if options.is_empty() {
        outln!("No provider is configured yet. Run `luxe auth login` to set one up.");
        return Ok(());
    }

    for option in options {
        outln!("{} ({})", option.name, option.id);
        outln!("  Credential: {}", describe_option(&option));
        if let CredentialStatus::Stored(info) = &option.status {
            if let Some(expires) = info.expires_at_ms {
                let now = now_ms();
                outln!(
                    "  Expires:    {}",
                    match expires.checked_sub(now) {
                        Some(remaining) if remaining > 0 =>
                            format!("in {}", human_duration(remaining / 1000)),
                        _ => "expired — refreshes on the next request".to_string(),
                    }
                );
            }
            if let Some(account) = &info.account {
                outln!("  Account:    {account}");
            }
        }
        if let Some(note) = &option.note {
            outln!("  Note:       {note}");
        }
        outln!("  File:       {}", service.store_path().display());
        outln!();
    }
    Ok(())
}

/// `luxe auth logout <provider>`.
fn logout(provider: String) -> Result<()> {
    let service = ProviderAuth::new();
    let name = service.provider(&provider).map(|option| option.name).unwrap_or(provider.clone());
    match service.logout(&provider)? {
        true => {
            outln!("✓ Removed the stored credential for {name}.");
            let ambient = service
                .provider(&provider)
                .map(|option| option.status)
                .filter(CredentialStatus::is_configured);
            if let Some(CredentialStatus::Env(name)) = ambient {
                outln!("  Note: {name} is still set in your environment, so it will be used.");
            }
            Ok(())
        }
        false => {
            // A key that lives somewhere else isn't ours to remove, but silence would read as a
            // failed logout when the provider plainly still works.
            match service.provider(&provider).map(|option| option.status) {
                Some(CredentialStatus::Env(variable)) => outln!(
                    "Nothing stored for {name} — it's configured by {variable} in your environment."
                ),
                Some(CredentialStatus::Config(file)) => outln!(
                    "Nothing stored for {name} — its key is written in {file}; edit that to remove it."
                ),
                _ => outln!("Nothing stored for {name}."),
            }
            Ok(())
        }
    }
}

/// Ask which provider, listing the configured ones first.
fn choose_provider(service: &ProviderAuth) -> Result<String> {
    let options = service.providers();
    outln!("\nWhich provider?\n");
    for (index, option) in options.iter().enumerate() {
        let subscription =
            if option.methods.iter().any(|method| method.kind == AuthMethodKind::Subscription) {
                " (subscription available)"
            } else {
                Default::default()
            };
        outln!(
            "  {:>2}) {}{}{}",
            index + 1,
            option.name,
            subscription,
            if option.status.is_configured() { "  — configured" } else { "" }
        );
    }
    loop {
        out!("\n  > ");
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
            bail!("cancelled");
        }
        let answer = line.trim();
        if answer.is_empty() {
            bail!("cancelled");
        }
        if let Some(option) = answer
            .parse::<usize>()
            .ok()
            .filter(|index| *index >= 1 && *index <= options.len())
            .map(|index| &options[index - 1])
            .or_else(|| {
                options
                    .iter()
                    .find(|option| option.id == answer || option.name.eq_ignore_ascii_case(answer))
            })
        {
            return Ok(option.id.clone());
        }
        outln!("  Pick 1–{}, or type a provider id.", options.len());
    }
}

/// Which login method to run: what was asked for, the only one there is, or the user's pick.
fn resolve_method(
    option: &ProviderOption,
    requested: Option<&str>,
    have_key: bool,
    interaction: &TerminalAuth,
) -> Result<String> {
    if let Some(requested) = requested {
        let wanted = normalize_method(requested);
        return match option.method(&wanted) {
            Some(method) => Ok(method.id.clone()),
            None => bail!(
                "{} has no '{requested}' login; it supports: {}",
                option.name,
                option.methods.iter().map(|m| m.id.as_str()).collect::<Vec<_>>().join(", ")
            ),
        };
    }
    // A key on the command line says what the user meant.
    if have_key {
        if let Some(method) = option.method("api-key") {
            return Ok(method.id.clone());
        }
    }
    if let Some(method) = option.default_method() {
        return Ok(method.id.clone());
    }
    if !interaction.is_interactive() {
        bail!(
            "{} supports several login methods; pass --method <{}>",
            option.name,
            option.methods.iter().map(|m| m.id.as_str()).collect::<Vec<_>>().join("|")
        );
    }

    outln!("\nHow do you want to sign in to {}?\n", option.name);
    for (index, method) in option.methods.iter().enumerate() {
        outln!("  {}) {}", index + 1, method.label);
        if let Some(detail) = &method.detail {
            outln!("     {detail}");
        }
    }
    loop {
        out!("\n  > ");
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).unwrap_or(0) == 0 {
            bail!("cancelled");
        }
        let answer = line.trim();
        if answer.is_empty() {
            return Ok(option.methods[0].id.clone());
        }
        if let Some(method) = answer
            .parse::<usize>()
            .ok()
            .filter(|index| *index >= 1 && *index <= option.methods.len())
            .map(|index| &option.methods[index - 1])
            .or_else(|| option.method(&normalize_method(answer)))
        {
            return Ok(method.id.clone());
        }
        outln!("  Pick 1–{}.", option.methods.len());
    }
}

/// Accept the spellings people actually type for a method.
fn normalize_method(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "subscription" | "oauth" | "sub" | "login" => "oauth".to_string(),
        "api-key" | "apikey" | "key" | "api" => "api-key".to_string(),
        other => other.to_string(),
    }
}

/// Read a key piped in on stdin.
fn read_key_from_stdin() -> Result<SecretString> {
    use std::io::Read;
    let mut buffer = String::new();
    std::io::stdin().read_to_string(&mut buffer).context("reading the key from stdin")?;
    let key = buffer.trim();
    if key.is_empty() {
        bail!("stdin was empty — pipe the key in, e.g. `pass show openai | luxe auth login openai --stdin`");
    }
    Ok(SecretString::from(key.to_string()))
}

/// How a provider resolves, for a person reading a table.
///
/// A local server that needs no key isn't "unconfigured" — saying so would send someone looking for
/// a credential that doesn't exist.
fn describe_option(option: &ProviderOption) -> String {
    match (&option.status, option.keyless) {
        (CredentialStatus::Missing, true) => "no credential needed".to_string(),
        (status, _) => describe_status(status),
    }
}

/// One-line description of how a provider's credential resolves.
fn describe_status(status: &CredentialStatus) -> String {
    match status {
        CredentialStatus::Missing => "—".to_string(),
        CredentialStatus::Env(name) => format!("env: {name}"),
        CredentialStatus::Config(file) => format!("config: {file}"),
        CredentialStatus::Local(endpoint) => format!("local server: {endpoint}"),
        CredentialStatus::Stored(info) => match info.kind {
            luxe_core::auth::CredentialKind::ApiKey => "api key (saved)".to_string(),
            luxe_core::auth::CredentialKind::OAuth => "subscription (saved)".to_string(),
        },
    }
}

fn human_duration(seconds: u64) -> String {
    match seconds {
        0..=90 => format!("{seconds}s"),
        91..=5400 => format!("{}m", seconds / 60),
        _ => format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use luxe_core::auth::{AuthMethod, StoredCredentialInfo};

    fn option(methods: Vec<AuthMethod>) -> ProviderOption {
        ProviderOption {
            id: "openai".into(),
            name: "OpenAI".into(),
            methods,
            status: CredentialStatus::Missing,
            note: None,
            console_url: None,
            keyless: false,
            needs_endpoint: false,
        }
    }

    #[test]
    fn method_spellings_are_forgiving() {
        for raw in ["oauth", "OAuth", "subscription", "sub", "login"] {
            assert_eq!(normalize_method(raw), "oauth", "{raw}");
        }
        for raw in ["api-key", "api_key", "apikey", "KEY", "api"] {
            assert_eq!(normalize_method(raw), "api-key", "{raw}");
        }
        assert_eq!(normalize_method("weird"), "weird");
    }

    #[test]
    fn a_single_method_needs_no_question() {
        let option = option(vec![AuthMethod::api_key(None)]);
        let resolved =
            resolve_method(&option, None, false, &TerminalAuth::new()).expect("unambiguous");
        assert_eq!(resolved, "api-key");
    }

    #[test]
    fn a_requested_method_is_validated_against_the_provider() {
        let option = option(vec![AuthMethod::api_key(None)]);
        let error = resolve_method(&option, Some("oauth"), false, &TerminalAuth::new())
            .expect_err("must refuse");
        assert!(format!("{error}").contains("no 'oauth' login"), "got {error}");
    }

    /// Passing a key states the intent, so an ambiguous provider doesn't need to ask.
    #[test]
    fn a_supplied_key_selects_the_api_key_method() {
        let option = option(vec![
            AuthMethod::subscription("Sign in with OpenRouter", None),
            AuthMethod::api_key(None),
        ]);
        let resolved =
            resolve_method(&option, None, true, &TerminalAuth::new()).expect("key implies api-key");
        assert_eq!(resolved, "api-key");
    }

    /// A scripted run must fail with instructions rather than block on a prompt it can't show.
    #[test]
    fn an_ambiguous_non_interactive_login_says_what_flag_to_pass() {
        let option =
            option(vec![AuthMethod::subscription("Sign in", None), AuthMethod::api_key(None)]);
        // `TerminalAuth::new()` is non-interactive under the test harness (stdin isn't a tty).
        let interaction = TerminalAuth::new();
        if interaction.is_interactive() {
            return; // running with a tty attached; the prompt path is exercised manually
        }
        let error = resolve_method(&option, None, false, &interaction).expect_err("must not block");
        assert!(format!("{error}").contains("--method"), "got {error}");
    }

    /// A local server that needs no key must not read as "unconfigured".
    #[test]
    fn a_keyless_provider_says_so_instead_of_showing_a_dash() {
        let mut local = option(vec![AuthMethod::api_key(None)]);
        local.keyless = true;
        assert_eq!(describe_option(&local), "no credential needed");
        // A provider that *does* need one still reads as unconfigured.
        let hosted = option(vec![AuthMethod::api_key(None)]);
        assert_eq!(describe_option(&hosted), "—");
    }

    #[test]
    fn status_lines_name_the_source() {
        assert_eq!(describe_status(&CredentialStatus::Missing), "—");
        assert_eq!(
            describe_status(&CredentialStatus::Env("OPENAI_API_KEY".into())),
            "env: OPENAI_API_KEY"
        );
        assert_eq!(
            describe_status(&CredentialStatus::Stored(StoredCredentialInfo {
                kind: luxe_core::auth::CredentialKind::OAuth,
                expires_at_ms: None,
                account: None,
            })),
            "subscription (saved)"
        );
    }

    #[test]
    fn durations_read_like_a_human_wrote_them() {
        assert_eq!(human_duration(45), "45s");
        assert_eq!(human_duration(600), "10m");
        assert_eq!(human_duration(7_500), "2h 5m");
    }
}
