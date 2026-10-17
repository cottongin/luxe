# AGENTS.md — Luxe

Rules for working in this repository. Each line is something a capable engineer would get wrong
without being told; anything the code already shows is left out, because this file rides in every
request. Process, tooling and the long explanations are in [`CONTRIBUTING.md`](CONTRIBUTING.md).

## Commands

```bash
cargo check -p <crate>                                      # the inner loop
cargo nextest run --workspace                               # ~15s once compiled
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo deny check
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps  # a broken doc link fails CI
cargo test --workspace --examples                           # nextest does NOT run these
python3 scripts/size-check.py                               # file-size ratchet (--report lists)
python3 scripts/fn-length-check.py                          # function-length ratchet
python3 scripts/voice-check.py                              # reads source comments too
python3 scripts/docs-check.py                               # the guide's links, frontmatter, voice
scripts/guide-bundle.sh                                     # after editing docs/guide
scripts/site-assets.sh --check                              # generated web assets are current
scripts/site-fonts.sh --check                               # webfonts cover those assets
(cd web && npm run cross)                                   # the demos boot in every engine
scripts/win-check.sh [--clippy]                             # the suite for Windows, under wine
scripts/install-dev.sh                                      # rebuild the release binary on PATH

cargo run -p luxe-cli --                                    # the TUI
cargo run -p luxe-cli -- --headless -p "..."                # one prompt, no UI
```

Run the whole gate before calling work done, and `scripts/win-check.sh` in **both modes** before
calling anything cross-platform done: CI also lints the Windows target, and a `#[cfg(unix)]` helper
left unused there is a `-D warnings` error the default run cannot see. The voice check reads
**source comments**, so one British spelling in a doc comment fails the build (it checks itself;
do not quote the words it bans). A new glyph that reaches a captured surface needs
`scripts/site-fonts.sh` after `site-assets.sh`, or it ships as tofu.

- **Never run two builds or test runs at once.** They thrash and both take longer. Wrap unattended
  runs in `timeout <secs>`; never poll with `while pgrep cargo`.
- **A slow run is compiling, not hanging.** `.config/nextest.toml` kills any test over ~120s.
- **`luxe` on `PATH` is `target/release/luxe`, and nothing above builds release.** When a reported
  defect contradicts the source, check the binary's date against the fix before re-debugging.

## Architecture boundaries

- **Dependencies point inward to `luxe-core`.** Allowed: `providers`, `tools`, `lsp`, `dap`, `mcp`
  and `tui` → `core`; `cli` → all of them. Forbidden: `core` → anything in-workspace; `tui` →
  `providers`/`tools` (go through core traits).
- **`core` is pure logic and traits: no UI, no HTTP.** No `reqwest`, no `ratatui`. Spawning a
  process is fine (git, hooks, formatters, the MCP stdio transport); network I/O lives in
  `luxe-providers`, `luxe-tools`' web tools and `luxe-cli`, with any policy (`luxe-core::update`)
  kept in core.
- **UI talks to core only over channels.** `Action` in, `Event` out. A UI never calls a provider or
  a tool directly and never blocks the runtime.
- **Cross-cutting behavior is a `Hook`**, not a special case threaded through the loop.

## Rust conventions

- **Modules:** `foo.rs` beside `foo/`, never `foo/mod.rs`, except `luxe-core/tests/common/`, since
  each top-level file under `tests/` is its own binary.
- **Errors:** `thiserror` enums in libraries, `anyhow` only in `luxe-cli`. No `unwrap()`/`expect()`
  on a fallible path; `expect()` is for real invariants and says why. **Branch on the variant,
  never on the message**: a `to_string()` match turns rewording an error into a silent behavior
  change. Messages are lowercase with no trailing period.
- **Async:** `tokio`; `spawn_blocking` for blocking work. **Turns must be abortable**: cancellation
  drops the future, so anything spawned needs `kill_on_drop` or its own process-group guard.
- **Logging:** `tracing`, structured fields over interpolation, secrets through the redaction
  layer. `info` is routine behavior worth reading, `debug` detail you go looking for, `warn` an
  oddity the run coped with, `error` a real failure. Don't name the module; `target` does.
- **Size:** at most 500 lines of production code per file (split by responsibility) and 80 lines
  per function (4 to 20 for new code). A wide `match` with thin arms is fine; a fat arm is not.
  `scripts/size-check.py` and `scripts/fn-length-check.py` are ratchets: an allowlisted entry may
  shrink, never grow, and a new one needs a reason a reviewer accepts.
- **Visibility:** `pub` means used from another crate; everything else is `pub(crate)` or
  narrower. `unreachable_pub` checks items but not fields, so narrow fields by hand.
- **Tests:** unit tests in-module; an inline test module past 150 lines moves to a sibling
  `foo/tests.rs`. Integration tests group into one binary per area (`tests/<area>/main.rs` plus
  modules). `core` is fully testable headless: mock providers and temp dirs, no UI, no network. A
  test in `examples/` runs only if its `[[example]]` declares `test = true`.
- **A test earns its place by being able to fail.** Watch a regression test fail on the unfixed
  code. `scripts/mutants.sh -f <file>` finds the tests that cannot fail; use it on anything
  security- or format-critical. A round-trip cannot catch a rename, so a persisted shape is pinned
  against a literal copied from real output (a `.luxe/sessions/*.jsonl` line, a real `gh` reply).
- **Every public item has a `///` doc.**

## Comments and docs

Lean, casual, senior-dev voice. **Less is more.** Comment the *why*: intent, trade-offs, gotchas,
constraints a reader would not guess. Never restate what the code says; a comment that paraphrases
the next line goes. If a block needs a comment to say *what* it does, extract a named function.

- **Module docs (`//!`)**: `Name — short description.`, a blank line, then prose framing the
  module in the architecture.
- **Item docs (`///`)**: sentences ending in a period: what, then why, then edge cases.
  Payload-carrying variants get an `Args:` line. Label-style field and variant docs are fragments
  with no period (`/// Configured accounts`).
- **Inline (`//`)**: terse fragments, no period. Reserved prefixes: `LEARN:`, `NOTE:`, `TODO:`.
- **Shipped source describes the code as it is.** No history ("used to", the incident that taught
  it), no phase numbers, ticket ids or design-note citations; the story goes in the commit message.
  Keep the one sentence of why the rule exists.
- Flag: docs that echo a name (`/// Session ID` on `session_id`), banners around trivial code,
  commented-out code, tutorial narration of language basics.

## Security invariants

- **Permissive by default, with always-on guardrails unless `--yolo`:** deny `.env` reads and
  catastrophic commands, ask on out-of-project work, force-push, commits and GitHub writes. The
  engine (`luxe-core::permission`) sits at the dispatch choke point, and judges every tool that
  runs code or reads paths (`test`, `debug`, search roots, `apply` modes), not just `bash`.
- **Enforcement is code, never prompt text.** A rule that must hold lives in the permission engine
  or a hook.
- **A project cannot grant itself trust.** Its `.luxe/permissions.json` only tightens, its
  `.luxe/hooks.json`, `gate.commands`, MCP servers and language servers are honored only under
  `--trust-project` (or the global-only `lsp.project_servers`), so an "always" approval is written
  to the **global** file. One answer governs every project-supplied binary
  (`project_tooling_allowed`); a new consumer reads it, never a second gate. That gate picks which
  binary Luxe runs, nothing more: a language server still runs `build.rs` and proc macros, so never
  write that Luxe does not run repository code.
- **Secrets:** `0600` files, `$ENV` or `!command`, wrapped in `secrecy::SecretString`, scrubbed by
  the process-wide `redact::process_secrets()` from tool results, logs and the wire capture.
- **No telemetry.** Every outbound call is one the user asked for or can turn off, carries nothing
  about the machine, and is listed in `docs/guide/security.md`. The update check sends **nothing**
  (no version, platform or id) and compares locally; a server told the running version is getting a
  per-user report on a schedule.
- **MCP servers are untrusted:** opt-in, behind one `mcp` gateway tool whose calls go through the
  permission engine; their descriptions and results are data, not instructions.
- **Instruction files are filtered, not trusted.** `luxe-core::instructions` strips what a reader
  of the rendered file cannot see (comments, comment link definitions, invisible characters) and
  withholds a file it cannot finish checking.

## Invariants you would otherwise break

- **A tool call and its result are one unit.** Both wire formats reject an unpaired half, and it
  replays on every later request, so one gap ends the session. `dispatch_one` returns a block for
  every call, the commit after a batch is unconditional (cut-off, cancelled and failed replies
  included), and `tool_pairing::repair` pairs halves on the outgoing copy. The durable log is never
  rewritten. Only a call that never *started* may be described as having had no effect.
- **An empty block is the same permanent 400.** A call that produced nothing commits nothing
  (`ReplyOutcome::Nothing`, and says so), `empty_blocks::scrub` drops empties from the outgoing
  copy, and an empty tool result is **filled**, never dropped.
- **Cancellation is a flag with one owner: use `Engine::set_cancelled`.** `watch::Sender::send` is
  a no-op with no subscribers, which is the normal state between turns.
- **Nothing the model can invoke runs unbounded.** `Tool::timeout` sets each deadline and dispatch
  drops the future at it or at cancel. `None` is only for a tool that waits on a human or owns a
  process group. Cancel ends the **batch**, not just the call.
- **A phase advances only where the gate says so.** `loop_control progress` is a claim;
  `ReviewIntegratorHook` alone moves the counter, after the commands pass and the reviewer (when
  on) approves. A reviewer that cannot approve pauses the loop (`GateOutcome::Held`). A refusal
  reaches the model as the tool's own result.
- **Context occupancy is a measurement.** Read it from the last `EntryKind::Usage` after the newest
  compaction; `estimate_tokens` is the fallback and counts **every** block kind.
- **A request's transcript is an *append* to the last one's.** Providers cache an exact prefix, so
  editing history in flight re-bills everything after the edit at the write rate. Pruning is gated
  on real window pressure and cuts in whole `prune_min_tokens` steps. Watch the **written** share on
  `/cost` and in `luxe wire`, not the hit rate.
- **The system prompt is byte-identical between turns of a session.** No volatile fact enters it;
  one-time context is **appended**. Reason about what reaches the model from a capture
  (`LUXE_WIRE_LOG` plus `luxe wire`), not from source.
- **`Action` and `Event` are a published protocol.** `luxe --rpc` streams them to clients nobody
  here compiles, and an unknown tag degrades to `Other`, so a rename breaks every client and no
  compiler notices. Adding is free; removing or retagging moves `PROTOCOL_VERSION`. Pinned literals
  in `tests/event_serialization.rs` hold it.
- **`AGENTS.md` is the only project document in the prompt.** Anything else is pointed at.
- **A tool must beat the shell command it replaces** (bounded output, fewer round trips, correct
  defaults, or a capability the shell lacks), and its description names the sibling for the
  adjacent question.
- **Glyphs are polish, never meaning.** Every icon sits beside the words it labels; prefer block
  elements, which every terminal draws.
- **Platform questions have one owner each:** `user_dirs` (home and Luxe's directories), `program`
  (finding programs, shells, detaching, killing a process tree), `paths` (path to string),
  `fs_atomic` (durable replace), `luxe-lsp::uri` (`file://` URLs).
- **Generated assets are generated; captures are recorded.** `web/src/assets/generated/` comes from
  the Rust source and CI checks it. `web/src/assets/captures/` are real sessions recorded by
  `scripts/site-record.sh`, which nothing can verify; change how Luxe looks and you owe both a
  `site-assets.sh` and a re-record.
- **The wordmark constants are the art** (`luxe-tui/src/widgets/wordmark/art.rs`): one drawing, no
  alternatives.
- **Two voices, two homes.** Prose a person reads follows `docs/brand/voice.md`; how the agent
  writes lives in `luxe-core::context`'s base harness, never in a mode or a tool description.

## Where things live

- `CONTRIBUTING.md`: process, tooling, publishing. `docs/guide/`: the user guide, also embedded in
  the binary for `luxe docs`.
- `.luxe/todos/<session-id>.json`: the `todo` tool's checklist, one file per session.
- A development checkout also has `PROJECT.md` (what must stay true; read it when a design call is
  ambiguous), `ROADMAP.md`, `PROGRESS.md`, `PLAN.md` and design notes, all excluded from releases.
