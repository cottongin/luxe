# Security

luxe runs a model's commands on your machine, so a hole in it matters. If you find one, please tell
me privately before anyone else.

## Reporting

[Open a private advisory](https://github.com/evanwhittaker/luxe/security/advisories/new) on GitHub,
or email [contact@luxe.dev](mailto:contact@luxe.dev). Say what you did, what happened, and the
luxe version (`luxe --version`). A transcript or a minimal repository that shows it helps most.

I'll answer within a few days, keep you posted while it's fixed, and credit you in the advisory and
the release notes unless you'd rather not be named. Please give the fix time to ship before you
publish.

## What counts

Anything that gets past what [Security and permissions](https://luxe.dev/docs/security/) promises:

- a tool call that runs, reads or writes without the prompt or the rule that should have stopped it
- a way for a project, an instruction file or an MCP server to grant itself trust you didn't give
- a secret reaching a tool result, a log, the wire capture or the model when it should be scrubbed
- a network request [the guide doesn't list](https://luxe.dev/docs/security/#what-luxe-sends-over-the-network)
- a release artifact that isn't what this repository built

What the model does with permission you gave it, including anything under `--yolo`, is working as
designed. So is a prompt injection that only gets the agent to ask you for something you then
approve; one that gets around the asking is a real bug.

## Supported versions

Fixes go into the newest release. `luxe update` gets you there.
