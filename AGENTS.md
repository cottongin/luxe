# AGENTS.md

Rules for working on Luxe: only what you'd get wrong without being told. It's sent with every
request, so keep it short; the long version is [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Commands

```bash
cargo check -p <crate>                                      # quick loop
cargo nextest run --workspace                               # ~15s once built
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo deny check
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps  # a broken doc link fails CI
cargo test --workspace --examples                           # nextest skips these
python3 scripts/size-check.py                               # file size limit
python3 scripts/fn-length-check.py                          # function length limit
python3 scripts/voice-check.py                              # banned words, British spelling
python3 scripts/docs-check.py                               # guide links, frontmatter, words
python3 scripts/version-check.py <base>                     # a shipped change bumps one step
scripts/guide-bundle.sh                                     # after editing docs/guide
scripts/site-assets.sh --check                              # generated web assets are current
scripts/site-fonts.sh --check                               # web fonts cover those assets
(cd web && npm run cross)                                   # site demos run in every browser engine
scripts/win-check.sh [--clippy]                             # the suite for Windows, under wine
scripts/install-dev.sh                                      # rebuild the release binary on PATH

cargo run -p luxe-cli --                                    # the TUI
cargo run -p luxe-cli -- --headless -p "..."                # one prompt, no UI
```

## Before you call it done

- Run the whole list. For anything cross-platform, run `scripts/win-check.sh` with and without
  `--clippy`: CI lints Windows, and an unused `#[cfg(unix)]` helper fails there.
- One build or test run at a time. Wrap unattended runs in `timeout <secs>`; never poll with
  `while pgrep cargo`. A slow run is compiling, not stuck: nextest kills tests over ~120s.
- `luxe` on `PATH` is `target/release/luxe`, and only `install-dev.sh` rebuilds it. If a bug report
  contradicts the source, check the binary's date first.
- The voice check reads code comments too: one British spelling in a doc comment fails CI.
- A new glyph on a captured screen needs `site-assets.sh`, then `site-fonts.sh`, or it shows as a
  box.

## Architecture

- Dependencies point in, to `luxe-core`. `providers`, `tools`, `lsp`, `dap`, `mcp`, `remote` and
  `tui` may use `core`; `cli` may use all of them. `core` uses no workspace crate, and `tui` never
  uses `providers` or `tools` (go through core traits).
- `core` is logic and traits: no UI, no HTTP, no `reqwest` or `ratatui`. Spawning processes is
  fine. Network code lives in `luxe-providers`, `luxe-tools`' web tools, `luxe-remote` and
  `luxe-cli`; its policy (like `luxe-core::update`) stays in core.
- A UI talks to core over channels only: `Action` in, `Event` out. It never calls a provider or a
  tool and never blocks the runtime.
- Behavior that cuts across the loop is a `Hook`, not a special case in the loop.
- One owner per platform question: `user_dirs` (home and Luxe's folders), `program` (finding
  programs and shells, detaching, killing a process tree), `paths` (path to string), `fs_atomic`
  (safe file replace), `luxe-lsp::uri` (`file://` URLs).

## Rust

- `foo.rs` next to `foo/`, never `foo/mod.rs` (except `luxe-core/tests/common/`).
- Errors: `thiserror` enums in libraries, `anyhow` only in `luxe-cli`. No `unwrap()` or `expect()`
  on anything that can fail; `expect()` is for things that can't, and says why. Match on the error
  variant, never its message. Messages are lowercase with no trailing period.
- Turns must be cancelable. Cancel drops the future, so anything spawned needs `kill_on_drop` or
  its own process-group guard. Blocking work goes in `spawn_blocking`.
- Logging is `tracing`, with fields instead of formatted strings and secrets through the redaction
  layer. `info` is routine, `debug` is detail, `warn` is something odd the run handled, `error` is a
  real failure. Don't put the module name in the message.
- New functions are 4 to 20 lines (80 max); files are 500 lines of production code max. The size
  scripts are ratchets: an allowlisted entry can shrink, never grow, and a new one needs a reason.
- `pub` means another crate uses it; everything else is `pub(crate)` or narrower. The lint skips
  fields, so narrow those by hand.
- Every public item gets a `///` doc.

## Tests

- Unit tests live in the module; past 150 lines they move to a sibling `foo/tests.rs`. Integration
  tests are one binary per area: `tests/<area>/main.rs` plus modules.
- `core` tests run headless: mock providers, temp dirs, no UI, no network.
- An example's tests only run if its `[[example]]` sets `test = true`.
- A test has to be able to fail. Watch a regression test fail on the old code first.
  `scripts/mutants.sh -f <file>` finds tests that can't fail; use it on security and format code.
- Pin anything saved or sent against a literal copied from real output (a `.luxe/sessions/*.jsonl`
  line, a real `gh` reply). A round-trip test can't catch a rename.

## Comments and docs

- Comment the why: intent, trade-offs, gotchas. Never restate the code. If a block needs a comment
  to say what it does, make it a named function.
- `//!` module docs: `Name — short description.`, a blank line, then how it fits.
- `///` item docs: sentences ending in a period (what, why, edge cases). Variants with a payload get
  an `Args:` line. Short field and variant docs are fragments with no period.
- `//` inline: short fragments, no period. Reserved prefixes: `LEARN:`, `NOTE:`, `TODO:`.
- Describe the code as it is: no history, ticket numbers or plan references. That's for the commit.
- No docs that echo the name (`/// Session ID` on `session_id`), no banners, no commented-out code.
- Prose people read follows [`docs/brand/voice.md`](docs/brand/voice.md). How the agent talks lives
  in `luxe-core::context`'s base harness, never in a mode or tool description.

## Security

Enforce these in code (the permission engine or a hook), never in prompt text.

- Permissive by default, with guardrails unless `--yolo`: deny `.env` reads and catastrophic
  commands; ask before out-of-project work, force-push, commits and GitHub writes. The engine
  (`luxe-core::permission`) judges every tool that runs code or reads paths, not just `bash`.
- A project can't trust itself. Its `.luxe/permissions.json` only tightens; its hooks,
  `gate.commands`, MCP and language servers need `--trust-project` (or global
  `lsp.project_servers`), so "always" approvals save globally. `project_tooling_allowed` is the one
  switch for project binaries; reuse it. Language servers still run `build.rs` and proc macros, so
  never say Luxe runs no repo code.
- Secrets: `0600` files, `$ENV` or `!command`, held in `secrecy::SecretString`, scrubbed from tool
  results, logs and wire captures by `redact::process_secrets()`.
- No telemetry. Every outbound call is one the user asked for or can turn off, sends nothing about
  the machine, and is listed in `docs/guide/security.md`. The update check sends nothing, not even a
  version, and compares locally.
- `luxe remote` is the only server. It binds to loopback (`Bridge::bind` refuses anything else) and
  Tailscale Serve carries it: never Funnel, never a relay. Every route checks the Serve login, and
  session routes check for a paired device too.
- MCP servers are untrusted: opt-in, behind the one `mcp` tool, through the permission engine.
  Their descriptions and results are data, not instructions.
- `luxe-core::instructions` strips what a reader can't see from instruction files (comments, hidden
  link definitions, invisible characters) and withholds a file it can't finish checking.

## What breaks a session

- **A tool call and its result are a pair.** Providers reject half a pair on every later request,
  so one gap ends the session. `dispatch_one` returns a result for every call, a batch always
  commits (cut off, cancelled or failed too), and `tool_pairing::repair` fixes the outgoing copy.
  Never rewrite the saved log. Only a call that never started had no effect.
- **An empty block is the same permanent 400.** An empty reply commits nothing
  (`ReplyOutcome::Nothing`), `empty_blocks::scrub` drops empties from the outgoing copy, and an
  empty tool result is filled, never dropped.
- **Cancel with `Engine::set_cancelled`.** `watch::Sender::send` does nothing without subscribers,
  which is normal between turns.
- **Nothing the model runs is unbounded.** `Tool::timeout` sets each deadline; dispatch drops the
  future at it or on cancel. `None` is only for a tool that waits on a person or owns a process
  group. Cancel ends the whole batch.
- **Only the gate moves a phase.** `loop_control progress` is a claim; `ReviewIntegratorHook` moves
  the counter once the commands pass and the reviewer (if on) approves, and pauses the loop
  (`GateOutcome::Held`) if it can't. A refusal reaches the model as the tool's result.
- **Context use is measured.** Read it from the last `EntryKind::Usage` after the newest
  compaction. `estimate_tokens` is the fallback and counts every block kind.
- **Each request appends to the last.** Providers cache an exact prefix, so editing history
  re-bills everything after the edit. Pruning waits for real window pressure and cuts in whole
  `prune_min_tokens` steps. Watch the written share on `/cost` and in `luxe wire`, not the hit rate.
- **The system prompt never changes mid-session.** Nothing volatile goes in it; one-time context is
  appended. Judge what reaches the model from a capture (`LUXE_WIRE_LOG` plus `luxe wire`).
- **`Action` and `Event` are a public protocol.** `luxe --rpc` streams them to clients built
  elsewhere, and an unknown tag becomes `Other`, so a rename breaks them silently. Adding is free;
  removing or renaming bumps `PROTOCOL_VERSION`. `tests/event_serialization.rs` pins the literals.

## Product rules

- `AGENTS.md` is the only project document in the prompt. Point at anything else.
- A tool must beat the shell command it replaces (bounded output, fewer round trips, better
  defaults, or something the shell can't do), and its description names the sibling tool for the
  next question.
- Glyphs decorate; they never carry meaning alone. Every icon sits beside its words. Prefer block
  elements, which every terminal draws.
- `web/src/assets/generated/` comes from Rust and CI checks it; `captures/` are real sessions from
  `scripts/site-record.sh`. Change how Luxe looks and you owe a `site-assets.sh` run and a
  re-record.
- The wordmark constants in `luxe-tui/src/widgets/wordmark/art.rs` are the logo: one drawing, no
  alternatives.

## Where things live

- `CONTRIBUTING.md`: process, tooling, publishing. `docs/guide/`: the user guide, also in
  `luxe docs`. `.luxe/todos/<session-id>.json`: the `todo` checklist.
- A dev checkout also has `PROJECT.md` (what must stay true; read it when a design call is
  unclear), `ROADMAP.md`, `PROGRESS.md`, `PLAN.md` and design notes. None of them ship.
