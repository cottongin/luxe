//! Hidden — the parts of an instruction file a reader never sees, found the way a renderer does.
//!
//! HTML comments, processing instructions, declarations and CDATA render as nothing, and so do
//! comment link definitions, so they are cut. Which text is markup, which a code sample and which a
//! fence is CommonMark's answer, read off the parser's events rather than guessed line by line: a
//! comment inside a code block or a code span is a sample and stays, and an indented fence is not
//! a fence. A comment left open hides what a browser hides: everything up to the next raw `-->`,
//! across blocks, or to the end of the file.

use pulldown_cmark::{Event, Parser, Tag};
use std::ops::Range;

/// `text` without its hidden parts, and how many came out; `None` when the file cannot be checked.
pub(super) fn strip(text: &str) -> Option<(String, usize)> {
    let mut spans = hidden_markup(text);
    spans.extend(super::link_comments::spans(text)?);
    let count = spans.len();
    Some((remove(text, spans), count))
}

/// Character references, named or numeric, that decode to an invisible character.
///
/// Read off the parser so a reference in a code block or span, which shows as written, stays.
pub(super) fn invisible_references(text: &str) -> Vec<Range<usize>> {
    Parser::new(text)
        .into_offset_iter()
        .filter_map(|(event, range)| match event {
            Event::Text(decoded)
                if text[range.clone()].starts_with('&')
                    && !decoded.is_empty()
                    && decoded.chars().all(super::is_invisible) =>
            {
                Some(range)
            }
            _ => None,
        })
        .collect()
}

/// What opens each kind of markup a browser draws nothing for, and what closes it. Outside SVG
/// and MathML a browser reads `<?…`, like `<![CDATA[…` and any other `<!…`, as a bogus comment
/// ended by the first `>`.
const HIDDEN: [(&str, &str); 2] = [("<!--", "-->"), ("<?", ">")];

/// Every piece of hidden markup the parser saw as HTML, block or inline.
fn hidden_markup(text: &str) -> Vec<Range<usize>> {
    let html: Vec<Range<usize>> = Parser::new(text)
        .into_offset_iter()
        .filter_map(|(event, range)| match event {
            Event::Start(Tag::HtmlBlock) | Event::InlineHtml(_) => Some(range),
            _ => None,
        })
        .collect();
    let mut spans = Vec::new();
    let mut at = 0;
    for (index, range) in html.iter().enumerate() {
        let mut from = range.start.max(at);
        while let Some((open, closer)) = next_opener(text, from..range.end) {
            let end = closing(text, open, closer, &html[index..]).unwrap_or(text.len());
            spans.push(open..end);
            (from, at) = (end, end);
        }
    }
    spans
}

/// The first hidden opener in `within`, and what closes it. A declaration or any other `<!…`, and
/// a `</` that names no tag, closes at its first `>`. An opener counts anywhere outside a tag, read
/// the way a browser's tokenizer reads one: inside a tag, attribute values included, nothing
/// starts.
fn next_opener(text: &str, within: Range<usize>) -> Option<(usize, &'static str)> {
    let slice = text.get(within.clone())?;
    let mut tag = None;
    for (at, c) in slice.char_indices() {
        if let Some(state) = tag {
            tag = TagState::next(state, c);
            continue;
        }
        if c != '<' {
            continue;
        }
        let rest = &slice[at..];
        let named = |after: &str| after.starts_with(|c: char| c.is_ascii_alphabetic());
        if let Some((_, closer)) = HIDDEN.iter().find(|(opener, _)| rest.starts_with(opener)) {
            return Some((within.start + at, closer));
        }
        // anything else after `<!`, or `</` naming no tag, is a bogus comment to `>`
        let closing_tag = rest.strip_prefix("</");
        if rest.starts_with("<!") || closing_tag.is_some_and(|after| !named(after)) {
            return Some((within.start + at, ">"));
        }
        if named(&rest[1..]) || closing_tag.is_some() {
            tag = Some(TagState::Name);
        }
    }
    None
}

/// Where a browser's tokenizer is inside a tag: just enough of the HTML spec's states to know
/// whether a quote opens a value and which `>` ends the tag.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TagState {
    Name,
    BeforeAttribute,
    Attribute,
    AfterAttribute,
    BeforeValue,
    Quoted(char),
    AfterQuoted,
    Unquoted,
    SelfClosing,
}

impl TagState {
    /// The state after `c`, or `None` once the tag has ended.
    fn next(self, c: char) -> Option<Self> {
        use TagState::*;
        let space = c.is_ascii_whitespace();
        Some(match (self, c) {
            (Quoted(quote), _) if c == quote => AfterQuoted,
            (Quoted(_), _) => self,
            (_, '>') => return None,
            (Attribute, _) if space => AfterAttribute,
            (Name | AfterQuoted | Unquoted, _) if space => BeforeAttribute,
            (Name | BeforeAttribute | Attribute | AfterAttribute | AfterQuoted, '/') => SelfClosing,
            (Name, _) => Name,
            (BeforeAttribute | AfterAttribute | BeforeValue, _) if space => self,
            (Attribute | AfterAttribute, '=') => BeforeValue,
            (BeforeAttribute | Attribute | AfterAttribute, _) => Attribute,
            (BeforeValue, '"' | '\'') => Quoted(c),
            (BeforeValue | Unquoted, _) => Unquoted,
            // anything else reads as the start of a new attribute, the way the spec reconsumes it
            (AfterQuoted | SelfClosing, _) => return BeforeAttribute.next(c),
        })
    }
}

/// Where markup opened at `open` ends. A comment runs to its `-->` in this HTML range or a later
/// one, since text between them is escaped and cannot close it; the search starts two in, inside
/// the opener, so `<!-->` and `<!--->` end at once, as browsers end them. Anything closed by `>`
/// ends within its own range, where the renderer's next tag would end it in a browser.
fn closing(text: &str, open: usize, closer: &str, html: &[Range<usize>]) -> Option<usize> {
    let start = open + 2;
    if closer == ">" {
        let range = html.first()?;
        let found = text[start..range.end].find('>').map(|at| start + at + 1);
        return Some(found.unwrap_or(range.end));
    }
    html.iter().filter(|range| range.end > start).find_map(|range| {
        let from = range.start.max(start);
        let found = text[from..range.end].find(closer).map(|at| from + at + closer.len());
        // `--!>` closes too, but only matters before the first `-->`, so look no further; it
        // needs dashes of its own, so `<!--!>` is still open
        let until = found.unwrap_or(range.end);
        let bang_from = from.max(open + 4).min(until);
        let bang = (closer == "-->").then(|| text[bang_from..until].find("--!>")).flatten();
        bang.map(|at| bang_from + at + 4).or(found)
    })
}

/// `text` with `spans` cut. A span that had its lines to itself takes them, newline and all, and
/// one that sat between blank lines takes one of them too, so a cut leaves no gap the author never
/// wrote; anything else in front of it, a container marker or a sentence, stays as it was.
fn remove(text: &str, mut spans: Vec<Range<usize>>) -> String {
    spans.sort_by_key(|span| span.start);
    let newlines: Vec<usize> = text.match_indices('\n').map(|(at, _)| at).collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for span in spans {
        if span.end <= at {
            continue; // inside one already cut
        }
        let span = whole_lines(text, &newlines, span.start.max(at)..span.end);
        let start = span.start.max(at);
        out.push_str(&text[at..start]);
        at = span.end;
        let took_line = start == 0 || text[..start].ends_with('\n');
        if took_line && out.ends_with("\n\n") && text[at..].starts_with('\n') {
            at += 1;
        }
    }
    out.push_str(&text[at..]);
    out
}

/// `span` widened to whole lines when nothing else is on them: only then does its newline go.
fn whole_lines(text: &str, newlines: &[usize], span: Range<usize>) -> Range<usize> {
    let before = newlines.partition_point(|at| *at < span.start);
    let line_start = before.checked_sub(1).map_or(0, |index| newlines[index] + 1);
    let after = newlines.partition_point(|at| *at < span.end);
    let line_end = newlines.get(after).map_or(text.len(), |at| at + 1);
    let alone = text[line_start..span.start].trim().is_empty()
        && text[span.end..line_end].trim().is_empty();
    if alone {
        line_start..line_end
    } else {
        span
    }
}
