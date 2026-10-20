#!/usr/bin/env python3
"""version-check — a change that ships says which version it ships as.

Compares the workspace version against a base ref, normally the pull request's target:

- a change to what reaches crates.io (anything under `crates/`, `Cargo.toml`, `Cargo.lock`) on top
  of a released version has to raise it, and by exactly one semver step, so a typo cannot skip a
  release. A version is released once a `v<version>` tag marks it, locally or, in a development
  checkout, on its `public` remote (the public repository). Until then it already names the
  release this change will ship in, so no bump is owed; it may still move, but only to another
  single step past the last release, as when a breaking change turns a pending patch into a minor;
- the version never goes down, shipped change or not;
- every internal path dependency names the workspace version, because cargo publishes that number
  and a stale `0.1.0` still resolves locally against a `0.1.1` beside it;
- `CHANGELOG.md` has a section for the version, which is what a release's notes are cut from.

Usage: scripts/version-check.py [base-ref]    (default: origin/main)
"""

from __future__ import annotations

import re
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SEMVER = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)
SHIPPED_FILES = {"Cargo.toml", "Cargo.lock"}
# A development checkout names the public repository this, and its tags are the releases
RELEASE_REMOTE = "public"
DEPENDENCY_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")


class VersionError(Exception):
    """A version that is not semver."""


def parse(version: str) -> tuple[int, int, int, tuple[str, ...]]:
    """`(major, minor, patch, pre-release identifiers)`; build metadata carries no precedence."""
    match = SEMVER.match(version)
    if not match:
        raise VersionError(f"{version!r} is not a semver version")
    major, minor, patch, pre = match.groups()
    return int(major), int(minor), int(patch), tuple(pre.split(".")) if pre else ()


def precedence(version: str) -> tuple:
    """A sort key that orders versions the way semver 2.0.0 section 11 does."""
    major, minor, patch, pre = parse(version)
    # A release outranks every pre-release of the same core
    release = (1,) if not pre else (0,)
    # Numeric identifiers rank below alphanumeric ones and compare as numbers
    identifiers = tuple((0, int(part), "") if part.isdigit() else (1, 0, part) for part in pre)
    return (major, minor, patch, release, identifiers)


def is_one_step(base: str, head: str) -> bool:
    """Whether `head` follows `base` by one release: a patch, minor or major bump, a pre-release of
    one, or the release (or a later pre-release) of the core `base` was a pre-release of."""
    if precedence(head) <= precedence(base):
        return False
    major, minor, patch, base_pre = parse(base)
    core = parse(head)[:3]
    steps = {(major, minor, patch + 1), (major, minor + 1, 0), (major + 1, 0, 0)}
    if base_pre:
        steps.add((major, minor, patch))
    return core in steps


def ships(paths: list[str]) -> bool:
    """Whether any changed path reaches what crates.io publishes."""
    return any(path.startswith("crates/") or path in SHIPPED_FILES for path in paths)


def stale_pins(manifests: dict[str, str], version: str) -> list[str]:
    """Internal path dependencies whose `version` is not the workspace's, as `file: name = pin`."""
    stale = []
    for name, text in manifests.items():
        manifest = tomllib.loads(text)
        for table in DEPENDENCY_TABLES:
            for dependency, spec in manifest.get(table, {}).items():
                if not isinstance(spec, dict) or "path" not in spec or "version" not in spec:
                    continue
                if spec["version"].lstrip("=^") != version:
                    stale.append(f"{name}: {dependency} = {spec['version']!r}")
    return stale


def changelog_has(changelog: str, version: str) -> bool:
    """Whether the changelog has a `## [version]` (or bare `## version`) section."""
    heading = rf"^## \[?{re.escape(version)}\]?(?:\s|$)"
    return re.search(heading, changelog, re.MULTILINE) is not None


def workspace_version(cargo_toml: str) -> str:
    """The `[workspace.package]` version every crate inherits."""
    try:
        return tomllib.loads(cargo_toml)["workspace"]["package"]["version"]
    except KeyError as missing:
        raise VersionError(f"Cargo.toml has no workspace.package.version ({missing})") from None


def git(*args: str) -> str:
    """Runs git in the repository and returns its stdout, failing loudly on a bad ref."""
    result = subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True)
    if result.returncode != 0:
        raise SystemExit(f"version-check: git {' '.join(args)} failed: {result.stderr.strip()}")
    return result.stdout


def problems(base_ref: str) -> list[str]:
    """Everything wrong with the working tree's version against `base_ref`, empty when it is fine."""
    head = workspace_version((ROOT / "Cargo.toml").read_text())
    base = workspace_version(git("show", f"{base_ref}:Cargo.toml"))
    changed = git("diff", "--name-only", f"{base_ref}...HEAD").split()
    found = []
    if precedence(head) < precedence(base):
        found.append(f"the version went down: {base} -> {head}")
    elif ships(changed) and (problem := shipped_problem(base, head, released())):
        found.append(problem)
    manifests = {str(p.relative_to(ROOT)): p.read_text() for p in ROOT.glob("crates/*/Cargo.toml")}
    found += [f"internal dependency not pinned to {head}: {pin}" for pin in stale_pins(manifests, head)]
    changelog = ROOT / "CHANGELOG.md"
    if not changelog.exists() or not changelog_has(changelog.read_text(), head):
        found.append(f"CHANGELOG.md has no `## [{head}]` section")
    return found


def released() -> list[str]:
    """Every version a `v<version>` tag marks, here or on the release remote."""
    tags = set(git("tag", "--list", "v*").split()) | set(release_remote_tags())
    return sorted(tag[1:] for tag in tags if SEMVER.match(tag[1:]))


def release_remote_tags() -> list[str]:
    """The release remote's `v*` tags, read without fetching; none when it isn't set up.

    A development checkout never sees the public repository's tags on its own, so a release made
    there would look unreleased here, and work would pile up under a version that already shipped.
    Offline, the local tags are all there is, and the check says so rather than failing.
    """
    if RELEASE_REMOTE not in git("remote").split():
        return []
    listing = subprocess.run(
        ["git", "ls-remote", "--tags", "--refs", RELEASE_REMOTE, "v*"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if listing.returncode != 0:
        reason = listing.stderr.strip()
        warning = f"warning: could not read `{RELEASE_REMOTE}`'s tags, so only local ones count"
        print(f"version-check: {warning}: {reason}", file=sys.stderr)
        return []
    return tags_in_listing(listing.stdout)


def tags_in_listing(listing: str) -> list[str]:
    """The tag names in `git ls-remote --tags --refs` output."""
    prefix = "refs/tags/"
    refs = (line.split("\t", 1)[-1] for line in listing.splitlines())
    return [ref[len(prefix) :] for ref in refs if ref.startswith(prefix)]


def shipped_problem(base: str, head: str, released: list[str]) -> str | None:
    """What is wrong with `head` for a change that ships on top of `base`, or `None`.

    A released base owes one step. An unreleased one already names the coming release, so it may
    stay, or move to another single step past the last release before it.
    """
    if base in released:
        return None if is_one_step(base, head) else bump_message(base, head)
    if head == base:
        return None
    earlier = [version for version in released if precedence(version) < precedence(base)]
    last = max(earlier, key=precedence, default=None)
    if is_one_step(last or base, head):
        return None
    since = f"the last release, {last}" if last else f"{base}, which is not released yet"
    return f"{head} is not one semver step past {since}"


def bump_message(base: str, head: str) -> str:
    """Says what a shipped change needed, naming the three versions it could have taken."""
    major, minor, patch, _ = parse(base)
    options = f"{major}.{minor}.{patch + 1}, {major}.{minor + 1}.0 or {major + 1}.0.0"
    if head == base:
        return f"this changes what ships but leaves the version at {base}; bump it to {options}"
    return f"{base} -> {head} is not one semver step; the next version is {options}"


def main() -> int:
    base_ref = sys.argv[1] if len(sys.argv) > 1 else "origin/main"
    try:
        found = problems(base_ref)
    except VersionError as error:
        found = [str(error)]
    for problem in found:
        print(f"version-check: {problem}", file=sys.stderr)
    if not found:
        print(f"version-check: ok against {base_ref}")
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
