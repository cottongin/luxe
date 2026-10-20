//! slash_flyout — the `/` door onto the command list.
//!
//! Opens when `/` starts the composer and filters as you type. It lists the same menu the palette
//! does — what can run here, the recently used first — keeping only commands with a slash trigger,
//! and ranks against the trigger, since what you are typing is a command's name. Enter runs the
//! selected command; Tab completes it into the composer so an argument can follow. Every
//! argument-taking command has a useful bare form (`/diff` opens a review, `/dock` toggles), so an
//! Enter that only completed would make the obvious key the one that does nothing.
//!
//! Past the trigger it keeps going, for a command that takes arguments: typing `/diff ` turns the
//! list into the command's own forms — `/diff turn`, `/diff pr <number or URL>` — narrowed as the
//! arguments are typed (`crate::slash_args`). Tab writes the form's words in, and Enter runs it
//! once it is whole: at once for a form with nothing left to fill (`/diff turn`), or with what was
//! typed for one that took a value (`/diff pr 12`). A form still waiting on its value is filled in
//! rather than run without it.

use crate::glyphs::Icon;
use crossterm::event::{KeyCode, KeyEvent};

use crate::commands::{Command, Menu, MenuEntry};

/// State for the slash command flyout.
#[derive(Clone, Debug)]
pub struct SlashFlyoutState {
    /// Every entry with a slash trigger, recently used first
    items: Vec<MenuEntry>,
    /// How many of `items` lead because they were used recently
    recent: usize,
    /// Shared query + filtered-index + cursor state (same nav as the `@` picker).
    nav: crate::picker::PickerNav,
    /// Which characters of each filtered row's trigger the query matched, alongside the filter
    matched: Vec<Vec<usize>>,
    /// The command whose arguments are being typed, once the draft is past its trigger
    arguments: Option<Arguments>,
}

/// The argument forms of one command, while they are what the flyout lists.
#[derive(Clone, Debug)]
struct Arguments {
    command: Command,
    /// What has been typed after the trigger, leading space trimmed
    typed: String,
}

impl Arguments {
    fn trigger(&self) -> &str {
        self.command.slash_name().unwrap_or_default()
    }

    /// Each form with its gloss.
    fn forms(&self) -> &[crate::commands::Usage] {
        &self.command.usage
    }

    /// The forms the typed arguments could still become, by index: the bare form only while
    /// nothing has been typed, since it is what Enter on the trigger alone would have run.
    fn fitting(&self) -> Vec<usize> {
        self.forms()
            .iter()
            .enumerate()
            .filter(|(_, (form, _))| {
                let words = crate::slash_args::words(form);
                match words.is_empty() {
                    true => self.typed.trim().is_empty(),
                    false => crate::slash_args::fit(&words, &self.typed).is_some(),
                }
            })
            .map(|(index, _)| index)
            .collect()
    }
}

impl SlashFlyoutState {
    /// Opens on the entries of `menu` that answer to a slash trigger.
    pub fn new(menu: Menu) -> Self {
        let menu = menu.filtered(|command| command.slash_name().is_some());
        let nav = crate::picker::PickerNav::new(menu.entries.len());
        Self { items: menu.entries, recent: menu.recent, nav, matched: Vec::new(), arguments: None }
    }

    /// Opens on `command`'s argument forms, narrowed to what `typed` could still become, or `None`
    /// when the command takes no arguments or nothing fits — a flyout listing nothing would only
    /// sit over a sentence being typed into `/compact` as noise.
    pub fn arguments(command: Command, typed: &str) -> Option<Self> {
        if command.usage.is_empty() {
            return None;
        }
        let mut state = Self {
            items: Vec::new(),
            recent: 0,
            nav: crate::picker::PickerNav::new(0),
            matched: Vec::new(),
            arguments: Some(Arguments { command, typed: String::new() }),
        };
        state.set_arguments(typed).then_some(state)
    }

    /// The trigger whose arguments are listed, while they are.
    pub fn arguments_of(&self) -> Option<&str> {
        self.arguments.as_ref().map(Arguments::trigger)
    }

    /// Narrows the forms to what `typed` could still become; `false` when none fits.
    ///
    /// The cursor stays on the form it was on while that form still fits, so typing into a value
    /// never moves it.
    pub fn set_arguments(&mut self, typed: &str) -> bool {
        let Some(arguments) = self.arguments.as_mut() else {
            return false;
        };
        let held = self.nav.selected_item_index();
        arguments.typed = typed.trim_start().to_string();
        let fitting = arguments.fitting();
        if fitting.is_empty() {
            return false;
        }
        let position = held.and_then(|held| fitting.iter().position(|index| *index == held));
        self.nav.apply_filter(typed, fitting);
        if let Some(position) = position {
            self.nav.select(position);
        }
        true
    }

    /// Enter or Tab on the form under the cursor.
    ///
    /// Args: `(run)` — Enter runs a form that is whole and fills one that is not; Tab only fills.
    fn on_form(&self, run: bool) -> SlashFlyoutOutcome {
        let Some(arguments) = self.arguments.as_ref() else {
            return SlashFlyoutOutcome::Consumed;
        };
        let Some((form, _)) =
            self.nav.selected_item_index().and_then(|at| arguments.forms().get(at))
        else {
            return SlashFlyoutOutcome::Edit;
        };
        let words = crate::slash_args::words(form);
        let typed = arguments.typed.trim();
        let placeholders =
            words.iter().any(|word| matches!(word, crate::slash_args::Word::Placeholder(_)));
        let complete =
            crate::slash_args::fit(&words, &arguments.typed).is_some_and(|fit| fit.complete);
        let with = |args: Option<String>| {
            let mut command = arguments.command.clone();
            command.args = args.filter(|args| !args.is_empty());
            SlashFlyoutOutcome::Dispatch(command)
        };
        match (run, placeholders) {
            // Nothing to fill: the form names everything, so it runs as written
            (true, false) => {
                let literals: Vec<&str> = words
                    .iter()
                    .filter_map(|word| match word {
                        crate::slash_args::Word::Literal(literal) => Some(*literal),
                        crate::slash_args::Word::Placeholder(_) => None,
                    })
                    .collect();
                with(Some(literals.join(" ")))
            }
            (true, true) if complete => with(Some(typed.to_string())),
            _ => match crate::slash_args::fill(arguments.trigger(), &words, &arguments.typed) {
                Some(draft) => SlashFlyoutOutcome::Fill(draft),
                None if run => with(Some(typed.to_string())),
                None => SlashFlyoutOutcome::Consumed,
            },
        }
    }

    /// Handles picker navigation while leaving text editing to the composer.
    pub fn on_key(&mut self, key: &KeyEvent) -> SlashFlyoutOutcome {
        use crossterm::event::KeyModifiers;

        if self.arguments.is_some() {
            match key.code {
                KeyCode::Enter if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                    return self.on_form(true);
                }
                KeyCode::Tab => return self.on_form(false),
                _ => {}
            }
        }
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                SlashFlyoutOutcome::Close
            }
            KeyCode::Esc => SlashFlyoutOutcome::Close,
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                SlashFlyoutOutcome::Edit
            }
            KeyCode::Enter => self
                .selected_command()
                .cloned()
                .map_or(SlashFlyoutOutcome::Consumed, SlashFlyoutOutcome::Dispatch),
            KeyCode::Tab => self
                .selected_command()
                .cloned()
                .map_or(SlashFlyoutOutcome::Consumed, SlashFlyoutOutcome::Complete),
            // Taken and dropped: left to fall through, shift+tab beside a completing tab would
            // switch the session's mode in the middle of typing a command
            KeyCode::BackTab => SlashFlyoutOutcome::Consumed,
            KeyCode::Up => {
                self.nav.move_up();
                SlashFlyoutOutcome::Consumed
            }
            KeyCode::Down => {
                self.nav.move_down();
                SlashFlyoutOutcome::Consumed
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.nav.move_up();
                SlashFlyoutOutcome::Consumed
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.nav.move_down();
                SlashFlyoutOutcome::Consumed
            }
            _ => SlashFlyoutOutcome::Edit,
        }
    }

    /// Replaces the query after the composer edits its source buffer, re-ranking the results.
    ///
    /// Ranks against the slash trigger ("diff", "dock", …), not the name — the user is typing a
    /// command's name, so "/di" must surface "/diff" whatever it is called. A keyword can still
    /// find a command, after every trigger that matched: `diagnostics` is `/lsp`'s word, and it
    /// must not push `/diff` down for somebody typing `/di`.
    pub fn set_query(&mut self, query: &str) {
        let query = query.trim();
        self.matched.clear();
        let filtered = if query.is_empty() {
            (0..self.items.len()).collect()
        } else {
            let triggers: Vec<&str> = self
                .items
                .iter()
                .map(|entry| entry.command.slash_name().unwrap_or_default().trim_start_matches('/'))
                .collect();
            let (mut filtered, matched): (Vec<usize>, Vec<Vec<usize>>) =
                crate::fuzzy::rank(query, &triggers)
                    .into_iter()
                    .map(|found| (found.idx, found.positions))
                    .unzip();
            // A keyword match lights nothing: the word it matched is not on the row
            self.matched = matched;
            let mut by_keyword: Vec<(usize, i64)> = self
                .items
                .iter()
                .enumerate()
                .filter(|(index, _)| !filtered.contains(index))
                .filter_map(|(index, entry)| Some((index, entry.command.keyword_score(query)?)))
                .collect();
            by_keyword.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
            filtered.extend(by_keyword.into_iter().map(|(index, _)| index));
            filtered
        };
        self.nav.apply_filter(query, filtered);
    }

    /// Which characters of the trigger at filtered position `position` the query matched.
    fn matched_at(&self, position: usize) -> &[usize] {
        self.matched.get(position).map_or(&[], Vec::as_slice)
    }

    /// Get the current query.
    pub fn query(&self) -> &str {
        self.nav.query()
    }

    /// Get the filtered items.
    pub fn filtered_items(&self) -> &[usize] {
        self.nav.filtered_indices()
    }

    /// Moves the selection to `filtered_index` (a mouse click on that row); clamped to range.
    pub fn select(&mut self, filtered_index: usize) {
        self.nav.select(filtered_index);
    }

    /// The cursor's row among the filtered items.
    pub(crate) fn cursor(&self) -> usize {
        self.nav.selected()
    }

    /// Get the selected command (if any).
    pub fn selected_command(&self) -> Option<&Command> {
        if let Some(arguments) = self.arguments.as_ref() {
            return Some(&arguments.command);
        }
        self.items.get(self.nav.selected_item_index()?).map(|entry| &entry.command)
    }

    /// The filtered position the rule under the recent lead sits above, while one is drawn.
    fn divider(&self) -> Option<usize> {
        (self.nav.query().trim().is_empty() && self.recent > 0).then_some(self.recent)
    }
}

/// Outcome of a slash flyout keypress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlashFlyoutOutcome {
    /// Dispatch this command and close the flyout.
    Dispatch(Command),
    /// Complete this command in the composer and close the flyout.
    Complete(Command),
    /// Replace the command being typed with this draft — a form's words, written in.
    Fill(String),
    /// Close without dispatching.
    Close,
    /// Key consumed but flyout stays open.
    Consumed,
    /// Let the composer edit the source buffer, then refresh the query.
    Edit,
}

/// Renders slash results directly above the input, via the shared [`crate::picker`] renderer.
///
/// The pane says what the command under the cursor does and how to give it arguments — the name is
/// already on the row, and the palette is where the whole story is told.
pub fn render_slash_flyout(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    edge: crate::picker::Edge,
    state: &SlashFlyoutState,
    theme: &crate::theme::styles::ThemeStyles,
    hit: &std::cell::RefCell<crate::mouse::HitRegions>,
) {
    use crate::picker::{render_picker, PickerRow, PickerView};

    if let Some(arguments) = state.arguments.as_ref() {
        return render_arguments(f, area, edge, state, arguments, theme, hit);
    }
    let filtered = state.nav.filtered_indices();
    let detail =
        |pos: usize| crate::command_detail::brief_lines(&state.items[filtered[pos]], theme);
    render_picker(
        f,
        area,
        edge,
        theme,
        hit,
        PickerView {
            glyph: Icon::SlashMenu.glyph(),
            title: "Commands",
            empty_message: "No matching commands",
            item_count: filtered.len(),
            selected: state.nav.selected(),
            row: |pos| {
                let command = &state.items[filtered[pos]].command;
                // The trigger is what was matched, and its slash is not part of it
                let matched = state.matched_at(pos).iter().map(|at| at + 1).collect();
                PickerRow {
                    label: command.name.to_string(),
                    detail: command.slash_name().map(str::to_owned),
                    detail_matched: matched,
                    ..Default::default()
                }
            },
            hit_target: crate::mouse::MouseTarget::SlashRow,
            // Tab is the one key here a reader would not guess: it writes the command into the
            // composer, for the arguments it takes, instead of running it
            hint: Some(match filtered.is_empty() {
                true => "esc close",
                false => "enter run · tab add arguments · ↑/↓ choose · esc close",
            }),
            tabs: None,
            divider: state.divider(),
            detail: Some(&detail),
            rows: None,
            list: crate::mouse::OverlayList::Slash,
        },
    );
}

/// The command's argument forms, each with what it does: the list a command turns into once its
/// trigger is typed.
fn render_arguments(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    edge: crate::picker::Edge,
    state: &SlashFlyoutState,
    arguments: &Arguments,
    theme: &crate::theme::styles::ThemeStyles,
    hit: &std::cell::RefCell<crate::mouse::HitRegions>,
) {
    use crate::picker::{render_picker, PickerRow, PickerView};

    let filtered = state.nav.filtered_indices();
    render_picker(
        f,
        area,
        edge,
        theme,
        hit,
        PickerView {
            glyph: Icon::SlashMenu.glyph(),
            title: arguments.command.name.as_ref(),
            empty_message: "No matching forms",
            item_count: filtered.len(),
            selected: state.nav.selected(),
            row: |pos| {
                let (form, gloss) = &arguments.forms()[filtered[pos]];
                PickerRow {
                    label: form.to_string(),
                    detail: (!gloss.is_empty()).then(|| gloss.to_string()),
                    ..Default::default()
                }
            },
            hit_target: crate::mouse::MouseTarget::SlashRow,
            // Tab fills a form in; Enter runs one that is whole, and fills one that is not
            hint: Some("enter run · tab fill in · ↑/↓ choose · esc close"),
            tabs: None,
            divider: None,
            detail: None,
            rows: None,
            list: crate::mouse::OverlayList::Slash,
        },
    );
}

#[cfg(test)]
mod tests;
