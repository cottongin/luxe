---
title: "Mention sources"
summary: "Add your own tab to the @ picker: Jira issues, Asana tasks, anything a program can list."
order: 4
group: "Extend"
---

A mention source adds a tab to the [`@` picker](mentions.md), filled by a program of yours. Pick a
row, and the agent receives the text your program gave for it.

## Add a source

```json title="~/.config/luxe/config.json"
{
  "mentions": [
    { "name": "Jira", "command": "jira-mentions", "icon": "\udb80\udf03" },
    { "name": "Asana", "kind": "task", "command": "asana-mentions", "args": ["--mine"] }
  ]
}
```

| Key | Does |
| --- | --- |
| `name` | The tab's name |
| `command`, `args` | The program to run, found on `PATH` or given as a full path, and its arguments |
| `kind` | The mention's prefix, as in `@[task:1209…]`. By default the name in lowercase, with anything but letters and digits as `-`: `Jira Cloud` is `jira-cloud`. |
| `icon` | One Nerd Font glyph for the tab, as the character or a JSON escape. Without one, the tab shows a tag; without a Nerd Font, a plain mark. |

Sources are read from your own config only, since each one runs a program. Their tabs come after
luxe's own: Files, Skills, GitHub and MCP, whose kinds `file`, `skill`, `github` and `mcp` are
taken. Two sources with the same kind get one tab, the first's.

## What the program prints

luxe runs the program, in the project, the first time its tab opens, and lists what it prints: one
JSON object per line. The rows are kept until luxe exits, so a new ticket, or a change to your
script, shows up the next time you start luxe. The program has no terminal and no stdin, so it
cannot prompt for a sign-in.

```json
{"id": "PROJ-12", "label": "PROJ-12 Login fails on Safari", "detail": "In progress", "text": "Jira issue PROJ-12 \"Login fails on Safari\" (In progress) https://acme.atlassian.net/browse/PROJ-12"}
```

| Field | Is |
| --- | --- |
| `id` | Required, a string or a number, without `]`. Goes in the mention: `@[jira:PROJ-12]`. |
| `label` | Required. The row in the picker, up to 200 characters. |
| `detail` | Shown beside the label, up to 200 characters |
| `text` | What the agent receives, up to 600 characters. The label when missing. |

A program has 15 seconds and up to 500 rows. A line that is not JSON, is over 8 KB, or lacks an
`id` or `label` is skipped without a word, so a tab that stays empty usually means the output is
wrong. If the program fails, the tab shows the last line it wrote to stderr, or its exit status,
and it runs again the next time the tab opens.

Make `text` enough for the agent to find the rest: a title and a link, or an ID it can look up with
a CLI or an MCP server. A mention you type by hand, or one not picked since luxe started, reaches
the agent as `Jira PROJ-12` alone. What a tracker says is other people's words, and it reaches your model
provider once you mention it.

## Example: your open Jira issues

```bash title="~/bin/jira-mentions"
#!/bin/sh
site=https://acme.atlassian.net
curl -sSf -u "$JIRA_USER:$JIRA_TOKEN" -G "$site/rest/api/3/search/jql" \
  --data-urlencode 'jql=assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC' \
  --data-urlencode 'fields=summary,status' |
jq -c --arg site "$site" '.issues[] | {
  id: .key,
  label: "\(.key) \(.fields.summary)",
  detail: .fields.status.name,
  text: "Jira issue \(.key) \"\(.fields.summary)\" (\(.fields.status.name)) \($site)/browse/\(.key)"
}'
```

Make it executable (`chmod +x`), put it on your `PATH`, and add the Jira entry above. For Asana,
see [Recipes](recipes.md#asana). Or ask luxe to write one for the tracker you use.
