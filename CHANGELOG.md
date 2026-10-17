# Changelog

Every release of Luxe, newest first. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html). While Luxe is below
1.0, a minor bump (0.x.0) may break config, flags or the `--rpc` protocol, and a patch bump never
does.

A pull request that changes what ships (`crates/`, `Cargo.toml`, `Cargo.lock`) bumps the workspace
version by one step and adds its section here; `scripts/version-check.py` holds it to that.

## [0.1.0]

The first public release.
