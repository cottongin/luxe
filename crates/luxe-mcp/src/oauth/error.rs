//! Errors — why a sign-in, a refresh or a discovery produced no token.

use crate::secret::SecretError;
use crate::token_store::TokenStoreError;

/// Why an OAuth step did not work out.
///
/// Each variant carries the sentence the reader sees, so a caller shows it as it is. What the
/// variants are for is the caller that has to act differently: a refresh the server
/// [refused](Self::Refused) sends the reader back to `luxe mcp login`, and nothing else does.
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    /// Discovery found nothing, or a document that failed its checks
    #[error("{0}")]
    Discovery(String),
    /// An endpoint that would send a code or a token somewhere it could be read
    #[error("{0}")]
    Insecure(String),
    /// The redirect was not an answer to this login's request
    #[error("state mismatch — possible CSRF; login aborted")]
    StateMismatch,
    /// The server answered and said no: an error redirect, an error status from the token endpoint
    #[error("{0}")]
    Refused(String),
    /// The server could not be reached, or its answer could not be read
    #[error("{0}")]
    Network(String),
    /// The login cannot go on as configured or as called
    #[error("{0}")]
    Config(String),
    /// The browser leg: the loopback listener, or nobody coming back to it
    #[error("{0}")]
    Callback(String),
    /// The token store could not be written
    #[error(transparent)]
    Storage(#[from] TokenStoreError),
    /// A configured secret did not resolve
    #[error(transparent)]
    Secret(#[from] SecretError),
}
