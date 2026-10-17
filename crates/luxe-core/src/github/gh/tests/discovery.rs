//! Discovery — who is signed in, which repository this is, and how `gh` failures are told apart.

use super::*;

pub(super) const SIGNED_IN: &str = r#"{"hosts":{"github.com":[{"state":"success","active":true,"host":"github.com","login":"Nora-Quinn","tokenSource":"keyring","scopes":"gist, read:org, repo, workflow","gitProtocol":"https"}]}}"#;
pub(super) const SIGNED_OUT: &str = r#"{"hosts":{}}"#;
pub(super) const BAD_TOKEN: &str = r#"{"hosts":{"github.com":[{"state":"error","error":"non-200 OK status code: 401 Unauthorized body: \"{\\r\\n  \\\"message\\\": \\\"Bad credentials\\\",\\r\\n  \\\"documentation_url\\\": \\\"https://docs.github.com/rest\\\",\\r\\n  \\\"status\\\": \\\"401\\\"\\r\\n}\"","active":true,"host":"github.com","login":"","tokenSource":"GH_TOKEN","gitProtocol":"https"}]}}"#;
pub(super) const REPO_VIEW: &str = r#"{"defaultBranchRef":{"name":"master"},"nameWithOwner":"nora-quinn/luxe-dev","url":"https://github.com/nora-quinn/luxe-dev"}"#;

/// A known login whose token GitHub no longer accepts.
pub(super) const REVOKED: &str = r#"{"hosts":{"github.com":[{"state":"error","error":"HTTP 401: Bad credentials (https://api.github.com/)","active":true,"host":"github.com","login":"someone","tokenSource":"/tmp/tmp.zrxMvye382/hosts.yml","gitProtocol":"https"}]}}"#;

#[test]
fn a_signed_in_account_is_read_from_auth_status() {
    let account = parse_account(SIGNED_IN, None).unwrap();
    assert_eq!(account, Account { host: "github.com".into(), login: "Nora-Quinn".into() });
    assert_eq!(parse_account(SIGNED_IN, Some("GitHub.com")).unwrap().login, "Nora-Quinn");
}

/// Signed out exits zero under `--json`, so the body is the only witness.
#[test]
fn signed_out_and_a_rejected_token_are_both_not_signed_in() {
    for body in [SIGNED_OUT, BAD_TOKEN, REVOKED] {
        assert!(
            matches!(parse_account(body, None), Err(GitHubError::NotAuthenticated { host: None })),
            "{body}"
        );
    }
    assert!(matches!(
        parse_account(SIGNED_IN, Some("ghe.example.com")),
        Err(GitHubError::NotAuthenticated { host: Some(host) }) if host == "ghe.example.com"
    ));
}

#[test]
fn a_repository_is_read_from_repo_view() {
    let repository = parse_repository(REPO_VIEW).unwrap();
    assert_eq!(repository.host, "github.com");
    assert_eq!(repository.slug(), "nora-quinn/luxe-dev");
    assert_eq!(repository.default_branch, "master");
    assert_eq!(repository.remote, None, "chosen from the local remotes, not by gh");
}

#[test]
fn output_gh_did_not_promise_is_unreadable_rather_than_a_guess() {
    assert!(matches!(parse_repository("not json"), Err(GitHubError::Unreadable(_))));
    assert!(matches!(parse_account("{}", None), Err(GitHubError::Unreadable(_))));
}

/// Each failure `gh` really printed lands on the variant a caller branches on.
#[test]
fn real_gh_failures_are_classified() {
    let kind = |error: GitHubError| match error {
        GitHubError::NotAuthenticated { .. } => "not signed in",
        GitHubError::NotGitHub => "not github",
        GitHubError::NotFound => "not found",
        GitHubError::RateLimited => "rate limited",
        other => panic!("unclassified: {other:?}"),
    };
    // Each sign-in pattern on a line of real output that carries it alone, so dropping any one
    // of them fails here rather than on somebody's machine
    for (stderr, expected) in [
        ("You are not logged into any GitHub hosts.", "not signed in"),
        ("Try authenticating with:  gh auth login -h github.com", "not signed in"),
        (
            "Alternatively, populate the GH_TOKEN environment variable with a GitHub API \
                 authentication token.",
            "not signed in",
        ),
        ("HTTP 401: Requires authentication (https://api.github.com/user)", "not signed in"),
        ("HTTP 401: Bad credentials (https://api.github.com/graphql)", "not signed in"),
        ("no git remotes found", "not github"),
        (
            "none of the git remotes configured for this repository point to a known GitHub \
                 host. To tell gh about a new GitHub host, please use `gh auth login`",
            "not github",
        ),
        (
            "GraphQL: Could not resolve to a Repository with the name \
                 'nosuchowner-zz/nosuchrepo-zz'. (repository)",
            "not found",
        ),
        // GitHub's documented wording for each way a budget runs out
        ("GraphQL: API rate limit exceeded for user ID 1234.", "rate limited"),
        ("HTTP 403: API rate limit already exceeded for user ID 1234.", "rate limited"),
        (
            "HTTP 403: You have exceeded a secondary rate limit. Please wait a few minutes \
                 before you try again.",
            "rate limited",
        ),
        ("HTTP 429: Too Many Requests (https://api.github.com/graphql)", "rate limited"),
    ] {
        assert_eq!(kind(classify_failure(stderr, "")), expected, "{stderr}");
    }
}

/// Unrecognized failures keep `gh`'s words, bounded and scrubbed, and an empty one says so.
#[test]
fn an_unrecognized_failure_is_passed_through_not_guessed_at() {
    let GitHubError::Failed { message } = classify_failure("", "something nobody anticipated")
    else {
        panic!("an unknown failure is Failed");
    };
    assert_eq!(message, "something nobody anticipated", "stdout is read when stderr is empty");

    let GitHubError::Failed { message } = classify_failure(&"x".repeat(5_000), "") else {
        panic!("an unknown failure is Failed");
    };
    assert!(message.chars().count() < 700, "bounded: {} chars", message.chars().count());

    assert!(matches!(
        classify_failure("  ", ""),
        GitHubError::Failed { message } if message == "no output"
    ));
}
