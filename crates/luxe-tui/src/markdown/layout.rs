//! Markdown layout — the walk over pulldown-cmark's events that turns source into lines.
//!
//! One forward walk, with each independent piece of state owned by what it describes: the inline
//! run being built, the list nesting, the fence being collected, the table being collected. The
//! dispatcher only routes; each arm moves one of those owners or calls a builder.

use super::fence::FenceBuilder;
use super::line::MarkdownLine;
use super::table::TableBuilder;
use super::voice::{
    alert_icon, alert_label, build_alert_style, build_code_style, build_heading_style,
    build_hr_style, build_link_style, build_link_url_style, build_quote_style, build_style,
    build_text_style, MarkdownVoice,
};
use crate::theme::styles::ThemeStyles;
use pulldown_cmark::TagEnd;
use pulldown_cmark::{BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};

/// True when a rendered line carries no visible content (guards against double blanks).
fn is_blank_md_line(line: &MarkdownLine) -> bool {
    // A fence's frame carries its content in `fence_rule` and leaves `line` empty, so the span
    // check alone called a rule blank — and the gap after a code block went missing, running the
    // closing rule straight into the next paragraph.
    line.fence_rule.is_none()
        && line.table.is_none()
        && line.line.spans.iter().all(|span| span.content.trim().is_empty())
}

/// Inserts a single blank line between top-level blocks so paragraphs, lists, headings, code, and
/// tables read as distinct blocks instead of running together. No-op at the start of the output or
/// right after an existing blank.
fn push_block_gap(lines: &mut Vec<MarkdownLine>) {
    if lines.last().is_some_and(|last| !is_blank_md_line(last)) {
        lines.push(MarkdownLine::prose(Line::default()));
    }
}

/// The dialect model markdown is read in, here and in the phone app's HTML.
pub(super) fn parser_options() -> Options {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);
    // Alerts (`> [!WARNING]`) are the GitHub dialect models were trained on and write unprompted.
    // Without this the tag is body text, and the quote opens with a literal `[!WARNING]`.
    opts.insert(Options::ENABLE_GFM);
    opts
}

/// Renders markdown in `voice`, preserving the structural block kinds conversation layout needs.
pub(crate) fn layout_markdown_in(
    src: &str,
    theme: &ThemeStyles,
    voice: MarkdownVoice,
) -> Vec<MarkdownLine> {
    let mut renderer = MarkdownRenderer::new(theme, voice);
    for event in Parser::new_ext(src, parser_options()) {
        renderer.event(event);
    }
    // Flush any remaining spans
    renderer.flush_prose();
    renderer.lines
}

/// The inline run being built: its spans, the modifiers open around them, and the link they sit in.
#[derive(Default)]
struct InlineState {
    line: Vec<Span<'static>>,
    style_stack: Vec<Modifier>,
    /// The url while inside a link
    link_url: Option<String>,
    /// What the link says, to leave out a url that repeats it
    link_text: String,
}

/// List nesting: how deep, whether each level is ordered, and each level's count so far.
#[derive(Default)]
struct ListState {
    depth: usize,
    /// Right after an item opens, so its first paragraph does not flush the prefix away
    item_just_started: bool,
    ordered: Vec<bool>,
    counters: Vec<usize>,
}

impl ListState {
    fn begin(&mut self, ordered: bool) {
        self.depth += 1;
        self.ordered.push(ordered);
        self.counters.push(0);
    }

    fn end(&mut self) {
        self.depth = self.depth.saturating_sub(1);
        self.ordered.pop();
        self.counters.pop();
    }

    /// The next item's prefix at the current depth: its number, or a bullet.
    fn next_prefix(&mut self) -> String {
        // depth is already incremented by Start(List), so top-level items have depth 1
        let indent = "  ".repeat(self.depth.saturating_sub(1));
        let counter = self
            .counters
            .last_mut()
            .expect("an item opens inside a list, and every list pushes its counter");
        *counter += 1;
        if self.ordered.last().copied().unwrap_or(false) {
            format!("{indent}{counter}. ")
        } else {
            format!("{indent}• ")
        }
    }
}

/// One walk's state, and the lines it has laid out so far.
struct MarkdownRenderer<'a> {
    theme: &'a ThemeStyles,
    voice: MarkdownVoice,
    lines: Vec<MarkdownLine>,
    inline: InlineState,
    list: ListState,
    fence: FenceBuilder,
    table: TableBuilder,
    heading_level: Option<HeadingLevel>,
    in_blockquote: bool,
    /// Right after an alert's word, so its gap is suppressed
    alert_just_labeled: bool,
}

impl<'a> MarkdownRenderer<'a> {
    fn new(theme: &'a ThemeStyles, voice: MarkdownVoice) -> Self {
        Self {
            theme,
            voice,
            lines: Vec::new(),
            inline: InlineState::default(),
            list: ListState::default(),
            fence: FenceBuilder::default(),
            table: TableBuilder::default(),
            heading_level: None,
            in_blockquote: false,
            alert_just_labeled: false,
        }
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Code(text) => self.code(&text),
            Event::Html(html) => {
                let span = Span::styled(html.to_string(), build_text_style(self.theme, self.voice));
                self.quote_prefix();
                self.append_inline(span, false);
            }
            Event::Text(text) => self.text(&text),
            Event::SoftBreak | Event::HardBreak => {
                let style =
                    build_style(&self.inline.style_stack, build_text_style(self.theme, self.voice));
                let hard = matches!(event, Event::HardBreak) || self.voice.breaks_on_soft_break();
                self.append_inline(Span::styled(if hard { "\n" } else { " " }, style), false);
            }
            Event::Rule => {
                // Horizontal rule — flush pending, add themed styled line
                self.flush_prose();
                push_block_gap(&mut self.lines);
                let hr_style = build_hr_style(self.theme, self.voice);
                let hr_span =
                    Span::styled("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━", hr_style);
                self.lines.push(MarkdownLine::prose(Line::from(vec![hr_span])));
            }
            Event::TaskListMarker(_) => {
                // Checkbox — treat as plain text for now
                let span = Span::styled("[ ] ", build_text_style(self.theme, self.voice));
                self.append_inline(span, false);
            }
            // Ignore other events (InlineHtml, FootnoteReference, etc.)
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                // Flush any pending inline spans as a completed line.
                if !self.list.item_just_started {
                    self.flush_prose();
                }
                // Blank line between top-level blocks; keep list-item paragraphs tight, and an
                // alert's word against the sentence it introduces.
                if self.list.depth == 0 && !self.list.item_just_started && !self.alert_just_labeled
                {
                    push_block_gap(&mut self.lines);
                }
                self.alert_just_labeled = false;
            }
            Tag::BlockQuote(kind) => self.begin_quote(kind),
            Tag::List(ordered) => {
                if self.list.depth == 0 {
                    push_block_gap(&mut self.lines);
                }
                self.list.begin(ordered.is_some());
            }
            Tag::Heading { level, .. } => {
                // Flush pending inline spans before heading
                self.flush_prose();
                push_block_gap(&mut self.lines);
                self.heading_level = Some(level);
            }
            Tag::Strong => self.inline.style_stack.push(Modifier::BOLD),
            Tag::Emphasis => self.inline.style_stack.push(Modifier::ITALIC),
            Tag::Strikethrough => self.inline.style_stack.push(Modifier::CROSSED_OUT),
            Tag::Link { dest_url, .. } => {
                self.inline.link_url = Some(dest_url.to_string());
                self.inline.link_text.clear();
            }
            Tag::Table(alignments) => {
                self.flush_prose();
                push_block_gap(&mut self.lines);
                self.table.begin(alignments);
            }
            // Flush any pending cell content from the previous row
            Tag::TableRow if self.table.has_cell() => self.table.end_cell(),
            Tag::TableCell => self.table.begin_cell(),
            Tag::CodeBlock(CodeBlockKind::Fenced(lang)) => {
                // Flush any pending inline spans before entering code block
                self.flush_prose();
                push_block_gap(&mut self.lines);
                self.fence.begin(Some(lang.to_string()));
            }
            Tag::Item => self.begin_item(),
            Tag::CodeBlock(CodeBlockKind::Indented) => {
                self.flush_prose();
                push_block_gap(&mut self.lines);
                self.fence.begin(None);
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph if !self.inline.line.is_empty() => self.flush_prose(),
            TagEnd::BlockQuote(_) => {
                if !self.inline.line.is_empty() {
                    self.flush_prose();
                }
                self.in_blockquote = false;
            }
            TagEnd::List(_) => self.list.end(),
            TagEnd::Heading { .. } => {
                // Flush pending inline spans before clearing heading state
                self.flush_prose();
                self.heading_level = None;
            }
            TagEnd::CodeBlock => {
                self.lines.extend(self.fence.finish(self.theme, self.voice));
            }
            TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough => {
                self.inline.style_stack.pop();
            }
            TagEnd::Link => self.finish_link(),
            TagEnd::Table => {
                self.lines.extend(self.table.finish(self.theme, self.voice));
            }
            // pulldown-cmark puts header cells directly under TableHead (no TableRow wrapper).
            TagEnd::TableHead if self.table.has_row() => self.table.end_head(),
            // Buffer the completed body row
            TagEnd::TableRow if self.table.has_row() => self.table.end_row(),
            TagEnd::TableCell => self.table.end_cell(),
            _ => {}
        }
    }

    /// Opens a quote, writing out an alert's word on a line of its own.
    fn begin_quote(&mut self, kind: Option<BlockQuoteKind>) {
        let opens_an_item = self.list.item_just_started && !self.inline.line.is_empty();
        if !self.list.item_just_started {
            self.flush_prose();
        }
        push_block_gap(&mut self.lines);
        self.in_blockquote = true;
        // the item's bullet is already on the line, which is where the marker normally goes
        if opens_an_item && kind.is_none() {
            let style = build_quote_style(self.theme, self.voice);
            self.inline.line.push(Span::styled("> ".to_string(), style));
        }
        // An alert carries its severity in the tag rather than in its prose, so dropping
        // the kind renders `> [!CAUTION]` as an ordinary quote and loses the only thing
        // that made it one. The word is written out — the color is the polish.
        if let Some(kind) = kind {
            self.lines.push(MarkdownLine::prose(Line::from(vec![
                Span::styled("> ".to_string(), build_quote_style(self.theme, self.voice)),
                Span::styled(
                    crate::glyphs::icon_label(alert_icon(kind).glyph(), alert_label(kind)),
                    build_alert_style(self.theme, self.voice, kind),
                ),
            ])));
            self.alert_just_labeled = true;
        }
    }

    /// Flushes what is pending, then starts the item's line with its number or bullet.
    fn begin_item(&mut self) {
        self.flush_prose();
        let prefix = self.list.next_prefix();
        self.quote_prefix();
        self.inline.line.push(Span::styled(prefix, build_text_style(self.theme, self.voice)));
        self.list.item_just_started = true;
    }

    /// The address follows the words, once, however many spans the words took — and not at all
    /// when the words are the address.
    fn finish_link(&mut self) {
        if let Some(url) = self.inline.link_url.take() {
            let repeats = url.trim_start_matches("mailto:") == self.inline.link_text.trim();
            if !url.is_empty() && !repeats && !self.fence.active() && !self.table.active() {
                self.inline.line.push(Span::styled(
                    format!(" ({url})"),
                    build_link_url_style(self.theme, self.voice),
                ));
            }
        }
    }

    fn code(&mut self, text: &str) {
        if self.inline.link_url.is_some() {
            self.inline.link_text.push_str(text);
        }
        let mut style = build_code_style(self.theme, self.voice);
        if let Some(level) = self.heading_level {
            style = build_heading_style(self.theme, level, self.voice);
        }
        self.quote_prefix();
        // a table cell holds its code too, or the identifier leaks out as prose later
        self.append_inline(Span::styled(text.to_string(), style), true);
    }

    fn text(&mut self, text: &str) {
        // First text in a list item clears the "just started" flag
        self.list.item_just_started = false;
        // One-time blockquote prefix at start of line (before any styled spans)
        self.quote_prefix();
        let mut style =
            build_style(&self.inline.style_stack, build_text_style(self.theme, self.voice));
        if let Some(level) = self.heading_level {
            style = build_heading_style(self.theme, level, self.voice);
        }
        if self.inline.link_url.is_some() {
            style = build_link_style(self.theme, self.voice);
            self.inline.link_text.push_str(text);
        }
        self.append_inline(Span::styled(text.to_string(), style), true);
    }

    /// The one-time `> ` that opens a line inside a quote.
    fn quote_prefix(&mut self) {
        if self.in_blockquote && self.inline.line.is_empty() {
            let style = build_quote_style(self.theme, self.voice);
            self.inline.line.push(Span::styled("> ".to_string(), style));
        }
    }

    /// Routes inline content to where it belongs: a fence collects its text, a table cell its
    /// words when `cells` says this kind of content goes there, and everything else the line.
    fn append_inline(&mut self, span: Span<'static>, cells: bool) {
        if self.fence.active() {
            self.fence.push(span.content.to_string());
        } else if cells && self.table.active() {
            self.table.push_cell(&span.content);
        } else {
            self.inline.line.push(span);
        }
    }

    /// Ends the pending inline run as a prose line, if there is one.
    fn flush_prose(&mut self) {
        if !self.inline.line.is_empty() {
            let line = Line::from(std::mem::take(&mut self.inline.line));
            self.lines.push(MarkdownLine::prose(line));
        }
    }
}
