//! Error — why an engine operation failed.

use super::*;

/// Engine error type.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// Provider error. Shown as the inner error alone: it already says it is a provider error,
    /// and saying so again read "provider error: provider error: …" everywhere.
    #[error("{0}")]
    Provider(ProviderError),
    /// Session error.
    #[error("session error: {0}")]
    Session(SessionError),
    /// JSON error.
    #[error("json error: {0}")]
    Json(serde_json::Error),
    /// A compaction that produced nothing worth keeping, so the transcript stands.
    #[error("compaction error: {0}")]
    Compaction(String),
    /// Nobody has chosen a model, so there is nothing to send the turn to.
    ///
    /// The gate, rather than the courtesy: a front-end should ask before it submits, and this is
    /// what holds when one does not. See [`crate::provider::ModelRef::is_unset`].
    #[error("no model selected — pick one with /model, or start Luxe with --model <provider/id>")]
    NoModel,
}

crate::error_cause::from_cause!(EngineError {
    ProviderError => Provider,
    SessionError => Session,
    serde_json::Error => Json,
});

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    /// Each variant's sentence, pinned to what it said before the derive, so wording a message is
    /// never a side effect. None carries a source: the cause is in the message.
    #[test]
    fn each_error_says_what_it_always_said() {
        let json = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        let json_said = format!("json error: {json}");
        let provider = ProviderError::Other("down".into());
        let provider_said = provider.to_string();
        let session = SessionError::Other("gone".into());
        let session_said = format!("session error: {session}");
        let no_model =
            "no model selected — pick one with /model, or start Luxe with --model <provider/id>";
        let cases: [(EngineError, String, bool); 5] = [
            (provider.into(), provider_said, false),
            (session.into(), session_said, false),
            (json.into(), json_said, false),
            (EngineError::Compaction("empty".into()), "compaction error: empty".into(), false),
            (EngineError::NoModel, no_model.into(), false),
        ];
        for (error, said, sourced) in cases {
            assert_eq!(error.to_string(), said);
            assert_eq!(error.source().is_some(), sourced, "{said}");
        }
    }
}
