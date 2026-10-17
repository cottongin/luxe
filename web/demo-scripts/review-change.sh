#!/usr/bin/env bash
# The change the Guided Review demo reads back.
#
# Applied by `review.rec` as its `--setup`, so the review has something with shape to it: three
# files, a real boundary fix, a new filter, and a test. A one-line change reviews as `Step 1 / 1`,
# which is a true rendering of a change too small to be worth the surface it is demonstrating.

set -euo pipefail

# 1. A new filter, with the overdue case pulled out of the CLI and given a name.
cat >>src/filter.js <<'JS'

/**
 * Tasks past their due date as of `today`, oldest first.
 *
 * Separate from `dueBy` on purpose: "due by Friday" is a plan and "overdue on Friday" is a
 * problem, and folding them into one predicate with a flag made every call site read wrong.
 */
export function overdue(tasks, today) {
  return tasks
    .filter((task) => !task.done && task.due && task.due < today)
    .sort((a, b) => a.due.localeCompare(b.due));
}
JS

# 2. The CLI grows a command for it. Python rather than sed: the usage string is multi-line, and a
# sed that inserts `\n` into it writes the two characters rather than a line break.
python3 - <<'PY'
import pathlib

path = pathlib.Path("src/cli.js")
source = path.read_text()
source = source.replace("todo due <DATE>", "todo due <DATE>\ntodo overdue")
source = source.replace(
    'import { dueBy, open, tagged } from "./filter.js";',
    'import { dueBy, open, overdue, tagged } from "./filter.js";',
)
source = source.replace(
    '  case "due":\n    show(dueBy(open(tasks), words[0]));\n    break;',
    '  case "due":\n    show(dueBy(open(tasks), words[0]));\n    break;\n'
    '  case "overdue":\n    show(overdue(tasks, new Date().toISOString().slice(0, 10)));\n    break;',
)
path.write_text(source)
PY

# 3. And a test for it, because the project's own AGENTS.md says a behavior change needs one.
python3 - <<'PY'
import pathlib

path = pathlib.Path("test/filter.test.js")
source = path.read_text()
source = source.replace(
    'import { dueBy, open, tagged } from "../src/filter.js";',
    'import { dueBy, open, overdue, tagged } from "../src/filter.js";',
)
source += '''
test("overdue skips finished work and puts the oldest first", () => {
  assert.deepEqual(
    overdue(tasks, "2026-03-21").map((t) => t.title),
    ["Ship the docs", "Fix the parser"],
  );
});
'''
path.write_text(source)
PY
