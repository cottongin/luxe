//! context_files — the project's context in full, and the way into any of it.
//!
//! The dashboard's PROJECT CONTEXT section names the first few of these and opens any of them.
//! This is the rest of the list: every context file, skill and MCP server behind that section,
//! grouped, with what each one governs, and the offer to start a file for a scope that has none.
//!
//! One flat grouped list rather than a menu of three kinds leading to three lists. The whole thing
//! is usually under a dozen rows, and a level you have to go back out of costs more than the
//! headings cost to skim. The box grows to its content and scrolls past the terminal's own height,
//! so a project carrying forty skills has a long list rather than rows nobody can reach. Past that
//! what it wants is a filter box — still not a submenu.
//!
//! It sits beside [`crate::configuration`] rather than inside it on purpose. Those are luxe's own
//! settings — a theme, a credential, a keymap. These belong to the *project*: a different author,
//! a different lifecycle, and they travel with the repository rather than the machine.
//!
//! Opening means leaving: the TUI hands the terminal back and the CLI runs `$EDITOR`
//! (`Action → update → Command → worker → Event → apply` does not reach outside the process, so
//! this rides the same [`crate::main_loop::TuiExit`] the configuration files already use).

use crate::glyphs::Icon;
use crate::theme::styles::ThemeStyles;
use layout::row_layout;
use luxe_core::instructions::ContextScope;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Clear, Padding, Paragraph};
use ratatui::Frame;
use std::path::{Path, PathBuf};

/// What a keypress on this surface asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextFilesOutcome {
    /// Hand this file to the reader's editor, which means leaving the TUI.
    EditFile(PathBuf),
    /// Leave the surface.
    Close,
    /// Key consumed; the surface stays as it is.
    Consumed,
}

/// Which group a row belongs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextKind {
    /// `AGENTS.md` and `CLAUDE.md` — instructions read into the system prompt
    Instructions,
    /// A skill's `SKILL.md`
    Skill,
    /// A configured MCP server, which opens the file that declares it
    McpServer,
}

impl ContextKind {
    /// The heading this group sits under.
    fn heading(self) -> &'static str {
        match self {
            Self::Instructions => "CONTEXT FILES",
            Self::Skill => "SKILLS",
            Self::McpServer => "MCP SERVERS",
        }
    }

    /// What one of these is called, for a row that has to say so in words.
    ///
    /// Singular: it labels one row rather than counting a group.
    pub(crate) const fn noun(self) -> &'static str {
        match self {
            Self::Instructions => "context file",
            Self::Skill => "skill",
            Self::McpServer => "MCP server",
        }
    }

    /// The icon this kind carries, wherever it is listed.
    pub(crate) fn glyph(self) -> &'static str {
        match self {
            Self::Instructions => Icon::ContextFile.glyph(),
            Self::Skill => Icon::Skill.glyph(),
            Self::McpServer => Icon::Mcp.glyph(),
        }
    }
}

/// One row: a file that exists, or the offer to start one that does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextFileRow {
    /// Which group it sits under
    pub kind: ContextKind,
    /// How the reader knows it — `AGENTS.md`, or a skill's name
    pub label: String,
    /// What it governs or does, said in words rather than left to the path
    pub scope: String,
    /// The file Enter opens
    pub path: PathBuf,
    /// Whether this row would create the file rather than open an existing one
    pub creates: bool,
    /// Whose rules an instruction file carries; `None` for anything else
    pub reach: Option<ContextScope>,
    /// Whether the file repeats one listed above it, so a turn reads it only once
    pub duplicate: bool,
}

impl ContextFileRow {
    /// What this row is, in the words its kind column shows.
    ///
    /// An instruction file says whose rules it carries rather than that it is a context file:
    /// `~/.config/luxe/agents.md` and `AGENTS.md` would both read "context file", one file twice,
    /// and the scope is what tells them apart.
    pub fn noun(&self) -> &'static str {
        match (self.kind, self.reach) {
            (ContextKind::Instructions, Some(ContextScope::Global)) => "global context",
            (ContextKind::Instructions, Some(ContextScope::Project)) => "project context",
            (ContextKind::Instructions, Some(ContextScope::Folder)) => "folder context",
            (kind, _) => kind.noun(),
        }
    }
}

/// The surface's state: the rows, which one the cursor is on, and where the box is scrolled to.
#[derive(Debug)]
pub struct ContextFilesState {
    rows: Vec<ContextFileRow>,
    selected: usize,
    /// First line of the window the last draw showed.
    ///
    /// It follows the cursor by the least it can: a window that re-anchored on the selection every
    /// frame would slide the whole list under a reader stepping one row up. Written by the draw
    /// that measured it — which line is on top is a property of the frame, like the dashboard
    /// column's record of whether it was on screen at all.
    scroll: std::cell::Cell<usize>,
    /// Lines that window had room for, which is what a page key means.
    viewport: std::cell::Cell<usize>,
}

impl ContextFilesState {
    /// Builds the surface for a session rooted at `cwd` from [`discover`].
    pub fn open(cwd: &Path) -> Self {
        Self::from_rows(discover(cwd, &luxe_core::project::instruction_dir(cwd)))
    }

    /// Builds the surface from an explicit list, for a capture that must not read the machine.
    pub fn from_rows(rows: Vec<ContextFileRow>) -> Self {
        Self {
            rows,
            selected: 0,
            scroll: std::cell::Cell::new(0),
            viewport: std::cell::Cell::new(0),
        }
    }

    /// The rows on offer.
    pub fn rows(&self) -> &[ContextFileRow] {
        &self.rows
    }

    /// Which row the cursor is on.
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Moves the cursor to `index`, clamped (mouse hover).
    pub fn select(&mut self, index: usize) {
        self.selected = index.min(self.rows.len().saturating_sub(1));
    }

    /// Handles a keypress.
    pub fn on_key(&mut self, key: &crossterm::event::KeyEvent) -> ContextFilesOutcome {
        use crossterm::event::KeyCode;
        let plain = key.modifiers.is_empty();
        match key.code {
            KeyCode::Esc | KeyCode::Left => ContextFilesOutcome::Close,
            KeyCode::Up | KeyCode::Char('k') if plain => {
                self.move_selection(-1);
                ContextFilesOutcome::Consumed
            }
            KeyCode::Down | KeyCode::Char('j') if plain => {
                self.move_selection(1);
                ContextFilesOutcome::Consumed
            }
            // A page is a viewport, so these clamp where the arrows wrap: a reader holding
            // PageDown through a long list is asking for the end of it, not for the top again.
            KeyCode::PageUp => {
                self.step_page(false);
                ContextFilesOutcome::Consumed
            }
            KeyCode::PageDown => {
                self.step_page(true);
                ContextFilesOutcome::Consumed
            }
            KeyCode::Home => {
                self.selected = 0;
                ContextFilesOutcome::Consumed
            }
            KeyCode::End => {
                self.selected = self.rows.len().saturating_sub(1);
                ContextFilesOutcome::Consumed
            }
            KeyCode::Enter | KeyCode::Right => self.activate(),
            _ => ContextFilesOutcome::Consumed,
        }
    }

    /// Walks the cursor one row for a wheel notch, without wrapping.
    ///
    /// The wheel moves the *cursor* here rather than the view under it, which is the opposite of
    /// what it does over the review's rail. The difference is what the cursor means: there it is
    /// the review's position in a rail beside prose you are reading, so dragging it along would
    /// stop you reading anything you are not standing in. Every row of this box is a file you are
    /// choosing to open, and a view that can drift off its own cursor in twelve rows only raises
    /// the question of what Enter would open.
    pub fn scroll_by(&mut self, up: bool) {
        let last = self.rows.len().saturating_sub(1);
        self.selected =
            if up { self.selected.saturating_sub(1) } else { (self.selected + 1).min(last) };
    }

    /// Moves the cursor a viewport's worth of rows, clamped at both ends.
    ///
    /// A page is measured in lines and the cursor counts rows, and the group headings between them
    /// mean the two differ. Stepping by the *lines* a page holds would overshoot by a heading per
    /// group crossed, so the reveal is left to place the window and this only has to land on a row
    /// roughly a screen away.
    fn step_page(&mut self, down: bool) {
        let page = self.viewport.get().max(1);
        let last = self.rows.len().saturating_sub(1);
        self.selected = if down {
            self.selected.saturating_add(page).min(last)
        } else {
            self.selected.saturating_sub(page)
        };
    }

    /// Where the drawn window starts, and what the frame it was measured on could hold.
    ///
    /// Called by the draw, which is the only thing that knows either. It moves the window the
    /// least it can: down far enough to put the cursor's own line on screen, up far enough to
    /// bring in the group heading it belongs under, and otherwise not at all. `cursor` is that
    /// pair of lines, absent only for an empty list.
    fn reveal(&self, cursor: Option<(usize, usize)>, total: usize, viewport: usize) -> usize {
        self.viewport.set(viewport);
        let mut start = self.scroll.get().min(total.saturating_sub(viewport));
        if let Some((line, anchor)) = cursor.filter(|_| viewport > 0) {
            if anchor < start {
                start = anchor;
            } else if line >= start + viewport {
                start = line + 1 - viewport;
            }
        }
        self.scroll.set(start);
        start
    }

    fn activate(&self) -> ContextFilesOutcome {
        match self.rows.get(self.selected) {
            Some(row) => ContextFilesOutcome::EditFile(row.path.clone()),
            None => ContextFilesOutcome::Close,
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let count = self.rows.len();
        if count == 0 {
            return;
        }
        let next = self.selected as isize + delta;
        self.selected = next.rem_euclid(count as isize) as usize;
    }
}

/// Everything a turn carries besides the conversation, in the order it is presented.
///
/// The one walk. Each group comes from the same call the thing that *uses* it makes —
/// [`luxe_core::instructions::context_files`] for prompt assembly's own walk,
/// [`luxe_core::skills::discover`], [`luxe_core::mcp_config::McpConfigStore::attributed`] — and the
/// dashboard's counts are a tally of what this returns rather than a second set of calls that has
/// to be kept in step: two implementations of "how many context files are there" would let the
/// dashboard disagree with the prompt.
///
/// `cwd` is the project root, which paths are shown relative to; `instructions_from` is where the
/// instruction walk starts, which is where luxe was launched — the same directory the prompt walks
/// from, or the list would come up short of what a turn reads.
pub fn discover(cwd: &Path, instructions_from: &Path) -> Vec<ContextFileRow> {
    let found = luxe_core::instructions::context_files(instructions_from);
    let mut rows: Vec<ContextFileRow> = found
        .iter()
        .map(|file| ContextFileRow {
            kind: ContextKind::Instructions,
            label: display_path(&file.path, cwd),
            scope: match &file.duplicate_of {
                Some(earlier) => {
                    format!("same as {} \u{2014} read once", display_path(earlier, cwd))
                }
                None => scope_of(file, cwd),
            },
            path: file.path.clone(),
            creates: false,
            reach: Some(file.scope),
            duplicate: file.duplicate_of.is_some(),
        })
        .collect();

    // Offered only where that scope has nothing, because an offer to create a file that exists is
    // a way to lose the one that does — and a second global file beside `~/.agents/AGENTS.md` is a
    // way to say everything twice.
    let project = luxe_core::instructions::project_context_file(instructions_from);
    if !found.iter().any(|file| file.path == project) {
        rows.push(ContextFileRow {
            kind: ContextKind::Instructions,
            label: display_path(&project, cwd),
            scope: "this project \u{2014} start one".into(),
            path: project,
            creates: true,
            reach: Some(ContextScope::Project),
            duplicate: false,
        });
    }
    if !found.iter().any(|file| file.scope == ContextScope::Global) {
        let global = luxe_core::instructions::global_context_file();
        rows.push(ContextFileRow {
            kind: ContextKind::Instructions,
            label: display_path(&global, cwd),
            scope: "every project \u{2014} start one".into(),
            path: global,
            creates: true,
            reach: Some(ContextScope::Global),
            duplicate: false,
        });
    }

    rows.extend(luxe_core::skills::discover(cwd).into_iter().map(|skill| ContextFileRow {
        kind: ContextKind::Skill,
        label: skill.name,
        scope: skill.description,
        path: skill.path,
        creates: false,
        reach: None,
        duplicate: false,
    }));

    // A server is a JSON entry, not a file, so the row opens the file that declared it — four can,
    // and only the highest-precedence one is the server actually running.
    rows.extend(
        luxe_core::mcp_config::McpConfigStore::attributed(cwd)
            .into_iter()
            .filter(|(server, _)| server.enabled)
            .map(|(server, source)| ContextFileRow {
                kind: ContextKind::McpServer,
                label: server.id,
                scope: display_path(&source, cwd),
                path: source,
                creates: false,
                reach: None,
                duplicate: false,
            }),
    );
    rows
}

/// `AGENTS.md` for something in the project, `~/.config/luxe/agents.md` for the global one.
///
/// A reader knows these files by where they sit relative to the work they are doing, and an
/// absolute path to a repository root tells them nothing they were asking. Home collapsing goes
/// through `paths`, which owns it, never off `$HOME`, which Windows does not set.
fn display_path(path: &Path, cwd: &Path) -> String {
    match path.strip_prefix(cwd) {
        Ok(relative) => luxe_core::paths::display(relative),
        Err(_) => luxe_core::paths::home_relative(path),
    }
}

/// What this file governs, said plainly — the path alone does not tell a reader its reach.
fn scope_of(file: &luxe_core::instructions::ContextFile, cwd: &Path) -> String {
    match file.scope {
        ContextScope::Global if file.path == luxe_core::instructions::global_context_file() => {
            "every project".into()
        }
        ContextScope::Global => "every project, and every agent that reads it".into(),
        ContextScope::Project => "this project".into(),
        ContextScope::Folder => match file.path.parent() {
            Some(folder) => format!("work under {}", display_path(folder, cwd)),
            None => "this folder".into(),
        },
    }
}

/// Draws the surface centered over whatever is behind it.
pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &ContextFilesState,
    theme: &ThemeStyles,
    hit: &std::cell::RefCell<crate::mouse::HitRegions>,
) {
    let width = area.width.saturating_sub(4).clamp(46, 82);
    let (lines, row_lines) = row_layout(state, theme);

    // The box grows to its content and stops two rows short of the viewport, which is the margin
    // every other overlay keeps. Past that the list scrolls rather than the box growing into the
    // terminal's own edges.
    let height =
        u16::try_from(lines.len().saturating_add(2 + usize::from(crate::widgets::FOOTER_ROWS)))
            .unwrap_or(u16::MAX)
            .min(area.height.saturating_sub(2).max(6))
            .clamp(6, area.height);
    let popup = Rect::new(
        area.x.saturating_add(area.width.saturating_sub(width) / 2),
        area.y.saturating_add(area.height.saturating_sub(height) / 2),
        width,
        height,
    );
    let block = crate::widgets::titled_panel(Icon::ContextFile.glyph(), "Project context", theme)
        .padding(Padding::left(1));
    let inner = crate::widgets::footed_body(&block, popup);
    let total = lines.len();
    let viewport = usize::from(inner.height);
    let scrolls = total > viewport;
    // The bar takes a column from the rows rather than sitting over them, so no row is half under
    // it and a press on the track lands on nothing rather than opening a file the reader was
    // reaching past.
    let rows_width = inner.width.saturating_sub(u16::from(scrolls));
    let start = state.reveal(row_lines.get(state.selected).copied(), total, viewport);

    {
        let mut regions = hit.borrow_mut();
        for (index, (line, _)) in row_lines.into_iter().enumerate() {
            let Some(offset) = line.checked_sub(start).filter(|offset| *offset < viewport) else {
                continue;
            };
            regions.push(
                Rect::new(inner.x, inner.y.saturating_add(offset as u16), rows_width, 1),
                crate::mouse::MouseTarget::ContextFileRow(index),
            );
        }
        if scrolls {
            regions.push_scroll(inner, crate::mouse::ScrollRegionId::ContextFiles);
        }
    }

    let window: Vec<Line<'static>> = lines.into_iter().skip(start).take(viewport.max(1)).collect();
    frame.render_widget(Clear, popup);
    crate::widgets::render_footed(
        frame,
        block,
        popup,
        "enter open in your editor · esc back",
        None,
        theme,
    );
    frame.render_widget(Paragraph::new(window), inner);
    if scrolls {
        crate::widgets::scrollbar(
            frame,
            Rect::new(inner.right().saturating_sub(1), inner.y, 1, inner.height),
            start,
            total,
            viewport,
            theme,
        );
    }
}

mod layout;

#[cfg(test)]
mod tests;
