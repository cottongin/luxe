//! Secrets — nothing that looks like a live credential may reach the log.

use std::fmt::Write as _;

/// Credential prefixes worth scanning for, each followed by enough key material to be real.
const SECRET_PREFIXES: &[&str] = &["sk-", "sk_live_", "ghp_", "gho_", "xoxb-", "AKIA", "Bearer "];

/// Minimum run of key-shaped characters after a prefix before it counts as a credential.
///
/// Without this, reading Luxe's own `redact.rs` — whose source *contains* every prefix — reports a
/// leak on every audit. The prefix alone is prose; the prefix plus a key is a secret.
const SECRET_BODY: usize = 16;

/// Whether `text` contains something shaped like a live credential.
pub(super) fn looks_like_credential(text: &str) -> Option<String> {
    for prefix in SECRET_PREFIXES {
        for (at, _) in text.match_indices(prefix) {
            let tail: String = text[at + prefix.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            if tail.chars().count() >= SECRET_BODY {
                let shown: String = tail.chars().take(4).collect();
                return Some(format!("{prefix}{shown}… ({} chars)", tail.chars().count()));
            }
        }
    }
    None
}

/// Nothing that looks like a live credential may reach the log.
pub(super) fn report(out: &mut String, text: &str) {
    let _ = writeln!(out, "## Secrets\n");
    let mut hits = Vec::new();
    for (line_number, line) in text.lines().enumerate() {
        if let Some(hit) = looks_like_credential(line) {
            hits.push(format!("line {}: {hit}", line_number + 1));
        }
    }
    if hits.is_empty() {
        let _ = writeln!(
            out,
            "  clean — no credential-shaped value in any request, header, or reply\n"
        );
    } else {
        for hit in hits.iter().take(10) {
            let _ = writeln!(out, "  ▲ {hit}");
        }
        let _ = writeln!(out);
    }
}
