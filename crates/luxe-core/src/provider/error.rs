//! Provider errors — what went wrong with a provider call, and what to do about it.
//!
//! The kind is what the engine acts on: retry, re-authenticate, compact. It is decided from the
//! most structured thing an adapter had — an HTTP status, then the provider's own error type —
//! and only then from the words.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Error kind for provider errors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorKind {
    /// Context window overflow.
    Overflow,
    /// Rate limit hit.
    RateLimit,
    /// Authentication/authorization failed.
    Auth,
    /// The request never reached the provider: connection refused or reset, DNS, TLS, a timeout.
    ///
    /// Distinct from [`Self::Other`] because nothing about the request was wrong, so the reaction is
    /// to try again rather than to change anything.
    Transport,
    /// The provider is up but could not serve this request just now: overloaded, or a 5xx.
    Unavailable,
    /// Other error.
    Other(String),
}

impl ProviderErrorKind {
    /// Whether sending the same request again is the fix: a rate limit, a request that never
    /// reached the provider, or a provider that could not serve it just now.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::RateLimit | Self::Transport | Self::Unavailable)
    }
}

impl fmt::Display for ProviderErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow => write!(f, "context window overflow"),
            Self::RateLimit => write!(f, "rate limit exceeded"),
            Self::Auth => write!(f, "authentication failed"),
            Self::Transport => write!(f, "could not reach the provider"),
            Self::Unavailable => write!(f, "the provider could not serve the request just now"),
            Self::Other(s) => write!(f, "{s}"),
        }
    }
}

/// Provider-level error.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// No usable credential for this provider, or the provider rejected the one we had. Carries a
    /// message that says what to do about it, because "401" does not.
    #[error("{0}")]
    Auth(String),
    /// The provider answered with an HTTP error status.
    ///
    /// Kept as the status and the body rather than one string, so its kind is decided by the
    /// status and only a 4xx without one of its own is read for what it says.
    #[error("provider error: HTTP {status}: {body}")]
    Http {
        /// The HTTP status code
        status: u16,
        /// What the provider said, as it said it
        body: String,
    },
    /// An error the provider reported with a type of its own, in a payload or mid-stream.
    ///
    /// Args: `(kind, message)` — the kind its type maps to, and the provider's words.
    #[error("provider error: {message}")]
    Reported {
        /// What the provider's own error type says to do about it
        kind: ProviderErrorKind,
        /// What the provider said
        message: String,
    },
    /// Anything else the provider refused.
    #[error("provider error: {0}")]
    Other(String),
    /// The stream broke after it started.
    #[error("stream error: {0}")]
    Stream(String),
}

impl ProviderError {
    /// The error a provider reported as `kind`, read from its words when the kind names nothing.
    pub fn reported(kind: ProviderErrorKind, message: String) -> Self {
        match kind {
            ProviderErrorKind::Other(_) => Self::Other(message),
            kind => Self::Reported { kind, message },
        }
    }

    /// Classify for the UI — an auth failure needs a different reaction than a rate limit.
    pub fn kind(&self) -> ProviderErrorKind {
        match self {
            Self::Auth(_) => ProviderErrorKind::Auth,
            Self::Http { status, body } => kind_of_status(*status, body),
            Self::Reported { kind, .. } => kind.clone(),
            Self::Other(message) | Self::Stream(message) => classify_provider_message(message),
        }
    }
}

/// What an HTTP error status says to do, reading the body only for a 4xx that names nothing.
fn kind_of_status(status: u16, body: &str) -> ProviderErrorKind {
    match status {
        401 | 403 => ProviderErrorKind::Auth,
        429 => ProviderErrorKind::RateLimit,
        408 | 500..=599 => ProviderErrorKind::Unavailable,
        400..=499 => classify_provider_message(body),
        _ => ProviderErrorKind::Other("provider".to_string()),
    }
}

/// Whether `code` stands on its own in `text`, rather than inside a longer number.
fn has_code(text: &str, code: &str) -> bool {
    let digit_at = |at: Option<char>| at.is_some_and(|c| c.is_ascii_digit());
    text.match_indices(code).any(|(at, _)| {
        !digit_at(text[..at].chars().next_back())
            && !digit_at(text[at + code.len()..].chars().next())
    })
}

/// Render an error with everything underneath it, for a message a person can act on.
///
/// `reqwest::Error`'s `Display` names the request and stops: "error sending request for url (...)".
/// The reason — `Connection refused`, `dns error`, `operation timed out`, `connection closed before
/// message completed` — lives in its `source` chain, three levels down, and `format!("{e}")` throws
/// all of it away. That left every transport failure in Luxe reporting the same uninformative line,
/// which is exactly the class of failure where the cause is the only useful part.
pub fn describe_error_chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut rendered = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let text = cause.to_string();
        // Providers nest errors that restate their parent; repeating it adds length, not meaning.
        if !rendered.contains(&text) {
            rendered.push_str(": ");
            rendered.push_str(&text);
        }
        source = cause.source();
    }
    rendered
}

/// Best-effort classification of a provider's error text.
///
/// Providers disagree about status codes and bodies, so this reads the message. It only picks a
/// reaction (retry, re-auth, compact); the original text is always what the user sees.
pub fn classify_provider_message(message: &str) -> ProviderErrorKind {
    let lower = message.to_ascii_lowercase();
    // First: an overflow body often quotes a token count, which no status code may be read out of
    if lower.contains("context length")
        || lower.contains("context_length")
        || lower.contains("maximum context")
        || lower.contains("too many tokens")
    {
        return ProviderErrorKind::Overflow;
    }
    if has_code(&lower, "401")
        || has_code(&lower, "403")
        || lower.contains("invalid_api_key")
        || lower.contains("invalid api key")
        || lower.contains("unauthorized")
        || lower.contains("authentication")
    {
        return ProviderErrorKind::Auth;
    }
    if has_code(&lower, "429") || lower.contains("rate limit") || lower.contains("rate_limit") {
        return ProviderErrorKind::RateLimit;
    }
    if lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("connection closed")
        || lower.contains("dns error")
        || lower.contains("tcp connect error")
        || lower.contains("timed out")
    {
        // Nothing about the request was wrong, so the reaction is "try again", not "fix your key"
        // or "shrink the prompt".
        return ProviderErrorKind::Transport;
    }
    if lower.contains("overloaded")
        || lower.contains("server_error")
        || lower.contains("internal server error")
        || lower.contains("bad gateway")
        || lower.contains("service unavailable")
        || lower.contains("gateway timeout")
        || lower.contains(" 529 ")
    {
        return ProviderErrorKind::Unavailable;
    }
    ProviderErrorKind::Other("provider".to_string())
}

#[cfg(test)]
mod tests;
