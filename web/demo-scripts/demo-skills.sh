#!/usr/bin/env bash
# Two project skills for the demos that show the `@` picker's Skills tab.
#
# Run as a `--seed`, so they're part of the project's history rather than a change the dock counts.

set -euo pipefail

mkdir -p .luxe/skills/test-first .luxe/skills/release-notes

cat >.luxe/skills/test-first/SKILL.md <<'MD'
---
name: test-first
description: Fixing a reported bug. Write the failing test before touching the code, then make it pass.
---

1. Reproduce the report as a test in `test/`, and run it with the `test` tool. It must fail, for
   the reason the report gives. A test that passes before the fix proves nothing.
2. Make the smallest change that turns it green. Leave neighboring behavior alone.
3. Run the whole suite with the `test` tool.
4. Say what the test pins down in one sentence, so a reviewer knows what it's guarding.
MD

cat >.luxe/skills/release-notes/SKILL.md <<'MD'
---
name: release-notes
description: Writing the entry for CHANGELOG.md after a user-visible change.
---

Lead with what changed for someone running `todo`, not with the refactor behind it. One line per
change, present tense, command names in backticks.
MD
