//! Mode — what the session may change right now: `build`, or `plan`.
//!
//! A mode is a constraint on the one agent the session already has, not a second agent. The system
//! prompt and the tool list are the same in every mode, so switching never moves the prefix a
//! provider caches. What changes is held in two places:
//!
//! - **Dispatch.** A mode's [`WriteScope`] is enforced at the choke point every tool call passes,
//!   on the same read-only rule drafting and answering turns use. A model told it is planning that
//!   tries an edit anyway is refused, not trusted.
//! - **The transcript.** The model learns the mode from a note appended to the next message it is
//!   sent ([`Mode::entered`], [`Mode::reminder`]), never from the system prompt, so the history
//!   records each change where it happened and a resume replays it.
//!
//! The shape is data — a name, a write scope, instructions, an optional model — so another mode
//! would be one more entry. Only the built-ins exist: nothing reads a mode from config.

use crate::provider::{ModelInfo, ThinkingLevel};

/// The default mode: every tool, every file.
pub const BUILD: &str = "build";
/// Reads, asks, and writes only the plan.
pub const PLAN: &str = "plan";
/// Where a plan goes when `plan.files` says nothing.
pub const DEFAULT_PLAN_FILE: &str = "PLAN.md";

/// What a mode may change in the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteScope {
    /// Anything the permission rules allow
    Anything,
    /// Only files matching these project-relative globs, through `write` and `edit`
    Only(Vec<String>),
    /// Nothing at all
    Nothing,
}

/// One mode: what it is called, what it may change, and what it tells the model.
#[derive(Debug, Clone, PartialEq)]
pub struct Mode {
    /// The name every surface uses, in lower case
    pub name: String,
    /// What the mode may change
    pub writes: WriteScope,
    /// What the model is told when the session enters the mode.
    ///
    /// `None` for a mode that needs no explaining beyond its name, which is what `build` is.
    pub instructions: Option<String>,
    /// The model the mode runs on. `None` keeps the session's.
    pub model: Option<ModelInfo>,
    /// The thinking level the mode runs at. `None` keeps the session's.
    pub thinking: Option<ThinkingLevel>,
}

impl Mode {
    /// The mode every session starts in unless told otherwise.
    pub fn build() -> Self {
        Self::base(BUILD, WriteScope::Anything)
    }

    /// A mode with a name and a write scope, and nothing else yet.
    fn base(name: impl Into<String>, writes: WriteScope) -> Self {
        Self { name: name.into(), writes, instructions: None, model: None, thinking: None }
    }

    /// The plan mode, writing only `files` (globs, project-relative).
    ///
    /// An empty list falls back to [`DEFAULT_PLAN_FILE`]: a plan mode that may write nothing could
    /// never finish its own job.
    pub fn plan(files: Vec<String>) -> Self {
        let files = if files.iter().all(|file| file.trim().is_empty()) {
            vec![DEFAULT_PLAN_FILE.to_string()]
        } else {
            files.into_iter().filter(|file| !file.trim().is_empty()).collect()
        };
        let instructions = plan_instructions(&files);
        Self { instructions: Some(instructions), ..Self::base(PLAN, WriteScope::Only(files)) }
    }

    /// A mode that changes nothing, for an engine that only ever reads (a review).
    pub fn read_only(name: impl Into<String>) -> Self {
        Self::base(name, WriteScope::Nothing)
    }

    /// Run this mode on `model`.
    pub fn with_model(mut self, model: ModelInfo) -> Self {
        self.model = Some(model);
        self
    }

    /// Run this mode at `thinking`.
    pub fn with_thinking(mut self, thinking: ThinkingLevel) -> Self {
        self.thinking = Some(thinking);
        self
    }

    /// The note the next message carries when the session has just entered this mode.
    pub fn entered(&self) -> String {
        let scope = self.scope_sentence();
        match &self.instructions {
            Some(instructions) => format!("[mode: {}] {scope}\n\n{instructions}", self.name),
            None => format!("[mode: {}] {scope}", self.name),
        }
    }

    /// The one line each later message carries while the mode holds, so a long session does not
    /// lose track of it. `None` for a mode with nothing to hold back.
    pub fn reminder(&self) -> Option<String> {
        match self.writes {
            WriteScope::Anything => None,
            _ => {
                Some(format!("[mode: {}] Still in {} mode: {}", self.name, self.name, self.scope()))
            }
        }
    }

    /// What the mode may change, as a phrase: `writes only PLAN.md`.
    pub fn scope(&self) -> String {
        match &self.writes {
            WriteScope::Anything => "changes files and runs commands".into(),
            WriteScope::Only(files) => format!("writes only {}", list(files)),
            WriteScope::Nothing => "changes nothing".into(),
        }
    }

    fn scope_sentence(&self) -> String {
        match &self.writes {
            WriteScope::Anything => format!(
                "The session is in {} mode: you may change files and run commands again.",
                self.name
            ),
            WriteScope::Only(files) => format!(
                "The session is in {} mode. You may read, search and ask, and you may write only \
                 {}. Everything else, the shell included, is refused until the user switches back \
                 to build mode.",
                self.name,
                list(files)
            ),
            WriteScope::Nothing => format!(
                "The session is in {} mode. You may read, search and ask, and nothing you call may \
                 change anything.",
                self.name
            ),
        }
    }
}

/// The modes a session can be in, in the order a cycle visits them. The first is the default.
#[derive(Debug, Clone, PartialEq)]
pub struct Modes {
    modes: Vec<Mode>,
}

impl Default for Modes {
    fn default() -> Self {
        Self::built_in(Mode::plan(Vec::new()))
    }
}

impl Modes {
    /// `build`, then the given plan mode.
    pub fn built_in(plan: Mode) -> Self {
        Self { modes: vec![Mode::build(), plan] }
    }

    /// One mode and no other, which is then the default and the only place to be.
    pub fn only(mode: Mode) -> Self {
        Self { modes: vec![mode] }
    }

    /// The mode a session starts in.
    pub fn default_mode(&self) -> &Mode {
        // `built_in` and `only` both put one in, and nothing removes it
        &self.modes[0]
    }

    /// The mode called `name`, if there is one.
    pub fn get(&self, name: &str) -> Option<&Mode> {
        let name = name.trim();
        self.modes.iter().find(|mode| mode.name.eq_ignore_ascii_case(name))
    }

    /// The mode a cycle moves to from `name`; the default when `name` is not one of these.
    pub fn after(&self, name: &str) -> &Mode {
        let at = self.modes.iter().position(|mode| mode.name == name);
        match at {
            Some(index) => &self.modes[(index + 1) % self.modes.len()],
            None => self.default_mode(),
        }
    }

    /// Every mode's name, in cycle order.
    pub fn names(&self) -> Vec<&str> {
        self.modes.iter().map(|mode| mode.name.as_str()).collect()
    }
}

/// A mode note found in a transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note<'a> {
    /// The mode it names
    pub mode: &'a str,
    /// Whether it is the one-line reminder rather than the note a switch leaves
    pub reminder: bool,
}

/// The mode note `text` is, when it is one.
///
/// Notes ride in user messages, so a transcript read back has to tell them from what the user wrote:
/// the engine to know which mode the model last heard about, a front-end to draw a switch as a
/// switch rather than as something the user said.
pub fn note(text: &str) -> Option<Note<'_>> {
    let rest = text.strip_prefix("[mode: ")?;
    let (mode, after) = rest.split_once(']')?;
    if mode.is_empty() || mode.contains(char::is_whitespace) {
        return None;
    }
    Some(Note { mode, reminder: after.trim_start().starts_with("Still in ") })
}

/// What the model is told on entering plan mode.
///
/// The plan's shape is the loop's: `## Phase N` headings from 0, each with checks that close it, so
/// a plan written here is one `luxe loop start` can work through and its reviewer can judge phase by
/// phase without anybody reformatting it.
fn plan_instructions(files: &[String]) -> String {
    // `Mode::plan` never leaves the list empty, so a list with no plain file holds a glob
    let target = match files.iter().find(|file| !file.contains(['*', '?']) && !file.ends_with('/'))
    {
        Some(file) => format!("`{file}`"),
        None => format!("a file matching `{}`", files[0]),
    };
    format!(
        "Plan rather than build. Reach a shared understanding first: read the code a decision \
         touches before you ask about it, then ask the open questions one at a time with the \
         `question` tool, resolving what each later decision depends on first. Do not propose a \
         plan while a decision it rests on is still open.\n\n\
         When the decisions are settled, write the plan to {target}: a `# ` title, a short \
         paragraph on the goal, then one `## Phase N — name` section per phase, numbered from 0. \
         Each phase says what to build and the commands or checks that prove it done, and is small \
         enough to review on its own. Then say in a sentence that the plan is ready. The user \
         decides whether to build it in this session, in a fresh one, or as a loop."
    )
}

/// `a`, `a and b`, `a, b and c`, each in backticks.
fn list(files: &[String]) -> String {
    let quoted: Vec<String> = files.iter().map(|file| format!("`{file}`")).collect();
    match quoted.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_starts_in_build_and_cycles_through_plan_and_back() {
        let modes = Modes::default();
        assert_eq!(modes.default_mode().name, BUILD);
        assert_eq!(modes.after(BUILD).name, PLAN);
        assert_eq!(modes.after(PLAN).name, BUILD);
        assert_eq!(modes.after("nonsense").name, BUILD, "an unknown name goes home");
        assert_eq!(modes.names(), vec![BUILD, PLAN]);
    }

    #[test]
    fn a_mode_is_found_by_its_name_in_any_case() {
        let modes = Modes::default();
        assert_eq!(modes.get(" Plan ").map(|mode| mode.name.as_str()), Some(PLAN));
        assert!(modes.get("planner").is_none(), "a near miss is not a mode");
    }

    /// The break this catches: a configured `"files": []` leaving a plan mode that could write
    /// nothing, which reads as a planner that refuses to finish.
    #[test]
    fn a_plan_mode_with_no_files_writes_the_default_plan() {
        for files in [Vec::new(), vec![" ".to_string()]] {
            assert_eq!(
                Mode::plan(files).writes,
                WriteScope::Only(vec![DEFAULT_PLAN_FILE.to_string()])
            );
        }
    }

    /// The plan has to come out in the shape the loop reads, or the handoff to `luxe loop` is a
    /// coincidence.
    #[test]
    fn plan_instructions_ask_for_the_shape_the_loop_reads() {
        let plan = Mode::plan(vec!["docs/design/*.md".into(), "PLAN.md".into()]);
        let note = plan.entered();
        assert!(note.starts_with("[mode: plan]"), "{note}");
        assert!(note.contains("`PLAN.md`"), "names the file to write, not the glob: {note}");
        assert!(note.contains("## Phase N"), "{note}");
        assert!(note.contains("numbered from 0"), "the loop starts at phase 0: {note}");
        assert!(note.contains("one at a time"), "{note}");
        assert!(
            note.contains("`docs/design/*.md` and `PLAN.md`"),
            "lists every writable file: {note}"
        );
    }

    /// The break this catches: a glob-only list falling back to `PLAN.md`, a file that config
    /// makes unwritable, so the agent is told to write somewhere it will be refused.
    #[test]
    fn a_glob_only_plan_mode_is_told_to_write_inside_its_glob() {
        let note = Mode::plan(vec!["docs/audit/*.md".into()]).entered();
        assert!(note.contains("to a file matching `docs/audit/*.md`"), "{note}");
        assert!(!note.contains("PLAN.md"), "names nothing it cannot write: {note}");
    }

    #[test]
    fn only_a_mode_that_holds_something_back_reminds() {
        assert!(Mode::build().reminder().is_none());
        let reminder = Mode::plan(Vec::new()).reminder().expect("plan holds writes back");
        assert!(reminder.contains("writes only `PLAN.md`"), "{reminder}");
        assert!(Mode::read_only("review").reminder().is_some());
    }

    /// Every note a mode writes is read back as that mode, and nothing a user would type is.
    #[test]
    fn a_note_is_read_back_as_the_mode_that_wrote_it() {
        let plan = Mode::plan(Vec::new());
        assert_eq!(note(&plan.entered()), Some(Note { mode: PLAN, reminder: false }));
        assert_eq!(note(&plan.reminder().unwrap()), Some(Note { mode: PLAN, reminder: true }));
        assert_eq!(note(&Mode::build().entered()), Some(Note { mode: BUILD, reminder: false }));
        assert_eq!(note("Please [mode: plan] this"), None, "only a leading note");
        assert_eq!(note("[hook] context"), None);
        assert_eq!(note("[mode: two words] x"), None);
    }

    #[test]
    fn leaving_plan_says_the_brakes_are_off() {
        let note = Mode::build().entered();
        assert!(note.starts_with("[mode: build]"), "{note}");
        assert!(note.contains("change files and run commands"), "{note}");
    }
}
