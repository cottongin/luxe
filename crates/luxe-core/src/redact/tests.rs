use super::*;

/// A literal the process resolved is scrubbed wherever the set is applied, with the text around it
/// intact; one under the length floor is left alone, since redacting a short word everywhere costs
/// more than it protects.
#[test]
fn a_secret_set_scrubs_long_literals_and_skips_short_ones() {
    let set = SecretSet::default();
    set.add("correct-horse-battery-staple");
    set.add("short");
    set.add("   ");
    let entry = "authorization: correct-horse-battery-staple, then a short note";
    let scrubbed = set.redact(entry);
    assert!(!scrubbed.contains("correct-horse"), "{scrubbed}");
    assert!(scrubbed.starts_with("authorization: ") && scrubbed.ends_with("a short note"));
    assert!(redact_secrets(entry, &[]).contains("correct-horse"), "no shape, no pattern");
}

/// Every pattern compiles and catches the example it was written for, so a typo in one fails here
/// instead of silently dropping it from the list.
#[test]
fn every_secret_pattern_compiles_and_matches_its_own_example() {
    assert_eq!(SECRET_PATTERNS.len(), PATTERNS.len());
    for ((regex, ..), (pattern, _, _, example)) in SECRET_PATTERNS.iter().zip(PATTERNS) {
        assert!(regex.is_match(example), "{pattern} missed its example {example}");
        assert!(redact_secrets(example, &[]).contains("[REDACTED"), "{example}");
    }
}

#[test]
fn test_redact_openai_api_key() {
    let text = "Using API key sk-abc123def456ghi789jkl012mno345pqr678";
    let redacted = redact_secrets(text, &[]);
    assert!(!redacted.contains("sk-abc123"));
    assert!(redacted.contains("[REDACTED_API_KEY]"));
}

#[test]
fn test_redact_anthropic_api_key() {
    let text = "Using API key sk-ant-abc123def456ghi789jkl012mno345pqr678";
    let redacted = redact_secrets(text, &[]);
    assert!(!redacted.contains("sk-ant-abc123"));
    assert!(redacted.contains("[REDACTED_API_KEY]"));
}

/// The formats keys are actually issued in, all of which segment themselves with hyphens.
///
/// An alphanumeric-only token body stopped at the first hyphen, so a live `sk-ant-api03-…`
/// matched nothing and passed through a transcript in the clear.
#[test]
fn segmented_key_formats_are_redacted() {
    for key in [
        "sk-ant-api03-abcDEF123ghiJKL456mnoPQR789stu",
        "sk-ant-api03-abcDEF123-_ghiJKL456mnoPQR789stuAA",
        "sk-proj-abcDEF123ghiJKL456mnoPQR789stu",
        "sk-svcacct-abcDEF123-ghiJKL456_mnoPQR789stu",
    ] {
        let redacted = redact_secrets(&format!("ANTHROPIC_API_KEY={key}"), &[]);
        assert!(!redacted.contains("abcDEF123"), "{key} survived as {redacted}");
        assert!(redacted.contains("[REDACTED"), "{key} → {redacted}");
    }
}

#[test]
fn test_redact_configured_secret() {
    let text = "Password is MySecretPassword123!";
    let redacted = redact_secrets(text, &["MySecretPassword123!".to_string()]);
    assert!(!redacted.contains("MySecretPassword123!"));
    assert!(redacted.contains("[REDACTED]"));
}

#[test]
fn test_redact_github_token() {
    let text = "Token: ghp_abc123def456ghi789jkl012mno345";
    let redacted = redact_secrets(text, &[]);
    assert!(!redacted.contains("ghp_abc123"));
    assert!(redacted.contains("[REDACTED_GITHUB_TOKEN]"));
}

#[test]
fn test_redact_aws_key() {
    let text = "AWS_KEY=AKIA1234567890ABCDEF";
    let redacted = redact_secrets(text, &[]);
    assert!(!redacted.contains("AKIA1234567890ABCDEF"));
    assert!(redacted.contains("[REDACTED_AWS_KEY]"));
}

#[test]
fn test_redact_multiple_secrets() {
    let text =
        "Key1: sk-abc123def456ghi789jkl012mno345pqr678, Key2: sk-ant-xyz789uvw456rst123opq890";
    let redacted = redact_secrets(text, &[]);
    assert!(!redacted.contains("sk-abc123"));
    assert!(!redacted.contains("sk-ant-xyz789"));
    assert_eq!(redacted.matches("[REDACTED_API_KEY]").count(), 2);
}

#[test]
fn test_no_secrets_to_redact() {
    let text = "Hello world, this is a normal message.";
    let redacted = redact_secrets(text, &[]);
    assert_eq!(redacted, text);
}

#[test]
fn test_empty_text() {
    let text = "";
    let redacted = redact_secrets(text, &[]);
    assert_eq!(redacted, "");
}

#[test]
fn test_empty_secret_list() {
    let text = "No secrets here.";
    let redacted = redact_secrets(text, &[]);
    assert_eq!(redacted, text);
}

#[test]
fn test_redact_private_key_block() {
    let text = "key:\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\nAAAA==\n-----END OPENSSH PRIVATE KEY-----\ndone";
    let redacted = redact_secrets(text, &[]);
    assert!(!redacted.contains("b3BlbnNzaC"), "key body must be gone: {redacted}");
    assert!(redacted.contains("[REDACTED_PRIVATE_KEY]"));
    assert!(redacted.contains("done"), "surrounding text preserved");
}

#[test]
fn test_redact_url_credentials() {
    let redacted = redact_secrets("clone https://alice:s3cr3t@github.com/x.git now", &[]);
    assert!(!redacted.contains("s3cr3t"), "password gone: {redacted}");
    assert!(redacted.contains("https://[REDACTED]@github.com/x.git"));
    assert!(redacted.contains("now"));
}

#[test]
fn test_redact_key_value_assignments() {
    for (text, secret) in [
        ("password=hunter2", "hunter2"),
        ("PASSWORD: hunter2", "hunter2"),
        ("{\"api_key\":\"abc123def\"}", "abc123def"),
        ("auth_token = \"zzzzzzzz\"", "zzzzzzzz"),
        ("AUTH_TOKEN=zzzzzzzz", "zzzzzzzz"),
        ("refresh_token=ya29.a0Af-longish-value", "ya29"),
        ("password=!hunter2#", "hunter2"),
        ("SECRET: correct-horse-battery-staple", "correct-horse"),
    ] {
        let redacted = redact_secrets(text, &[]);
        assert!(redacted.contains("[REDACTED]"), "should redact value in {text:?}: {redacted}");
        assert!(!redacted.contains(secret), "{secret} survived {text:?} as {redacted}");
    }
}

/// The reported failure. Tool output is redacted at the dispatch choke point *before the model
/// reads it*, so a pattern that fires on ordinary code does not merely over-redact a log — it
/// hands the agent source that no longer says what the file says. These eight shapes stood for
/// 238 wrong substitutions across luxe's own tree.
#[test]
fn ordinary_code_reaches_the_model_exactly_as_written() {
    for code in [
        "CancellationToken::cancelled() resolves when cancellation is requested",
        "AuthMethodKind::ApiKey => methods.push(m),",
        "fn exchange(token: &str) -> Result<Credential>",
        "pub struct Config { api_key: Option<ApiKey>, secret: SecretString }",
        "let token = editor.buffer;",
        "let secret = SECRET_HEADERS.contains(name);",
        "Secret::expose_secret(&received)",
        "access_key: String,",
        "the auth token is stored securely",
    ] {
        assert_eq!(redact_secrets(code, &[]), code, "code was mangled on the way to the model");
    }
}

/// Where the guess gives up, on record: a lowercase, unquoted, punctuation-free password in
/// running prose has the shape of a word, and every way of catching it also catches
/// `api_key: String`. The forms it actually appears in — quoted in a config, SHOUTING in an
/// environment, or carrying so much as one digit — are all still caught above, and a caller
/// that knows the literal passes it in `configured_secrets`.
#[test]
fn a_password_indistinguishable_from_a_word_is_a_known_gap() {
    let bare = "auth_token = zzzzzzzz";
    assert_eq!(redact_secrets(bare, &[]), bare);
    assert!(!redact_secrets(bare, &["zzzzzzzz".to_string()]).contains("zzzzzzzz"));
}

/// A published credential format is proof, not a guess, so none of the value rules above apply
/// to it — a `sk-` key is redacted wherever it appears, including in the middle of code.
#[test]
fn a_real_credential_is_still_caught_inside_code() {
    let line = "let key = ApiKey::new(\"sk-ant-api03-abcDEF123ghiJKL456mnoPQR789stu\");";
    let redacted = redact_secrets(line, &[]);
    assert!(!redacted.contains("abcDEF123"), "{redacted}");
    assert!(redacted.contains("ApiKey::new"), "the code around it survives: {redacted}");
}

#[test]
fn test_redact_vendor_keys() {
    let g = redact_secrets("key AIzaSyA1234567890abcdefghijklmnopqrstuvw done", &[]);
    assert!(g.contains("[REDACTED_GOOGLE_KEY]"), "{g}");
    let s = redact_secrets("stripe sk_live_0123456789abcXYZ", &[]);
    assert!(s.contains("[REDACTED_STRIPE_KEY]"), "{s}");
}

#[test]
fn test_normal_prose_is_untouched() {
    // Requires an explicit `:`/`=`, so 'token' in prose isn't mangled.
    let text = "The auth token is stored securely and the password policy is strict.";
    assert_eq!(redact_secrets(text, &[]), text);
}
