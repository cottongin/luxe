//! Secret redaction — scrub known-secret patterns and resolved literals from what a sink keeps.
//!
//! One implementation for every sink: tool results at dispatch, the log writer, the wire capture.
//! Patterns catch a credential by its published shape; [`SecretSet`] catches the ones with no shape
//! at all (a custom provider's key, a `!command` secret, an MCP bearer) because the process that
//! resolved them registered the literal.

use regex::Regex;
use secrecy::{ExposeSecret, SecretString};
use std::sync::RwLock;

/// Shortest literal [`SecretSet`] keeps. Below it a value is as likely a word as a key, and
/// redacting a common word out of every tool result costs more than it protects.
const MIN_SECRET_CHARS: usize = 8;

/// Literal secrets to scrub, kept as `SecretString`s so `Debug` never prints them.
#[derive(Default)]
pub struct SecretSet {
    literals: RwLock<Vec<SecretString>>,
}

impl SecretSet {
    /// Remember `secret`, trimmed; shorter than `MIN_SECRET_CHARS` or already known is a no-op.
    pub fn add(&self, secret: &str) {
        let secret = secret.trim();
        if secret.chars().count() < MIN_SECRET_CHARS {
            return;
        }
        let mut literals = self.literals.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !literals.iter().any(|known| known.expose_secret() == secret) {
            literals.push(SecretString::from(secret.to_string()));
        }
    }

    /// [`redact_secrets`] with every literal in the set.
    pub fn redact(&self, text: &str) -> String {
        let literals = self.literals.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        let exposed: Vec<&str> = literals.iter().map(ExposeSecret::expose_secret).collect();
        redact_with(text, &exposed)
    }
}

impl std::fmt::Debug for SecretSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.literals.read().map(|literals| literals.len()).unwrap_or_default();
        write!(f, "SecretSet({count} redacted)")
    }
}

/// The credentials this process has resolved.
///
/// Process-wide on purpose: keys and tokens are resolved per request, deep in the provider and MCP
/// crates, while every place that redacts sits in another crate, and all of them need every one.
pub fn process_secrets() -> &'static SecretSet {
    static SECRETS: std::sync::LazyLock<SecretSet> = std::sync::LazyLock::new(SecretSet::default);
    &SECRETS
}

/// Redact secrets from a string: the given literals, then every known pattern.
pub fn redact_secrets(text: &str, configured_secrets: &[String]) -> String {
    let literals: Vec<&str> = configured_secrets.iter().map(String::as_str).collect();
    redact_with(text, &literals)
}

fn redact_with(text: &str, literals: &[&str]) -> String {
    let mut result = text.to_string();
    // longest first, so a secret that contains another is not half-replaced
    let mut sorted = literals.to_vec();
    sorted.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    for secret in sorted.into_iter().filter(|secret| !secret.is_empty()) {
        result = result.replace(secret, "[REDACTED]");
    }
    redact_patterns(&result)
}

/// How much a match is worth acting on, as opposed to hiding.
///
/// Redaction treats every pattern alike — over-redacting costs nothing. Anything that *warns* a
/// human has the opposite economics, and the two ends of this list are not remotely equally
/// trustworthy: `AKIA...` is a credential, while the `key: value` catch-all matches the Rust
/// parameter `token: &str`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretConfidence {
    /// A published credential format — the shape alone is close to proof.
    Format,
    /// A secret-ish name next to a value; ordinary code hits this constantly.
    Shape,
}

/// Every secret-shaped substring in `text`, with how much each match is worth acting on.
///
/// Redaction only needs to know that *something* matched; a reviewer-facing warning needs the token
/// itself, because a placeholder and a live credential have the same shape and very different
/// consequences.
pub fn secret_shaped_spans(text: &str) -> Vec<(String, SecretConfidence)> {
    let mut spans = Vec::new();
    for (pattern, _, confidence) in SECRET_PATTERNS.iter() {
        spans.extend(pattern.find_iter(text).map(|found| (found.as_str().to_owned(), *confidence)));
    }
    spans.extend(
        ASSIGNMENT
            .captures_iter(text)
            .filter(|caps| could_be_a_credential(caps))
            .map(|caps| (caps[0].to_owned(), SecretConfidence::Shape)),
    );
    spans
}

/// Redact known secret patterns from a string.
fn redact_patterns(text: &str) -> String {
    let mut result = std::borrow::Cow::Borrowed(text);
    for (pattern, replacement, _) in SECRET_PATTERNS.iter() {
        if let std::borrow::Cow::Owned(replaced) = pattern.replace_all(&result, *replacement) {
            result = std::borrow::Cow::Owned(replaced);
        }
    }
    redact_assignments(&result)
}

/// Blank the value out of `password=…`, `"api_key": "…"` and friends.
///
/// The one pattern here with no published format behind it, so it is the one that has to guess —
/// and it is applied to **tool output**, which for a coding agent is mostly source code. Guessing
/// wrong in that direction is not free the way over-redacting a log is: it hands the model
/// corrupted code: matching any `key`-ish word next to any `:` or `=` turns
/// `CancellationToken::cancelled()` into `CancellationToken:[REDACTED]`.
///
/// Kept whole by [`could_be_a_credential`], which asks the question the shape can actually answer:
/// could this value be a credential *at all*, or is it plainly a type name?
fn redact_assignments(text: &str) -> String {
    ASSIGNMENT
        .replace_all(text, |caps: &regex::Captures<'_>| match could_be_a_credential(caps) {
            true => format!("{}{}{}[REDACTED]", &caps[1], &caps[2], &caps[3]),
            false => caps[0].to_string(),
        })
        .into_owned()
}

/// Whether a matched assignment's value could be a secret rather than a fragment of code.
///
/// Three ways to qualify, and a value needs one of them:
///
/// - **a digit**, which every issued credential has and a type name does not — this is what keeps
///   `token: Option`, `api_key: String` and `secret: ApiKey` whole;
/// - **quotes**, which is how a config file writes one;
/// - **a SHOUTING key**, which is how an environment holds one.
///
/// What it gives up is a lowercase, unquoted, digit-free password — a value with the shape of a
/// word, which no issued key has and which nothing can catch without also catching `let token =
/// editor.buffer`. A caller holding that literal passes it in `configured_secrets`.
fn could_be_a_credential(caps: &regex::Captures<'_>) -> bool {
    let (key, separator, value) = (&caps[2], &caps[3], &caps[4]);
    value.chars().any(|c| c.is_ascii_digit())
        || separator.contains(['"', '\''])
        || !key.chars().any(|c| c.is_ascii_lowercase())
}

/// `key = value` for a secret-ish key, split into prefix / key / separator / value.
///
/// The value's alphabet is the point: it admits everything a credential is written in (base64,
/// base64url, hex, and the punctuation a human password uses) and refuses the characters that only
/// appear in code — `&`, `<`, `>`, `(`, `)`, `:`, `;`, `,`. That single choice is what stops a Rust
/// path (`Secret::expose`), a match arm (`ApiKey =>`), a reference (`token: &str`) and a generic
/// (`token: Option<String>`) from ever reaching the guess above. The leading group is a stand-in
/// for the lookbehind this regex engine does not have: a keyword glued to a preceding letter or
/// digit is part of a longer identifier (`CancellationToken`), while one after `_` is a field name
/// in its own right (`refresh_token`).
static ASSIGNMENT: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(concat!(
        r#"(?i)(^|[^A-Za-z0-9])"#,
        r#"(password|passwd|pwd|secret|token|api[-_]?key|access[-_]?key|auth[-_]?token)"#,
        r#"(["']?\s*[:=]\s*["']?)"#,
        r#"([A-Za-z0-9_\-./+=~!@#$%^*]{6,})"#,
    ))
    .expect("a literal pattern that is compiled by the test suite on every run")
});

/// Every secret pattern: the regex, its replacement, how far to trust a match, and an example it
/// must catch (pinned by a test, so a typo cannot quietly drop a pattern).
const PATTERNS: [(&str, &str, SecretConfidence, &str); 11] = [
    // OpenAI-style API keys (`sk-`), which covers Anthropic's `sk-ant-` too.
    //
    // The token body has to admit `-` and `_`: every current key format segments itself
    // (`sk-ant-api03-…`, `sk-proj-…`, `sk-svcacct-…`), and an alphanumeric-only run stops dead at
    // the first hyphen — `sk-ant-api03-…` got no further than `api03` and passed through in the
    // clear.
    (
        r"sk-[a-zA-Z0-9_-]{20,}",
        "[REDACTED_API_KEY]",
        SecretConfidence::Format,
        "sk-ant-api03-abcdefghijklmnopqrst",
    ),
    (
        r"(?i)bearer\s+[a-zA-Z0-9\-._~+/]+=*",
        "Bearer [REDACTED]",
        SecretConfidence::Format,
        "Authorization: Bearer abc.def-ghi",
    ),
    (
        r"gh[pous]_[a-zA-Z0-9]{20,}",
        "[REDACTED_GITHUB_TOKEN]",
        SecretConfidence::Format,
        "ghp_abcdefghijklmnopqrstuv",
    ),
    (
        r"github_pat_[a-zA-Z0-9]{20,}",
        "[REDACTED_GITHUB_TOKEN]",
        SecretConfidence::Format,
        "github_pat_abcdefghijklmnopqrstuv",
    ),
    (
        r"xox[baprs]-[a-zA-Z0-9\-]+",
        "[REDACTED_SLACK_TOKEN]",
        SecretConfidence::Format,
        "xoxb-1234-abcd",
    ),
    (r"AKIA[0-9A-Z]{16}", "[REDACTED_AWS_KEY]", SecretConfidence::Format, "AKIAABCDEFGHIJKLMNOP"),
    (
        r"eyJ[a-zA-Z0-9_-]*\.eyJ[a-zA-Z0-9_-]*\.[a-zA-Z0-9_-]*",
        "[REDACTED_JWT]",
        SecretConfidence::Format,
        "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2ln",
    ),
    (
        r"AIza[0-9A-Za-z_\-]{35}",
        "[REDACTED_GOOGLE_KEY]",
        SecretConfidence::Format,
        "AIzaabcdefghijklmnopqrstuvwxyz012345678",
    ),
    (
        r"(?:sk|rk)_(?:live|test)_[0-9A-Za-z]{10,}",
        "[REDACTED_STRIPE_KEY]",
        SecretConfidence::Format,
        "rk_live_abcdefghij12",
    ),
    // the whole armored block, for RSA, EC, OPENSSH and PKCS8 alike
    (
        r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
        "[REDACTED_PRIVATE_KEY]",
        SecretConfidence::Format,
        "-----BEGIN RSA PRIVATE KEY-----\nMIIabc\n-----END RSA PRIVATE KEY-----",
    ),
    // credentials embedded in a URL: keep the scheme
    (
        r"([a-zA-Z][a-zA-Z0-9+.\-]*://)[^\s:/@]+:[^\s/@]+@",
        "${1}[REDACTED]@",
        SecretConfidence::Format,
        "https://user:pass@example.com",
    ),
];

/// [`PATTERNS`], compiled once.
///
/// `redact_secrets` runs at the engine's tool-result choke point, on every tracing event and over
/// review prompts, so recompiling thirteen regexes per string costs whole seconds on a large
/// review.
static SECRET_PATTERNS: std::sync::LazyLock<Vec<(Regex, &'static str, SecretConfidence)>> =
    std::sync::LazyLock::new(|| {
        PATTERNS
            .iter()
            .map(|(pattern, replacement, confidence, _)| {
                let regex = Regex::new(pattern).expect("a literal pattern the test suite compiles");
                (regex, *replacement, *confidence)
            })
            .collect()
    });

#[cfg(test)]
mod tests;
