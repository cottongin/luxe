//! Link comments — link reference definitions used as comments, `[//]: # (…)`.
//!
//! Markdown renders a definition to nothing, so one whose destination is `#` or empty (`<>`) is a
//! comment by convention: its title, and its label too, reach the model and no reader. Whether a
//! line *is* a definition depends on all of CommonMark — containers, lazy continuation, indented
//! code, titles over several lines, escapes — so the parser's own answer is used rather than a
//! second guess at it. A definition naming a real destination is a link and stays.
//!
//! The parser keeps only the first definition of a label, and a repeated `[//]` is the usual case.
//! So the parse runs over a copy whose labels are respelled, byte for byte, in control characters
//! nobody types: every candidate label up front gets a name of its own, and each definition found
//! is renamed to a spent one before the next parse, so a definition never shadows another and the
//! copy keeps the original's every offset and its whole block structure.
//!
//! A file that still has unrenamed definitions after [`ROUNDS`] parses is no instruction file
//! anybody wrote by hand, so it is refused whole rather than guessed at.

use std::ops::Range;

/// Parses after the first before giving up on a file that reuses one short label more often than
/// there are placeholder names for it.
const ROUNDS: usize = 32;

/// What can stand in front of a definition on its line: indent, quote and list markers.
const CONTAINER: &[u8] = b" \t>-*+0123456789.)";

/// The bytes a respelled label is made of: control characters CommonMark neither folds nor reads
/// as space. The first one alone spells a spent label.
const PLACEHOLDERS: [u8; 26] = [
    1, 2, 3, 4, 5, 6, 7, 8, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
];

/// Where each comment definition sits in `text`, or `None` when the rounds ran out first.
pub(super) fn spans(text: &str) -> Option<Vec<Range<usize>>> {
    let mut scratch = text.as_bytes().to_vec();
    name_every_label(&mut scratch);
    let mut seen = std::collections::HashSet::new();
    let mut comments = Vec::new();
    for _ in 0..=ROUNDS {
        // a spent definition is recorded again under the spent name, so only new ones count
        let found: Vec<_> = definitions(as_str(&scratch))
            .into_iter()
            .filter(|(span, _)| seen.insert(span.start))
            .collect();
        if found.is_empty() {
            return Some(comments);
        }
        for (span, comment) in found {
            if comment {
                comments.push(span.clone());
            }
            if !spend(&mut scratch, span.start) && label_repeats(as_str(&scratch), span.start) {
                return None; // it would go on hiding every later definition of its label
            }
        }
    }
    None
}

/// Every definition the parser recorded, and whether it is a comment.
fn definitions(text: &str) -> Vec<(Range<usize>, bool)> {
    let parser = pulldown_cmark::Parser::new(text);
    let recorded = parser.reference_definitions().iter();
    recorded
        .map(|(_, def)| {
            let dest: String = def.dest.chars().filter(|c| *c != '\u{7f}').collect();
            (def.span.clone(), matches!(dest.as_str(), "#" | ""))
        })
        .collect()
}

fn as_str(scratch: &[u8]) -> &str {
    std::str::from_utf8(scratch)
        .expect("only whole labels between ASCII brackets are respelled, and only in ASCII")
}

/// Gives every `[label]:` that starts a line, after its container markers, a name of its own.
fn name_every_label(scratch: &mut [u8]) {
    let mut next = 1_u64;
    let mut at = 0;
    while at < scratch.len() {
        let line_end =
            scratch[at..].iter().position(|b| *b == b'\n').map_or(scratch.len(), |n| at + n);
        let skip = scratch[at..line_end].iter().take_while(|b| CONTAINER.contains(b)).count();
        if let Some(label) =
            label_at(scratch, at + skip, line_end).filter(|_| scratch.get(at + skip) == Some(&b'['))
        {
            spell(&mut scratch[label], next);
            next += 1;
        }
        at = line_end + 1;
    }
}

/// The label of a definition whose `[` is at `open`, if it closes before `limit` with `]:` and
/// may be respelled. Not one holding `<` or `>`, which every HTML end marker does, so respelling
/// cannot end or extend an HTML block; not a blank one, which is no label; and not one near the
/// parser's length limit, which respelling could move it across.
fn label_at(scratch: &[u8], open: usize, limit: usize) -> Option<Range<usize>> {
    let mut at = open + 1;
    while at < limit {
        match scratch[at] {
            b'\\' => at += 2,
            b'[' => return None,
            b']' if scratch.get(at + 1) == Some(&b':') => {
                let label = &scratch[open + 1..at];
                let respellable = !label.iter().all(u8::is_ascii_whitespace)
                    && !label.iter().any(|b| matches!(b, b'<' | b'>'))
                    && label.len() <= 500;
                return respellable.then_some(open + 1..at);
            }
            b']' => return None,
            _ => at += 1,
        }
    }
    None
}

/// Renames the definition whose `[` is at `open` to the spent label, so the next parse finds the
/// one its old name was hiding; `false` when its label cannot be respelled.
fn spend(scratch: &mut [u8], open: usize) -> bool {
    let Some(label) = label_at(scratch, open, scratch.len()) else { return false };
    for byte in &mut scratch[label] {
        if !byte.is_ascii_whitespace() {
            *byte = PLACEHOLDERS[0];
        }
    }
    true
}

/// Whether the definition at `open` has a label that some later `[label]:` matches, compared the
/// way CommonMark compares labels: case folded, runs of whitespace as one space.
fn label_repeats(text: &str, open: usize) -> bool {
    let Some(close) = text[open..].find("]:") else { return false };
    let wanted = normalized(&text[open + 1..open + close]);
    let mut rest = open + close + 2;
    while let Some(at) = text[rest..].find('[') {
        let start = rest + at + 1;
        let Some(end) = text[start..].find(']') else { return false };
        let candidate = &text[start..start + end];
        if text[start + end..].starts_with("]:") && normalized(candidate) == wanted {
            return true;
        }
        rest = start;
    }
    false
}

fn normalized(label: &str) -> String {
    label.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Writes `name` into `label` in placeholder digits; a short label runs out of names and wraps,
/// which only costs another round.
fn spell(label: &mut [u8], mut name: u64) {
    for byte in label.iter_mut() {
        *byte = PLACEHOLDERS[(name % PLACEHOLDERS.len() as u64) as usize];
        name /= PLACEHOLDERS.len() as u64;
    }
    if label.iter().all(|byte| *byte == PLACEHOLDERS[0]) {
        label[0] = PLACEHOLDERS[1]; // never the spent name
    }
}
