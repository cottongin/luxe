# Contributing to Luxe

Process and tooling for people working on Luxe itself. The rules an agent has to hold to are in
[`AGENTS.md`](AGENTS.md), deliberately kept short because it rides in every request; this file is
the long version, for humans, and nothing here is loaded into a prompt.

The user guide is `docs/guide/`, published at luxe.dev/docs; how it is written is in
`docs/brand/voice.md`, and `scripts/docs-check.py` and `guide_check` hold it to the code.
`luxe docs` embeds a copy of it, `crates/luxe-cli/src/guide.md`: run `scripts/guide-bundle.sh` after
editing a page, and a test fails until you do.

## Licensing your contribution

Luxe is dual licensed under MIT (`LICENSE-MIT`) or Apache-2.0 (`LICENSE-APACHE`), at the user's
option. Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any
additional terms or conditions.

## AI assistance

AI help is welcome. Luxe is built with plenty of it, much of it Luxe working on itself. Three things
to hold to:

- **Say so.** Name the tool and how much of the change it wrote in the pull request: "written with
  Luxe on Claude Sonnet 5.5, then reviewed and edited by me", or "asked ChatGPT how the cache works,
  wrote the fix myself". Tab completion doesn't count. A commit can carry the Linux kernel's
  `Assisted-by: AGENT:MODEL` trailer.
- **Understand it.** You should be able to explain what your change does and how it fits without
  asking the tool. That's the bar for any change, and the one an agent makes easiest to skip.
- **Write to people in your own words.** An issue or a pull request description is read by a person,
  so keep it short, and label anything a model wrote.

## The gate

```bash
cargo build   --workspace --all-targets
cargo nextest run --workspace                   # falls back to `cargo test --workspace`
cargo clippy  --workspace --all-targets -- -D warnings
cargo fmt     --all --check
cargo deny    check
python3 scripts/size-check.py
python3 scripts/fn-length-check.py
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

Run `cargo check -p <crate>` while iterating and the whole gate before calling a slice done.

### Size ratchets

Short files and functions are cheaper to read, for people and for the agents working here. The
limits are 500 lines of production code per file, 150 per inline test module (past that, it moves
to a sibling `tests.rs`), 800 per test-only file, and 80 per function by clippy's count. They
arrived after the code did, so each script carries an allowlist of what was already over:
`scripts/size-allowlist.txt` and `scripts/fn-length-allowlist.txt`. An entry may shrink, never
grow; when one shrinks, `--tighten` lowers the recorded number, and the check fails until you run
it, so the gain is kept. Adding an entry needs a reason a reviewer accepts, written on its line: a
dispatch table, a data table, a protocol enum. `baseline` marks an entry nobody has judged yet.

`scripts/comment-report.py` is the same idea for comments, as a report rather than a gate: the
ratio alone says little, so it lists the long blocks and the lines narrating history, which are
the ones worth rereading.

`fn-length-check.py` runs clippy in `target/fn-length` so it never invalidates the gate's own clippy
cache, and measures `#[async_trait]` methods itself, since clippy cannot see inside a macro's output.

### Test-runner discipline

Prefer `cargo nextest run`. It runs the suite in parallel — the whole workspace in about ten
seconds once compiled — and `.config/nextest.toml` **kills any test that exceeds ~120s**, so a hang
or a runaway cannot wedge the machine. There are no doctests; if any are added, also run
`cargo test --workspace --doc`.

Test *execution* is fast and *compilation* dominates wall time, so a slow-looking run is almost
always rebuilding test targets rather than hanging. Killing it and relaunching only restarts the
compile. Never run two full builds or test runs at once — they thrash CPU and RAM and both take
longer. For unattended runs use `timeout <secs> <cmd>` rather than polling with an unbounded
`while pgrep cargo` loop, which spins forever on a single orphaned child.

Scope while iterating: `cargo nextest run -p <crate>`, or `-E 'test(<name>)'`.

### Checking that a test can fail

A green suite is evidence only if its tests can go red, and coverage does not measure that — it
records which lines ran, not whether anything checked them. `cargo mutants` does measure it: it
edits the source in small ways and reports which edits no test noticed.

```bash
cargo install cargo-mutants --locked
scripts/mutants.sh -p luxe-core -f crates/luxe-core/src/permission/'*'.rs
```

Through the script, not `cargo mutants` directly: it runs in a memory-capped systemd scope, because
a mutant that turns a loop into one that never ends allocates without limit, and on a desktop the
kernel's answer to that took the terminal down with the run. `.cargo/mutants.toml` holds the rest —
nextest as the runner, eight test threads rather than one per core.

Run it on a file you are about to trust, not on the workspace — it rebuilds per mutant, so scope
is the difference between minutes and hours. It needs a suite that already passes reliably; a flaky
test makes every result meaningless.

Read the survivors as questions, not defects. Most are answered by one missing case; some are
equivalent mutants that change nothing observable and should be left alone. Two were real bugs:
`permission/wildcard.rs` scored 61% and the gap was a lone `&` that hid `rm -rf /` from every bash
rule, and the same pass found `EntryKind::Unknown` breaking the forward-compatibility promise its
own doc comment made. Both had passing tests over them the whole time.

## Architecture

```
crates/
├── luxe-core/        engine: types, session, context, agent loop, compaction, permissions, hooks
├── luxe-providers/   provider adapters and the model catalog
├── luxe-tools/       read/write/edit/bash/grep/glob/web/question/skill/git/todo/test
├── luxe-lsp/         the language-server client and the code-intelligence tools
├── luxe-dap/         the debug-adapter client and the `debug` tool
├── luxe-mcp/         the MCP transports
├── luxe-tui/         the Ratatui front end
└── luxe-cli/         the binary: config, wiring, TUI, headless and RPC
```

Dependencies point inward to `luxe-core`, which has no UI or HTTP and defines the `Provider`,
`Tool` and `Hook` traits. A front end subscribes to the event stream, and that stream is published:
`Action` in, `Event` out, as JSON lines over stdio for `luxe --rpc`. The TUI is one subscriber.

## Seeing exactly what goes to the model

Prompt size drives cost and time to first token, and the expensive failure is invisible: providers
cache by prefix, so one changed byte in the system prompt re-bills everything after it.
`LUXE_WIRE_LOG` records the traffic and `luxe wire` measures it: prefix churn and fixed-overhead
share, where the characters go (system prompt per `## section`, tool schemas, conversation, in
tokens calibrated from reported usage), whether the cacheable prefix stays identical between turns
(naming the first changed byte when it does not), whether anything is sent twice, whether text
arrived incrementally, and whether a credential leaked.

```bash
LUXE_WIRE_LOG=/tmp/wire.jsonl luxe --headless -p "what does src/main.rs do?"
luxe wire /tmp/wire.jsonl --tools
```

## Dogfooding: the debug/release gap

The `luxe` on your `PATH` is a symlink to `target/release/luxe`, but everything above builds and
tests **debug**. Nothing rebuilds release for you, so a fix can pass the entire gate and still be
missing from the binary you actually launch. The symptom is a bug you already fixed appearing to
come back, which reads like a regression and is not one.

```bash
scripts/install-dev.sh            # build release, point ~/.local/bin/luxe at it
scripts/install-dev.sh --gate     # ...only if the full gate passes first
```

When a reported defect contradicts the source, check `ls -la target/release/luxe` against the
fix's commit date **before** re-debugging it.

## Windows is a gate, not a hope

Three platforms are first-class and Windows is the one that breaks silently, so it gets a loop
rather than a wish. `scripts/win-check.sh` cross-builds with `cargo-xwin` (clang-cl fetches the
MSVC CRT and SDK headers) and runs the suite under wine in about twenty seconds.

```bash
scripts/win-check.sh                                    # the whole suite
scripts/win-check.sh --clippy                           # the only local lint that sees #[cfg(windows)]
scripts/win-check.sh -- -p luxe-tools -E 'test(glob)'   # one crate, one filter
```

Needs `clang-cl`, `lld-link`, `wine`, `cargo install cargo-xwin`, and — the one that is easy to
miss — the Windows standard library:

```bash
rustup target add x86_64-pc-windows-msvc
```

That last one needs **rustup**, and a distribution's `rust` package is not it: Arch's ships one
target and no way to add another, so `win-check.sh` fails at `can't find crate for core` with
nothing to suggest but "install the target you cannot install". If your Rust comes from `mise`
(`mise use -g rust@stable`), rustup is what it manages underneath and the command above works as
written; declaring the target in `mise.toml` keeps it reproducible:

```toml
[tools]
rust = { version = "stable", targets = ["x86_64-pc-windows-msvc"] }
```

It runs a *stricter* environment than CI on purpose. GitHub's `windows-latest` image puts Git's
`usr/bin` on `PATH`, so `sh`, `bash` and `grep` are all quietly present and a whole class of
failure never appears; win-check hands the run MinGit's `cmd` directory and nothing else, which is
what a default Git for Windows install actually looks like. Its first baseline was 32 failures
where CI reported 15.

Wine is an approximation. It gets `PATH` order, path semantics, process spawning and
`FlushFileBuffers` right — which is where the bugs were — and it models neither ACLs nor antivirus
nor the console. Green here means worth pushing; the matrix job is still the authority.

Two things follow from having it. **Clippy runs per-platform in CI**, because a lint only sees the
code its target compiles — before this, every `#[cfg(windows)]` block shipped unlinted, and the
first run found a dead helper. And **a test that names a program has named a platform**: `true` is
a unix executable rather than a universal idea, `sh` is not installed on Windows, and `printf` is
not a `cmd` builtin. Reach for `luxe_core::program::has_posix_shell()` (skipping, and saying why)
or the `common::always_succeeds()` fixture.

### macOS

`cargo check` needs the target's std, not a linker:

```bash
rustup target add aarch64-apple-darwin
cargo check -p luxe-core -p luxe-tui -p luxe-dap --all-targets --target aarch64-apple-darwin
```

Crates pulling reqwest (ring) or tree-sitter cannot cross-*check* here, because their build scripts
compile C and that needs a cross C compiler. win-check gets around it for Windows by bringing one;
for macOS, CI's runner is the answer. A from-source build therefore still needs a C toolchain —
dropping syntect's Oniguruma removed the flakiest of the three, not all of them.

## Measuring the TUI

Frame cost is the one property of the TUI that cannot be read off the source: a repaint that looks
linear may be memoized, and a memo that looks sound may have quietly stopped engaging. Three tools,
in the order you reach for them.

```bash
cargo run -p luxe-tui --release --example render_perf   # what one frame costs, per surface
cargo nextest run -p luxe-tui --test frame_budget       # the ceilings CI holds it to
```

`render_perf` reports wall time **and allocation count** for every surface, at several transcript
lengths and several review sizes. The second number is the one that travels: it is identical on any
machine, so a claim made here can be checked in review. `frame_budget` turns the same measurement
into assertions — chiefly that a steady frame's cost does not scale with the length of the session,
which is the mistake the transcript made for months. A budget that fails is not automatically
wrong; read the number. Ten allocations more because a surface gained a row is a ceiling to raise.
A hundred thousand more is something that stopped being memoized.

Allocation count does not see everything. A scan where a lookup belongs allocates nothing, so the
review path is checked by the **shape of its curve** instead — one review size against another,
which divides the machine out and so cannot flake the way an absolute millisecond budget would.
Watch that column in `render_perf` when touching anything that resolves a review's ids.

For the real binary under real input:

```bash
scripts/make-big-session.py .luxe/sessions/<project>/<id>.jsonl 40 big-01   # a long transcript
scripts/tui-latency.py --resume big-01 --keys 240 --rate 80                 # hold a key down
scripts/tui-latency.py --resume big-01 --env LUXE_FRAME_PROFILE=/tmp/f.csv  # per-frame timings
```

`tui-latency.py` drives `luxe` on a pty at a fixed key rate and reports what the process did about
it — including **how long it kept catching up after the typing stopped**, which is the number a
freeze actually is. `LUXE_FRAME_PROFILE` makes the binary write a CSV row per painted frame (what
woke it, how long the update and the draw took, how much of the draw was transcript layout, how
many events were folded into it). Off by default and free when off.

## Manual verification

For anything visual or behavioral — tool calls, streaming, rendering, compaction, real agent
behavior — run it against a real model. A local one (LM Studio, Ollama) costs nothing per try;
cross-check with a hosted provider when a behavior looks model-specific.

## Generated assets

Everything under `web/src/assets/generated/` comes out of the Rust source and is committed; CI runs
`--check` and fails on stale output.

```bash
scripts/site-assets.sh              # regenerate all of it
scripts/site-assets.sh --check      # what CI runs

# ...or one at a time, straight to stdout
cargo run -p luxe-tui --example wordmark_svg  -- tile        # wordmark | mark | tile
cargo run -p luxe-tui --example theme_css                    # CSS custom properties
cargo run -p luxe-tui --example fixture_svg   -- --theme luxe --scenario chat-tools
cargo run -p luxe-tui --example fixture_svg   -- --census    # glyphs still drawn by a font
cargo run -p luxe-tui --example fixture_cast  -- --list      # scripted demos (asciicast v2)

scripts/site-fonts.sh               # rebuild subset webfonts (needs fonttools + brotli)
scripts/social-card.sh              # re-render web/public/social.png from the card page
python3 scripts/voice-check.py --review
```

The Tools page's bars are recorded, not generated: `scripts/tool-bench.sh NAME=REPO:CASES ...` runs
each tool and the shell command it replaces in each checkout you name (the cases are in
`scripts/tool-bench/`), then in Luxe itself, and writes `web/src/data/tool-bench.json` naming every
repository's commit. Each shell row is also put through the other agents' own output limits, since
that, not the raw output, is what their models are sent. Nothing checks the file; run it on
committed trees when a tool's output changes. The page draws from the first repository.

## The website

Astro + Svelte islands, static, Cloudflare Pages, Node 24+. See [`web/README.md`](web/README.md).

```bash
cd web && npm install && npm run dev     # http://localhost:4321
cd web && npm run build && npm run check
cd web && npm run deploy                 # needs `wrangler login` once
```

## Versioning

One version for the whole workspace (`[workspace.package]` in `Cargo.toml`), following semver, and
the version on `main` is always the next release. The first pull request to the public repository
that changes what ships (`crates/`, `Cargo.toml`, `Cargo.lock`) after a release, which is when a
`v<version>` tag marks the current one, bumps it by exactly one step, updates the internal
path-dependency pins to match, and adds a `## [x.y.z]` section to `CHANGELOG.md`. Later pull
requests before the next release owe no bump and add their entries to that section; one that
breaks something may still move a pending patch to a minor, one step past the last release. The
`Version` workflow runs `scripts/version-check.py` against the target branch to hold it to that;
changes to the site, the guide or CI owe nothing.

```bash
python3 scripts/version-check.py origin/main    # what the pull request check runs
python3 scripts/version_check_test.py           # the check's own tests
```

Below 1.0, a minor bump may break config, flags or the `--rpc` protocol; a patch never does.

## Releasing

A release is a tag. When `main` holds the version to release, with its `CHANGELOG.md` section
written:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

`.github/workflows/release.yml`, which `dist` generates from `dist-workspace.toml`, then builds
every target, puts the archives, the install scripts and their updater on the GitHub release with
an attestation for each file, publishes the Homebrew formula to `evanwhittaker/homebrew-tap`, and
publishes the crates to crates.io (`publish-crates.yml`) before announcing. Never edit
`release.yml`; change `dist-workspace.toml` and run `dist generate`.

The crates go up by crates.io Trusted Publishing, so no token is stored: each crate's trusted
publisher is this repository's `release.yml`. A crate whose version crates.io already has is
skipped, so a re-run after a partial publish carries on. `cargo publish --workspace --dry-run`
packages and builds every crate the way the registry will, dependencies first.

## Development-only files

A development checkout carries planning files, design notes and evaluation rigs that Luxe never
needs at runtime. They are marked `export-ignore` in `.gitattributes`, and
`scripts/publish-clean.sh <dir>` writes a release tree without them, then scans what it wrote for
anything that must not ship.
