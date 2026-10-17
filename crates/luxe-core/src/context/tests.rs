#![allow(clippy::unwrap_used)]

use super::*;

/// The named `## Section` of an assembled prompt, for assertions that must not match prose
/// elsewhere ("write clean code" is not a git status).
fn section(prompt: &str, heading: &str) -> String {
    let start = prompt.find(heading).unwrap_or_else(|| panic!("{heading} missing:\n{prompt}"));
    let rest = &prompt[start..];
    let end =
        rest[heading.len()..].find("\n## ").map(|at| at + heading.len()).unwrap_or(rest.len());
    rest[..end].to_string()
}

/// A minimal context every test starts from and mutates — one place to touch when the prompt
/// grows a layer.
fn ctx() -> PromptContext {
    PromptContext {
        cwd: PathBuf::from("/tmp/test"),
        os: "linux".to_string(),
        shell: "bash".to_string(),
        git: None,
        agents_md: vec![],
        tools: vec![],
        skills: vec![],
    }
}

/// An agent that can run commands is told where Luxe's own guide is, so a question about Luxe
/// is answered from it; one that cannot run `luxe docs` is not told about it.
#[test]
fn the_prompt_points_at_luxes_own_guide() {
    let mut context = ctx();
    context.tools = vec!["bash".into(), "read".into()];
    let prompt = SystemPromptBuilder::new().build(&context);
    assert!(prompt.contains("`luxe docs`"), "{prompt}");
    context.tools = vec!["read".into()];
    assert!(!SystemPromptBuilder::new().build(&context).contains("luxe docs"));
}

#[test]
fn build_minimal_prompt() {
    let prompt = SystemPromptBuilder::new().build(&ctx());

    // Should contain base harness and environment.
    assert!(prompt.contains("## Role"));
    assert!(prompt.contains("## Honesty"));
    assert!(prompt.contains("## Memory"));
    assert!(prompt.contains("## Environment"));
    assert!(prompt.contains("linux"));
    assert!(prompt.contains("bash"));
    // Always-on minimal-change discipline.
    assert!(prompt.contains("## Working discipline"));
    assert!(prompt.contains("smallest diff that fully solves the task"));
    assert!(prompt.contains("## Plans"));
    assert!(prompt.contains("## Engineering standards"));
    // The standards are numbers a model can check while writing, not adjectives
    assert!(prompt.contains("4–20 lines"));
    assert!(prompt.contains("500 lines or less"));
    assert!(prompt.contains("at most 2 levels of indentation"));
    assert!(prompt.contains("regression test"));
}

#[test]
fn communication_contract_is_always_on_and_concrete() {
    let prompt = SystemPromptBuilder::new().build(&ctx());

    assert!(prompt.contains("## Communication"));
    // The rules that actually reduce reading load, each pinned so it can't quietly regress.
    assert!(prompt.contains("Answer first"), "lead with the answer");
    assert!(prompt.contains("Cut filler"));
    assert!(prompt.contains("can already see tool calls"), "the UI already shows them");
    assert!(prompt.contains("plain, straightforward language"), "no jargon");
    assert!(prompt.contains("Write like a human"));
    assert!(prompt.contains("Don't use em dashes"), "the commonest tell of machine prose");
    assert!(prompt.contains("Report finished work in a couple of lines"));
    assert!(prompt.contains("One question at a time"));
    // Ordering: how to talk comes before how to code.
    let communication = prompt.find("## Communication").unwrap();
    let standards = prompt.find("## Engineering standards").unwrap();
    assert!(communication < standards);
}

/// Every tool the harness names is either always shipped or named only when it is callable.
///
/// The failure this catches is silent and expensive: an instruction to reach for a tool the
/// request does not carry spends the words *and* buys a failed call. It already happened once,
/// to `debug`, when the lazy-tools split deferred a tool the charter had been naming for
/// months, so the guard is fed a prompt built with no tools at all.
#[test]
fn the_harness_names_no_tool_it_cannot_offer() {
    // Hard-wrapped prose puts newlines mid-phrase ("the `todo`\ntool is your plan"), so the
    // search runs against a flattened copy rather than the laid-out one.
    let flat = |prompt: &str| prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let toolless = flat(&SystemPromptBuilder::new().build(&ctx()));
    let mut named: Vec<&str> = HARNESS_TOOLS
        .iter()
        .copied()
        .filter(|tool| toolless.contains(&format!("`{tool}` tool")))
        .collect();
    named.sort_unstable();

    // `question` and `todo` are core: shipped in every request, so naming them
    // unconditionally is honest. Anything else must have dropped its name along with the tool.
    assert_eq!(
        named,
        vec!["question", "todo"],
        "a charter named a tool that is not in the request: {named:?}"
    );

    // ...and when the tool *is* callable, the prompt says so, so nobody pays an extra hop.
    let mut context = ctx();
    context.tools = HARNESS_TOOLS.iter().map(|t| t.to_string()).collect();
    let full = flat(&SystemPromptBuilder::new().build(&context));
    for tool in HARNESS_TOOLS {
        assert!(full.contains(&format!("`{tool}` tool")), "{tool} goes unnamed when callable");
    }
}

/// The honesty contract rides in every prompt.
///
/// It is product, not taste. A model asked for something it cannot determine will reach for a
/// plausible answer instead of admitting the gap, and a fabricated one is worse than a refusal
/// precisely because nothing distinguishes it from a real one — so each clause here is a
/// specific shape that failure takes, and losing any of them silently makes Luxe less
/// trustworthy rather than merely less tidy.
#[test]
fn the_honesty_contract_is_always_on() {
    {
        let prompt = SystemPromptBuilder::new().build(&ctx());
        let honesty = section(&prompt, "## Honesty").to_lowercase();

        assert!(honesty.contains("never present a guess as a fact"), "label the claim");
        assert!(honesty.contains("i don't know"), "not knowing has to be sayable");
        assert!(honesty.contains("invented answer"), "a made-up answer is the worse failure");
        assert!(honesty.contains("gather evidence"), "the tools are where facts come from");
        assert!(honesty.contains("never report work you didn't do"), "no unrun claims");
        assert!(honesty.contains("correct yourself"), "a wrong answer is retracted out loud");

        // Ordering: whether what a reader reads is true outranks how much of it there is.
        let honesty_at = prompt.find("## Honesty").unwrap();
        assert!(honesty_at < prompt.find("## Communication").unwrap());
        assert!(prompt.find("## Role").unwrap() < honesty_at);
    }
}

/// Work reported without being run is the most common way an agent is wrong, so the charter
/// says which of the two it is.
#[test]
fn the_charter_asks_for_verification_before_a_report() {
    let discipline = section(&SystemPromptBuilder::new().build(&ctx()), "## Working discipline");
    assert!(discipline.contains("Verify before you report"), "{discipline}");
}

/// The commit is the reader's move, and the charter has to say so.
///
/// The permission engine is what holds this (`git`/`bash` commit default to `ask`); the line
/// here is what keeps the prompt from firing at the end of every turn.
#[test]
fn the_charter_leaves_the_commit_to_the_person_reviewing() {
    let discipline = section(&SystemPromptBuilder::new().build(&ctx()), "## Working discipline");

    assert!(discipline.contains("not committing it"), "{discipline}");
    assert!(discipline.contains("explicitly asked for a commit"), "the exception: {discipline}");
    assert!(discipline.contains("let the user decide"), "{discipline}");
}

/// Root and terminal prompts are the user's to run, and the charter says how to hand them
/// over, so the engine's refusal of `sudo` is rarely the first the model hears of it.
#[test]
fn the_charter_hands_root_and_prompts_to_the_user() {
    let discipline = section(&SystemPromptBuilder::new().build(&ctx()), "## Working discipline");
    assert!(discipline.contains("the user's to run"), "{discipline}");
    assert!(discipline.contains("`sudo`"), "{discipline}");
    assert!(discipline.contains("fenced code block"), "and the handoff: {discipline}");
}

#[test]
fn skills_section_appears_only_when_present_with_load_hint() {
    let builder = SystemPromptBuilder::new();
    let mut context = ctx();
    assert!(!builder.build(&context).contains("## Skills"), "no section when empty");

    context.skills = vec!["- **Tidy** — keep diffs small".to_string()];
    let prompt = builder.build(&context);
    assert!(prompt.contains("## Skills"));
    assert!(prompt.contains("**Tidy**"));
    assert!(prompt.contains("\"action\": \"load\""), "tells the model how to load: {prompt}");
}

#[test]
fn build_with_git_status() {
    let mut context = ctx();
    context.os = "macos".to_string();
    context.shell = "zsh".to_string();
    context.git = Some(GitStatus { branch: "main".to_string() });

    let environment = section(&SystemPromptBuilder::new().build(&context), "## Environment");
    assert!(environment.contains("**git**: main"));
    // The dirty flag flips on the agent's first edit and would re-bill the cached prefix.
    assert!(!environment.contains("dirty"), "{environment}");
    assert!(!environment.contains("clean"), "{environment}");
}

#[test]
fn build_with_tools() {
    let mut context = ctx();
    context.tools = ["bash", "read", "grep", "write"].map(String::from).to_vec();

    let prompt = SystemPromptBuilder::new().build(&context);

    assert!(prompt.contains("## Tools"));
    // Guidance, not an index: the request's tool schemas already name every tool.
    assert!(prompt.contains("`read` over cat and sed -n"));
    assert!(prompt.contains("`grep` over rg"));
    assert!(!prompt.contains("`glob`"), "an unregistered tool is never advertised");
    assert!(prompt.contains("`write` over heredocs"), "the habit that skips format-on-save");
    assert!(prompt.contains("never the shell"), "{prompt}");
    assert!(!prompt.contains("`edit` over"), "edit isn't registered here");
    assert!(!prompt.contains("**write**"), "tools are not re-listed: {prompt}");
}

#[test]
fn tool_guidance_is_omitted_when_it_would_say_nothing() {
    let builder = SystemPromptBuilder::new();
    let mut context = ctx();
    assert!(!builder.build(&context).contains("## Tools"), "no tools at all");

    context.tools = vec!["read".to_string(), "grep".to_string()];
    assert!(!builder.build(&context).contains("## Tools"), "nothing to prefer over: no bash");

    context.tools = vec!["bash".to_string()];
    assert!(!builder.build(&context).contains("## Tools"), "bash alone has no alternative");

    context.tools = vec!["bash".to_string(), "read".to_string()];
    let prompt = builder.build(&context);
    assert!(!prompt.contains("never the shell"), "no tool to change files with: {prompt}");
}

#[test]
fn build_with_agents_md() {
    let mut context = ctx();
    context.agents_md = vec!["# Custom Instructions\n\nDo X, Y, Z.".to_string()];

    let prompt = SystemPromptBuilder::new().build(&context);

    assert!(prompt.contains("## Agent Instructions"));
    assert!(prompt.contains("Custom Instructions"));
}

/// `AGENTS.md` is the only project document that rides in every request.
///
/// A second always-on file was a standing invitation to park a plan in the prompt: the one in
/// this repo reached 43,000 characters, most of it a phase index that is a distractor on every
/// task that is not planning. No other harness ships one, and the agent can read a spec when
/// the work calls for it.
#[test]
fn no_second_project_document_rides_in_the_prompt() {
    let prompt = SystemPromptBuilder::new().build(&ctx());
    assert!(!prompt.contains("Project Spine"), "{prompt}");
}

/// The prompt is the prefix every provider caches, so nothing that moves with the work may
/// enter it. A todo pointer used to, and a live counter in one would have re-written the whole
/// prefix on every `todo` call; the list itself is one call away and the resume briefing
/// carries it verbatim when it matters.
#[test]
fn no_live_work_state_reaches_the_cached_prefix() {
    let prompt = SystemPromptBuilder::new().build(&ctx());

    assert!(!prompt.contains("## Current Task"), "{prompt}");
    for heading in ["## Memory", "## Plans"] {
        let text = section(&prompt, heading);
        let counted = text.split_whitespace().collect::<Vec<_>>().windows(3).any(|words| {
            words[1] == "of"
                && words[0].chars().all(|c| c.is_ascii_digit())
                && words[2].chars().all(|c| c.is_ascii_digit())
        });
        assert!(!counted, "no todo counter in the cached prefix: {text}");
    }
}

/// The testing contract rides in every prompt.
///
/// It is product, not taste: "write a test" is an instruction a model already follows, and
/// following only that is what produced the hollow suites this section exists to prevent. Each
/// clause below is one of those failure shapes, so losing any of them silently is a
/// regression in what Luxe writes rather than in how it reads.
#[test]
fn the_testing_contract_is_always_on() {
    {
        let prompt = SystemPromptBuilder::new().build(&ctx()).to_lowercase();

        assert!(prompt.contains("## testing"), "no testing section");
        assert!(prompt.contains("able to fail"), "the point of a test is that it can fail");
        assert!(prompt.contains("name the break a test catches"), "an assertion needs a target");
        assert!(prompt.contains("unfixed"), "a regression test is run against the bug first");
        assert!(prompt.contains("one case per branch"), "one input must not stand for all");
    }
}

/// The measured build is the shipped build, byte for byte.
///
/// `/context` reads its rows from `build_measured`, and the provider caches the prefix
/// `build` produces. If they ever diverge, the report describes a prompt nobody sent and the
/// divergence shows up as a bigger invoice rather than as a failure.
#[test]
fn the_measured_build_reassembles_the_shipped_prompt_exactly() {
    let mut context = ctx();
    context.agents_md = vec!["AGENTS content".to_string()];
    context.tools = vec!["bash".to_string(), "read".to_string()];
    context.skills = vec!["skill1".to_string()];
    let builder = SystemPromptBuilder::new();

    assert_eq!(join_sections(&builder.build_measured(&context)), builder.build(&context));
    // And with every optional layer absent, which is the other end of the assembly.
    let bare = ctx();
    assert_eq!(join_sections(&builder.build_measured(&bare)), builder.build(&bare));
}

/// Sections are grouped by the lever that changes them, not by heading.
#[test]
fn measured_sections_carry_the_layer_a_reader_can_act_on() {
    let mut context = ctx();
    context.agents_md = vec!["AGENTS content".to_string()];
    context.tools = vec!["bash".to_string(), "read".to_string()];
    context.skills = vec!["skill1".to_string()];
    let sections = SystemPromptBuilder::new().build_measured(&context);

    let layers: Vec<PromptSection> = sections.iter().map(|entry| entry.section).collect();
    assert!(layers.contains(&PromptSection::Instructions));
    assert!(layers.contains(&PromptSection::Skills));
    assert!(
        layers.iter().filter(|layer| **layer == PromptSection::Harness).count() >= 2,
        "role/environment/tool guidance all bill to the harness: {layers:?}"
    );
    assert!(
        sections.iter().all(|entry| !entry.text.is_empty()),
        "an empty section is not a section"
    );
}

#[test]
fn build_section_order() {
    let mut context = ctx();
    context.agents_md = vec!["AGENTS content".to_string()];
    context.tools = vec!["bash".to_string(), "read".to_string()];
    context.skills = vec!["skill1".to_string()];

    let prompt = SystemPromptBuilder::new().build(&context);

    // What every session shares, then what is about this one: Role → Honesty → Communication →
    // Engineering standards → Memory → Tools → Agent Instructions → Skills → Environment.
    let role_pos = prompt.find("## Role").unwrap();
    let honesty_pos = prompt.find("## Honesty").unwrap();
    let communication_pos = prompt.find("## Communication").unwrap();
    let standards_pos = prompt.find("## Engineering standards").unwrap();
    let memory_pos = prompt.find("## Memory").unwrap();
    let env_pos = prompt.find("## Environment").unwrap();
    let tools_pos = prompt.find("## Tools").unwrap();
    let agents_pos = prompt.find("## Agent Instructions").unwrap();
    let skills_pos = prompt.find("## Skills").unwrap();

    assert!(role_pos < honesty_pos);
    assert!(honesty_pos < communication_pos);
    assert!(communication_pos < standards_pos);
    assert!(standards_pos < memory_pos);
    assert!(memory_pos < tools_pos);
    assert!(tools_pos < agents_pos);
    assert!(agents_pos < skills_pos);
    assert!(skills_pos < env_pos);

    // And the shared run ends where the project's part begins
    let sections = SystemPromptBuilder::new().build_measured(&context);
    assert_eq!(&prompt[shared_len(&sections)..][..22], "## Agent Instructions\n");
    assert!(prompt[..shared_len(&sections)].ends_with("\n\n"));
}

/// Nothing about this directory or session sits in the shared part, so a session in another
/// worktree gets the same bytes there and reads them from cache.
#[test]
fn two_directories_share_everything_before_the_project() {
    let mut here = ctx();
    here.tools = vec!["bash".to_string()];
    let mut there = here.clone();
    there.cwd = "/somewhere/else".into();
    there.git = None;
    let builder = SystemPromptBuilder::new();
    let (a, b) = (builder.build_measured(&here), builder.build_measured(&there));
    let (a_text, b_text) = (join_sections(&a), join_sections(&b));
    assert_ne!(a_text, b_text);
    assert_eq!(shared_len(&a), shared_len(&b));
    assert_eq!(a_text[..shared_len(&a)], b_text[..shared_len(&b)]);
    assert!(shared_len(&a) > a_text.len() / 2, "the harness is most of it");
}
