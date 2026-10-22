//! Guide manifest — strict advisory model output for immutable review items.
//!
//! A manifest can order and explain captured item IDs, but it cannot supply source content. It is
//! strict where honesty depends on it — the item set is checked for exact, non-duplicating coverage
//! and a violation rejects the batch whole — and lenient about the prose and the section label,
//! which are advisory presentation and get bounded or defaulted instead of failing.

use super::{ChapterCoverageError, ChapterKind, ReviewItemRef};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// Maximum number of model-proposed chapters accepted for one review.
pub const MAX_GUIDE_CHAPTERS: usize = 16;
/// Sanity cap on sections one *batch* may propose before its reply is treated as runaway.
///
/// Higher than the guide's own limit on purpose: [`super::reduce_groups`] condenses the assembled
/// guide by merging neighbors, so a batch that returns eighteen sections is a shaping problem, not
/// a reason to throw away its prose.
const MAX_GUIDE_BATCH_CHAPTERS: usize = 64;
/// Maximum user-facing title length after control filtering.
pub const MAX_GUIDE_TITLE_CHARS: usize = 120;
/// Maximum rationale or impact length after control filtering.
pub const MAX_GUIDE_TEXT_CHARS: usize = 600;
/// Maximum serialized item key length accepted from a model.
pub const MAX_GUIDE_ITEM_ID_CHARS: usize = 160;
/// Maximum length of a per-item note — a sentence, not a second rationale.
const MAX_GUIDE_NOTE_CHARS: usize = 240;
const MAX_GUIDE_NOTES_PER_CHAPTER: usize = 64;
/// Longest model reply accepted and scanned for a manifest, in bytes.
///
/// Generous next to a real manifest, and small enough that a model which ignored the schema entirely
/// cannot make us scan a novel for balanced braces. Shared with the streaming collector so both ends
/// agree on what "too long" means.
pub const MAX_GUIDE_OUTPUT_BYTES: usize = 64 * 1024;

/// Maximum length of one account bullet — a claim, not a paragraph.
pub const MAX_GUIDE_ACCOUNT_CHARS: usize = 240;
/// Most account bullets kept. A reviewer who needs more than this is reading the walkthrough.
pub const MAX_GUIDE_ACCOUNT_ITEMS: usize = 5;

/// Strict model output schema with no authority over canonical source content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuideManifest {
    /// Ordered chapter proposals.
    pub chapters: Vec<GuideManifestChapter>,
    /// A summary of the whole change, written for the reviewer.
    ///
    /// Optional, and asked for only once however many batches a review takes. Absent from a
    /// degraded pass, which is why the presentation falls back to the raw record of the turn.
    #[serde(default)]
    pub brief: Option<GuideManifestBrief>,
}

/// The model's summary of a change, addressed to whoever is about to review it.
///
/// Distinct from replaying the conversation: the reviewer already read the agent's closing message
/// before opening the review, so repeating it verbatim spends the most valuable rows on the screen
/// saying something they just saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuideManifestBrief {
    /// The problem the change sets out to solve, in one or two sentences.
    pub intent: String,
    /// What the change actually does — a few claims a reviewer can check against the diff.
    #[serde(default)]
    pub account: Vec<String>,
    /// Accepted and discarded.
    ///
    /// Never rendered: a model's account of its own evidence is the self-report this subsystem
    /// declines to trust, and the harness knows whether anything ran after the final edit from the
    /// turn itself. The field exists only so `deny_unknown_fields` does not reject a reply from a
    /// model that still volunteers one.
    #[serde(default, skip_serializing)]
    pub verification: Option<String>,
}

/// One model-proposed grouping of immutable review item IDs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuideManifestChapter {
    /// Concise user-facing label.
    pub title: String,
    /// Grounded explanation for grouping the referenced immutable items.
    pub rationale: String,
    /// Optional grounded impact statement.
    #[serde(default)]
    pub impact: Option<String>,
    /// Semantic display priority.
    pub kind: GuideManifestKind,
    /// Only known source item IDs are valid.
    pub item_ids: Vec<String>,
    /// One sentence per item: what this piece does, and why it is here.
    ///
    /// Advisory and optional — a note naming an item this chapter does not own is dropped rather
    /// than failing the batch, because prose is never allowed to decide which items exist.
    #[serde(default)]
    pub item_notes: Vec<GuideManifestNote>,
}

/// A model-authored sentence about one immutable review item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuideManifestNote {
    /// The item this note describes.
    pub item_id: String,
    /// The sentence itself.
    pub note: String,
}

/// String-only manifest kind accepted from advisory model output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuideManifestKind {
    /// Primary implementation or contract work.
    Core,
    /// Direct downstream integration work.
    Consequence,
    /// Verification work.
    Test,
    /// Documentation, configuration, or glue.
    Supporting,
    /// Generated, lockfile, or mechanical source.
    Generated,
    /// A label outside the vocabulary, presented as supporting work.
    ///
    /// Kind is display priority, not truth: models invent neighboring words (`context`, `docs`,
    /// `refactor`) and losing a whole section's explanation over one of them is a bad trade.
    #[serde(other)]
    Other,
}

impl From<GuideManifestKind> for ChapterKind {
    fn from(value: GuideManifestKind) -> Self {
        match value {
            GuideManifestKind::Core => Self::Core,
            GuideManifestKind::Consequence => Self::Consequence,
            GuideManifestKind::Test => Self::Test,
            GuideManifestKind::Supporting | GuideManifestKind::Other => Self::Supporting,
            GuideManifestKind::Generated => Self::Generated,
        }
    }
}

/// Rejects invalid advisory output before it can affect the immutable review presentation.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum GuideManifestError {
    /// Model output was not one strict manifest object.
    #[error("guide manifest JSON is invalid: {message}")]
    InvalidJson {
        /// Bounded parser diagnostic without model output.
        message: String,
    },
    /// The reply contained no JSON object at all.
    #[error("model reply contained no JSON manifest")]
    NoJson,
    /// The reply opened a manifest it never closed — almost always the output cap.
    #[error("model reply was cut off before the manifest closed")]
    Truncated,
    /// The manifest proposed too many chapters.
    #[error("guide manifest has too many chapters")]
    TooManyChapters,
    /// A user-facing field became empty or exceeded its bound after control filtering.
    #[error("guide manifest has an invalid {field}")]
    InvalidText {
        /// Field name suitable for diagnostics.
        field: &'static str,
    },
    /// A proposed item ID is empty or too large.
    #[error("guide manifest has an invalid item ID")]
    InvalidItemId,
    /// The manifest referenced an absent immutable source item.
    #[error("guide manifest referenced an unknown source item")]
    UnknownItem,
    /// The manifest omitted or duplicated a source item.
    #[error(transparent)]
    Coverage(#[from] ChapterCoverageError),
}

impl GuideManifest {
    /// Parses one model response before it can affect the review presentation.
    ///
    /// Tolerant about *packaging*, strict about content. Asking for "JSON only" does not stop a
    /// model wrapping the object in a ```` ```json ```` fence or opening with a sentence, and a
    /// bare `serde_json::from_str` rejects both — which silently cost every review its narrative.
    /// So the manifest is located structurally (the first balanced `{…}` that deserializes) while
    /// the schema itself stays `deny_unknown_fields` and the item bijection stays the only
    /// authority over the item set.
    pub fn from_model_json(output: &str) -> Result<Self, GuideManifestError> {
        if output.len() > MAX_GUIDE_OUTPUT_BYTES {
            return Err(GuideManifestError::InvalidJson {
                message: "model reply exceeded the scan bound".into(),
            });
        }
        let candidates = json_object_slices(output);
        if candidates.is_empty() {
            // An opened-but-unclosed object is the fingerprint of hitting the output cap; no `{`
            // at all means the model answered in prose.
            return Err(if opens_unclosed_object(output) {
                GuideManifestError::Truncated
            } else {
                GuideManifestError::NoJson
            });
        }
        let mut first_error = None;
        for candidate in candidates {
            match serde_json::from_str::<Self>(candidate) {
                Ok(manifest) => return Ok(manifest),
                Err(error) => {
                    first_error.get_or_insert_with(|| GuideManifestError::InvalidJson {
                        message: error.to_string().chars().take(200).collect(),
                    });
                }
            }
        }
        Err(first_error.unwrap_or(GuideManifestError::NoJson))
    }

    /// Converts one batch's advisory output into proto-chapters, or rejects it whole.
    ///
    /// **Strict about the item set, lenient about prose.** Which items exist and where they land is
    /// the anti-fabrication guarantee, so an unknown, duplicated or missing ID rejects the batch.
    /// Titles, rationales, impacts and the section *kind* are advisory presentation: they are
    /// bounded, filtered and defaulted rather than fatal, because rejecting a batch over a long
    /// sentence or an invented label throws away the explanation the reviewer came for.
    ///
    /// Coverage is checked against `allowed` (the batch's items) rather than the whole review, so
    /// one failed pass degrades alone instead of discarding every other section's guidance.
    /// The reviewer-facing brief, sanitized and bounded, or `None` when the model omitted it.
    ///
    /// Read before [`GuideManifest::into_groups`] consumes the manifest. Advisory throughout: a
    /// brief that fails its bounds is dropped, never a reason to reject a batch whose item
    /// coverage is sound — prose does not get to decide which code is in the review.
    pub fn brief(&self) -> Option<super::ReviewBrief> {
        let brief = self.brief.as_ref()?;
        let intent = bounded_text(brief.intent.clone(), MAX_GUIDE_TEXT_CHARS, "intent").ok()?;
        if intent.is_empty() {
            return None;
        }
        let account = brief
            .account
            .iter()
            .filter_map(|claim| {
                bounded_text(claim.clone(), MAX_GUIDE_ACCOUNT_CHARS, "account").ok()
            })
            .filter(|claim| !claim.is_empty())
            .take(MAX_GUIDE_ACCOUNT_ITEMS)
            .collect();
        Some(super::ReviewBrief { intent, account })
    }

    pub fn into_groups(
        self,
        allowed: &[super::ReviewItemRef],
    ) -> Result<Vec<super::GuideGroup>, GuideManifestError> {
        if self.chapters.len() > MAX_GUIDE_BATCH_CHAPTERS {
            return Err(GuideManifestError::TooManyChapters);
        }
        let available = allowed
            .iter()
            .map(|item| (item_key(item).to_owned(), item.clone()))
            .collect::<HashMap<_, _>>();
        let mut groups = Vec::with_capacity(self.chapters.len());
        let mut seen: HashMap<String, ()> = HashMap::new();
        for proposed in self.chapters {
            let title = bounded_text(proposed.title, MAX_GUIDE_TITLE_CHARS, "title")?;
            let rationale = bounded_text(proposed.rationale, MAX_GUIDE_TEXT_CHARS, "rationale")?;
            // The prompt has the model leave out an impact the rationale already gives, and an
            // empty one is the same answer rather than a broken guide
            let impact = proposed
                .impact
                .filter(|text| !filtered(text).trim().is_empty())
                .map(|text| bounded_text(text, MAX_GUIDE_TEXT_CHARS, "impact"))
                .transpose()?;
            let mut items = Vec::with_capacity(proposed.item_ids.len());
            for id in proposed.item_ids {
                if id.is_empty()
                    || id.chars().count() > MAX_GUIDE_ITEM_ID_CHARS
                    || id.chars().any(char::is_control)
                {
                    return Err(GuideManifestError::InvalidItemId);
                }
                let item = available.get(&id).cloned().ok_or(GuideManifestError::UnknownItem)?;
                if seen.insert(id, ()).is_some() {
                    return Err(GuideManifestError::Coverage(
                        ChapterCoverageError::DuplicateItem { item },
                    ));
                }
                items.push(item);
            }
            // Notes attach only to items this chapter owns; anything else is a model slip we drop
            // rather than display against the wrong code.
            let owned: Vec<&str> = items.iter().map(item_key).collect();
            let notes = proposed
                .item_notes
                .into_iter()
                .filter(|note| owned.contains(&note.item_id.as_str()))
                .filter_map(|note| {
                    bounded_text(note.note, MAX_GUIDE_NOTE_CHARS, "note")
                        .ok()
                        .map(|text| super::ItemNote { item: note.item_id, note: text })
                })
                .take(MAX_GUIDE_NOTES_PER_CHAPTER)
                .collect();
            groups.push(super::GuideGroup {
                title,
                rationale,
                impact,
                kind: ChapterKind::from(proposed.kind),
                items,
                notes,
            });
        }
        let missing: Vec<_> =
            allowed.iter().filter(|item| !seen.contains_key(item_key(item))).cloned().collect();
        if !missing.is_empty() {
            return Err(GuideManifestError::Coverage(ChapterCoverageError::MissingItems {
                items: missing,
            }));
        }
        Ok(groups)
    }
}

/// Every balanced top-level `{…}` slice in `output`, outermost only, in order.
///
/// Brace counting is string- and escape-aware, so a `{` inside a rationale cannot end the object
/// early. Nested objects never yield their own slice: only the outer one is a manifest candidate.
fn json_object_slices(output: &str) -> Vec<&str> {
    let bytes = output.as_bytes();
    let mut slices = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'{' {
            index += 1;
            continue;
        }
        match object_end(bytes, index) {
            Some(end) => {
                slices.push(&output[index..=end]);
                index = end + 1;
            }
            // Unclosed from here on: nothing later can close either.
            None => break,
        }
    }
    slices
}

/// Whether `output` opens an object that never closes — the shape a truncated reply leaves behind.
fn opens_unclosed_object(output: &str) -> bool {
    let bytes = output.as_bytes();
    bytes
        .iter()
        .position(|byte| *byte == b'{')
        .is_some_and(|start| object_end(bytes, start).is_none())
}

/// Index of the `}` closing the object that starts at `start`, or `None` when it never closes.
fn object_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, byte) in bytes.iter().enumerate().skip(start) {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(offset);
                }
            }
            _ => {}
        }
    }
    None
}

fn filtered(text: &str) -> String {
    text.chars().filter(|character| !character.is_control()).collect()
}

/// Filters control characters and clips `text` to `maximum` characters.
///
/// Over-long prose is *clipped*, not rejected: a model that writes six hundred and forty characters
/// of rationale has still explained the change, and failing the batch over the last forty threw the
/// explanation away. An empty field is still an error — a section with no title says nothing.
fn bounded_text(
    text: String,
    maximum: usize,
    field: &'static str,
) -> Result<String, GuideManifestError> {
    let text = filtered(&text).trim().to_owned();
    if text.is_empty() {
        return Err(GuideManifestError::InvalidText { field });
    }
    if text.chars().count() <= maximum {
        return Ok(text);
    }
    let clipped: String = text.chars().take(maximum.saturating_sub(1)).collect();
    Ok(format!("{}…", clipped.trim_end()))
}

fn item_key(item: &ReviewItemRef) -> &str {
    match item {
        ReviewItemRef::Hunk(id) => id.as_str(),
        ReviewItemRef::File(id) => id.as_str(),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod brief_tests {
    use super::*;

    fn manifest_json(brief: &str) -> String {
        format!(r#"{{"chapters":[],"brief":{brief}}}"#)
    }

    #[test]
    fn a_brief_is_parsed_bounded_and_sanitized() {
        let json = manifest_json(
            r#"{"intent":"Quitting looked frozen.","account":["Shows a toast on q.","Repaints once before teardown."],"verification":"857 tests pass; nothing covers the slow path."}"#,
        );
        let manifest = GuideManifest::from_model_json(&json).expect("valid manifest");
        let brief = manifest.brief().expect("a brief");
        assert_eq!(brief.intent, "Quitting looked frozen.");
        assert_eq!(brief.account.len(), 2);
    }

    /// A model that still volunteers the retired `verification` field must not fail the batch:
    /// `deny_unknown_fields` would reject the whole reply over one sentence nobody renders.
    #[test]
    fn a_retired_brief_field_is_accepted_and_dropped() {
        let json = manifest_json(
            r#"{"intent":"Quitting looked frozen.","verification":"857 tests pass."}"#,
        );
        let manifest = GuideManifest::from_model_json(&json).expect("the reply still parses");
        assert_eq!(manifest.brief().expect("a brief").intent, "Quitting looked frozen.");
    }

    /// Prose is advisory: an over-long brief is clipped and an empty one is dropped, and neither
    /// rejects a batch whose item coverage is sound — words do not decide which code is reviewed.
    #[test]
    fn an_unusable_brief_never_fails_the_batch() {
        let long = "x".repeat(MAX_GUIDE_TEXT_CHARS + 50);
        let json = manifest_json(&format!(r#"{{"intent":"{long}"}}"#));
        let manifest = GuideManifest::from_model_json(&json).expect("the manifest still parses");
        let brief = manifest.brief().expect("clipped, not discarded");
        assert_eq!(brief.intent.chars().count(), MAX_GUIDE_TEXT_CHARS);
        assert!(brief.intent.ends_with('…'));
        assert!(manifest.into_groups(&[]).is_ok(), "and the batch still stands");

        let empty = GuideManifest::from_model_json(&manifest_json(r#"{"intent":"   "}"#))
            .expect("the manifest still parses");
        assert!(empty.brief().is_none(), "a brief with nothing in it is dropped");
        assert!(empty.into_groups(&[]).is_ok());
    }

    #[test]
    fn a_manifest_without_a_brief_is_still_valid() {
        let manifest =
            GuideManifest::from_model_json(r#"{"chapters":[]}"#).expect("valid manifest");
        assert!(manifest.brief().is_none());
    }

    #[test]
    fn account_claims_are_capped_and_control_characters_filtered() {
        let claims: Vec<String> =
            (0..MAX_GUIDE_ACCOUNT_ITEMS + 3).map(|index| format!("\"claim {index}\"")).collect();
        let json = manifest_json(&format!(
            r#"{{"intent":"Does\u0007 a thing.","account":[{}]}}"#,
            claims.join(",")
        ));
        let manifest = GuideManifest::from_model_json(&json).expect("valid manifest");
        let brief = manifest.brief().expect("a brief");
        assert_eq!(brief.intent, "Does a thing.", "control characters are filtered");
        assert_eq!(brief.account.len(), MAX_GUIDE_ACCOUNT_ITEMS);
    }
}
