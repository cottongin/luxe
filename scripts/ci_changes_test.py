#!/usr/bin/env python3
"""Tests for ci-changes.py. Run: python3 scripts/ci_changes_test.py"""

import importlib.util
import unittest
from pathlib import Path
from typing import Any

# The script's name has a hyphen, like its neighbors, so it is loaded by path rather than imported
SPEC = importlib.util.spec_from_file_location("ci_changes", Path(__file__).with_name("ci-changes.py"))
assert SPEC and SPEC.loader, "ci-changes.py sits beside this file"
cc: Any = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cc)


class IsInput(unittest.TestCase):
    def test_prose_is_not(self):
        for path in ["README.md", "CHANGELOG.md", "docs/brand/voice.md", ".pi/x.txt", "LICENSE-MIT",
                     ".github/dependabot.yml"]:
            with self.subTest(path):
                self.assertFalse(cc.is_input(path))

    def test_markdown_the_build_or_suite_reads_is(self):
        # luxe-cli compiles its guide bundle in; the suite checks docs/guide against the code
        for path in ["crates/luxe-cli/guide.md", "crates/luxe-core/README.md", "docs/guide/index.md"]:
            with self.subTest(path):
                self.assertTrue(cc.is_input(path))

    def test_everything_else_is(self):
        for path in ["crates/luxe-core/src/lib.rs", "Cargo.lock", "web/src/pages/index.astro",
                     ".github/workflows/ci.yml", "scripts/ci-changes.py", "docs.rs", "mdbook.toml"]:
            with self.subTest(path):
                self.assertTrue(cc.is_input(path))


class NothingToDiff(unittest.TestCase):
    """Skipping has to be proven: without a base to diff against, everything runs."""

    def test_no_base_or_a_new_branch_cannot_be_diffed(self):
        for base in ["", "0000000000000000000000000000000000000000"]:
            with self.subTest(base):
                self.assertIsNone(cc.changed_since(base))

    def test_a_base_the_history_lacks_cannot_be_diffed(self):
        self.assertIsNone(cc.changed_since("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"))

    def test_a_real_base_lists_what_changed(self):
        self.assertIsInstance(cc.changed_since("HEAD"), list)


if __name__ == "__main__":
    unittest.main()
