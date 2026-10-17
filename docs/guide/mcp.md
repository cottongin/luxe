---
title: "MCP servers"
summary: "Give the agent tools from MCP servers: add one, sign in, and control what it may do."
order: 3
group: "Extend"
---

An [MCP](https://modelcontextprotocol.io) server gives the agent new tools: a browser, a database,
your issue tracker. Luxe reaches every server through one `mcp` tool, so a server's tools cost
nothing in a request until the agent looks for them:

```text
mcp({ search: "screenshot" })                                    find tools by keyword or server
mcp({ tool: "chrome/take_screenshot" })                          one tool's parameters
mcp({ tool: "chrome/take_screenshot", args: "{\"format\":\"png\"}" })   call it
```

## Add a server

Add servers to `~/.config/luxe/mcp.json`. The `mcpServers` format other tools write works as it is:

```json title="~/.config/luxe/mcp.json"
{
  "mcpServers": {
    "chrome": { "command": "npx", "args": ["-y", "chrome-devtools-mcp@latest"] },
    "linear": { "url": "https://mcp.linear.app/mcp", "auth": { "kind": "oauth" } }
  }
}
```

A server is a local program (`command` and `args`) or a remote address (`url`). Or from the command
line:

```bash
luxe mcp add --id chrome --name "Chrome" --command "npx -y chrome-devtools-mcp@latest"
luxe mcp list     # configured servers
luxe mcp tools    # every server's tools
luxe mcp remove --id chrome
```

`luxe mcp add` and `luxe mcp remove` change only the project's `.luxe/mcp.json`, so a server added
this way is a [project's server](#servers-a-project-brings), and one in your own `mcp.json` is
removed by editing that file. `luxe mcp list` starts nothing: it shows every configured server,
disabled ones included, and marks a project's entries `not started (project)`, with or without
`--trust-project`. In a session, `/mcp` shows each server, how it connects and its tools.

Luxe also reads `~/.config/mcp/mcp.json`, and a project's `.mcp.json` and `.luxe/mcp.json`. Where
two files name the same server, the project's wins.

### Server settings

| Key | Does |
| --- | --- |
| `command`, `args`, `env` | Start a local server. An `env` value can use `${VAR}` or `$env:VAR`; an unset one is empty, and Luxe logs which. |
| `url`, `headers` | Connect to a remote server. `headers` take `${VAR}` the same way. |
| `auth` | Sign in: see [below](#sign-in) |
| `lifecycle` | `lazy` (the default) connects on first use and disconnects when idle. `eager` connects at start; `keep_alive` stays connected. |
| `timeout_secs` | How long one call may take. 60 by default. A call that runs over stops the server; the next call starts it again. |
| `exclude_tools` | Tool names to hide from the agent |
| `enabled` | `false` turns a server off without removing it |

## Sign in

A remote server signs in with a token or with OAuth.

```json title="~/.config/luxe/mcp.json"
{
  "mcpServers": {
    "internal": {
      "url": "https://mcp.example.com/mcp",
      "auth": { "kind": "bearer", "token_env": "INTERNAL_MCP_TOKEN" }
    },
    "linear": { "url": "https://mcp.linear.app/mcp", "auth": { "kind": "oauth" } }
  }
}
```

For OAuth, sign in once in a browser, and Luxe refreshes the token from then on:

```bash
luxe mcp login --id linear
```

The agent never starts a sign-in. Tokens are kept in `~/.local/state/luxe/mcp-auth.json`, readable
only by you. With `LUXE_STATE_DIR` set they are kept there instead, so a moved state directory does
not share the default one's sign-ins. If that file stops being valid JSON, Luxe moves it to
`mcp-auth.json.bad` (or a dated name, if one is already there) rather than write over it, and you
sign in again.

A token only travels over `https`. A server with `auth` set whose `url` is plain `http` is refused
unless it is on this machine, and so is an OAuth endpoint, whether you wrote it or the server
described it. The server's metadata must also name itself as the issuer it was asked for.

Some providers, such as Asana, register each app and give you a client ID and secret. Add them,
with the redirect address you registered:

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
      }
    }
  }
}
```

The redirect must be on `127.0.0.1`, `localhost` or `[::1]`, with the port you registered. A secret
or token can be written three ways:

- `${VAR}`: an environment variable. Programs the agent runs can read it too. If it is unset,
  sign-in stops and names the variable.
- `!command`: a command that prints it, such as `"!pass show asana/mcp-secret"`. It runs at sign-in
  and on each refresh, without a terminal, for up to 30 seconds. Only your own config can use this.
- The value itself, in a file only you can read (`chmod 600`).

Never put a secret in a project's `.mcp.json` or `.luxe/mcp.json`, which are shared with the
repository. [Asana, end to end](recipes.md#asana) walks through the whole setup.

## Point the agent at a server

In the `@` picker, the **MCP** tab lists each server with its state: `22 tools · signed in`, or
`not signed in — luxe mcp login --id linear`. Mentioning one, as in *@[mcp:asana] what is due this
week?*, sends the agent straight to it, with the names of its tools the first time.

## Control what a server may do

A tool call is checked by your permission rules, by its `server/tool` name:

```json title="~/.config/luxe/permissions.json"
{
  "permission": {
    "mcp": {
      "asana/get_*": "allow",
      "asana/create_*": "ask",
      "asana/delete_*": "deny"
    }
  }
}
```

The most specific rule wins. `deny` holds everywhere, even with `--yolo`. `ask` holds only when
someone is there to answer; in a headless or loop run it is allowed, unless `agent.on_ask` is
`deny`.

What a tool returns goes to your model provider, as the result of that call. Signing in and
`luxe mcp tools` send nothing to a model. If a server's data should only go to a provider your
organization approves, deny its tools until you are on one.

A server's descriptions and results are treated as information, never as instructions.

### Servers a project brings

A project's `.mcp.json` names programs that would run on your machine. Luxe starts a project's
servers in an interactive session, or with `--trust-project`, but not in a headless or loop run. To
decide for every run, set `lsp.project_servers` in your own config; see
[Tools](tools.md#a-server-the-project-installed).

`luxe mcp tools` and `luxe mcp login` do not ask whether to trust a project's servers. They skip
project servers unless you pass `--trust-project` or set `lsp.project_servers` to `always` in your
own config, and print the skipped server ids and how to allow them. Login refuses a skipped
server before contacting its OAuth endpoints. Your global `never` setting still wins over the
flag. Servers in your own config do not need project trust.
