//! Rendering — search hits as text a model can read, and the HTML to plain-text clean-up they need.

use super::*;

/// `YYYY-MM-DD` for a Unix timestamp, by civil-date arithmetic (no date dependency in the tree).
pub(super) fn iso_date(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Results as compact markdown: the queries that ran, then each hit with its passage.
///
/// Not raw JSON. A pretty-printed array of `{title, url, snippet}` spends a third of its characters
/// on punctuation and field names the model already knows.
///
/// `caveats` names any filter the backend could not apply, and leads rather than trails: a model
/// reading a domain-filtered list that was never filtered will draw a conclusion from it, and the
/// correction has to arrive before the results do.
pub(super) fn render_hits(queries: &[String], hits: &[SearchHit], caveats: &[String]) -> String {
    let mut out = String::new();
    for caveat in caveats {
        out.push_str(&format!("Note: {caveat}\n"));
    }
    if !caveats.is_empty() {
        out.push('\n');
    }
    if hits.is_empty() {
        out.push_str(&format!("No results for {}.", queries.join(" / ")));
        return out;
    }
    if queries.len() > 1 {
        out.push_str(&format!("Searched: {}\n\n", queries.join(" · ")));
    }
    for (index, hit) in hits.iter().enumerate() {
        out.push_str(&format!(
            "{}. {}\n   {}\n",
            index + 1,
            plain_text(&hit.title, TITLE_CHARS),
            hit.url
        ));
        let snippet = plain_text(&hit.snippet, SNIPPET_CHARS);
        if !snippet.is_empty() {
            out.push_str(&format!("   {snippet}\n"));
        }
    }
    crate::output_limit::cap_lines(
        out.trim_end(),
        SEARCH_MAX_LINES,
        "lines",
        "Narrow the query, or `web_fetch` a specific result for its full text.",
    )
}

/// Most rendered lines a search returns, matching the bound the other search tools apply.
pub(super) const SEARCH_MAX_LINES: usize = 200;

/// Longest snippet kept per result. Generous, because answering without a follow-up fetch is the
/// point — but bounded, because Firecrawl's description can be several thousand characters of a
/// page's markdown, and ten of those is a search that costs more than the page it replaced.
pub(super) const SNIPPET_CHARS: usize = 1_000;

/// Longest title kept. A title that runs on is a page with a bad title, not information.
pub(super) const TITLE_CHARS: usize = 160;

/// One line of readable text from whatever markup a vendor happened to send.
///
/// Every backend decorates differently and none of it survives being read aloud: Brave wraps the
/// matched words in `<strong>`, Firecrawl hands back the page's own markdown — headings, fences and
/// all — and any of them may return an HTML entity. Left alone, a numbered list of results turns
/// into a numbered list of markup, which costs tokens and reads worse.
pub(super) fn plain_text(raw: &str, limit: usize) -> String {
    let raw = unlink(raw);
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    let mut last_was_space = true;
    for ch in raw.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if in_tag => {}
            _ if ch.is_whitespace() => {
                if !last_was_space {
                    out.push(' ');
                    last_was_space = true;
                }
            }
            _ => {
                out.push(ch);
                last_was_space = false;
            }
        }
    }
    let out = decode_entities(out.trim());
    match out.chars().count() > limit {
        true => out.chars().take(limit).collect::<String>() + "…",
        false => out,
    }
}

/// `[text](url)` → `text`.
///
/// A result already carries its own URL on the line above, so a snippet's inline links are pure
/// weight — and Firecrawl's descriptions are the page's markdown, where a docs.rs paragraph can
/// spend more characters on link targets than on prose.
pub(super) fn unlink(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(open) = rest.find('[') {
        let (before, from_open) = rest.split_at(open);
        // `[text](url)`, with no nesting and no line break inside — anything else is not a link.
        let link = from_open
            .find("](")
            .filter(|close| !from_open[1..*close].contains(['[', '\n']))
            .and_then(|close| {
                let after_text = &from_open[close + 2..];
                let end = after_text.find(')')?;
                let target = &after_text[..end];
                // The target has to look like one, or `[0](1)` is an array index read as a link.
                let addressed = target.starts_with("http")
                    || target.starts_with(['/', '#', '.'])
                    || target.starts_with("mailto:");
                (addressed && !target.contains(' '))
                    .then_some((&from_open[1..close], close + 2 + end + 1))
            });
        out.push_str(before);
        match link {
            Some((text, consumed)) => {
                out.push_str(text);
                rest = &from_open[consumed..];
            }
            None => {
                out.push('[');
                rest = &from_open[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The handful of HTML entities that actually show up in a search snippet.
pub(super) fn decode_entities(text: &str) -> String {
    // `&amp;` last, so `&amp;lt;` decodes to `&lt;` rather than to `<`.
    [
        ("&quot;", "\""),
        ("&#x27;", "'"),
        ("&#39;", "'"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&nbsp;", " "),
        ("&amp;", "&"),
    ]
    .iter()
    .fold(text.to_string(), |text, (entity, glyph)| text.replace(entity, glyph))
}
