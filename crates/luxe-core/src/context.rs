//! Context system — system prompt composition.
//!
//! Assembles what the model actually sees, layer by layer:
//! 1. Base harness prompt (the persona, then the always-on charters: honesty, communication,
//!    engineering standards, memory, working discipline)
//! 2. Environment (cwd, OS, shell, git status)
//! 3. Tool guidance (what a per-tool schema can't say — not an index)
//! 4. AGENTS.md (global + project, nearest-wins order)
//! 5. Skills index (when skills are discovered)
//!
//! Durable project documents are NOT injected — they are pointed at. `AGENTS.md` is the one file
//! that rides in every request; a plan or a spec is something the agent opens when the work calls
//! for it (repo-as-memory).
//!
//! The prompt is the same in every mode. A mode changes what the session may do, held at dispatch
//! and told to the model in the transcript (`luxe_core::mode`), so switching never moves the prefix.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Tools the always-on charters name by name.
///
/// A prompt that tells the model to reach for a tool the request does not carry is worse than
/// silent: it spends the instruction *and* buys a failed call. Every name in the harness therefore
/// has to be either always shipped or emitted conditionally, and this list is what the test holds
/// the two sides to.
#[cfg(test)]
const HARNESS_TOOLS: &[&str] = &["question", "todo"];

/// Where questions about luxe itself are answered: its own guide, built into the binary and run
/// through `bash`. Static text, so the prompt stays byte-identical between turns; without it, a
/// model asked how to configure luxe answers from what other harnesses do.
const LUXE_DOCS: &str = " Questions about luxe itself are answered by its guide: `luxe docs`.";

/// Purpose-built tools paired with the shell command a model reaches for instead.
///
/// The prompt only names the pairs whose tool is actually callable, so a deny-listed or
/// not-yet-activated tool is never advertised.
///
/// The two spellings of `sed` are both here on purpose, because they are two different habits with
/// two different answers and only one of them was ever named. Across 44,000 logged shell calls,
/// `sed -n 'N,Mp'` — a line range, which is `read`'s `offset`/`limit` — outnumbered `sed -i` by six
/// to one and was the single largest un-steered case in the corpus.
///
/// Writing is named in both its shell spellings for the same reason: left to itself a model batches
/// a heredoc, three `sed -i`s and the test run into one call, which saves round-trips and skips
/// everything that follows a tool edit.
const SHELL_ALTERNATIVES: &[(&str, &str)] = &[
    ("read", "cat and sed -n"),
    ("glob", "find"),
    ("grep", "rg"),
    ("write", "heredocs and cat >"),
    ("edit", "sed -i and >> appends"),
    ("git", "raw git and gh"),
    ("test", "the test runner"),
];

/// Prompt context — what the builder needs to assemble the system prompt.
#[derive(Debug, Clone)]
pub struct PromptContext {
    /// Current working directory.
    pub cwd: PathBuf,
    /// Operating system (e.g., "linux", "macos").
    pub os: String,
    /// Shell (e.g., "bash", "zsh").
    pub shell: String,
    /// Git branch and dirty status, if available.
    pub git: Option<GitStatus>,
    /// AGENTS.md content (global + project, in order).
    pub agents_md: Vec<String>,
    /// Names of the tools the model can call this turn.
    pub tools: Vec<String>,
    /// Skills documentation lines.
    pub skills: Vec<String>,
}

/// Git status information.
#[derive(Debug, Clone)]
pub struct GitStatus {
    /// Current branch name.
    pub branch: String,
}

/// Which layer of the system prompt a measured section came from.
///
/// The report groups by *lever*, not by heading: everything a user cannot change without changing
/// the product is [`PromptSection::Harness`], and the three layers they can change get their own
/// name. Ordering is display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PromptSection {
    /// Role, communication, standards, memory, discipline, environment, tool guidance.
    Harness,
    /// The `AGENTS.md` chain.
    Instructions,
    /// The discoverable skill index.
    Skills,
    /// The deferred-tool catalog, present only under lazy tools.
    DeferredTools,
    /// The one-time resume briefing, appended after everything cacheable.
    Appended,
}

/// One assembled section and the layer it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MeasuredSection {
    pub(crate) section: PromptSection,
    /// The assembled text, exactly as it reaches the joined prompt.
    pub(crate) text: String,
    /// Whether every session of this build and role gets the same text, as opposed to one about
    /// this project, directory or session. The shared sections come first, so a provider that
    /// caches by explicit breakpoint can end one there and share it across sessions.
    pub(crate) shared: bool,
}

/// The system prompt builder — assembles the layered prompt.
pub struct SystemPromptBuilder;

impl Default for SystemPromptBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemPromptBuilder {
    /// Create a new system prompt builder.
    pub fn new() -> Self {
        Self
    }

    /// Build the complete system prompt from the given context.
    pub fn build(&self, ctx: &PromptContext) -> String {
        join_sections(&self.build_measured(ctx))
    }

    /// [`Self::build`], keeping each section and the layer it came from.
    ///
    /// The same assembly, so the two cannot drift: `build` is this joined. Section boundaries are
    /// lost the moment the prompt becomes one string, and re-finding them by scanning for `## `
    /// would put the contract in the heading text — where a copy-edit could silently move a
    /// category's worth of tokens from one row of `/context` to another.
    pub(crate) fn build_measured(&self, ctx: &PromptContext) -> Vec<MeasuredSection> {
        let mut sections = Vec::new();
        let mut push = |section: PromptSection, text: String, shared: bool| {
            if !text.is_empty() {
                sections.push(MeasuredSection { section, text, shared });
            }
        };

        // What every session shares first, what is about this one after. A new session — a
        // worktree, another project, a second agent — then reads the harness from the provider's
        // cache instead of writing it again because a directory name sat in the middle of it.
        push(PromptSection::Harness, self.build_base_harness(ctx), true);
        push(PromptSection::Harness, self.build_tool_docs(ctx), true);
        if !ctx.agents_md.is_empty() {
            push(PromptSection::Instructions, self.build_agents_md(ctx), false);
        }
        if !ctx.skills.is_empty() {
            push(PromptSection::Skills, self.build_skills(ctx), false);
        }
        push(PromptSection::Harness, self.build_environment(ctx), false);

        sections
    }

    /// Build the base harness prompt.
    fn build_base_harness(&self, ctx: &PromptContext) -> String {
        let mut sections = Vec::new();

        sections.push(format!("## Role\n{DEFAULT_ROLE}"));

        // Honesty first among the charters: how much a reader has to read is a comfort problem,
        // and whether what they read is true is not
        sections.push(HONESTY.to_string());

        // Reading is the one thing a user does on every turn, and a model's default register is
        // long and dense. `LUXE_SKIP_COMM` drops the section, for measuring what it costs.
        if std::env::var("LUXE_SKIP_COMM").is_err() {
            sections.push(COMMUNICATION.to_string());
        }

        // Numbers rather than adjectives: "small functions" is advice a model agrees with and then
        // reads its own way, while "4–20 lines" is checkable while writing. The edit hook holds
        // the size limits too, so a file that grows past one is said so at the time.
        sections.push(ENGINEERING_STANDARDS.to_string());

        // "Write a test" is an instruction a model already follows, and following only that is
        // what produces suites that are large, green, and hollow
        sections.push(TESTING.to_string());

        sections.push(memory_section(ctx));
        sections.push(plans_section());

        // The commit paragraph is the cheap half of that rule: the permission engine is what holds
        // it (`git`/`bash` commit default to `ask`), and this keeps the prompt from firing at all.
        // The same pairing for root, which the engine denies.
        sections.push(WORKING_DISCIPLINE.to_string());

        sections.join("\n\n")
    }

    /// Build the environment section.
    fn build_environment(&self, ctx: &PromptContext) -> String {
        let mut parts = Vec::new();
        parts.push(format!("**cwd**: `{}`", ctx.cwd.display()));
        parts.push(format!("**OS**: {}", ctx.os));
        parts.push(format!("**shell**: {}", ctx.shell));

        // Branch only — never the dirty flag. It flips on the agent's own first edit, and this
        // section sits inside the prefix every provider caches, so a flip re-bills the whole
        // prompt. `git status` answers the question on demand, exactly.
        if let Some(ref git) = ctx.git {
            parts.push(format!("**git**: {}", git.branch));
        }

        format!("## Environment\n{}", parts.join(" | "))
    }

    /// Build the tool-guidance section.
    ///
    /// Deliberately **not** an index: every callable tool already ships in the request with its
    /// own name, description, and JSON schema, so re-listing them costs tokens on every turn and
    /// says nothing — and on the wrap-up call, where tools are withheld, it would advertise tools
    /// that aren't there. What survives is the one thing a per-tool schema structurally cannot
    /// express: which tool to reach for when `bash` would also work.
    fn build_tool_docs(&self, ctx: &PromptContext) -> String {
        let has = |name: &str| ctx.tools.iter().any(|tool| tool == name);
        if !has("bash") {
            return String::new();
        }
        let pairs: Vec<String> = SHELL_ALTERNATIVES
            .iter()
            .filter(|(tool, _)| has(tool))
            .map(|(tool, shell)| format!("- `{tool}` over {shell}"))
            .collect();
        if pairs.is_empty() {
            return String::new();
        }

        format!(
            "## Tools\n\
             Every tool in luxe is designed to beat the shell command it replaces by providing \
             structured, bounded output for a fraction of the context. They also make your actions \
             easier for the user to read, which improves clarity and efficiency. So, always reach \
             for the purpose-built tool before `bash`, which should be reserved for everything they \
             don't cover. When you do use it, give each call a short `description` of what it does \
             so the user can follow along.\n{}{}",
            pairs.join("\n"),
            if has("edit") || has("write") {
                "\nFiles change through `edit` and `write`, never the shell, even when one `bash` \
                 call could do several steps at once: an `edit` is formatted, checked for \
                 diagnostics, tracked for review, and easier for the user to read. A heredoc is \
                 none of those."
            } else {
                ""
            }
        )
    }

    /// Build the AGENTS.md section.
    fn build_agents_md(&self, ctx: &PromptContext) -> String {
        let content = ctx.agents_md.join("\n\n");
        format!("## Agent Instructions\n{}", content)
    }

    /// Build the skills section: a bulleted index plus how to load a skill's full instructions.
    fn build_skills(&self, ctx: &PromptContext) -> String {
        format!(
            "## Skills\n{}\nLoad a skill's full instructions on demand with \
             `skill({{ \"action\": \"load\", \"name\": \"...\" }})` before doing matching work.",
            ctx.skills.join("\n")
        )
    }
}

/// Joins measured sections into the prompt exactly as it goes on the wire.
///
/// The separator is load-bearing: the cached prefix is compared byte for byte between turns, so
/// this is the one place the joining is written down.
pub(crate) fn join_sections(sections: &[MeasuredSection]) -> String {
    sections.iter().map(|section| section.text.as_str()).collect::<Vec<_>>().join("\n\n")
}

/// Bytes at the start of [`join_sections`]' output that are the leading run of shared sections,
/// separator included, so what follows begins at a heading.
pub(crate) fn shared_len(sections: &[MeasuredSection]) -> usize {
    let run = sections.iter().take_while(|section| section.shared).count();
    if run == 0 {
        return 0;
    }
    let text: usize = sections[..run].iter().map(|section| section.text.len()).sum();
    // a separator after each, the last one only when something follows it
    text + 2 * (run - 1) + if run < sections.len() { 2 } else { 0 }
}

/// The `## Memory` section: the repository as what outlives the conversation, and where to ask
/// about luxe itself when the agent can run `luxe docs`.
fn memory_section(ctx: &PromptContext) -> String {
    let docs = if ctx.tools.iter().any(|tool| tool == "bash") { LUXE_DOCS } else { "" };
    format!(
        "## Memory\n\
         The repository is your durable memory. Commit messages, comments, and context files create \
         a historic record of progress that persists across context windows and chat sessions. Use \
         them. Anything worth keeping beyond this turn, such as a plan for a large effort, a notable \
         finding, or a hand-off, should be saved as a file in the repo where the next reader will \
         find it.{docs}"
    )
}

/// The `## Plans` section: the live `todo` checklist.
///
/// `LUXE_TODO_PROMPT` swaps the guidance for a named variant, so a change to this wording can be
/// measured against the alternatives rather than argued about. Unset means the shipped wording.
fn plans_section() -> String {
    let rules = match std::env::var("LUXE_TODO_PROMPT").unwrap_or_default().as_str() {
        // Cost-framed wording: two "do NOT"s and no reason to reach for one.
        "restrictive" => {
            "The `todo` tool is a live checklist for a *genuinely multi-step* effort (several \
             distinct actions). Create it once, keep exactly one item in_progress, and update it \
             only when a real step finishes. Do NOT open a todo for a single edit or a trivial fix, \
             and do NOT emit a `todo` call after every action."
        }
        // Claude Code / OpenCode wording, verbatim in spirit.
        "aggressive" => {
            "Use the `todo` tool VERY frequently to track your tasks and give the user visibility \
             into your progress. It is EXTREMELY helpful for planning and for breaking large tasks \
             into smaller steps. If you do not use this tool when planning, you may forget to do \
             important tasks, and that is unacceptable. Mark todos completed as soon as they are \
             done; keep exactly one item in_progress."
        }
        _ => PLANS,
    };
    format!("## Plans\n{rules}")
}

/// Who the agent is, in every session and every mode.
pub(crate) const DEFAULT_ROLE: &str =
    "You are an agentic engineering assistant operating in luxe, a coding \
agent harness. Build features incrementally with small, safe diffs, and verify them before calling \
the work done.";

/// The always-on honesty contract. Each rule names a failure shape rather than asking for a
/// disposition: "be accurate" is advice a model agrees with and then breaks.
const HONESTY: &str = "## Honesty
You are trusted for your knowledge, expertise, and accuracy. Use the tools at your disposal to \
gather evidence and confirm the facts. Be honest about what you know and what you don't.
- **Never present a guess as a fact.** Simply state when you're unsure, or go look to find out.
- **\"I don't know\", and what would settle it, is a complete answer.** An invented answer, even a \
plausible one, is a worse failure than admitting the gap.
- **Never report work you didn't do.** A test you didn't run didn't pass.
- **Correct yourself out loud.** The moment you notice an earlier statement was wrong, briefly say \
so and carry on. This keeps the user updated along the way.";

/// How to talk to the person on the other end.
const COMMUNICATION: &str = "## Communication
You think and act much faster than the user is able to read, let alone understand. Every output \
should earn its place in the transcript by concisely conveying information that is worth the cost \
of the user's time and effort to read.
- **Answer first.** Lead with the answer, then the why, then caveats. Never open by restating the \
question or by announcing what you are about to say.
- **Cut filler.** No \"Great question\", no \"Certainly\", no \"I'll now…\". The user can already see \
tool calls and diffs, so don't restate them.
- **Use plain, straightforward language.** Keep things simple and avoid technical jargon. \
\"Invariant\" takes longer to understand than \"something that doesn't change\". Shorter outputs \
don't beat simpler ones.
- **Write like a human.** Don't use em dashes. Use contractions and keep it casual. Conveying information quickly and clearly is more important than using proper \
grammar or terminology.
- **One question at a time.** You can ask the user for input via the `question` tool. Start with a \
single question and two or three distinct options, mark the one you recommend, and give each \
option a one-line reason. Wait for their answer before asking the next thing.
- **Report finished work in a couple of lines.** What changed, what you checked, and what you left.";

/// Code-quality rules, as numbers a model can check while writing.
const ENGINEERING_STANDARDS: &str = "## Engineering standards
In addition to following good coding habits, the following standards are optimized for how you \
interact with the code. Shorter files and functions are faster and cheaper to `read`. Unique names \
limit `grep` results, and descriptive errors help solve problems faster. So, unless the project's \
own conventions say otherwise:
- Keep functions from 4–20 lines and split them if longer.
- Files should be 500 lines or less. When they grow longer, split them by responsibility.
- Use early returns instead of nested `if`s, with at most 2 levels of indentation.
- Comments should say *why*, never *what*.
- Use specific names and types: no `data`, `handler`, or `Manager`.
- Errors should be detailed and never swallowed.
- luxe features specialized tools to read, write, and debug code. Use them.";

/// What makes a test worth having.
const TESTING: &str = "## Testing
Tests are extremely valuable, but only when used thoughtfully and consistently. New behavior gets \
a test; a bug fix gets a regression test, and every test should actually be able to fail.
- **Prove the fix.** Run the new test against the *unfixed* code and watch it fail first.
- **Name the break a test catches.** Before writing an assertion, say which change should make it \
fail. If you can't, you're just describing code, not testing it.
- **One case per branch.** An input that trips every condition at once proves none of them. Give \
each condition its own case, and cover what must *not* match.";

/// Shipped `todo` guidance.
const PLANS: &str = "The `todo` tool is your plan, and the only insight the user has into what it \
entails or whether it even exists. Always open one before you start any work spanning three or \
more steps. If the user hands you a list of tasks, pass every item at once as `subjects` in a \
single call, then begin working on the list. Mark each item completed the moment it's done, and \
add new items along the way as needed. Creating a plan and maintaining it keeps the user informed \
and the work focused.";

/// How to go about a change, and who owns it once it is made.
const WORKING_DISCIPLINE: &str = "## Working discipline
You should understand a change before you make it. Read the code it touches and trace the flow \
for every place the change has to reach. Then reach for the simplest thing that works: reuse \
what's already in the codebase; use the standard library; use a native platform feature; use an \
existing dependency. Ship the smallest diff that fully solves the task. Verify before you report. \
Finishing a change is not committing it. Leave the work in the working tree unless you were \
explicitly asked for a commit; simply report that the work is done and let the user decide. Some \
commands are the user's to run, such as `sudo`, passwords, etc. To facilitate, you can give them \
the exact command in a fenced code block to run in their own terminal and continue once they say \
it's done.";

#[cfg(test)]
mod tests;
