//! commands — the one list of what luxe can do, for the palette and the slash flyout.
//!
//! Both doors read this registry: `Ctrl+K` lists everything that can run where you are, `/` lists
//! the ones with a slash trigger. A command is data — a name you would say out loud, a sentence
//! saying what happens, the argument forms, and what it needs before it can do anything — and
//! [`crate::app`] owns what running one means.
//!
//! # Naming
//!
//! - Verb plus object, four words at most: "New session", "Review changes".
//! - No parentheses, colons or argument lists in a name. Those are `Command::usage`.
//! - A command that only reports is named for what it reports: "Tools", "Session cost".
//! - Two names never differ only by an adverb; if they would, one is misnamed or they are one
//!   command.
//! - The summary is one present-tense sentence saying what *happens*.
//!
//! Search ranks against the name, the slash trigger and the keywords, never the summary: prose in
//! the haystack matches every query: typing `auth` would return eight rows.

use std::borrow::Cow;

use luxe_core::keymap::{KeyMap, UiAction};

mod built_in;
pub(crate) mod id;
mod rank;

use built_in::built_in;
pub use rank::rank;

/// How many of the most recently used commands lead an unfiltered list.
pub const RECENT_SHOWN: usize = 5;

/// How a command runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandKind {
    /// Handled by id in [`crate::app`]'s slash handler, with any arguments typed after it
    Slash,
    /// Dispatched as this UI action, the same call its key makes
    Action(UiAction),
}

/// Something a command needs before it can do anything.
///
/// Data rather than a closure, so a [`Command`] stays comparable and printable and the question can
/// be asked of a [`CommandContext`] without an `App`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// The conversation on screen, rather than a review
    Chat,
    /// A Guided Review on screen
    Review,
    /// A pull request's review on screen
    PullRequestReview,
    /// A diff on screen: the Changes pane or a review
    Diff,
    /// The Changes pane open over the transcript
    ChangesPane,
    /// Something said in this conversation to act on
    Turn,
    /// An image in the conversation
    Image,
    /// Messages waiting for a turn of their own
    Waiting,
    /// A git checkout
    Repository,
}

impl Need {
    /// Whether `context` meets this need.
    pub fn met(self, context: &CommandContext) -> bool {
        let chat = context.surface == Surface::Chat;
        match self {
            Self::Chat => chat,
            Self::Review => context.surface == Surface::Review,
            Self::PullRequestReview => {
                context.surface == Surface::Review && context.pull_request_review
            }
            Self::Diff => context.surface == Surface::Review || (chat && context.changes_pane),
            Self::ChangesPane => chat && context.changes_pane,
            Self::Turn => context.has_turn,
            Self::Image => context.has_image,
            Self::Waiting => context.messages_waiting,
            Self::Repository => context.in_repository,
        }
    }

    /// What is missing, to finish the sentence "{command} needs …".
    pub fn describe(self) -> &'static str {
        match self {
            Self::Chat => "the conversation on screen",
            Self::Review => "a review open",
            Self::PullRequestReview => "a pull request's review open",
            Self::Diff => "a diff on screen",
            Self::ChangesPane => "the preview open",
            Self::Turn => "something in the conversation",
            Self::Image => "an image in the conversation",
            Self::Waiting => "messages waiting for a turn",
            Self::Repository => "a git checkout",
        }
    }
}

/// Which full-screen surface the palette opened over.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Surface {
    /// The conversation, and anything drawn over it
    #[default]
    Chat,
    /// Guided Review
    Review,
}

/// What the app can say about itself when a menu is built, for [`Need::met`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandContext {
    /// The surface on screen
    pub surface: Surface,
    /// Whether the review on screen is of a pull request
    pub pull_request_review: bool,
    /// Whether the Changes pane is open
    pub changes_pane: bool,
    /// Whether the conversation holds a message of yours
    pub has_turn: bool,
    /// Whether the conversation holds an image
    pub has_image: bool,
    /// Whether any message is waiting for a turn
    pub messages_waiting: bool,
    /// Whether the project is a git checkout
    pub in_repository: bool,
}

/// One argument form and what it does: `("/dock left", "move it to the left")`.
pub type Usage = (Cow<'static, str>, Cow<'static, str>);

/// Old slash triggers and the commands they became.
const RENAMED_SLASHES: &[(&str, &str)] = &[("/peek", "/preview")];

/// A single command: what it is called, what it does, and what it needs.
///
/// The text fields are `Cow` so a command can come from somewhere other than this file without
/// the built-ins paying for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    /// Stable id: `cmd.new`, or the action's own id (`tui.app.quit`)
    pub id: Cow<'static, str>,
    /// What you would say out loud to ask for it
    pub name: Cow<'static, str>,
    /// The `/…` trigger, when it has one
    pub slash: Option<Cow<'static, str>>,
    /// One sentence: what happens when you run it
    pub summary: Cow<'static, str>,
    /// The rest, for the detail pane; may be empty
    pub detail: Cow<'static, str>,
    /// Argument forms, each with a gloss that may be empty
    pub usage: Vec<Usage>,
    /// Words a reader might type that are not in the name: "exit" finds Quit
    pub keywords: Vec<Cow<'static, str>>,
    /// How it runs
    pub kind: CommandKind,
    /// The action whose key runs the same thing, for showing that key beside it
    pub keys: Option<UiAction>,
    /// What it needs before it can do anything; all of them must hold
    pub needs: Cow<'static, [Need]>,
    /// Arguments typed after the slash trigger, once submitted
    pub args: Option<String>,
}

impl Command {
    /// A command handled by id in the slash handler.
    fn slash(id: &'static str, name: &'static str, slash: &'static str) -> Self {
        Self::new(Cow::Borrowed(id), name, Some(slash), CommandKind::Slash, None)
    }

    /// A command that dispatches `action`, named by the action's own label.
    fn action(action: UiAction) -> Self {
        Self::new(
            Cow::Borrowed(action.id()),
            action.label(),
            None,
            CommandKind::Action(action),
            Some(action),
        )
    }

    fn new(
        id: Cow<'static, str>,
        name: &'static str,
        slash: Option<&'static str>,
        kind: CommandKind,
        keys: Option<UiAction>,
    ) -> Self {
        Self {
            id,
            name: Cow::Borrowed(name),
            slash: slash.map(Cow::Borrowed),
            summary: Cow::Borrowed(""),
            detail: Cow::Borrowed(""),
            usage: Vec::new(),
            keywords: Vec::new(),
            kind,
            keys,
            needs: Cow::Borrowed(&[]),
            args: None,
        }
    }

    fn with_slash(mut self, slash: &'static str) -> Self {
        self.slash = Some(Cow::Borrowed(slash));
        self
    }

    fn summary(mut self, summary: &'static str) -> Self {
        self.summary = Cow::Borrowed(summary);
        self
    }

    fn detail(mut self, detail: &'static str) -> Self {
        self.detail = Cow::Borrowed(detail);
        self
    }

    fn usage(mut self, usage: &[(&'static str, &'static str)]) -> Self {
        self.usage = usage
            .iter()
            .map(|(form, gloss)| (Cow::Borrowed(*form), Cow::Borrowed(*gloss)))
            .collect();
        self
    }

    fn keywords(mut self, keywords: &[&'static str]) -> Self {
        self.keywords = keywords.iter().copied().map(Cow::Borrowed).collect();
        self
    }

    fn keys(mut self, action: UiAction) -> Self {
        self.keys = Some(action);
        self
    }

    fn needs(mut self, needs: &'static [Need]) -> Self {
        self.needs = Cow::Borrowed(needs);
        self
    }

    /// The `/name` trigger this command answers to, if any.
    pub fn slash_name(&self) -> Option<&str> {
        self.slash.as_deref()
    }

    /// The first need `context` does not meet, or `None` when the command can run.
    pub fn unmet(&self, context: &CommandContext) -> Option<Need> {
        self.needs.iter().copied().find(|need| !need.met(context))
    }

    /// Whether the command can run in `context`.
    pub fn available(&self, context: &CommandContext) -> bool {
        self.unmet(context).is_none()
    }

    /// What a query is matched against, fuzzily: the name, then the trigger without its slash.
    ///
    /// The name leads, so a match position below its length is a character of the name, and one
    /// past it is a character of the trigger. The slash is left off because the matcher counts a
    /// word after a space as a stronger boundary than one after a `/`, which let a keyword outrank
    /// the command whose trigger it was.
    pub fn haystack(&self) -> String {
        match self.slash_name() {
            Some(slash) => format!("{} {}", self.name, slash.trim_start_matches('/')),
            None => self.name.to_string(),
        }
    }

    /// How well `query` names one of the command's keywords, when it begins one of its words.
    ///
    /// Keywords are vocabulary, not text to scatter a match across: "exit" finds Quit, and "hunk"
    /// must not find Compact through the letters of "shrink". So a keyword matches only from the
    /// start of one of its words, and scores a little under a name matched as well, so a command
    /// found by what it is called outranks one found by what it is about.
    pub fn keyword_score(&self, query: &str) -> Option<i64> {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return None;
        }
        self.keywords
            .iter()
            .map(|keyword| keyword.to_lowercase())
            .filter(|keyword| {
                keyword.starts_with(&query)
                    || keyword.split(' ').any(|word| word.starts_with(&query))
            })
            .filter_map(|keyword| {
                crate::fuzzy::rank(&query, &[keyword.as_str()]).first().map(|found| found.score)
            })
            .max()
            .map(|score| score - KEYWORD_PENALTY)
    }
}

/// What a keyword match gives up against a name matched equally well.
const KEYWORD_PENALTY: i64 = 8;

/// One row of a menu: the command, and the key bound to it right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuEntry {
    /// The command
    pub command: Command,
    /// Its first bound chord, as the keymap writes it (`Ctrl+G`)
    pub chord: Option<String>,
}

/// The commands that can run somewhere, in the order a menu lists them unfiltered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Menu {
    /// Recently used first, then everything else by name
    pub entries: Vec<MenuEntry>,
    /// How many of `entries` lead because they were used recently
    pub recent: usize,
}

impl Menu {
    /// The same menu, keeping only what `keep` accepts; the recent lead shrinks with it.
    pub fn filtered(self, keep: impl Fn(&Command) -> bool) -> Self {
        let recent =
            self.entries.iter().take(self.recent).filter(|entry| keep(&entry.command)).count();
        let entries = self.entries.into_iter().filter(|entry| keep(&entry.command)).collect();
        Self { entries, recent }
    }
}

/// The commands the palette and `/` ran, most recent first, and where they are kept.
///
/// In memory by default, which is what a test or an embedder gets: nothing it runs reaches the
/// reader's state file. The TUI points it at the runtime state when it launches.
#[derive(Clone, Debug, Default)]
pub struct RecentCommands {
    ids: Vec<String>,
    store: Option<std::path::PathBuf>,
}

impl RecentCommands {
    /// Reads what `path` remembers, and keeps writing it there.
    pub fn persisted(path: std::path::PathBuf) -> Self {
        let ids = luxe_core::runtime_state::RuntimeState::load_from(&path).recent_commands;
        Self { ids, store: Some(path) }
    }

    /// Records `id` as the most recently used.
    pub fn record(&mut self, id: &str) {
        match &self.store {
            Some(path) => {
                self.ids = luxe_core::runtime_state::RuntimeState::remember_command_at(path, id);
            }
            None => luxe_core::runtime_state::push_recent(&mut self.ids, id),
        }
    }

    /// The ids, most recent first.
    pub fn ids(&self) -> &[String] {
        &self.ids
    }
}

/// The full command registry.
#[derive(Clone, Debug)]
pub struct CommandRegistry {
    /// Every command, whether or not it can run right now
    pub commands: Vec<Command>,
}

impl CommandRegistry {
    /// Every command luxe knows.
    pub fn all() -> Self {
        Self::from_sources([built_in()])
    }

    /// [`Self::all`], built once: for a caller that asks every frame, like the composer lighting a
    /// command as it is typed.
    pub fn shared() -> &'static Self {
        static SHARED: std::sync::OnceLock<CommandRegistry> = std::sync::OnceLock::new();
        SHARED.get_or_init(Self::all)
    }

    /// Joins lists of commands into one, the first to claim an id keeping it.
    ///
    /// The built-ins come first, so nothing added later can take over a name luxe already answers
    /// to.
    pub fn from_sources(sources: impl IntoIterator<Item = Vec<Command>>) -> Self {
        let mut commands: Vec<Command> = Vec::new();
        for command in sources.into_iter().flatten() {
            let taken = commands.iter().any(|kept| {
                kept.id == command.id
                    || (command.slash.is_some() && kept.slash_name() == command.slash_name())
            });
            if !taken {
                commands.push(command);
            }
        }
        Self { commands }
    }

    /// Finds a slash command in submitted editor text and attaches its arguments.
    ///
    /// Every command is found, whether or not it can run here: a typed command that cannot run is
    /// told why, which "unknown command" would not do.
    /// Args: `(input)` — a renamed command's old trigger still works, so fingers and notes that
    /// learned it keep working.
    pub fn parse_slash(&self, input: &str) -> Option<Command> {
        let trimmed = input.trim();
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let name = parts.next()?;
        if !name.starts_with('/') {
            return None;
        }
        let name = RENAMED_SLASHES
            .iter()
            .find_map(|(old, new)| (*old == name).then_some(*new))
            .unwrap_or(name);
        let mut command =
            self.commands.iter().find(|command| command.slash_name() == Some(name))?.clone();
        command.args =
            parts.next().map(str::trim).filter(|args| !args.is_empty()).map(str::to_owned);
        Some(command)
    }

    /// The commands that can run in `context`, the recently used first.
    ///
    /// Args: `(context, keymap, recent)` — `recent` most recent first, as the runtime state keeps
    /// it. A recent command that cannot run here is skipped rather than dropped: it leads again
    /// once you are somewhere it works.
    pub fn arrange(&self, context: &CommandContext, keymap: &KeyMap, recent: &[String]) -> Menu {
        let entry = |command: &Command| MenuEntry {
            command: command.clone(),
            chord: command
                .keys
                .and_then(|action| keymap.chords_for(action))
                .and_then(|chords| chords.first())
                .map(ToString::to_string),
        };
        let listed: Vec<&Command> =
            self.commands.iter().filter(|command| command.available(context)).collect();
        let mut entries: Vec<MenuEntry> = recent
            .iter()
            // a renamed action's old id, remembered before the rename, finds it under the new one
            .map(|id| UiAction::from_id(id).map_or(id.as_str(), |action| action.id()))
            .filter_map(|id| listed.iter().find(|command| command.id == id))
            .take(RECENT_SHOWN)
            .map(|command| entry(command))
            .collect();
        let lead = entries.len();
        let mut rest: Vec<&Command> = listed
            .into_iter()
            .filter(|command| !entries[..lead].iter().any(|kept| kept.command.id == command.id))
            .collect();
        rest.sort_by(|left, right| {
            left.name.to_lowercase().cmp(&right.name.to_lowercase()).then(left.id.cmp(&right.id))
        });
        entries.extend(rest.into_iter().map(entry));
        Menu { entries, recent: lead }
    }

    /// Get all commands.
    pub fn iter(&self) -> impl Iterator<Item = &Command> {
        self.commands.iter()
    }

    /// Get the number of commands.
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}

#[cfg(test)]
mod tests;
