//! Markdown as HTML — model text for the phone app, drawn the way the transcript draws it.
//!
//! The line renderer lays text out in cells at a width it knows; a browser reflows on every turn of
//! the phone, so this walks the same events, in the same dialect, into elements instead, and keeps
//! the TUI's choices: the alert's word and mark, a fence's language and copy affordance on its
//! rule, a link's address after its words. How those look (bullets, `> `, the rules) is the app's
//! stylesheet. Code is highlighted into the same roles the terminal paints, each a class naming
//! its theme token (`t-accent-secondary`), never an inline style: the app's CSP forbids those, and
//! the token's color comes from the user's theme as it is now.
//!
//! What the model wrote is data: every character of it is escaped, raw HTML shows as its source
//! the way the TUI shows it, and a link only gets an `href` when it leaves for the web or mail.

use pulldown_cmark::{Alignment, BlockQuoteKind, CodeBlockKind, Event, Parser, Tag, TagEnd};

use super::fence::fence_language;
use super::layout::parser_options;
use super::voice::{alert_icon, alert_label};
use crate::glyphs::{GlyphTier, Icon};
use crate::highlight::{highlight_roles, Role};

/// Which reader a block is rendered for, as in the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Voice {
    /// Assistant prose, fences highlighted
    #[default]
    Answer,
    /// Model reasoning: line breaks kept, fences left plain so the block stays muted
    Reasoning,
}

/// The app's font ships with the Nerd Font marks, so every glyph is drawn at that tier.
const TIER: GlyphTier = GlyphTier::Nerd;

/// `src` as HTML for `voice`, with no wrapping element: the app supplies that and its class.
///
/// Partial input is fine. A reply is rendered as it streams, and pulldown-cmark closes whatever is
/// still open at the end, so an unterminated fence is a fence.
pub fn render(src: &str, voice: Voice) -> String {
    let mut writer = HtmlWriter { voice, ..HtmlWriter::default() };
    for event in Parser::new_ext(src, parser_options()) {
        writer.event(event);
    }
    writer.out
}

/// One source line highlighted as HTML, the way a diff row's code is: on its own, as the TUI's
/// diff views highlight it. Everything in `text` comes back escaped.
pub fn code_line(text: &str, lang: Option<&str>) -> String {
    let mut out = String::new();
    // the highlighter keeps every character of a line, so only empty text has no first line
    for (role, part) in highlight_roles(text, lang).first().into_iter().flatten() {
        role_span_into(&mut out, *role, part);
    }
    out
}

/// `text` in the span for its highlight role, or bare when the role is body text.
fn role_span_into(out: &mut String, role: Role, text: &str) {
    let Some(class) = role_class(role) else { return escape_into(out, text) };
    out.push_str(&format!("<span class=\"{class}\">"));
    escape_into(out, text);
    out.push_str("</span>");
}

/// One walk's state: the HTML so far, and the open things whose end changes what is written.
#[derive(Default)]
struct HtmlWriter {
    out: String,
    voice: Voice,
    link: Option<Link>,
    fence: Option<Fence>,
    table: Option<Table>,
}

/// A link being written: where it goes, what it says so far, and whether it got an `href`.
struct Link {
    url: String,
    words: String,
    live: bool,
}

/// A code block being collected, since it is highlighted whole.
struct Fence {
    lang: Option<String>,
    code: String,
}

/// A table being written: how each column aligns, and which cell is next.
struct Table {
    alignments: Vec<Alignment>,
    cell: usize,
    head: bool,
}

impl HtmlWriter {
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            // block HTML shows as the source the model wrote, as it does in the terminal
            Event::Text(text) | Event::Html(text) => self.text(&text),
            Event::Code(code) => self.code(&code),
            Event::SoftBreak if self.voice == Voice::Reasoning => self.out.push_str("<br>"),
            Event::SoftBreak => self.out.push(' '),
            Event::HardBreak => self.out.push_str("<br>"),
            Event::Rule => self.out.push_str("<hr>"),
            // inline HTML, footnotes and the rest: the terminal leaves them out too
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.out.push_str("<p>"),
            Tag::Heading { level, .. } => self.out.push_str(&format!("<{level}>")),
            Tag::BlockQuote(kind) => self.open_quote(kind),
            Tag::CodeBlock(kind) => {
                self.fence = Some(Fence { lang: fence_lang(kind), code: "".into() })
            }
            Tag::List(Some(1)) => self.out.push_str("<ol>"),
            Tag::List(Some(start)) => self.out.push_str(&format!("<ol start=\"{start}\">")),
            Tag::List(None) => self.out.push_str("<ul>"),
            Tag::Item => self.out.push_str("<li>"),
            Tag::Table(alignments) => self.open_table(alignments),
            Tag::TableHead | Tag::TableRow => self.open_row(),
            Tag::TableCell => self.open_cell(),
            Tag::Emphasis => self.out.push_str("<em>"),
            Tag::Strong => self.out.push_str("<strong>"),
            Tag::Strikethrough => self.out.push_str("<del>"),
            Tag::Link { dest_url, .. } => self.open_link(&dest_url),
            // an image's words are its alt text, which follows as text
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        let close = match tag {
            TagEnd::Paragraph => "</p>",
            TagEnd::Heading(level) => return self.out.push_str(&format!("</{level}>")),
            TagEnd::BlockQuote(_) => "</blockquote>",
            TagEnd::CodeBlock => return self.close_fence(),
            TagEnd::List(true) => "</ol>",
            TagEnd::List(false) => "</ul>",
            TagEnd::Item => "</li>",
            TagEnd::Table => return self.close_table(),
            TagEnd::TableHead => return self.close_head(),
            TagEnd::TableRow => "</tr>",
            TagEnd::TableCell => return self.close_cell(),
            TagEnd::Emphasis => "</em>",
            TagEnd::Strong => "</strong>",
            TagEnd::Strikethrough => "</del>",
            TagEnd::Link => return self.close_link(),
            _ => "",
        };
        self.out.push_str(close);
    }

    fn text(&mut self, text: &str) {
        if let Some(fence) = &mut self.fence {
            return fence.code.push_str(text);
        }
        if let Some(link) = &mut self.link {
            link.words.push_str(text);
        }
        escape_into(&mut self.out, text);
    }

    fn code(&mut self, code: &str) {
        if let Some(link) = &mut self.link {
            link.words.push_str(code);
        }
        self.out.push_str("<code>");
        escape_into(&mut self.out, code);
        self.out.push_str("</code>");
    }

    /// Opens a quote; an alert writes its word out first, since its severity is in the tag rather
    /// than in its prose, and the color is only polish.
    fn open_quote(&mut self, kind: Option<BlockQuoteKind>) {
        let Some(kind) = kind else { return self.out.push_str("<blockquote>") };
        let label = alert_label(kind);
        self.out.push_str(&format!("<blockquote class=\"alert {}\">", label.to_lowercase()));
        self.out.push_str("<p class=\"alert-label\">");
        escape_into(&mut self.out, &icon_label(alert_icon(kind), label));
        self.out.push_str("</p>");
    }

    fn open_link(&mut self, url: &str) {
        let live = leaves_for_the_web(url);
        if live {
            self.out.push_str("<a href=\"");
            escape_into(&mut self.out, url);
            self.out.push_str("\" target=\"_blank\" rel=\"noopener noreferrer\">");
        } else {
            self.out.push_str("<span class=\"link\">");
        }
        self.link = Some(Link { url: url.to_string(), words: String::new(), live });
    }

    /// Closes a link and writes its address after it, once, unless the words already are the
    /// address or a table cell holds it, as the terminal does.
    fn close_link(&mut self) {
        let Some(link) = self.link.take() else { return };
        self.out.push_str(if link.live { "</a>" } else { "</span>" });
        let repeats = link.url.trim_start_matches("mailto:") == link.words.trim();
        if link.url.is_empty() || repeats || self.table.is_some() {
            return;
        }
        self.out.push_str("<span class=\"url\"> (");
        escape_into(&mut self.out, &link.url);
        self.out.push_str(")</span>");
    }
}

impl HtmlWriter {
    /// Writes a code block between its rules: the language and copy on the top one. An empty
    /// block draws nothing, as in the terminal.
    fn close_fence(&mut self) {
        let Some(fence) = self.fence.take() else { return };
        if fence.code.is_empty() {
            return;
        }
        self.out.push_str("<figure class=\"fence\"><figcaption>");
        if let Some(language) = fence_language(fence.lang.as_deref()) {
            self.out.push_str("<span class=\"lang\">");
            let icon = crate::widgets::language_icon(&language);
            escape_into(&mut self.out, &icon_label(icon, &language));
            self.out.push_str("</span>");
        }
        self.out.push_str("<span class=\"fill\"></span><button type=\"button\" class=\"copy\">");
        escape_into(&mut self.out, &icon_label(Icon::Copy, "copy"));
        self.out.push_str("</button></figcaption><pre><code>");
        self.fence_body(&fence);
        self.out.push_str("</code></pre></figure>");
    }

    /// The code, highlighted for an answer and plain for reasoning, which a palette would pull out
    /// of its muted voice.
    fn fence_body(&mut self, fence: &Fence) {
        if self.voice == Voice::Reasoning {
            return escape_into(&mut self.out, fence.code.trim_end_matches('\n'));
        }
        let lines = highlight_roles(&fence.code, fence.lang.as_deref());
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                self.out.push('\n');
            }
            for (role, text) in line {
                role_span_into(&mut self.out, *role, text);
            }
        }
    }

    fn open_table(&mut self, alignments: Vec<Alignment>) {
        self.table = Some(Table { alignments, cell: 0, head: true });
        self.out.push_str("<div class=\"table\"><table><thead>");
    }

    fn open_row(&mut self) {
        if let Some(table) = &mut self.table {
            table.cell = 0;
        }
        self.out.push_str("<tr>");
    }

    fn open_cell(&mut self) {
        let Some(table) = &mut self.table else { return };
        let tag = if table.head { "th" } else { "td" };
        let align = match table.alignments.get(table.cell) {
            Some(Alignment::Right) => " class=\"right\"",
            Some(Alignment::Center) => " class=\"center\"",
            _ => "",
        };
        self.out.push_str(&format!("<{tag}{align}>"));
    }

    fn close_cell(&mut self) {
        let Some(table) = &mut self.table else { return };
        self.out.push_str(if table.head { "</th>" } else { "</td>" });
        table.cell += 1;
    }

    /// pulldown-cmark puts the header cells straight under the head, with no row of their own.
    fn close_head(&mut self) {
        if let Some(table) = &mut self.table {
            table.head = false;
        }
        self.out.push_str("</tr></thead><tbody>");
    }

    fn close_table(&mut self) {
        self.table = None;
        self.out.push_str("</tbody></table></div>");
    }
}

/// The language a code block declared: an indented one declares none.
fn fence_lang(kind: CodeBlockKind<'_>) -> Option<String> {
    match kind {
        CodeBlockKind::Fenced(lang) => Some(lang.to_string()),
        CodeBlockKind::Indented => None,
    }
}

/// `{glyph} {label}`, the shape the terminal's labels take at the Nerd tier.
fn icon_label(icon: Icon, label: &str) -> String {
    format!("{} {label}", icon.at(TIER))
}

/// The classes a role is drawn with: its theme token, then its weight or slant. Plain text takes
/// none, which keeps a highlighted block about half the size.
fn role_class(role: Role) -> Option<String> {
    use ratatui::style::Modifier;
    if role == Role::Text {
        return None;
    }
    let (token, modifier) = role.token();
    let mut class = format!("t-{}", luxe_core::theme::css::css_name(token));
    if modifier.contains(Modifier::BOLD) {
        class.push_str(" b");
    }
    if modifier.contains(Modifier::ITALIC) {
        class.push_str(" i");
    }
    Some(class)
}

/// Whether a link may be followed from the app: to the web or to mail, never `javascript:`, a
/// `data:` page, or a path on the bridge itself.
fn leaves_for_the_web(url: &str) -> bool {
    let lower = url.trim_start().to_ascii_lowercase();
    ["https://", "http://", "mailto:"].iter().any(|scheme| lower.starts_with(scheme))
}

/// Appends `text` with every character that means something in HTML escaped, quotes included,
/// since some of it lands in attributes.
fn escape_into(out: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(character),
        }
    }
}

#[cfg(test)]
mod tests;
