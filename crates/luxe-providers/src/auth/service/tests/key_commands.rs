//! Key commands at login; unix only, since the failing command is `sh` syntax.
#![cfg(unix)]

use super::*;

/// A key command that fails says why, from its own stderr, instead of "resolved to an empty value".
#[tokio::test]
async fn a_failing_key_command_says_why_at_login() {
    let (_dir, service) = service();
    let error = service
        .login("openai", "api-key", Scripted::new(&["!echo 'vault is locked' >&2; exit 3"]))
        .await
        .expect_err("must refuse");
    assert!(error.to_string().contains("vault is locked"), "got {error}");
    assert!(service.store().credential("openai").is_none(), "nothing may be stored");
}
