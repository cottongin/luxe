// `node --import ./register.mjs --test`: resolves the app's bare imports through index.html's
// import map, as the browser does, so the tests run the vendored files the phone runs rather than
// whatever npm would find.

import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";

const app = new URL("../app/", import.meta.url);
const page = readFileSync(new URL("index.html", app), "utf8");
const block = /<script type="importmap">([\s\S]*?)<\/script>/.exec(page);
if (!block) throw new Error("index.html has no import map");
const imports = JSON.parse(block[1]).imports;

registerHooks({
  resolve(specifier, context, next) {
    const mapped = imports[specifier];
    if (!mapped) return next(specifier, context);
    return next(new URL(mapped.replace(/^\/app\//, ""), app).href, context);
  },
});
