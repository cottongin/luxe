//! Error cause — `From` for an error variant that says its cause in its message.
//!
//! thiserror's `#[from]` also makes the wrapped error the variant's `source()`, so a caller that
//! prints the chain (anyhow's `{:#}`, a log walking sources) says the cause twice. The TUI and the
//! tool results print `to_string()` alone, so the cause belongs in the message, and these variants
//! keep it there only. A struct variant names its field `error`, not `source`, for the same reason:
//! thiserror treats a field called `source` as the source.

/// `impl From<Cause> for Error` for each `Cause => Variant`, without making the cause a source.
macro_rules! from_cause {
    ($error:ty { $($cause:ty => $variant:ident),+ $(,)? }) => {
        $(
            impl From<$cause> for $error {
                fn from(cause: $cause) -> Self {
                    Self::$variant(cause)
                }
            }
        )+
    };
}

pub(crate) use from_cause;

/// An error's message and every source under it, joined the way a chain-walking printer joins
/// them, for a test checking that a cause is said once.
#[cfg(test)]
pub(crate) fn chain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut said = error.to_string();
    let mut cause = error.source();
    while let Some(next) = cause {
        said.push_str(&format!(": {next}"));
        cause = next.source();
    }
    said
}

#[cfg(test)]
mod tests;
