//! Failure — why a `debug` call produced no answer.

use crate::client::DapError;

/// Why a `debug` call produced no answer.
pub(super) enum Failure {
    /// The call was wrong: a missing argument, an unknown action, a session that is gone
    Usage(String),
    /// The debugger said no
    Dap(DapError),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(message) => formatter.write_str(message),
            Self::Dap(error) => error.fmt(formatter),
        }
    }
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::Usage(message)
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        Self::Usage(message.to_string())
    }
}

impl From<DapError> for Failure {
    fn from(error: DapError) -> Self {
        Self::Dap(error)
    }
}
