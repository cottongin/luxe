#!/usr/bin/env python3
"""Tests for version-check.py. Run: python3 scripts/version_check_test.py"""

import importlib.util
import subprocess
import tempfile
import unittest
from pathlib import Path
from typing import Any

# The script's name has a hyphen, like its neighbors, so it is loaded by path rather than imported
SPEC = importlib.util.spec_from_file_location(
    "version_check", Path(__file__).with_name("version-check.py")
)
assert SPEC and SPEC.loader, "version-check.py sits beside this file"
vc: Any = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(vc)


class Precedence(unittest.TestCase):
    def test_orders_the_spec_example(self):
        ordered = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0",
            "1.0.1",
            "1.1.0",
            "2.0.0",
        ]
        self.assertEqual(sorted(reversed(ordered), key=vc.precedence), ordered)

    def test_build_metadata_carries_no_precedence(self):
        self.assertEqual(vc.precedence("1.0.0+build.7"), vc.precedence("1.0.0"))

    def test_rejects_what_is_not_semver(self):
        for bad in ["1.0", "01.0.0", "v1.0.0", "1.0.0-", "1.0.0-01"]:
            with self.subTest(bad), self.assertRaises(vc.VersionError):
                vc.parse(bad)


class OneStep(unittest.TestCase):
    def test_each_bump_is_one_step(self):
        for head in ["0.1.4", "0.2.0", "1.0.0", "0.2.0-rc.1"]:
            with self.subTest(head):
                self.assertTrue(vc.is_one_step("0.1.3", head))

    def test_a_skipped_or_unzeroed_bump_is_not(self):
        for head in ["0.1.5", "0.3.0", "0.2.3", "2.0.0", "1.1.0"]:
            with self.subTest(head):
                self.assertFalse(vc.is_one_step("0.1.3", head))

    def test_the_same_version_is_not_a_step(self):
        self.assertFalse(vc.is_one_step("0.1.3", "0.1.3"))
        self.assertFalse(vc.is_one_step("0.2.0-rc.1", "0.2.0-rc.1"))

    def test_a_pre_release_steps_to_its_release_or_a_later_one(self):
        self.assertTrue(vc.is_one_step("0.2.0-rc.1", "0.2.0"))
        self.assertTrue(vc.is_one_step("0.2.0-rc.1", "0.2.0-rc.2"))
        self.assertFalse(vc.is_one_step("0.2.0-rc.2", "0.2.0-rc.1"))

    def test_a_release_cannot_step_to_its_own_pre_release(self):
        self.assertFalse(vc.is_one_step("0.2.0", "0.2.0-rc.1"))


class Ships(unittest.TestCase):
    def test_crates_and_the_root_manifests_ship(self):
        for path in ["crates/luxe-core/src/lib.rs", "Cargo.toml", "Cargo.lock"]:
            with self.subTest(path):
                self.assertTrue(vc.ships([path]))

    def test_the_site_docs_and_lookalikes_do_not(self):
        for path in ["web/src/pages/index.astro", "docs/guide/index.md", "cratesfoo", "web/Cargo.toml"]:
            with self.subTest(path):
                self.assertFalse(vc.ships([path]))


class Pins(unittest.TestCase):
    def manifest(self, line):
        return {"crates/a/Cargo.toml": f"[package]\nname = 'a'\n\n[dependencies]\n{line}\n"}

    def test_a_stale_internal_pin_is_named(self):
        stale = vc.stale_pins(self.manifest('b = { path = "../b", version = "0.1.0" }'), "0.2.0")
        self.assertEqual(stale, ["crates/a/Cargo.toml: b = '0.1.0'"])

    def test_a_current_or_exact_pin_passes(self):
        for pin in ["0.2.0", "=0.2.0"]:
            line = f'b = {{ path = "../b", version = "{pin}" }}'
            with self.subTest(pin):
                self.assertEqual(vc.stale_pins(self.manifest(line), "0.2.0"), [])

    def test_registry_and_unversioned_path_deps_are_not_internal_pins(self):
        self.assertEqual(vc.stale_pins(self.manifest('serde = { version = "1" }'), "0.2.0"), [])
        self.assertEqual(vc.stale_pins(self.manifest('b = { path = "../b" }'), "0.2.0"), [])

    def test_dev_dependencies_are_checked_too(self):
        text = '[dev-dependencies]\nb = { path = "../b", version = "0.1.0" }\n'
        self.assertEqual(len(vc.stale_pins({"m": text}, "0.2.0")), 1)


class Changelog(unittest.TestCase):
    def test_finds_a_bracketed_or_bare_heading(self):
        self.assertTrue(vc.changelog_has("# Changelog\n\n## [0.2.0] - 2026-11-01\n", "0.2.0"))
        self.assertTrue(vc.changelog_has("## 0.2.0\n", "0.2.0"))

    def test_another_version_or_level_does_not_count(self):
        self.assertFalse(vc.changelog_has("## [0.2.0-rc.1]\n", "0.2.0"))
        self.assertFalse(vc.changelog_has("## [0.2.01]\n", "0.2.0"))
        self.assertFalse(vc.changelog_has("### [0.2.0]\n", "0.2.0"))


class AgainstARepository(unittest.TestCase):
    """The git wiring: a real repository, a base branch, and a change on top of it."""

    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.root = Path(self.dir.name)
        self.saved_root, vc.ROOT = vc.ROOT, self.root
        self.write_version("0.1.0")
        self.write("crates/a/src/lib.rs", "")
        self.run_git("init", "-q", "-b", "main")
        self.commit("base")
        self.run_git("checkout", "-q", "-b", "change")

    def tearDown(self):
        vc.ROOT = self.saved_root
        self.dir.cleanup()

    def write(self, path, text):
        (self.root / path).parent.mkdir(parents=True, exist_ok=True)
        (self.root / path).write_text(text)

    def write_version(self, version):
        self.write("Cargo.toml", f"[workspace.package]\nversion = '{version}'\n")
        self.write("CHANGELOG.md", f"## [{version}]\n")
        self.write("crates/a/Cargo.toml", f"[dependencies]\nb = {{ path = '../b', version = '{version}' }}\n")

    def run_git(self, *args):
        subprocess.run(["git", *args], cwd=self.root, check=True, capture_output=True)

    def commit(self, message):
        self.run_git("add", "-A")
        self.run_git("-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-qm", message)

    def test_a_shipped_change_without_a_bump_fails(self):
        self.write("crates/a/src/lib.rs", "pub fn new() {}\n")
        self.commit("change")
        self.assertIn("leaves the version at 0.1.0", " ".join(vc.problems("main")))

    def test_a_shipped_change_with_a_bump_passes(self):
        self.write("crates/a/src/lib.rs", "pub fn new() {}\n")
        self.write_version("0.1.1")
        self.commit("change")
        self.assertEqual(vc.problems("main"), [])

    def test_a_site_change_needs_no_bump(self):
        self.write("web/index.html", "<p>hi</p>\n")
        self.commit("change")
        self.assertEqual(vc.problems("main"), [])


if __name__ == "__main__":
    unittest.main()
