#!/usr/bin/env python3
"""ci-changes — whether a pull request touched anything CI has to build.

Pull-request CI always runs so it can be a required check; a required check that never starts
blocks the merge for good. This decides whether the jobs have work: prose cannot change what the
compiler, the suite or the site says, so a prose-only change skips them and reports a pass.

Prose is only what is provably not an input: Markdown, `docs/`, `.pi/`, the licenses and
Dependabot's schedule, except that nothing under `crates/` counts (`luxe-cli` compiles its bundled
`guide.md` in) and neither does `docs/guide/`, which the suite checks against the code and the site
renders.

Usage: scripts/ci-changes.py [base-sha]    prints `code=true|false` for $GITHUB_OUTPUT.

When there is nothing to diff against, the jobs run: no base (a run by hand), the all-zero base a
push of a new branch reports, or a base the history no longer holds after a force-push. Skipping
has to be proven; running never needs to be.
"""

from __future__ import annotations

import re
import subprocess
import sys

PROSE = re.compile(r"\.md$|^docs/|^\.pi/|^LICENSE-|^\.github/dependabot\.yml$")
INPUTS_AMONG_PROSE = re.compile(r"^crates/|^docs/guide/")


def is_input(path: str) -> bool:
    """Whether a change to `path` can change what CI reports."""
    return bool(INPUTS_AMONG_PROSE.search(path)) or not PROSE.search(path)


def changed_since(base: str) -> list[str] | None:
    """The paths changed since HEAD's merge base with `base`, or `None` when that cannot be said."""
    if re.fullmatch(r"0*", base):
        return None
    diff = ["git", "diff", "--name-only", f"{base}...HEAD"]
    result = subprocess.run(diff, capture_output=True, text=True)
    if result.returncode != 0:
        print(f"ci-changes: {' '.join(diff)} failed, so everything runs: {result.stderr.strip()}",
              file=sys.stderr)
        return None
    return result.stdout.split()


def main() -> int:
    changed = changed_since(sys.argv[1] if len(sys.argv) > 1 else "")
    code = changed is None or any(is_input(path) for path in changed)
    print(f"code={str(code).lower()}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
