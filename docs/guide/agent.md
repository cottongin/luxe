---
title: "How the agent works"
summary: "How the agent writes to you, asks you questions, plans, and gets a second opinion."
order: 11
group: "Using luxe"
---

## How it writes

The agent is told to keep its replies short and direct, in every mode:

- The answer first, then the reason, then any caveats
- No filler, and no narrating tool calls you can already see
- Plain words over jargon, casual and quick to read
- One question at a time, with a recommended option
- Finished work in a couple of lines: what changed, what was checked, what is left

It is also told to be honest about what it knows. A guess is labeled as a guess, and "I don't know"
is a complete answer. It never says something passed that it did not run, and corrects a wrong
answer as soon as it notices.

When it changes code, it reads what the change touches first, then reaches for the simplest thing
that works: what is already in the codebase, then the standard library, then a platform feature,
then a dependency the project already has. It leaves the work uncommitted unless you ask for a
commit. A command that is yours to run, such as `sudo`, it gives you in a code block to run in your
own terminal.

None of this is a setting. To change how it talks to you, say so ("go more technical with me"), or
put it in your project's [`AGENTS.md`](project-context.md) to make it stick.

## Questions

When the agent needs a decision, it asks in a picker. Each option has a one-line reason, the one it
recommends is marked and selected, so agreeing is one keypress. The last option is always
**Type your own answer**: start typing anywhere and it's picked, so `enter` sends what you wrote.

![A question from the agent, with its recommended answer selected](images/agent-question.svg)

Number keys pick an option, and in a question with one answer they send it. Your answer stays in
the conversation as a card, like the one above the picker. In runs with nobody to answer, such as
`--headless`, the agent takes the default it named, or else its recommended option.

## Plan, then build

A session is in one of two modes, named in the composer's title and in the dock's **MODEL**
section:

| Mode | Does |
| --- | --- |
| **build** | Changes files and runs commands. The default. |
| **plan** | Reads, searches and asks, and writes only `PLAN.md` |

Switch with `shift+tab`, by clicking either, or with `/mode plan` and `/mode build`.
`luxe --plan` starts in plan mode. In plan mode the composer, and the questions the agent asks
there, take a second color.

Plan mode is held in code, not asked for. An edit to any other file, a shell command or a test run
is refused before it runs, and the agent is told why. Switching changes neither the system prompt
nor the tools, so it does not re-read the conversation at full price.

In plan mode the agent asks its open questions one at a time, then writes the plan as one
`## Phase N` section per phase, numbered from 0, which is the shape [`luxe loop`](loop.md) works
through. When a turn writes the plan, luxe asks what next:

- **Build it here**, with the planning conversation still in context
- **Build it in a fresh session**, starting from `PLAN.md` alone
- **Run it as a loop**: switch to build and say how to start [`luxe loop`](loop.md), which works
  phase by phase with your checks and a review at each
- **Keep planning**

To plan into other files, or on a stronger model:

```json title="~/.config/luxe/config.json"
{
  "plan": {
    "files": ["PLAN.md", "docs/design/*.md"],
    "model": "anthropic/claude-opus-5",
    "thinking": "high"
  }
}
```

The first plain file in `files` is where the plan goes. A model of its own costs something:
each model keeps its own prompt cache, so entering plan mode on another model reads the whole
conversation again at full price. `--model` or `LUXE_MODEL` wins over `plan.model`, and
`--thinking` over `plan.thinking`.

To start every session in plan mode:

```json title="~/.config/luxe/config.json"
{ "agent": { "default_mode": "plan" } }
```

## A second opinion

`/review` has a reviewer read the uncommitted changes, new files included, in a fresh context: a separate session that
never sees this conversation and changes nothing. Add what to look at hardest:
`/review error handling`. What it would fix goes to the agent as its next turn, and the agent may
push back on a finding it thinks is wrong. A clean review is only shown.

The reviewer runs on your session's model unless you name another. A fresh context is most of what
makes a review useful; another vendor's model adds a second set of blind spots, for a second bill.

```json title="~/.config/luxe/config.json"
{ "reviewer": { "model": "openai/gpt-5.6", "thinking": "high" } }
```

The same reviewer is the second check in [`luxe loop`](loop.md#check-each-phase).

## Which model a session starts on

In order, the first that is set:

1. `--model` and `--thinking` on the command line
2. `LUXE_MODEL` and `LUXE_PROVIDER` in the environment
3. The model you last chose in this project
4. The model you last chose anywhere
5. `default_model` in `models.json`

luxe never picks a model for you, and signing in doesn't either: the model picker opens on the
provider you signed in to, at the top of its models, for you to choose. With none of these set,
sending a message opens the model picker, and `--headless` and the loop refuse to start. To always
start from your configured default rather than your last choice:

```json title="~/.config/luxe/config.json"
{ "agent": { "remember_model": false } }
```

## Turn limits

A turn runs until the agent stops calling tools or you stop it with `ctrl+esc`. To cap the rounds
of tool calls in one turn, where calls the model makes together count as one round:

```json title="~/.config/luxe/config.json"
{ "agent": { "max_tool_iterations": 200 } }
```

At the cap, the agent gets one last reply with no tools, to summarize what it did and what is left.
`LUXE_MAX_TOOL_ITERATIONS` sets the cap for one run, and `0` lifts it.
