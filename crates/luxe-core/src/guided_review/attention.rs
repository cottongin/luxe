//! Attention signals — what the walkthrough might not tell you.
//!
//! The guide is written by the same agent whose work is under review, so it is the right narrator
//! for everything it can be expected to disclose and the wrong one for the rest. That line decides
//! what belongs here: a file edited without ever being read, and a credential-shaped addition. The
//! agent cannot report the first — it does not know — and may not notice the second. luxe can see
//! both because it is the harness that produced the change.
//!
//! Guessing that a gate was weakened is not here either. Its shapes (a removed `#[test]`, a CI
//! file touched) are what moving a test into its own file or editing a workflow look like, so it
//! fired on ordinary work far more often than on a real weakening.
//!
//! Who wrote a change is deliberately not here: read off the transcript it is a guess, wrong often
//! enough to cost more than it tells anyone, since a formatter, a pasted file, or a path spelled
//! two ways all read as a stranger's work.
//!
//! What is *not* here matters as much. Coverage gaps and fan-in are graph facts the walkthrough
//! already carries beside the file being read, and a missing test is work the test-gap advisor
//! hands to the agent directly. Ranked here they would turn the list into a to-do the reviewer has
//! to translate back into a prompt, and bury the signals above under hundreds of rows.
//!
//! Detection is deliberately conservative. A noisy list is worse than none — one false alarm and the
//! reviewer stops reading the only part of the review that says where to start.

use super::{FileId, GuidedReview, HunkId, ReviewFile, ReviewLineKind};
use serde::{Deserialize, Serialize};

/// Why one part of a change deserves a closer look.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionKind {
    /// An added line looks like a committed credential.
    SecretShaped,
    /// The agent edited this file without ever reading it.
    BlindEdit,
}

impl AttentionKind {
    /// A short reviewer-facing badge.
    pub fn badge(self) -> &'static str {
        match self {
            Self::SecretShaped => "secret?",
            Self::BlindEdit => "unread",
        }
    }

    /// How an overflow row names what it stands for (`7 more untested changed symbols`).
    pub const fn plural(self) -> &'static str {
        match self {
            Self::SecretShaped => "credential-shaped lines",
            Self::BlindEdit => "files edited without being read",
        }
    }
}

/// One ranked call-out, anchored where the reviewer can jump to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttentionSignal {
    /// Why this deserves attention.
    pub kind: AttentionKind,
    /// Project-relative path, when the signal belongs to one file.
    pub path: Option<String>,
    /// One-line explanation, grounded in what was found.
    pub detail: String,
    /// Whether this stands for several files rather than the one named in `path`.
    ///
    /// An aggregate must not badge a specific file: "9 files the agent did not write" says nothing
    /// about the one that happens to be listed first.
    #[serde(default)]
    pub aggregate: bool,
    /// File to select when the reviewer follows this signal.
    pub file_id: Option<FileId>,
    /// Hunk to select, when the signal is narrower than a file.
    pub hunk_id: Option<HunkId>,
}

/// Rows one signal kind may occupy before the rest collapse into a counted aggregate.
///
/// Two, not ten. A real review produced 236 `untested` rows and pushed every other call-out — a
/// possible credential, a blind edit — off the screen. A list that can be dominated
/// by its cheapest signal is not a ranking.
const MAX_ROWS_PER_KIND: usize = 2;

/// Ranks the parts of a review worth looking at first. Empty when nothing stands out.
///
/// Ordered by kind (most consequential first) and capped per kind, so the list stays short enough to
/// read in full — which is the only length at which it changes anyone's behavior. What stays on the
/// reading list is decided from [`review_flags`], which the cap never touches.
pub fn review_attention(review: &GuidedReview) -> Vec<AttentionSignal> {
    let mut signals = review_flags(review);
    // Exact repeats only: two hunks of one file tripping the same detector say the same thing
    signals.dedup_by(|left, right| left.kind == right.kind && left.detail == right.detail);
    cap_per_kind(signals)
}

/// Every signal in a review, one per flagged line or file, with nothing merged or capped.
///
/// What [`super::review_budget`] decides the reading list from, and what a front-end reads to say
/// why each file is on it: the capped call-outs stand for the rest of a kind with one counted row,
/// and a hunk only that row stands for must still stay on the list and still say why.
pub fn review_flags(review: &GuidedReview) -> Vec<AttentionSignal> {
    let mut signals = Vec::new();
    for file in &review.files {
        let Some(path) = file_path(file) else {
            continue;
        };
        signals.extend(file_signals(file, &path));
    }
    signals.extend(provenance_signals(review));
    signals.sort_by(|left, right| left.kind.cmp(&right.kind).then(left.detail.cmp(&right.detail)));
    signals
}

/// Keeps the first [`MAX_ROWS_PER_KIND`] rows of each kind, replacing any overflow with one counted
/// aggregate row that still navigates to the next example.
fn cap_per_kind(signals: Vec<AttentionSignal>) -> Vec<AttentionSignal> {
    let mut capped: Vec<AttentionSignal> = Vec::with_capacity(signals.len().min(16));
    let mut index = 0usize;
    while index < signals.len() {
        let kind = signals[index].kind;
        let run_end =
            signals[index..].iter().take_while(|signal| signal.kind == kind).count() + index;
        let run = &signals[index..run_end];
        // An existing aggregate already speaks for a set; never re-aggregate it.
        let shown = if run.iter().any(|signal| signal.aggregate) {
            run.len().min(MAX_ROWS_PER_KIND.max(1))
        } else {
            run.len().min(MAX_ROWS_PER_KIND)
        };
        capped.extend(run[..shown].iter().cloned());
        if let Some(overflow) = run.len().checked_sub(shown).filter(|count| *count > 0) {
            let next = &run[shown];
            capped.push(AttentionSignal {
                kind,
                path: next.path.clone(),
                detail: format!("{overflow} more {}", kind.plural()),
                aggregate: true,
                file_id: next.file_id.clone(),
                hunk_id: next.hunk_id.clone(),
            });
        }
        index = run_end;
    }
    capped
}

/// Signals readable from the diff text alone: one per added line shaped like a credential, in any
/// file, since a key pasted into a README or a config is still a key.
fn file_signals(file: &ReviewFile, path: &str) -> Vec<AttentionSignal> {
    let mut signals = Vec::new();
    for hunk in &file.hunks {
        let added = hunk.lines.iter().filter(|line| line.kind == ReviewLineKind::Addition);
        for _ in added.filter(|line| looks_like_secret(&line.text)) {
            signals.push(secret_signal(file, hunk, path));
        }
    }
    signals
}

fn secret_signal(file: &ReviewFile, hunk: &super::ReviewHunk, path: &str) -> AttentionSignal {
    AttentionSignal {
        kind: AttentionKind::SecretShaped,
        path: Some(path.to_owned()),
        detail: format!("{path} adds a line that looks like a credential"),
        aggregate: false,
        file_id: Some(file.id.clone()),
        hunk_id: Some(hunk.id.clone()),
    }
}

/// Signals only the harness can see, from what the agent actually did.
fn provenance_signals(review: &GuidedReview) -> Vec<AttentionSignal> {
    let Some(provenance) = review.provenance.as_ref() else {
        return Vec::new();
    };
    let mut signals = Vec::new();
    let changed: Vec<(String, &ReviewFile)> =
        review.files.iter().filter_map(|file| Some((file_path(file)?, file))).collect();

    for (path, file) in &changed {
        if provenance.written_paths.contains(path) && !provenance.read_paths.contains(path) {
            signals.push(AttentionSignal {
                kind: AttentionKind::BlindEdit,
                path: Some(path.clone()),
                detail: format!("{path} was edited without being read first"),
                aggregate: false,
                file_id: Some(file.id.clone()),
                hunk_id: None,
            });
        }
    }

    signals
}

fn file_path(file: &ReviewFile) -> Option<String> {
    file.new_path
        .as_deref()
        .or(file.old_path.as_deref())
        .map(|path| path.to_string_lossy().into_owned())
}

/// Whether redaction rewrites this line, i.e. it contains something shaped like a credential.
/// Whether a line adds something shaped like a *live* credential.
///
/// The shape alone is not enough. The scanner's own tests, a README's setup example and a fixture
/// all contain the shape on purpose, and a warning that fires on them is a warning reviewers learn
/// to scroll past (this very file would flag itself). So a match only counts when the matched token
/// does not look manufactured and the author has not marked the line.
fn looks_like_secret(text: &str) -> bool {
    use crate::redact::SecretConfidence;
    if is_allowlisted_line(text) {
        return false;
    }
    crate::redact::secret_shaped_spans(text).iter().any(|(span, confidence)| {
        let worth_warning = match confidence {
            SecretConfidence::Format => true,
            // A named-value match needs to be an actual assignment in actual code: `token: &str` is
            // a function parameter, and a line *about* secrets is documentation. A published key
            // format above still counts anywhere, comment or not — pasting one into a comment leaks
            // it exactly as well as assigning it.
            SecretConfidence::Shape => {
                (span.contains('"') || span.contains('\'')) && !is_comment_line(text)
            }
        };
        worth_warning && !is_placeholder_token(span)
    })
}

/// Whether a line is entirely a comment, in any of the syntaxes a review is likely to contain.
fn is_comment_line(text: &str) -> bool {
    let trimmed = text.trim_start();
    ["//", "#", "--", "/*", "*", "<!--", ";;", "%"].iter().any(|marker| trimmed.starts_with(marker))
}

/// Whether the author declared this line's credential shape deliberate.
///
/// Some fixtures have to look real — a test proving the detector still fires cannot use an obvious
/// placeholder. Rather than exempt test files wholesale (a credential committed to a test is still
/// committed), take the answer every scanner takes: an explicit inline marker, which the author
/// writes, a reviewer can see in the diff, and anyone can grep for. Third-party spellings are
/// honored so a project already carrying them does not have to re-annotate.
fn is_allowlisted_line(text: &str) -> bool {
    const ALLOW_MARKERS: [&str; 4] =
        ["luxe:allow-secret", "gitleaks:allow", "pragma: allowlist secret", "nosec"];
    ALLOW_MARKERS.iter().any(|marker| text.contains(marker))
}

/// Whether a secret-shaped token was obviously typed by a human as an example.
///
/// Real credentials come out of a CSPRNG, so they have no runs, no repeats and no words in them.
/// Placeholders are the opposite on all three counts, which is what every secret scanner leans on:
/// judge the token's randomness, not the pattern it matched.
fn is_placeholder_token(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    const PLACEHOLDER_WORDS: [&str; 12] = [
        "example",
        "sample",
        "dummy",
        "fake",
        "placeholder",
        "redacted",
        "changeme",
        "yourkey",
        "your-key",
        "notreal",
        "abcdef",
        "123456",
    ];
    if PLACEHOLDER_WORDS.iter().any(|word| lower.contains(word)) {
        return true;
    }
    // The random part only — a shared prefix like `sk-` says nothing about the token's entropy.
    let body: Vec<char> = lower
        .rsplit(['-', '_'])
        .next()
        .unwrap_or(&lower)
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect();
    if body.len() < 8 {
        return false;
    }
    let mut longest_run = 1usize;
    let mut longest_repeat = 1usize;
    let mut run = 1usize;
    let mut repeat = 1usize;
    for pair in body.windows(2) {
        let (previous, current) = (pair[0] as u32, pair[1] as u32);
        run = if current == previous + 1 { run + 1 } else { 1 };
        repeat = if current == previous { repeat + 1 } else { 1 };
        longest_run = longest_run.max(run);
        longest_repeat = longest_repeat.max(repeat);
    }
    // `abcdefgh`, `xxxxxxxx`: six of either is a keyboard, not a key.
    if longest_run >= 6 || longest_repeat >= 6 {
        return true;
    }
    // A token drawing on very few distinct characters cannot carry a credential's entropy.
    let distinct = body.iter().collect::<std::collections::BTreeSet<_>>().len();
    distinct * 4 < body.len()
}

#[cfg(test)]
mod tests;
