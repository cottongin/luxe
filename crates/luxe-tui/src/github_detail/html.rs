//! HTML in GitHub bodies — reduced to the markdown it amounts to, so it reads as GitHub shows it.
//!
//! GitHub renders the HTML people and bots put in descriptions and comments — Dependabot's commit
//! lists are `<details>` around `<ul>` of `<a><code>` — and CommonMark treats an HTML block as
//! opaque, so it reached the card as tags. This maps the handful that carry structure (lists,
//! links, code, emphasis, a `<summary>`'s heading) to markdown, drops the rest, and decodes the
//! entities. Code is left alone: a fenced block or an inline span that shows HTML means to show it.

/// `body` with its HTML reduced to markdown, outside code.
pub(super) fn readable(body: &str) -> String {
    if !body.contains('<') && !body.contains('&') {
        return body.to_string();
    }
    let mut out = String::with_capacity(body.len());
    let mut fence: Option<&str> = None;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let marker = ["```", "~~~"].into_iter().find(|marker| trimmed.starts_with(marker));
        match (fence, marker) {
            (None, Some(marker)) => {
                fence = Some(marker);
                out.push_str(line);
            }
            (Some(open), Some(marker)) if open == marker => {
                fence = None;
                out.push_str(line);
            }
            (Some(_), _) => out.push_str(line),
            (None, None) => reduce_line(line, &mut out),
        }
    }
    collapse_blank_runs(&out)
}

/// One line outside a fence, its tags reduced; inline code spans pass through untouched.
fn reduce_line(line: &str, out: &mut String) {
    let mut links: Vec<String> = Vec::new();
    let mut rest = line;
    let mut in_code = false;
    while let Some(index) = rest.find(['<', '`', '&']) {
        let (before, from) = rest.split_at(index);
        out.push_str(before);
        if let Some(after) = from.strip_prefix('`') {
            in_code = !in_code;
            out.push('`');
            rest = after;
            continue;
        }
        if in_code {
            out.push_str(&from[..1]);
            rest = &from[1..];
            continue;
        }
        if from.starts_with('&') {
            let (decoded, used) = entity(from);
            out.push_str(decoded);
            rest = &from[used..];
            continue;
        }
        let Some(end) = from.find('>') else {
            out.push_str(from);
            return;
        };
        let tag = &from[1..end];
        // `a < b` is prose, not a tag
        if tag.is_empty() || tag.starts_with(char::is_whitespace) {
            out.push('<');
            rest = &from[1..];
            continue;
        }
        let markdown = markdown_for(tag, &mut links);
        // An item already at the start of a line opens no blank one, which would loosen the list
        let at_line_start = out.is_empty() || out.ends_with('\n');
        match markdown.strip_prefix("\n- ") {
            Some(rest) if at_line_start => {
                out.push_str("- ");
                out.push_str(rest);
            }
            _ => out.push_str(&markdown),
        }
        rest = &from[end + 1..];
    }
    out.push_str(rest);
}

/// What one tag becomes. Links keep their address on a stack until they close.
fn markdown_for(tag: &str, links: &mut Vec<String>) -> String {
    let closing = tag.starts_with('/');
    let name: String = tag
        .trim_start_matches('/')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    match (name.as_str(), closing) {
        ("summary", false) => "\n**".into(),
        ("summary", true) => "**\n".into(),
        ("li", false) => "\n- ".into(),
        ("br" | "p" | "div" | "details" | "ul" | "ol" | "blockquote" | "table" | "tr", _) => {
            "\n".into()
        }
        ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => "\n### ".into(),
        ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => "\n".into(),
        ("code" | "kbd" | "samp", _) => "`".into(),
        ("b" | "strong", _) => "**".into(),
        ("i" | "em", _) => "*".into(),
        ("a", false) => match attribute(tag, "href") {
            Some(href) => {
                links.push(href);
                "[".into()
            }
            None => String::new(),
        },
        ("a", true) => links.pop().map(|href| format!("]({href})")).unwrap_or_default(),
        ("img", false) => attribute(tag, "alt").unwrap_or_default(),
        ("td" | "th", false) => " ".into(),
        _ => String::new(),
    }
}

/// The value of attribute `name` in a tag's text, quoted either way.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let at = lower.find(&format!("{name}="))? + name.len() + 1;
    let value = &tag[at..];
    let quote = value.chars().next()?;
    let inner = if quote == '"' || quote == '\'' {
        let value = &value[1..];
        &value[..value.find(quote)?]
    } else {
        value.split(char::is_whitespace).next()?
    };
    let mut decoded = String::new();
    reduce_line(inner, &mut decoded);
    Some(decoded)
}

/// The character an entity at the start of `from` names, and how many bytes it took; an `&` that
/// starts none of these is itself.
fn entity(from: &str) -> (&'static str, usize) {
    const NAMED: &[(&str, &str)] = &[
        ("&amp;", "&"),
        ("&lt;", "<"),
        ("&gt;", ">"),
        ("&quot;", "\""),
        ("&#39;", "'"),
        ("&apos;", "'"),
        ("&nbsp;", " "),
    ];
    NAMED
        .iter()
        .find(|(name, _)| from.starts_with(name))
        .map_or(("&", 1), |(name, value)| (*value, name.len()))
}

/// No more than one blank line in a row: block tags each opened a line of their own.
fn collapse_blank_runs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.split('\n') {
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out.truncate(out.trim_end().len());
    out
}

#[cfg(test)]
mod tests {
    use super::readable;

    /// Dependabot's commit list, from luxe-dev #197's real body: a summary heading, then a list
    /// of linked commit ids, with an entity decoded and no tag left.
    #[test]
    fn a_dependabot_commit_list_reads_as_a_list() {
        let body = "Updates `rustix` from 1.1.4 to 1.1.5\n<details>\n<summary>Commits</summary>\n<ul>\n<li><a href=\"https://github.com/bytecodealliance/rustix/commit/287214b\"><code>287214b</code></a> chore: Release rustix version 1.1.5</li>\n<li><a href=\"https://github.com/bytecodealliance/rustix/commit/0cea188\"><code>0cea188</code></a> fix(bug): ns-&gt;us overflow handling</li>\n</ul>\n</details>";
        assert_eq!(
            readable(body),
            "Updates `rustix` from 1.1.4 to 1.1.5\n\n**Commits**\n\n- \
             [`287214b`](https://github.com/bytecodealliance/rustix/commit/287214b) chore: \
             Release rustix version 1.1.5\n- \
             [`0cea188`](https://github.com/bytecodealliance/rustix/commit/0cea188) fix(bug): \
             ns->us overflow handling"
        );
    }

    /// HTML a body shows on purpose — fenced, or in an inline span — is left as written, and so
    /// is a `<` that starts no tag.
    #[test]
    fn code_and_comparisons_keep_their_angle_brackets() {
        let body = "Renders `<div>` now.\n\n```html\n<b>bold</b>\n```\nWhen a < b, <b>stop</b>.";
        assert_eq!(
            readable(body),
            "Renders `<div>` now.\n\n```html\n<b>bold</b>\n```\nWhen a < b, **stop**."
        );
    }

    /// A body with no HTML comes back as it was.
    #[test]
    fn plain_markdown_is_untouched() {
        let body = "Fixes [#12](https://github.com/a/b/issues/12).\n\n- one\n- two";
        assert_eq!(readable(body), body);
    }
}
