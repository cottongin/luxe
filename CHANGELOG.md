# Changelog

Every release of Luxe, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). While Luxe is below
1.0, a minor bump (0.x.0) may break config, flags or the `--rpc` protocol, and a patch bump never
does.

The version on `main` is always the next release. The first change to what ships (`crates/`,
`Cargo.toml`, `Cargo.lock`) after a release bumps it one step and opens its section here, and later
changes add to that section until it ships; `scripts/version-check.py` holds them to it.

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
