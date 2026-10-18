# Changelog

Every release of Luxe, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). While Luxe is below
1.0, a minor bump (0.x.0) may break config, flags or the `--rpc` protocol, and a patch bump never
does.

The version on `main` is always the next release. The first change to what ships (`crates/`,
`Cargo.toml`, `Cargo.lock`) after a release bumps it one step and opens its section here, and later
changes add to that section until it ships; `scripts/version-check.py` holds them to it.

## [0.1.2]

### Fixed

- Compaction works with thinking models. Summaries were cut off at 4,096 output tokens and thrown
  away, so after three failures automatic compaction switched itself off and the window filled
  until the model could not answer. `/compact` now gives the real reason when it fails.
- On Windows, a timed-out or cancelled command ends everything it started, not just its shell, and
  the agent's commands no longer rename your terminal tab.
- On Windows, the file tools accept Git Bash paths (`/c/Users/...`), `glob` reads `src\*.py` as a
  folder, and the agent's Python commands run in UTF-8 mode, so printing characters outside your
  code page no longer crashes them.
- On Windows, an npm tool installed as a `.cmd` shim (such as `pyright`) is found, and Python is
  looked up as `py -3`, then `python`.
- The debugger checks that `debugpy` is installed before offering Python debugging, and
  `luxe doctor` says how to install it.
- A new session started from the start screen keeps its working indicator and timer from the first
  moment.

### Changed

- The dock shows "compacting" while a summary is written, and its context reading drops as soon as
  it finishes. `--rpc` clients get the new `compaction_started` and `compaction_stopped` events.
- An empty reply from the model at a nearly full context window suggests `/compact` instead of a
  retry.
- Signing in with GitHub: the choices are clickable, and the device code has a Copy button (or
  press `c`).
- The installer's closing message says how to start Luxe.

## [0.1.1]

### Fixed

- Editing a file with the edit or write tool, or format-on-save rewriting it, keeps the file's
  permissions, so an edited script stays executable. A kept mode is now exact rather than trimmed
  by the umask.
- The changes pane's header keeps its position (`8 of 14`) whole, shortening the path instead, and
  keeps a gap between the line counts and the FILE / DIFF switch.

### Changed

- `grep` names each file once with its matching lines under it, rather than repeating the path on
  every line: 9–28% fewer bytes than `rg -n` on the searches measured. `scopes: true` also labels
  each run of matches with the function or type around it.
- The install guide leads with `curl -fsSL https://luxe.dev/install.sh | sh` and
  `powershell -c "irm https://luxe.dev/install.ps1 | iex"`.

## [0.1.0]

The first public release.
