//! Terminal login interaction — the [`AuthInteraction`] behind `luxe auth login`.
//!
//! Everything a login needs from a plain terminal: an echo-suppressed secret prompt, an arrow-key
//! chooser, a browser launch, and progress lines. The flow logic lives in `luxe-providers`; this is
//! only the front-end half of it, and the TUI supplies its own.
//!
//! Writes to **stderr**, not stdout, so `luxe auth list | grep` stays scriptable and a login run
//! inside a pipeline doesn't corrupt it.

use std::io::{IsTerminal, Write};

use luxe_core::auth::{AuthError, AuthEvent, AuthInteraction, AuthPrompt, AuthPromptKind};
use secrecy::SecretString;

#[derive(Debug, Default)]
pub(crate) struct TerminalAuth {
    /// A key supplied on the command line, answering the first secret prompt without asking.
    api_key: Option<SecretString>,
    /// Answer every prompt from stdin lines, no raw mode (a pipe, a script, `--stdin`).
    non_interactive: bool,
    /// Whether an unanswered prompt is on screen.
    ///
    /// A flow can abandon its own question — the loopback callback beats the paste prompt every
    /// time a browser is available — and the cursor is then parked after a `>` that will never be
    /// answered. Progress printed there lands *on* the prompt line; this lets it start a new one.
    prompt_open: std::sync::atomic::AtomicBool,
}

impl TerminalAuth {
    /// Interactive by default; falls back to line reads when stdin isn't a terminal.
    pub(crate) fn new() -> Self {
        Self {
            api_key: None,
            non_interactive: !std::io::stdin().is_terminal(),
            prompt_open: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Pre-answer the key prompt (from `--api-key` or `--stdin`).
    pub(crate) fn with_api_key(mut self, key: Option<SecretString>) -> Self {
        self.api_key = key;
        self
    }

    /// Close an abandoned prompt line before writing anything else under it.
    fn close_open_prompt(&self) {
        if self.prompt_open.swap(false, std::sync::atomic::Ordering::Relaxed) {
            let _ = writeln!(std::io::stderr());
        }
    }

    /// Whether this interaction can ask a question at all.
    pub(crate) fn is_interactive(&self) -> bool {
        !self.non_interactive
    }
}

#[async_trait::async_trait]
impl AuthInteraction for TerminalAuth {
    async fn prompt(&self, prompt: AuthPrompt) -> Result<SecretString, AuthError> {
        // A key handed in on the command line answers the first secret prompt, so a scripted login
        // never blocks on a question it can't be asked.
        if matches!(prompt.kind, AuthPromptKind::Secret) {
            if let Some(key) = &self.api_key {
                return Ok(key.clone());
            }
        }

        self.prompt_open.store(true, std::sync::atomic::Ordering::Relaxed);
        let interactive = self.is_interactive();
        let answer = match &prompt.kind {
            AuthPromptKind::Select(choices) => {
                let choices = choices.clone();
                let message = prompt.message.clone();
                spawn_blocking(move || select(&message, &choices, interactive)).await
            }
            AuthPromptKind::Secret => {
                let prompt = prompt.clone();
                spawn_blocking(move || read_line(&prompt, true, interactive)).await
            }
            AuthPromptKind::Text => {
                let prompt = prompt.clone();
                spawn_blocking(move || read_line(&prompt, false, interactive)).await
            }
        };
        self.prompt_open.store(false, std::sync::atomic::Ordering::Relaxed);
        answer
    }

    fn notify(&self, event: AuthEvent) {
        // A question the flow abandoned still owns the cursor; start a fresh line under it.
        self.close_open_prompt();
        let mut err = std::io::stderr();
        match event {
            AuthEvent::Info { message, links } => {
                let _ = writeln!(err, "  {}", style::dim(&message));
                for link in links {
                    let rendered = style::link(&link.url);
                    let _ = match link.label {
                        Some(label) => writeln!(err, "  {label}: {rendered}"),
                        None => writeln!(err, "  {rendered}"),
                    };
                }
            }
            AuthEvent::OpenUrl { url, instructions } => {
                let _ = writeln!(err, "\n  {}", style::bold("Opening your browser…"));
                let _ = writeln!(err, "    {}", style::link(&url));
                if let Some(instructions) = instructions {
                    let _ = writeln!(err, "    {}", style::dim(&instructions));
                }
                open_browser(&url);
            }
            AuthEvent::DeviceCode { user_code, verification_uri, expires_in_secs, .. } => {
                let _ = writeln!(err, "\n  {}", style::bold("Authorize this device"));
                let _ = writeln!(err, "    1. Open {}", style::link(&verification_uri));
                let _ = writeln!(err, "    2. Enter the code {}", style::accent(&user_code));
                let _ = writeln!(
                    err,
                    "       {}",
                    style::dim(&format!("valid for {} minutes", expires_in_secs / 60))
                );
                open_browser(&verification_uri);
                let _ = writeln!(err, "\n  {}", style::dim("Waiting for you to authorize…"));
            }
            AuthEvent::Progress { message } => {
                let _ = writeln!(err, "  {} {message}", style::dim("·"));
            }
        }
        let _ = err.flush();
    }
}

/// Run a blocking terminal read off the async runtime.
async fn spawn_blocking<F>(read: F) -> Result<SecretString, AuthError>
where
    F: FnOnce() -> Result<SecretString, AuthError> + Send + 'static,
{
    match tokio::task::spawn_blocking(read).await {
        Ok(result) => result,
        Err(error) => Err(AuthError::Input(format!("could not read your answer: {error}"))),
    }
}

/// Read one answer, optionally without echoing it.
fn read_line(
    prompt: &AuthPrompt,
    secret: bool,
    interactive: bool,
) -> Result<SecretString, AuthError> {
    let mut err = std::io::stderr();
    let _ = write!(err, "\n  {}", style::bold(&prompt.message));
    if let Some(placeholder) = &prompt.placeholder {
        let hint = match prompt.allow_empty {
            true => format!(" {}", style::dim(&format!("[{placeholder}]"))),
            false => format!(" {}", style::dim(&format!("e.g. {placeholder}"))),
        };
        let _ = write!(err, "{hint}");
    }
    let _ = write!(err, "\n  > ");
    let _ = err.flush();

    let answer = if secret && interactive { read_hidden()? } else { read_stdin_line()? };
    if secret {
        let _ = writeln!(err);
    }
    if answer.trim().is_empty() && !prompt.allow_empty {
        return Err(AuthError::Cancelled);
    }
    Ok(SecretString::from(answer))
}

/// Read a line from stdin, treating EOF as a cancel.
fn read_stdin_line() -> Result<String, AuthError> {
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) => Err(AuthError::Cancelled),
        Ok(_) => Ok(line.trim_end_matches(['\n', '\r']).to_string()),
        Err(error) => Err(AuthError::Input(format!("could not read stdin: {error}"))),
    }
}

/// Read a secret with echo suppressed, showing one dot per character.
///
/// Raw mode is restored on every exit path including the error ones — leaving a terminal in raw mode
/// is the kind of bug people reboot over.
fn read_hidden() -> Result<String, AuthError> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

    crossterm::terminal::enable_raw_mode()
        .map_err(|error| AuthError::Input(format!("could not switch the terminal: {error}")))?;
    let mut buffer = String::new();
    let outcome = loop {
        match event::read() {
            Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => match key.code {
                KeyCode::Enter => break Ok(std::mem::take(&mut buffer)),
                KeyCode::Esc => break Err(AuthError::Cancelled),
                KeyCode::Char('c' | 'd') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    break Err(AuthError::Cancelled)
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let erase = "\u{8} \u{8}".repeat(buffer.chars().count());
                    buffer.clear();
                    let _ = write!(std::io::stderr(), "{erase}");
                    let _ = std::io::stderr().flush();
                }
                KeyCode::Backspace => {
                    if buffer.pop().is_some() {
                        let _ = write!(std::io::stderr(), "\u{8} \u{8}");
                        let _ = std::io::stderr().flush();
                    }
                }
                KeyCode::Char(ch) => {
                    buffer.push(ch);
                    let _ = write!(std::io::stderr(), "•");
                    let _ = std::io::stderr().flush();
                }
                _ => {}
            },
            Ok(Event::Paste(text)) => {
                let dots = "•".repeat(text.chars().count());
                buffer.push_str(&text);
                let _ = write!(std::io::stderr(), "{dots}");
                let _ = std::io::stderr().flush();
            }
            Ok(_) => {}
            Err(error) => break Err(AuthError::Input(format!("could not read a key: {error}"))),
        }
    };
    let _ = crossterm::terminal::disable_raw_mode();
    outcome
}

/// Present a numbered list and read one choice.
fn select(
    message: &str,
    choices: &[luxe_core::auth::AuthChoice],
    interactive: bool,
) -> Result<SecretString, AuthError> {
    if choices.is_empty() {
        return Err(AuthError::Input("nothing to choose from".to_string()));
    }
    // A scripted run takes the first (documented as the default) option rather than hanging.
    if !interactive {
        return Ok(SecretString::from(choices[0].id.clone()));
    }

    let mut err = std::io::stderr();
    let _ = writeln!(err, "\n  {}", style::bold(message));
    for (index, choice) in choices.iter().enumerate() {
        let _ = writeln!(err, "    {}) {}", index + 1, choice.label);
        if let Some(detail) = &choice.detail {
            let _ = writeln!(err, "       {}", style::dim(detail));
        }
    }
    loop {
        let _ = write!(err, "  > ");
        let _ = err.flush();
        let answer = read_stdin_line()?;
        let trimmed = answer.trim();
        if trimmed.is_empty() {
            // Enter takes the first option; echo it, or the transcript shows an unanswered prompt.
            let _ = writeln!(err, "  {}", style::dim(&format!("→ {}", choices[0].label)));
            return Ok(SecretString::from(choices[0].id.clone()));
        }
        // Accept the number or the id itself, so `1` and `device` both work.
        if let Some(choice) = trimmed
            .parse::<usize>()
            .ok()
            .filter(|index| *index >= 1 && *index <= choices.len())
            .map(|index| &choices[index - 1])
            .or_else(|| choices.iter().find(|choice| choice.id == trimmed))
        {
            return Ok(SecretString::from(choice.id.clone()));
        }
        let _ = writeln!(err, "  {}", style::dim(&format!("Pick 1–{}.", choices.len())));
    }
}

/// Terminal styling, applied only where it's welcome.
///
/// Color is opt-out in three directions: a redirected stderr (a log file shouldn't carry escapes),
/// `NO_COLOR` (the convention), and `TERM=dumb`. Every helper degrades to the plain string, so the
/// output reads the same either way.
pub(crate) mod style {
    use std::io::IsTerminal;

    /// Whether to emit SGR sequences at all.
    fn enabled() -> bool {
        static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ENABLED.get_or_init(|| {
            std::io::stderr().is_terminal()
                && std::env::var_os("NO_COLOR").is_none()
                && std::env::var("TERM").map(|term| term != "dumb").unwrap_or(true)
        })
    }

    fn paint(code: &str, text: &str) -> String {
        match enabled() {
            true => format!("\u{1b}[{code}m{text}\u{1b}[0m"),
            false => text.to_string(),
        }
    }

    /// A heading.
    pub(crate) fn bold(text: &str) -> String {
        paint("1", text)
    }

    /// Secondary detail.
    pub(crate) fn dim(text: &str) -> String {
        paint("2", text)
    }

    /// The one thing on the line the user must act on (a device code).
    pub(super) fn accent(text: &str) -> String {
        paint("1;36", text)
    }

    /// A completed step.
    pub(crate) fn success(text: &str) -> String {
        paint("32", text)
    }

    /// A URL, rendered for whoever is reading.
    ///
    /// In a terminal: a clickable OSC-8 link labeled with the host and path, because the full
    /// authorize URL is four wrapped lines of query string that nobody reads. Anywhere else: the
    /// whole URL, because a log or a pipe has no other way to recover it.
    pub(super) fn link(url: &str) -> String {
        if !enabled() {
            return url.to_string();
        }
        let label = short_url(url);
        let painted = paint("4;36", &label);
        format!("\u{1b}]8;;{url}\u{7}{painted}\u{1b}]8;;\u{7}")
    }

    /// `https://host/path…` — enough to recognize where a link goes, without the query string.
    fn short_url(url: &str) -> String {
        match url.split_once('?') {
            Some((head, _)) => format!("{head}…"),
            None => url.to_string(),
        }
    }
}

/// Best-effort browser launch. A failure is fine — the URL is printed either way.
fn open_browser(url: &str) {
    // Only ever an http(s) URL from a flow we built; never user input handed to a shell.
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return;
    }
    let candidates: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("open", &[])]
    } else if cfg!(target_os = "windows") {
        &[("cmd", &["/C", "start", ""])]
    } else {
        &[("xdg-open", &[]), ("gio", &["open"]), ("wslview", &[])]
    };
    for (command, args) in candidates {
        let launched = std::process::Command::new(command)
            .args(*args)
            .arg(url)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if launched.is_ok() {
            return;
        }
    }
    tracing::debug!("no browser launcher available; the URL was printed instead");
}

#[cfg(test)]
mod tests {
    use super::*;
    use luxe_core::auth::AuthChoice;

    fn choices() -> Vec<AuthChoice> {
        vec![
            AuthChoice { id: "browser".into(), label: "Browser".into(), detail: None },
            AuthChoice { id: "device".into(), label: "Device code".into(), detail: None },
        ]
    }

    /// A scripted `luxe auth login` must not hang on a question; it takes the leading option, which
    /// is why flows list their default first.
    #[test]
    fn a_non_interactive_select_takes_the_first_option() {
        let answer = select("pick", &choices(), false).expect("resolves");
        assert_eq!(secrecy::ExposeSecret::expose_secret(&answer), "browser");
    }

    #[test]
    fn an_empty_choice_list_is_an_error_rather_than_a_panic() {
        assert!(select("pick", &[], false).is_err());
    }

    #[tokio::test]
    async fn a_supplied_key_answers_the_secret_prompt_without_a_terminal() {
        let auth =
            TerminalAuth::new().with_api_key(Some(SecretString::from("sk-supplied".to_string())));
        let answer = auth
            .prompt(AuthPrompt::secret("Key:", None))
            .await
            .expect("answered from the command line");
        assert_eq!(secrecy::ExposeSecret::expose_secret(&answer), "sk-supplied");
    }

    /// The launcher takes URLs from our own flows, but a guard costs nothing and forecloses passing
    /// anything else to a shell-adjacent command.
    #[test]
    fn only_http_urls_are_handed_to_a_launcher() {
        open_browser("file:///etc/passwd");
        open_browser("; rm -rf /");
    }
}
