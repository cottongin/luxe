---
title: "Hooks"
summary: "Run your own commands at points in a turn: block, rewrite or audit tool calls, and add context."
order: 5
group: "Extend"
---

A hook runs a command of yours at a point in a turn, such as before each tool call. It receives
the event as JSON on stdin and can answer with a decision as JSON on stdout. Any executable works:
a shell script, Python, a compiled program.

## Add a hook

Hooks go in `~/.config/luxe/hooks.json`. This one asks before any recursive delete and refuses a
wipe of `/` or your home folder:

```json title="~/.config/luxe/hooks.json"
{
  "BeforeToolUse": [
    {
      "matcher": { "bash": "rm *" },
      "command": ["/home/you/.config/luxe/hooks/guard.sh"],
      "on_timeout": "deny"
    }
  ]
}
```

```bash title="~/.config/luxe/hooks/guard.sh"
#!/usr/bin/env bash
cmd=$(jq -r '.ToolUse.args.command // empty')
case "$cmd" in
  *"rm -rf /"*|*"rm -rf ~"*) echo '{"decision":"deny","reason":"refusing a root or home wipe"}' ;;
  *)                         echo '{"decision":"ask","reason":"recursive delete"}' ;;
esac
```

A project's `.luxe/hooks.json` runs only when you start Luxe with `--trust-project`, since opening a
repository should never run its commands. When both are loaded, a project's hooks run after yours.

### Hook settings

| Key | Default | Does |
| --- | --- | --- |
| `command` | | The program and its arguments, run without a shell. Use `["sh", "-c", "…"]` for shell features. |
| `matcher` | every call | Which tool calls it runs for; see [Matchers](#matchers) |
| `timeout_ms` | `5000` | One deadline for sending the event, reading output and waiting for exit. On timeout, it and everything it started are stopped. |
| `on_timeout` | `allow` | What happens if the hook times out, fails to start, exits non-zero or prints something that is not a decision: `allow`, `deny` or `ask` |

`on_timeout: "deny"` makes a guard fail closed. `"ask"` puts the question to you instead, at
`BeforeToolUse`; at other points it acts as `allow`. A
`decision` Luxe does not recognize, a typo or different casing included, gets the same treatment,
with a warning in the log, and its `args` and `context` are dropped.

## Matchers

A `BeforeToolUse` hook without a matcher runs on every tool call, and each run starts a process.
A matcher skips the calls you do not care about, without starting anything:

```text
"matcher": "bash"                                   a tool name, or a pattern like "web_*"
"matcher": { "bash": "rm *", "edit": "src/**" }     a tool and what the call is about
```

The second form matches if any entry does. The patterns are the same as in
[`permissions.json`](security.md): `"rm *"` finds the `rm` in `cd /tmp && rm -rf x`, and a
pattern for `read` or `edit` matches the path, for `web_fetch` the address.

Only `BeforeToolUse` sees a call's arguments. At `AfterToolUse` only the tool name is matched, and
points without a tool call ignore the matcher.

## Hook points

| Point | Runs | stdin | Can |
| --- | --- | --- | --- |
| `SessionStart` | A session starts or resumes, including a switch to it from another session | `{"Other": null}` | |
| `SessionEnd` | A session ends, including a switch to another session; Luxe waits for the hook | `{"Other": null}` | |
| `UserPromptSubmit` | You send a message, including one sent while the agent works | `{"Other": {"content": "…"}}` | block it, add context |
| `BeforeAgentStart` | A turn is about to start | `{"Other": null}` | |
| `AgentStart` | Luxe is about to call the model | `{"Other": null}` | |
| `BeforeToolUse` | Before each tool call | `{"ToolUse": {"tool_name": "bash", "args": {…}}}` | block it, ask you, rewrite it |
| `AfterToolUse` | After each tool call | `{"ToolResult": {"tool_name": "bash", "result": "…", "is_error": false}}` | add context to the result |
| `FileChanged` | `write`, `edit`, an applied rename or code action, or an `ast_grep` rewrite changed a file | `{"FileChanged": {"path": "/abs/file", "operation": "edit"}}` | |
| `BeforeCompact`, `AfterCompact` | Around compaction | `{"Other": {"tokens_before": 12345}}` | |
| `AgentEnd` | A turn finished | `{"Other": {"effective_context_percent": 12.5}}` | |
| `Error` | A turn failed | `{"Other": {"message": "…"}}` | |
| `PermissionRequested`, `PermissionResolved` | An approval prompt opens or is answered | `{"Other": {"tool": "bash", "phase": "requested", …}}` | |
| `ReviewGate`, `LoopControl` | `luxe loop` only | | |

A hook at a point with nothing under **Can** still runs, for logging or notifications, but its
answer is ignored. Point names are matched loosely (`before_tool_use` works). An unknown name skips
that hook alone, with a warning naming the nearest real point.

## Answer with a decision

```json
{ "decision": "deny", "reason": "rm -rf is blocked by policy" }
```

| Field | Where | Does |
| --- | --- | --- |
| `decision` | Required in any answer | `deny` blocks the call or message. `ask` raises a permission prompt, at `BeforeToolUse`. `allow`, or no output at all, means no opinion. An answer without `decision` counts as one Luxe cannot read. |
| `reason` | with `deny` or `ask` | Shown to the model when a call is blocked, to you when a message is blocked, and on the prompt |
| `args` | `BeforeToolUse` | New values for the call's arguments. Keys you leave out keep their values. |
| `context` | `AfterToolUse`, `UserPromptSubmit` | Text added to the tool result, or saved with your message, for the model to see |

```json
{ "decision": "allow", "args": { "command": "bun test" } }
```

When several hooks answer the same point, their context and rewrites are combined in order. A
`deny` ends it: hooks after it do not run.

### What a hook cannot do

- **Approve a call.** `allow` means no opinion; a hook can only make a call harder. To auto-approve,
  write a rule in [`permissions.json`](security.md).
- **Rewrite its way past the rules.** A rewritten call is checked by the permission rules as
  rewritten, and that is what runs. If a rewritten call matches an `ask` rule, you are asked about
  the new command.

### When nobody is there to answer

In a headless or loop run, a hook's `ask` is allowed, like every other `ask`. To change that:

- `--on-ask deny`, or `"agent": { "on_ask": "deny" }` in `config.json`, refuses every `ask` in an
  unattended run, and tells the model why.
- `{ "permission": { "hook_ask": "deny" } }` in `permissions.json` turns every hook's `ask` into a
  block everywhere, even when you are there. `"allow"` silences them instead.

A hook's prompt offers **Allow once** and **Reject**, since a hook decides each call afresh.

## What a hook sees

The event a hook receives is not redacted: it is a program you installed, with the same access you
have, and the call's arguments have already gone to your model provider. What a hook returns, its
`reason` and `context`, is scrubbed of anything that looks like a secret before it is shown or
saved. Read a hook script before installing it, as you would anything on your `PATH`.

## Examples

**Add lint output to every edit:**

```json title="~/.config/luxe/hooks.json"
{
  "AfterToolUse": [
    {
      "matcher": "edit",
      "command": ["sh", "-c", "jq -n --arg out \"$(npm run -s lint 2>&1 | tail -20)\" '{decision: \"allow\", context: $out}'"]
    }
  ]
}
```

Luxe already adds language server errors after every edit; a hook like this is for a different
check.

**Refuse messages that contain an internal token:**

```bash title="~/.config/luxe/hooks/prompt-policy.sh"
#!/usr/bin/env bash
prompt=$(jq -r '.Other.content // empty')
if printf '%s' "$prompt" | grep -qE 'CORP-SECRET-[0-9]+'; then
  echo '{"decision":"deny","reason":"the message contained an internal token"}'
fi
```

**Copy the sessions before compaction:**

```json title="~/.config/luxe/hooks.json"
{ "BeforeCompact": [{ "command": ["sh", "-c", "cp -r .luxe/sessions /tmp/luxe-precompact-$$"] }] }
```

[Recipes](recipes.md#a-guard-that-asks-a-model) has a hook that asks a classifier model whether a
command is destructive.

## Coming from Claude Code

| Claude Code | Luxe |
| --- | --- |
| `PreToolUse`, `PostToolUse` | `BeforeToolUse`, `AfterToolUse` |
| `UserPromptSubmit` with `additionalContext` | `UserPromptSubmit` with `context` |
| `SessionStart`, `SessionEnd` | the same |
| `PreCompact`, `PostCompact` | `BeforeCompact`, `AfterCompact` |
| `Stop` | `AgentEnd`, which observes only |
| `permissionDecision: "ask"` | `"decision": "ask"` |
| `updatedInput` | `args`, checked again by the permission rules |
| `matcher` and `if` | `matcher`, in either form |
| `permissionDecision: "allow"`, which skips the prompt | No equivalent: use `permissions.json` |
