//! Render — a message as the app shows it, made on the host so the app parses nothing.
//!
//! Markdown, highlighted code, tool glyphs and the status line's wording are the TUI's, and the
//! bridge doesn't depend on the TUI, so `luxe-cli` hands one in as a [`Renderer`]. What a user
//! message shows (the reader's own words, and a brief Luxe wrote to the agent as one line saying
//! so) is core's call (`luxe_core::briefs`), so that is made here. The bridge keeps no session
//! state, so the app asks: once for a finished message, and a few times a second for one still
//! growing or a status that changed.

mod diff;
mod file;
mod github;
mod review;
mod tree;
mod workspace;

pub use github::GithubMarks;
pub use tree::{FileMark, MarkHue};

use luxe_core::briefs;
use luxe_core::file_diff::{FileDiff, FileDiffFailure};
use luxe_core::github::overview::{ConversationEntry, Overview};
use luxe_core::github::ListedSection;
use luxe_core::guided_review::{
    ChapterId, ChapterReading, GuidedReview, ReviewChapter, ReviewFile, ReviewFinding,
};
use luxe_core::project_files::{ProjectFile, ProjectFileFailure};
use luxe_core::provider::{PlanUsage, ThinkingLevel};
use luxe_core::usage::SessionUsage;
use luxe_core::workspace::{FileChangeStatus, TodoSummary, WorkspaceChangesSummary};
use luxe_core::ModelRef;
use luxe_core::TodoSummaryStatus;

/// Turns model text into the app's HTML, and draws the app's icon from the same art the TUI's
/// wordmark is.
pub trait Renderer: Send + Sync {
    /// `text` as HTML, for `voice`. Everything in `text` is data: it must come back escaped.
    fn markdown(&self, text: &str, voice: Voice) -> String;
    /// The glyph a tool's card wears, as plain text.
    fn tool_glyph(&self, tool: &str) -> String;
    /// The status line's facts, worded as the TUI's bar words them.
    fn status(&self, facts: &StatusFacts) -> Vec<StatusSegment>;
    /// The glyph beside a user message's "image(s) attached" line, as plain text.
    fn image_glyph(&self) -> String {
        String::new()
    }
    /// One line of source as HTML, highlighted for `language`. It must come back escaped.
    fn code_line(&self, text: &str, _language: Option<&str>) -> String {
        escape(text)
    }
    /// The mark the dock puts beside a todo, as plain text.
    fn todo_mark(&self, _status: TodoSummaryStatus) -> String {
        String::new()
    }
    /// The mark the dock puts beside a changed file, as plain text.
    fn change_mark(&self, _status: FileChangeStatus) -> String {
        String::new()
    }
    /// The mark the file tree puts before a file named `name`, by its language.
    fn file_mark(&self, _name: &str) -> FileMark {
        FileMark::default()
    }
    /// The mark the file tree puts before a folder, as plain text.
    fn folder_mark(&self) -> String {
        String::new()
    }
    /// The mark before the checked-out branch, as the TUI's project section has it.
    fn branch_mark(&self) -> String {
        String::new()
    }
    /// The mark beside `mode`'s name, as the TUI's composer title has it, as plain text.
    fn mode_mark(&self, _mode: &str) -> String {
        String::new()
    }
    /// The marks `/github` draws, as plain text.
    fn github_marks(&self) -> GithubMarks {
        GithubMarks::default()
    }
    /// How long before `now` `at` was, both in seconds since the Unix epoch, as plain text: `3h
    /// ago`. Empty from a renderer without the TUI's words for it.
    fn ago(&self, _at: u64, _now: u64) -> String {
        String::new()
    }
    /// A GitHub body with the HTML people and bots put in it reduced to the markdown it amounts
    /// to, as GitHub shows it. The result goes through [`Renderer::markdown`].
    fn github_readable(&self, body: &str) -> String {
        body.to_owned()
    }
    /// The home-screen icon, `side` pixels square, in `colors`, as PNG bytes; none from a
    /// renderer with no art to draw it from.
    fn home_icon(&self, _side: u32, _colors: TileColors) -> Option<Vec<u8>> {
        None
    }
}

/// The icon's two colors: the theme's background for the plate, its accent for the letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileColors {
    /// The square behind the letter
    pub plate: [u8; 3],
    /// The letter
    pub ink: [u8; 3],
}

/// What a session last reported about itself, as the app forwards it from the frames.
#[derive(Debug, Default, serde::Deserialize)]
pub struct StatusFacts {
    /// From `state_changed`
    pub model: Option<ModelRef>,
    /// From `state_changed`
    pub thinking: Option<ThinkingLevel>,
    /// From `context_usage`
    pub context: Option<ContextUsage>,
    /// From `plan_usage`
    pub plan: Option<PlanUsage>,
    /// From `session_usage`
    pub session: Option<SessionUsage>,
}

/// A `context_usage` frame's content.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub struct ContextUsage {
    /// Tokens in the window
    pub used_tokens: u32,
    /// The window's size
    pub context_window: u32,
}

/// One fact on the status line, as words; the bridge escapes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSegment {
    /// Which fact, as the class the app styles it by: `context`, `cost`, `thinking` or `model`
    pub kind: &'static str,
    /// The mark in front of it, if any
    pub glyph: Option<String>,
    /// The words
    pub label: String,
    /// How loudly to color it
    pub tone: StatusTone,
}

/// How loudly a status segment is colored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusTone {
    /// Body text
    Plain,
    /// The theme's warning color
    Warning,
    /// The theme's error color
    Error,
}

/// Which reader model text is rendered for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Voice {
    /// An answer
    Answer,
    /// Model reasoning, which keeps its line breaks and leaves its code plain
    Reasoning,
}

/// The renderer a bridge has until it is handed the TUI's: every word shown, escaped, with no
/// formatting and no glyphs.
pub(crate) struct PlainText;

impl Renderer for PlainText {
    fn markdown(&self, text: &str, _voice: Voice) -> String {
        format!("<p class=\"plain\">{}</p>", escape(text))
    }

    fn tool_glyph(&self, _tool: &str) -> String {
        String::new()
    }

    fn status(&self, facts: &StatusFacts) -> Vec<StatusSegment> {
        let model = facts.model.as_ref().map(|model| model.display().to_string());
        let segment =
            |label| StatusSegment { kind: "model", glyph: None, label, tone: StatusTone::Plain };
        model.into_iter().map(segment).collect()
    }
}

/// What the app asks to have rendered, in one round trip.
#[derive(Debug, serde::Deserialize)]
pub(crate) struct RenderRequest {
    pub(crate) pieces: Vec<Piece>,
}

/// One thing to render.
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Piece {
    /// An answer's text
    Answer { text: String },
    /// Reasoning's text
    Reasoning { text: String },
    /// A user message's content blocks, as the wire carries them
    User { content: Vec<WireBlock> },
    /// A tool, by name
    Tool { name: String },
    /// The status line
    Status(Box<StatusFacts>),
    /// The todos from `workspace_updated`, as the dock's PLAN shows them
    Plan { todos: Box<TodoSummary> },
    /// The changed files from `workspace_updated`, each a way into its diff
    Changes { changes: Box<WorkspaceChangesSummary> },
    /// A `file_diff_loaded` frame's diff
    Diff { diff: Box<FileDiff> },
    /// A `file_diff_failed` frame's reason
    DiffFailed { error: FileDiffFailure },
    /// A `project_file_read` frame's file
    File { file: Box<ProjectFile> },
    /// A `project_file_read_failed` frame's reason
    FileFailed { error: ProjectFileFailure },
    /// The Files tab's rows, marked from the changes the dock has
    Tree { rows: Vec<tree::TreeRow>, changes: Option<Box<WorkspaceChangesSummary>> },
    /// The project panel's bar: the session's folder name behind the tree's folder mark, then the
    /// branch from a `workspace_updated` frame, marked when the tree is dirty
    ProjectName {
        name: String,
        #[serde(default)]
        branch: Option<String>,
        #[serde(default)]
        dirty: bool,
    },
    /// The status line's mode button: the mode's name behind its mark
    ModeName { mode: String },
    /// A `github_listed` section, as `/github`'s list draws it. Args: `now`, the app's clock in
    /// seconds since the Unix epoch, as in the pieces below
    GithubList { repository: Option<String>, section: Box<ListedSection>, now: u64 },
    /// A `github_overview_read` overview's header, above the detail's tabs
    GithubHeader { overview: Box<Overview>, now: u64 },
    /// Its Overview tab
    GithubOverview { overview: Box<Overview>, now: u64 },
    /// A `github_conversation_read` frame's entries, `opened_by_you` from the overview
    GithubConversation {
        entries: Vec<ConversationEntry>,
        now: u64,
        #[serde(default)]
        opened_by_you: bool,
    },
    /// An `open_review_shown` review as the guide, with `reading` from the same frame. The app
    /// sends the review without its hunks' lines, which the guide doesn't show
    ReviewGuide {
        review: Box<GuidedReview>,
        #[serde(default)]
        reading: Vec<ChapterReading>,
        #[serde(default)]
        walked: Vec<ChapterId>,
    },
    /// One chapter opened. Args: `files`, only the files it names, holding only its hunks
    ReviewChapter {
        chapter: Box<ReviewChapter>,
        files: Vec<ReviewFile>,
        place: review::Place,
        reading: Option<ChapterReading>,
        /// The open session's comments; the chapter shows the ones on it
        #[serde(default)]
        findings: Vec<ReviewFinding>,
    },
}

/// A content block as the wire carries it, where only text is read. The app sends any other block
/// as just its type, so a photo's bytes don't make the round trip.
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type", content = "content", rename_all = "snake_case")]
pub(crate) enum WireBlock {
    /// Text
    Text(String),
    /// An image, counted
    Image,
    /// Anything else
    #[serde(other)]
    Other,
}

/// The pieces' HTML, in the order they were asked for.
#[derive(Debug, serde::Serialize)]
pub(crate) struct Rendered {
    pub(crate) html: Vec<String>,
}

/// `piece` as the app's HTML.
pub(crate) fn render(renderer: &dyn Renderer, piece: &Piece) -> String {
    match piece {
        // what the agent wrote for Luxe to read (a remediation report, a draft) isn't the reader's
        Piece::Answer { text } => renderer.markdown(&briefs::for_the_reader(text), Voice::Answer),
        Piece::Reasoning { text } => renderer.markdown(text, Voice::Reasoning),
        Piece::User { content } => user_message(content) + &images_attached(renderer, content),
        Piece::Tool { name } => escape(&renderer.tool_glyph(name)),
        Piece::Status(facts) => renderer.status(facts).iter().map(status_segment).collect(),
        Piece::Plan { todos } => workspace::plan(renderer, todos),
        Piece::Changes { changes } => workspace::changes(renderer, changes),
        Piece::Diff { diff } => diff::file(renderer, diff),
        Piece::DiffFailed { error } => diff::failed(error),
        Piece::File { file } => file::file(renderer, file),
        Piece::FileFailed { error } => file::failed(error),
        Piece::Tree { rows, changes } => tree::tree(renderer, rows, changes.as_deref()),
        Piece::ProjectName { name, branch, dirty } => {
            tree::project_name(renderer, name, branch.as_deref(), *dirty)
        }
        Piece::ModeName { mode } => mode_name(renderer, mode),
        Piece::GithubList { .. }
        | Piece::GithubHeader { .. }
        | Piece::GithubOverview { .. }
        | Piece::GithubConversation { .. } => github_piece(renderer, piece),
        Piece::ReviewGuide { review, reading, walked } => {
            review::guide(renderer, review, reading, walked)
        }
        Piece::ReviewChapter { chapter, files, place, reading, findings } => {
            review::chapter(renderer, chapter, files, place, reading.as_ref(), findings)
        }
    }
}

fn github_piece(renderer: &dyn Renderer, piece: &Piece) -> String {
    let cx = |now: &u64| github::Context::new(renderer, *now);
    match piece {
        Piece::GithubList { repository, section, now } => {
            github::list(&cx(now), repository.as_deref(), section)
        }
        Piece::GithubHeader { overview, now } => github::header(&cx(now), overview),
        Piece::GithubOverview { overview, now } => github::overview(&cx(now), overview),
        Piece::GithubConversation { entries, now, opened_by_you } => {
            github::conversation(&cx(now), entries, *opened_by_you)
        }
        _ => String::new(),
    }
}

/// `mode` behind its mark, or alone from a renderer without one.
fn mode_name(renderer: &dyn Renderer, mode: &str) -> String {
    let mark = renderer.mode_mark(mode);
    if mark.is_empty() {
        return escape(mode);
    }
    format!(
        "<span class=\"mode-mark\" aria-hidden=\"true\">{}</span> {}",
        escape(&mark),
        escape(mode)
    )
}

fn status_segment(segment: &StatusSegment) -> String {
    let tone = match segment.tone {
        StatusTone::Plain => "",
        StatusTone::Warning => " tone-warning",
        StatusTone::Error => " tone-error",
    };
    let glyph = segment.glyph.as_deref().map(|glyph| {
        format!("<span class=\"seg-glyph\" aria-hidden=\"true\">{}</span> ", escape(glyph))
    });
    format!(
        "<span class=\"seg seg-{}{tone}\">{}{}</span>",
        segment.kind,
        glyph.unwrap_or_default(),
        escape(&segment.label)
    )
}

/// A user message as the transcript shows it: the reader's words as they wrote them, without the
/// notes that ride along for the model, and each brief as one line.
fn user_message(content: &[WireBlock]) -> String {
    let texts = content.iter().map(|block| match block {
        WireBlock::Text(text) => Some(text.as_str()),
        WireBlock::Image | WireBlock::Other => None,
    });
    briefs::readers_texts(texts).iter().map(|(_, text)| user_block(text)).collect()
}

/// The line the TUI puts under a message that carried images.
fn images_attached(renderer: &dyn Renderer, content: &[WireBlock]) -> String {
    let count = content.iter().filter(|block| matches!(block, WireBlock::Image)).count();
    if count == 0 {
        return String::new();
    }
    let glyph = renderer.image_glyph();
    let glyph = if glyph.is_empty() { glyph } else { format!("{} ", escape(&glyph)) };
    format!("<p class=\"attached\">{glyph}{count} image(s) attached</p>")
}

/// One block of a user message. A question from a review is the reader's own words, so it shows
/// what they asked under where they asked it.
fn user_block(text: &str) -> String {
    let asked = briefs::review_questions(text);
    if !asked.is_empty() {
        return asked.iter().map(asked_in_review).collect();
    }
    match briefs::label(text) {
        Some(label) => format!("<p class=\"brief\">{}</p>", escape(label)),
        None => format!("<p class=\"words\">{}</p>", escape(text)),
    }
}

fn asked_in_review(asked: &briefs::ReviewQuestion) -> String {
    format!(
        "<p class=\"words\"><span class=\"chip\">Asked in review \u{b7} {}</span>\n{}</p>",
        escape(&asked.location),
        escape(&asked.question)
    )
}

/// `text` with everything that means something in HTML escaped.
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
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
    out
}

#[cfg(test)]
mod tests;
