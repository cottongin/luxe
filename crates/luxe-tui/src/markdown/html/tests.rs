use super::{code_line, render, Voice};

fn answer(src: &str) -> String {
    render(src, Voice::Answer)
}

/// What a model writes is shown, never run: block HTML is escaped and inline HTML left out.
///
/// The break this catches: text written into the page unescaped, which would let a reply run
/// script in the app.
#[test]
fn model_html_is_shown_not_run() {
    let html = answer("<script>alert(1)</script>\n\nhi <img src=x onerror=alert(1)> & bye");
    assert!(!html.contains("<script"), "{html}");
    assert!(!html.contains("<img"), "{html}");
    assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"), "{html}");
    assert!(html.contains("&amp; bye"), "{html}");
}

/// The break this catches: any scheme getting an `href`, so a tap on `javascript:` runs it.
#[test]
fn only_web_and_mail_links_are_followed() {
    let unsafe_link = answer("[tap](javascript:alert(1))");
    assert!(!unsafe_link.contains("href"), "{unsafe_link}");
    assert!(unsafe_link.contains("<span class=\"link\">tap</span>"), "{unsafe_link}");
    let page = answer("[docs](https://example.com/a)");
    assert!(page.contains("<a href=\"https://example.com/a\""), "{page}");
    assert!(answer("[me](mailto:a@b.c)").contains("href=\"mailto:a@b.c\""));
    assert!(!answer("[up](/api/sessions)").contains("href"));
}

/// The break this catches: an address written into `href` unescaped, so a quote in it closes the
/// attribute and opens one of the model's choosing.
#[test]
fn a_link_cannot_add_attributes() {
    let html = answer("[a](<https://x.y/\" onclick=\"steal()>)");
    assert!(!html.contains("\" onclick"), "{html}");
    assert!(html.contains("&quot; onclick=&quot;steal()"), "{html}");
}

/// The address follows the words, as in the terminal, but not when the words are the address.
///
/// The break this catches: the repeat check dropped, doubling every bare link.
#[test]
fn a_link_names_its_address_unless_its_words_are_it() {
    let named = answer("[docs](https://example.com)");
    assert!(named.contains("<span class=\"url\"> (https://example.com)</span>"), "{named}");
    let bare = answer("<https://example.com>");
    assert!(!bare.contains("class=\"url\""), "{bare}");
}

/// The break this catches: a fence losing its language label, copy control or highlighting
/// classes, which are what make it read like the terminal's.
#[test]
fn a_fence_is_framed_with_its_language_and_copy() {
    let html = answer("```Rust\nfn main() {}\n```");
    assert!(html.contains("<span class=\"lang\">"), "{html}");
    assert!(html.contains(" rust</span>"), "{html}");
    assert!(html.contains("<button type=\"button\" class=\"copy\">"), "{html}");
    assert!(html.contains("<span class=\"t-accent-secondary\">fn</span>"), "{html}");
    assert!(html.contains("<span class=\"t-accent\">main</span>"), "{html}");
}

/// The break this catches: a diff row's code left unhighlighted, highlighted for the wrong
/// language, or written into the page unescaped.
#[test]
fn a_line_of_code_is_highlighted_and_escaped() {
    let html = code_line("fn main() { a < b }", Some("rs"));
    assert!(html.starts_with("<span class=\"t-accent-secondary\">fn</span>"), "{html}");
    assert!(html.contains("&lt;"), "{html}");
    assert!(!html.contains(" < "), "{html}");
    assert_eq!(code_line("a < b", None), "a &lt; b");
}

/// A fence that says it is plain text names no language, and an empty one draws nothing.
///
/// The break this catches: the label taken from the raw info string instead of
/// `fence_language`, or an empty block drawn as an empty frame.
#[test]
fn a_plain_or_empty_fence_names_nothing() {
    let plain = answer("```text\nx\n```");
    assert!(plain.contains("<figure class=\"fence\">"), "{plain}");
    assert!(!plain.contains("class=\"lang\""), "{plain}");
    assert_eq!(answer("```\n```"), "");
}

/// Streaming input ends wherever the model has got to; a fence still open is still a fence.
///
/// The break this catches: code collected and only written at a closing fence that has not
/// arrived yet, so a streaming block shows nothing.
#[test]
fn an_unfinished_fence_is_still_drawn() {
    let html = answer("Here:\n\n```rust\nlet x");
    assert!(html.contains("<pre><code>"), "{html}");
    assert!(html.contains("x</code>"), "{html}");
}

/// Reasoning keeps its line breaks and leaves its code plain; an answer reflows.
///
/// The break this catches: the voice ignored, so reasoning runs its notes together or wears a
/// syntax palette.
#[test]
fn reasoning_keeps_breaks_and_plain_code() {
    assert_eq!(render("a\nb", Voice::Reasoning), "<p>a<br>b</p>");
    assert_eq!(answer("a\nb"), "<p>a b</p>");
    let fence = render("```rust\nfn main() {}\n```", Voice::Reasoning);
    assert!(!fence.contains("t-"), "{fence}");
    assert!(fence.contains("fn main() {}</code>"), "{fence}");
}

/// The break this catches: an alert drawn as a plain quote, losing the only thing that made it
/// one.
#[test]
fn an_alert_writes_its_word() {
    let html = answer("> [!WARNING]\n> careful");
    assert!(html.starts_with("<blockquote class=\"alert warning\"><p class=\"alert-label\">"));
    assert!(html.contains(" Warning</p>"), "{html}");
    assert!(html.contains("<p>careful</p>"), "{html}");
}

/// The break this catches: the column count not reset per row, so the second body row takes
/// alignments that belong to cells past the end.
#[test]
fn table_cells_keep_their_column_alignment() {
    let html = answer("| a | b |\n|---|--:|\n| 1 | 2 |\n| 3 | 4 |");
    assert!(html.contains("<thead><tr><th>a</th><th class=\"right\">b</th></tr></thead>"));
    assert!(html.contains("<tr><td>3</td><td class=\"right\">4</td></tr>"), "{html}");
}

/// The break this catches: a list's start number dropped, renumbering a list the model resumed.
#[test]
fn an_ordered_list_keeps_its_start() {
    assert!(answer("3. x\n4. y").starts_with("<ol start=\"3\"><li>x</li>"));
    assert!(answer("1. x").starts_with("<ol><li>"));
}
