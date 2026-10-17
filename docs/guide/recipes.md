---
title: "Recipes"
summary: "Extensions put together: Asana, subagents, a classifier guard, Herdr, and a local model."
order: 8
group: "Extend"
---

## Asana

Two ways to bring Asana in, and they work together: its MCP server, so the agent can read and update
tasks, and a mention source, so you can point at one task with `@`.

### The MCP server

Asana registers each app, so each person makes one of their own.

1. **Register an MCP app** in Asana's developer console (**My apps**). Set its redirect URL to
   `http://127.0.0.1:8765/callback`, and leave **This is a native or command-line app** unchecked.
   Share it with your workspace. Your organization may need an admin to approve it.
2. **Keep the secret in your environment.** In `~/.zshrc` or `~/.bashrc`, then open a new
   terminal:

   ```bash
   export ASANA_MCP_CLIENT_SECRET='…'
   ```

3. **Add the server**, with your app's client ID, and hide the tools Asana marks as internal:

   ```json title="~/.config/luxe/mcp.json"
   {
     "mcpServers": {
       "asana": {
         "url": "https://mcp.asana.com/v2/mcp",
         "auth": {
           "kind": "oauth",
           "client_id": "1209…",
           "client_secret": "${ASANA_MCP_CLIENT_SECRET}",
           "redirect_uri": "http://127.0.0.1:8765/callback"
         },
         "exclude_tools": [
           "get_project_internal", "search_objects_internal", "log_widget_event",
           "create_task_confirm", "save_task_changes_confirm", "create_project_confirm",
           "create_project_confirm_populate", "save_project_changes_confirm"
         ]
       }
     }
   }
   ```

4. **Sign in** and check what you can use:

   ```bash
   luxe mcp login --id asana
   luxe mcp tools
   ```

5. **Decide what the agent may do.** Task names and comments go to your model provider when a tool
   returns them. To let reads through and ask before anything changes:

   ```json title="~/.config/luxe/permissions.json"
   {
     "permission": {
       "mcp": {
         "asana/create_*": "ask",
         "asana/update_*": "ask",
         "asana/delete_*": "ask",
         "asana/add_*": "ask"
       }
     }
   }
   ```

   If Asana's data may only go to a provider your organization approves, use
   `"asana/*": "deny"` until you are on one. In a headless or loop run, make the writes `deny`:
   nobody is there to answer an `ask`.

6. **Use it:** *@[mcp:asana] what is due this week?*

### Your tasks in the `@` picker

This needs a personal access token (**My apps → Personal access tokens**), since the MCP sign-in's
token works only for the MCP server. Your workspace's ID is in
`https://app.asana.com/api/1.0/workspaces`, read with the same token.

```bash title="~/bin/asana-mentions"
#!/bin/sh
curl -sSf -H "Authorization: Bearer $ASANA_PAT" -G https://app.asana.com/api/1.0/tasks \
  --data-urlencode assignee=me --data-urlencode "workspace=$ASANA_WORKSPACE" \
  --data-urlencode completed_since=now --data-urlencode limit=100 \
  --data-urlencode opt_fields=name,due_on,permalink_url |
jq -c '.data[] | (if .due_on then "due \(.due_on)" else "no due date" end) as $due | {
  id: .gid,
  label: .name,
  detail: $due,
  text: "Asana task \"\(.name)\" (\($due)) \(.permalink_url)"
}'
```

```json title="~/.config/luxe/config.json"
{ "mentions": [{ "name": "Asana", "command": "asana-mentions" }] }
```

A task you pick goes in as `@[asana:1209…]` and reaches the agent as its name, due date and link,
and the agent can look it up through the MCP server. See [Mention sources](mention-sources.md).

## Subagents

A subagent is a separate context that does one job and returns a summary, which is what a headless
Luxe run is:

```bash
luxe --headless --read-only --model openai/gpt-5.6 --format json \
  -p "review src/parser.rs for edge cases" |
  jq -rs 'map(select(.type == "message" and .text != "")) | last.text'
```

`--read-only` keeps the subagent from changing anything, and `--model` lets it be a different model
from the one doing the work. The `jq` filter keeps the last message with text in it, which is the
summary. For a review of your own uncommitted work, [`/review`](agent.md#a-second-opinion) does
this without the wrapper.

To let the agent start one itself, wrap that command in an MCP server with a
`delegate(task, model)` tool. Then:

- Rule it like any tool: `"mcp": { "delegate/*": "ask" }` in `permissions.json` puts each one in
  front of you.
- Give the server a `timeout_secs` longer than a review takes. A call gets 60 seconds by default,
  and one that runs out stops the subagent.
- Return the subagent's stderr when it fails, so the agent sees why.
- Give a subagent that writes its own worktree, with `git worktree add`, so it stays out of yours.

Luxe refuses to nest more than two levels deep, since an agent that starts two subagents per turn
would multiply fast. `LUXE_MAX_DEPTH` raises the limit.

## A guard that asks a model

A `BeforeToolUse` [hook](hooks.md) can ask a small classifier model whether a command is
destructive, and ask you when it might be. This one calls a typed decision model running on your
own machine, and the request follows that model's API; any model that returns a probability works.

```bash title="~/.config/luxe/hooks/risk-gate.sh"
#!/usr/bin/env bash
set -euo pipefail
command=$(jq -r '.ToolUse.args.command')

risk=$(jq -n --arg c "$command" '{
    state: { command: $c },
    questions: { destructive: { type: "noul",
      instructions: "Would running this command cause irreversible data loss?" } }
  }' |
  curl -sS --max-time 2 "${DECISION_URL:-http://localhost:8000}/v1/systemone" \
    -H 'Content-Type: application/json' --data @- |
  jq -r '.answers.destructive.noul')

if awk "BEGIN{exit !($risk > 0.8)}"; then
  jq -n --arg r "scored $risk for irreversible data loss" '{decision:"ask", reason:$r}'
else
  echo '{"decision":"allow"}'
fi
```

```json title="~/.config/luxe/hooks.json"
{
  "BeforeToolUse": [
    {
      "matcher": "bash",
      "command": ["/home/you/.config/luxe/hooks/risk-gate.sh"],
      "timeout_ms": 2500,
      "on_timeout": "ask"
    }
  ]
}
```

- **It asks rather than blocks.** A hook can only make a call harder, so the worst a wrong answer
  costs is a prompt.
- **It adds to your rules, not replaces them.** Keep what must always hold in
  [`permissions.json`](security.md).
- **It runs before every matching call.** The `matcher` keeps it off `read` and `grep`; narrow it
  further, such as `{ "bash": "rm *" }`, to cost less.
- **`on_timeout: "ask"`** puts the question to you if the classifier is down.
- **Nobody answers in a headless run**, so its `ask` is allowed there. Use `--on-ask deny`, or
  `"hook_ask": "deny"` in `permissions.json`, to make it hold.

## Run Luxe in Herdr

[Herdr](https://herdr.dev) keeps coding agents in panes and shows which ones are working, waiting
on you, or done. Luxe reports its state to Herdr on its own when it runs in a Herdr pane:
**working** during a turn, **blocked** when it needs you, and **done** when the turn ends.

```bash
curl -fsSL https://herdr.dev/install.sh | sh
herdr
luxe        # in a pane
```

To let the agent drive Herdr, such as splitting panes or starting other agents, add Herdr's skill:

```bash
mkdir -p ~/.config/luxe/skills/herdr
curl -fsSL https://raw.githubusercontent.com/ogulcancelik/herdr/master/SKILL.md \
  -o ~/.config/luxe/skills/herdr/SKILL.md
```

`LUXE_HERDR=0` or `{ "herdr": false }` in Luxe's config turns the reporting off.

### Luxe colors in Herdr

Herdr has no theme files — it takes a built-in theme plus per-token overrides. This is Luxe's
own palette in those tokens, so the pane and the multiplexer around it read as one program:

```toml title="~/.config/herdr/config.toml"
[theme]
name = "vesper"          # the closest built-in; every token below is overridden anyway

[theme.custom]
accent        = "#ddc084"  # champagne, the one color Luxe leads with
text          = "#e4dfd6"
subtext0      = "#97938c"
overlay0      = "#4d4a45"
overlay1      = "#97938c"
sidebar_bg    = "#12100e"  # the base surface, same as Luxe's background
panel_bg      = "#12100e"  # the tab bar sits on it too, so the chrome is one field
surface0      = "#201e1a"
surface1      = "#2e2b27"
surface_dim   = "#201e1a"
active_row_bg = "#201e1a"  # the current space, at rest
selection_bg  = "#2e2b27"  # the Navigate cursor, which should be the brighter of the two
mauve         = "#d2a5cd"  # branch names
yellow        = "#ddc084"  # working — an agent at work glows in the accent
green         = "#81caa0"  # idle / done
red           = "#e68485"  # blocked, needs you
peach         = "#faaa51"  # interrupted
blue          = "#85c7d7"  # Luxe has one cool hue, so blue and teal share it
teal          = "#85c7d7"

[ui]
accent = "#ddc084"         # borders and navigation chrome, which read off their own key
```

Reload it in place with `prefix+shift+r`, or `herdr server reload-config`. For any of the other
[themes](appearance.md) Luxe ships, the hexes are in its theme file and the token names above do
not change.

The one token worth a second thought is `panel_bg`: Herdr draws the active tab as `panel_bg` text
on an `accent` fill, so it has to stay dark enough to read there. Lifting it to `#201e1a` gives the
tab bar a surface of its own if you prefer the chrome delineated.

## A local model in LM Studio

1. Load a model in LM Studio and start its server.
2. In Luxe, open **Configuration** and choose **Connect a local or custom model**, then
   **LM Studio**. Leave the endpoint blank for the default.
3. Choose the model in the picker that opens.

LM Studio ignores the chat-template settings Luxe sends a local server by default, so for
thinking levels to reach the model, send them as `reasoning_effort`, and start each session at a
sensible level. Connecting already wrote an `lmstudio` provider to `models.json`, so add these keys
to it rather than replacing the file:

```json title="~/.config/luxe/models.json"
{
  "providers": {
    "lmstudio": { "compat": { "thinking_format": "openai" } }
  },
  "models": {
    "lmstudio/qwen3.8-27b": { "reasoning": true, "thinking": "medium" }
  }
}
```

[Custom providers](custom-providers.md) covers samplers and request formats.
