use std::error::Error;
use std::path::PathBuf;

use crate::guided_review::{
    ActiveReviewRecordStoreError, GuidedReviewError, LastTurnRecordStoreError,
    PullRequestResolveError, ReviewContextSourceError, ReviewSessionStoreError, SourceStoreError,
    TurnBaselineStoreError,
};
use crate::session::SessionError;

use super::chain;

fn io() -> std::io::Error {
    std::io::Error::other("disk on fire")
}

fn json() -> serde_json::Error {
    serde_json::from_str::<serde_json::Value>("{").expect_err("an unclosed object")
}

/// Every error that names its cause in its message says it once through the whole chain, so a
/// printer that walks sources (anyhow's `{:#}`, a log) never repeats it.
#[test]
fn each_error_names_its_cause_once_through_the_chain() {
    let path = PathBuf::from("a/b");
    let (op, io_cause, json_cause) = ("read", io().to_string(), json().to_string());
    let github = || crate::github::GitHubError::Run { program: "gh", source: io() };
    let github_said = github().to_string();
    let resolve = |error| PullRequestResolveError::GitHub { number: 1, repository: None, error };
    let hex = "zz".parse::<u8>().expect_err("not hex digits");
    let hex_cause = hex.to_string();
    let errors: Vec<(Box<dyn Error>, &str)> = vec![
        (
            Box::new(crate::formatter::FormatterError::Io {
                command: "fmt".into(),
                action: "run",
                error: io(),
            }),
            &io_cause,
        ),
        (Box::new(crate::lsp::DiagnosticsError::Read { path: "a".into(), error: io() }), &io_cause),
        (
            Box::new(crate::mention_source::MentionSourceError::Spawn {
                command: "x".into(),
                error: io(),
            }),
            &io_cause,
        ),
        (Box::new(SessionError::from(io())), &io_cause),
        (Box::new(SessionError::from(json())), &json_cause),
        (Box::new(SessionError::JsonParse { line: 1, error: json() }), &json_cause),
        (
            Box::new(LastTurnRecordStoreError::Io {
                operation: op,
                path: path.clone(),
                error: io(),
            }),
            &io_cause,
        ),
        (
            Box::new(LastTurnRecordStoreError::Deserialize { path: path.clone(), error: json() }),
            &json_cause,
        ),
        (Box::new(LastTurnRecordStoreError::from(json())), &json_cause),
        (Box::new(ReviewContextSourceError::Read { path: path.clone(), error: io() }), &io_cause),
        (
            Box::new(ReviewSessionStoreError::Io {
                operation: op,
                path: path.clone(),
                error: io(),
            }),
            &io_cause,
        ),
        (
            Box::new(ReviewSessionStoreError::Deserialize { path: path.clone(), error: json() }),
            &json_cause,
        ),
        (Box::new(ReviewSessionStoreError::from(json())), &json_cause),
        (
            Box::new(ActiveReviewRecordStoreError::Io {
                operation: op,
                path: path.clone(),
                error: io(),
            }),
            &io_cause,
        ),
        (
            Box::new(ActiveReviewRecordStoreError::Deserialize {
                path: path.clone(),
                error: json(),
            }),
            &json_cause,
        ),
        (Box::new(ActiveReviewRecordStoreError::from(json())), &json_cause),
        (
            Box::new(SourceStoreError::Io { operation: op, path: path.clone(), error: io() }),
            &io_cause,
        ),
        (
            Box::new(SourceStoreError::Deserialize { path: path.clone(), error: json() }),
            &json_cause,
        ),
        (Box::new(SourceStoreError::from(json())), &json_cause),
        (
            Box::new(TurnBaselineStoreError::Io { operation: op, path: path.clone(), error: io() }),
            &io_cause,
        ),
        (
            Box::new(TurnBaselineStoreError::Deserialize { path: path.clone(), error: json() }),
            &json_cause,
        ),
        (Box::new(TurnBaselineStoreError::from(json())), &json_cause),
        (
            Box::new(GuidedReviewError::GitSpawn {
                operation: "diff",
                repo_path: path.clone(),
                error: io(),
            }),
            &io_cause,
        ),
        (Box::new(GuidedReviewError::ReadPath { path: path.clone(), error: io() }), &io_cause),
        // GitHub's own message once, and the OS error it stands on still under it
        (Box::new(resolve(github())), &github_said),
        (Box::new(resolve(github())), &io_cause),
        (Box::new(crate::command_hooks::HookIoError::Stdin(io())), &io_cause),
        (Box::new(crate::theme::ThemeError::from(hex)), &hex_cause),
        (Box::new(crate::loop_controller::LoopError::from(io())), &io_cause),
        (Box::new(crate::loop_controller::LoopError::from(json())), &json_cause),
        (Box::new(crate::mcp_config::McpConfigError::from(io())), &io_cause),
        (Box::new(crate::mcp_config::McpConfigError::from(json())), &json_cause),
        (Box::new(crate::mcp_stdio::McpStdioError::from(io())), &io_cause),
        (Box::new(crate::mcp_stdio::McpStdioError::from(json())), &json_cause),
        (Box::new(crate::theme::ThemeError::from(io())), &io_cause),
        (Box::new(crate::theme::ThemeError::from(json())), &json_cause),
        (Box::new(crate::TodoError::from(io())), &io_cause),
        (Box::new(crate::TodoError::from(json())), &json_cause),
        (Box::new(crate::compaction::CompactionError::from(SessionError::from(io()))), &io_cause),
        (Box::new(crate::engine::EngineError::from(SessionError::from(io()))), &io_cause),
        (Box::new(crate::engine::EngineError::from(json())), &json_cause),
    ];
    let repeated: Vec<String> = errors
        .iter()
        .map(|(error, cause)| (chain(error.as_ref()), *cause))
        .filter(|(said, cause)| said.matches(cause).count() != 1)
        .map(|(said, _)| said)
        .collect();
    assert!(repeated.is_empty(), "these say their cause more or less than once: {repeated:#?}");
}
