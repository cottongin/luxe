// The libraries installed here only lend their type declarations to the check of the vendored
// files the phone runs, so the two have to be the same versions.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const read = (path) => JSON.parse(readFileSync(new URL(path, import.meta.url), "utf8"));

// The break this catches: `scripts/remote-vendor.sh` moving Preact on while package.json stays
// behind (or a bump here alone), so the app is checked against an API it doesn't run
test("the type packages are the vendored versions", () => {
  const installed = read("package.json").devDependencies;
  for (const { package: name, version } of read("../app/vendor/manifest.json")) {
    assert.equal(installed[name], version, `${name} in package.json`);
  }
});
