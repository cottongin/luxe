# Changelog

Every release of Luxe, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). While Luxe is below
1.0, a minor bump (0.x.0) may break config, flags or the `--rpc` protocol, and a patch bump never
does.

The version on `main` is always the next release. The first change to what ships (`crates/`,
`Cargo.toml`, `Cargo.lock`) after a release bumps it one step and opens its section here, and later
changes add to that section until it ships; `scripts/version-check.py` holds them to it.

## [0.1.0]

The first public release.
