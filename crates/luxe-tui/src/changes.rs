//! Changes — one file's diff, in the column the conversation usually has.
//!
//! The Context Dock's FILES tree lists names and raises the obvious question: *what did it do
//! to that one?* Guided Review answers it properly — pinned source, chapters, comments, a model
//! paid to explain the change — which is the right price for reading a change and the wrong one
//! for glancing at a file. This surface is the glance: `Action::LoadFileDiff` in, one
//! `Event::FileDiffLoaded` back, nothing persisted, no model asked anything.
//!
//! It takes the conversation column rather than the screen so the two things a reader does next
//! stay to hand — click a different file in the dock, or type. The composer keeps focus
//! throughout, which is why this owns no keys of its own beyond the ones the transcript already
//! answers to.

use luxe_core::config::DiffLayout;
use luxe_core::file_diff::{FileDiff, FileDiffFailure, FileDiffRequestId};
use luxe_core::guided_review::ReviewFile;
use luxe_core::project_files::{FileReadRequestId, ProjectFile, ProjectFileFailure};

mod cells;
pub(crate) use cells::CellPick;

/// How far one wheel notch moves the diff.
pub(crate) const SCROLL_STEP: usize = 3;

/// What the pane has to show for the file it is pointed at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChangesLoad {
    /// A capture is in flight and nothing has arrived yet.
    Loading,
    /// The file's change, ready to render.
    Ready(Box<ReviewFile>),
    /// The file matches its base — opened, and there is genuinely nothing in it.
    Unchanged,
    /// The capture failed, with the reason to show instead.
    Failed(FileDiffFailure),
}

/// Which of a file's two faces the pane is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeekView {
    /// What changed, against `HEAD`
    Diff,
    /// The file as it is now
    File,
}

/// The file's own text, for the File view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TextLoad {
    /// Not asked for: the pane is on the diff and has not needed it
    Idle,
    /// Asked for, not here yet
    Loading,
    /// Here
    Ready(Box<ProjectFile>),
    /// Could not be read
    Failed(ProjectFileFailure),
}

/// One entry of the pane's own file list, mirroring the dock's order.
///
/// Carried rather than re-derived so the pane can say `3 of 7` and walk to the next file without a
/// workspace snapshot — which is not guaranteed to be there, and reflows underneath it when it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChangedFile {
    /// Project-relative path, as the snapshot reported it.
    pub(crate) path: String,
    /// Lifecycle marker, so the header can say what happened without color.
    pub(crate) status: luxe_core::FileChangeStatus,
    /// Text additions, or `None` when git cannot say.
    pub(crate) additions: Option<u64>,
    /// Text deletions, or `None` when git cannot say.
    pub(crate) deletions: Option<u64>,
}

/// The Changes pane, while it is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChangesView {
    pub(crate) path: String,
    /// Every changed file, in the dock's display order, for `N of M` and for stepping.
    pub(crate) files: Vec<ChangedFile>,
    pub(crate) load: ChangesLoad,
    /// In-flight capture, so a superseded answer is discarded instead of shown.
    pub(crate) request: Option<FileDiffRequestId>,
    /// First visible diff row.
    pub(crate) scroll: usize,
    /// Whether long source lines soft-wrap instead of being clipped.
    ///
    /// On by default, unlike the review: that surface can scroll sideways, and this one cannot, so
    /// clipping here would put content out of reach rather than one keystroke away.
    pub(crate) word_wrap: bool,
    /// A layout the reader has pinned for this pane; `None` follows `diff.layout`.
    pub(crate) split: Option<bool>,
    /// Rows the last frame could show at once, so paging matches what was on screen.
    pub(crate) viewport_rows: std::cell::Cell<usize>,
    /// Whether the last frame drew two columns, so a toggle can flip what is actually showing
    /// rather than a preference the width may have overruled.
    pub(crate) side_by_side: std::cell::Cell<bool>,
    /// Whether the last *loaded* file had two sides to draw.
    ///
    /// Held across the frame or two the next capture takes, so the footer's layout shortcut does
    /// not blink out mid-switch. It cannot be inferred from `side_by_side`: a pane too narrow to
    /// split resolves that to `false` for a file that has both sides perfectly well.
    pub(crate) two_sided: std::cell::Cell<bool>,
    /// Largest useful scroll offset, written by the renderer each frame.
    pub(crate) scroll_max: std::cell::Cell<usize>,
    /// Which face of the file is showing
    pub(crate) view: PeekView,
    /// The file's text, once the File view has asked for it
    pub(crate) text: TextLoad,
    /// In-flight read, so a superseded one is discarded
    pub(crate) text_request: Option<FileReadRequestId>,
    /// Lines picked in the File view, as `(anchor, head)` 1-based line numbers in pick order
    pub(crate) selection: Option<(u32, u32)>,
    /// Whether the keyboard is in the pane rather than the composer or the tree
    pub(crate) focused: bool,
    /// The File view line the keyboard is on, 1-based
    pub(crate) cursor: Option<u32>,
    /// Visual-line mode: every move grows the pick from where it began, as `V` does in vim
    pub(crate) visual: bool,
    /// Text selected with the mouse, by the character
    pub(crate) cells: Option<CellPick>,
}

impl ChangesView {
    /// Opens the pane on `path`, with `files` as the list it can walk.
    pub(crate) fn new(path: String, files: Vec<ChangedFile>) -> Self {
        Self {
            path,
            files,
            load: ChangesLoad::Loading,
            request: None,
            scroll: 0,
            word_wrap: true,
            split: None,
            viewport_rows: std::cell::Cell::new(1),
            side_by_side: std::cell::Cell::new(false),
            two_sided: std::cell::Cell::new(false),
            scroll_max: std::cell::Cell::new(0),
            view: PeekView::Diff,
            text: TextLoad::Idle,
            text_request: None,
            selection: None,
            focused: false,
            cursor: None,
            visual: false,
            cells: None,
        }
    }

    /// The picked lines, first to last, whichever way the drag went.
    pub(crate) fn selected_lines(&self) -> Option<(u32, u32)> {
        self.selection.map(|(anchor, head)| (anchor.min(head), anchor.max(head)))
    }

    /// The picked lines' text, for a copy.
    pub(crate) fn selected_text(&self) -> Option<String> {
        let (first, last) = self.selected_lines()?;
        let TextLoad::Ready(file) = &self.text else { return None };
        let lines: Vec<&str> = file
            .text
            .lines()
            .skip(first.saturating_sub(1) as usize)
            .take((last - first + 1) as usize)
            .collect();
        (!lines.is_empty()).then(|| lines.join("\n"))
    }

    /// The mention a question about the picked lines carries: `@[file:src/a.rs:12-30]`.
    ///
    /// A pointer, like every mention: the model reads the lines with its own tools, so the history
    /// stays small and never holds a copy that goes stale the moment the agent edits the file.
    pub(crate) fn selection_mention(&self) -> Option<String> {
        let (first, last) = self.selected_lines()?;
        Some(match first == last {
            true => format!("@[file:{}:{first}]", self.path),
            false => format!("@[file:{}:{first}-{last}]", self.path),
        })
    }

    /// Whether the visible file has a change to show, and so a Diff view at all.
    pub(crate) fn changed(&self) -> bool {
        self.current().is_some()
    }

    /// Takes a file's text, under the same staleness guard as a diff.
    pub(crate) fn accept_text(&mut self, request: &FileReadRequestId, file: ProjectFile) -> bool {
        if self.text_request.as_ref() != Some(request) || file.path != self.path {
            return false;
        }
        self.text_request = None;
        self.text = TextLoad::Ready(Box::new(file));
        self.cells = None;
        true
    }

    /// Takes a failed read under the same guard.
    pub(crate) fn fail_text(
        &mut self,
        request: &FileReadRequestId,
        path: &str,
        error: ProjectFileFailure,
    ) -> bool {
        if self.text_request.as_ref() != Some(request) || path != self.path {
            return false;
        }
        self.text_request = None;
        self.text = TextLoad::Failed(error);
        true
    }

    /// Whether this file draws side by side at `source` cells per column.
    ///
    /// Reads `diff.layout` like the tool cards and the review do: a pane that hard-coded its own
    /// threshold would be the one surface a reader opens *to look at a diff* that the diff setting
    /// does not reach.
    ///
    /// The width has the last word either way — a pinned preference cannot conjure room, and a
    /// file with only one side has nothing to put in the second column.
    pub(crate) fn split_active(
        &self,
        configured: DiffLayout,
        source: usize,
        file: Option<&ReviewFile>,
    ) -> bool {
        let two_sided = file.is_some_and(crate::guided_review::has_two_sides);
        // A pin is this pane's answer for this session; `split` rather than `auto` because
        // pinning happened on a pane the width rule had already decided against.
        match self.split {
            Some(true) => DiffLayout::Split.splits(source, two_sided),
            Some(false) => false,
            None => configured.splits(source, two_sided),
        }
    }

    /// Pins the layout to the opposite of what is showing.
    ///
    /// Records the *resolved* state's opposite rather than flipping the stored option, so the first
    /// press always changes what is on screen — flipping `None` to `Some(false)` where the width was
    /// keeping it unified anyway would be a key that does nothing.
    pub(crate) fn toggle_split(&mut self) {
        self.split = Some(!self.side_by_side.get());
    }

    pub(crate) fn file(&self) -> Option<&ReviewFile> {
        match &self.load {
            ChangesLoad::Ready(file) => Some(file),
            _ => None,
        }
    }

    /// Points the pane at a different file, discarding whatever the last one had loaded.
    pub(crate) fn show(&mut self, path: String) {
        self.path = path;
        self.load = ChangesLoad::Loading;
        self.request = None;
        self.text = TextLoad::Idle;
        self.text_request = None;
        self.selection = None;
        self.cursor = None;
        self.visual = false;
        self.cells = None;
        self.scroll = 0;
    }

    /// Lines in the File view's text, or `None` while there is no text to move through.
    pub(crate) fn text_lines(&self) -> Option<u32> {
        match &self.text {
            TextLoad::Ready(file) if !file.binary => {
                Some(u32::try_from(file.text.lines().count()).unwrap_or(u32::MAX).max(1))
            }
            _ => None,
        }
    }

    /// Moves the File view's cursor by `delta` lines, to a line that exists, scrolling to keep it
    /// on screen. With `extend`, or in visual-line mode, the pick grows from where it began to
    /// where the cursor lands; otherwise a pick is dropped, the way moving the caret drops a
    /// selection everywhere else.
    pub(crate) fn move_cursor(&mut self, delta: i64, extend: bool) {
        let Some(lines) = self.text_lines() else { return };
        let extend = extend || self.visual;
        let from = self.cursor.unwrap_or_else(|| self.top_line());
        let to = (i64::from(from) + delta).clamp(1, i64::from(lines));
        let to = u32::try_from(to).unwrap_or(1);
        self.cursor = Some(to);
        self.selection = match (extend, self.selection) {
            (true, Some((anchor, _))) => Some((anchor, to)),
            (true, None) => Some((from, to)),
            (false, _) => None,
        };
        self.keep_cursor_in_view();
    }

    /// Enters visual-line mode on the cursor's line, or leaves it and drops the pick.
    pub(crate) fn toggle_visual(&mut self) {
        if self.visual {
            self.visual = false;
            self.selection = None;
            return;
        }
        let at = self.cursor.unwrap_or_else(|| self.top_line());
        self.cursor = Some(at);
        self.visual = true;
        self.selection = Some((at, at));
    }

    /// Puts the cursor at the pick's other end, as `o` does in vim's visual mode, so either end can
    /// be moved.
    pub(crate) fn swap_pick_ends(&mut self) {
        if let Some((anchor, head)) = self.selection {
            self.selection = Some((head, anchor));
            self.cursor = Some(anchor);
            self.keep_cursor_in_view();
        }
    }

    /// The first line on screen, where a cursor starts when the pane is first entered.
    pub(crate) fn top_line(&self) -> u32 {
        u32::try_from(self.scroll + 1).unwrap_or(u32::MAX)
    }

    /// Scrolls the least that puts the cursor's line on screen.
    fn keep_cursor_in_view(&mut self) {
        let Some(cursor) = self.cursor else { return };
        let line = cursor.saturating_sub(1) as usize;
        let rows = self.viewport_rows.get().max(1);
        if line < self.scroll {
            self.scroll = line;
        } else if line >= self.scroll + rows {
            self.scroll = line + 1 - rows;
        }
    }

    /// Accepts a capture, unless a newer request has already superseded it.
    ///
    /// Both halves of the guard matter: a stale *request id* is a capture the reader has moved on
    /// from, and a diff whose path is no longer the visible one is the same thing arriving by a
    /// different route (a refresh that raced a click).
    pub(crate) fn accept(&mut self, request: &FileDiffRequestId, diff: FileDiff) -> bool {
        if !self.owns(request) || diff.path != self.path {
            return false;
        }
        self.request = None;
        self.load = diff.file.map_or(ChangesLoad::Unchanged, ChangesLoad::Ready);
        self.scroll = 0;
        self.cells = None;
        true
    }

    /// Accepts a failure under the same staleness guard as a capture.
    pub(crate) fn fail(
        &mut self,
        request: &FileDiffRequestId,
        path: &str,
        error: FileDiffFailure,
    ) -> bool {
        if !self.owns(request) || path != self.path {
            return false;
        }
        self.request = None;
        self.load = ChangesLoad::Failed(error);
        true
    }

    /// Whether `request` is the one this pane is waiting on.
    fn owns(&self, request: &FileDiffRequestId) -> bool {
        self.request.as_ref() == Some(request)
    }

    /// Index of the visible file in the pane's list, when the list still carries it.
    pub(crate) fn position(&self) -> Option<usize> {
        self.files.iter().position(|file| file.path == self.path)
    }

    /// The visible file's snapshot row, for the header's status marker and counts.
    pub(crate) fn current(&self) -> Option<&ChangedFile> {
        self.files.iter().find(|file| file.path == self.path)
    }

    /// The path one step from the visible file, wrapping at both ends.
    ///
    /// Wraps because the list is short and circular movement is what every other picker in Luxe
    /// does; stopping dead at the last file would read as a broken key. A visible file that has
    /// left the list — reverted while it was open — steps back *onto* the list rather than off it.
    pub(crate) fn step(&self, forward: bool) -> Option<String> {
        let Some(current) = self.position() else {
            let landing = if forward { 0 } else { self.files.len().checked_sub(1)? };
            return self.files.get(landing).map(|file| file.path.clone());
        };
        if self.files.len() < 2 {
            return None;
        }
        let next = if forward {
            (current + 1) % self.files.len()
        } else {
            current.checked_sub(1).unwrap_or(self.files.len() - 1)
        };
        Some(self.files[next].path.clone())
    }

    /// Replaces the file list from a newer workspace snapshot, keeping the visible file put.
    ///
    /// The snapshot refreshes on every tool call, so the list under the pane changes while it is
    /// open. Reordering is fine; losing the reader's place is not — a file that has been reverted
    /// away stays visible until they leave it, rather than the pane silently jumping elsewhere.
    pub(crate) fn resync(&mut self, files: Vec<ChangedFile>) {
        self.files = files;
    }

    /// Scrolls by `lines`, clamped to what the last frame said was useful.
    pub(crate) fn scroll_by(&mut self, up: bool, lines: usize) {
        let max = self.scroll_max.get();
        self.scroll =
            if up { self.scroll.saturating_sub(lines) } else { (self.scroll + lines).min(max) };
    }

    /// Scrolls by one rendered screenful.
    pub(crate) fn page(&mut self, up: bool) {
        self.scroll_by(up, self.viewport_rows.get().max(1));
    }

    /// Jumps to the top or the bottom of the diff.
    pub(crate) fn jump(&mut self, top: bool) {
        self.scroll = if top { 0 } else { self.scroll_max.get() };
    }

    /// Toggles soft wrapping.
    ///
    /// The offset is a row index rather than a visual line, so wrapping changes how tall the rows
    /// are without moving the reader off the one they were on.
    pub(crate) fn toggle_word_wrap(&mut self) {
        self.word_wrap = !self.word_wrap;
    }
}

#[cfg(test)]
mod tests;
