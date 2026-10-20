// A Luxe of the test's own: a session in a fresh git project, driven by the mock model, and a
// bridge serving it, all under one temporary directory, so nothing touches the real config.

import { spawn, execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

const REPO = resolve(dirname(fileURLToPath(import.meta.url)), "../../../..");

/** The tailnet login the bridge lets in, sent as Tailscale Serve would send it. */
export const LOGIN = "e2e@luxe.test";

/** The `luxe` to test: `LUXE_BIN`, or the debug build. */
const LUXE = process.env.LUXE_BIN ?? join(REPO, "target/debug/luxe");

/** Starts a session and a bridge for the mock model on `modelPort`. */
export async function startHost(modelPort) {
  const root = mkdtempSync(join(tmpdir(), "luxe-e2e-"));
  const env = isolatedEnv(root);
  const project = gitProject(root);
  writeConfig(env.LUXE_CONFIG_DIR, modelPort);
  const children = [];
  const host = { root, project, children, stop: () => stop(host) };
  host.sessionId = await startSession(env, project, children);
  host.invitation = invite(env);
  host.url = await startBridge(env, children);
  return host;
}

function isolatedEnv(root) {
  const dirs = { LUXE_CONFIG_DIR: "config", LUXE_STATE_DIR: "state", LUXE_RUNTIME_DIR: "run", XDG_CACHE_HOME: "cache" };
  /** @type {NodeJS.ProcessEnv} */
  const env = { ...process.env, LUXE_MODELS_DEV_REFRESH: "0", HOME: join(root, "home") };
  for (const [name, dir] of Object.entries(dirs)) mkdirSync((env[name] = join(root, dir)), { recursive: true });
  return env;
}

function gitProject(root) {
  const project = join(root, "project");
  mkdirSync(project);
  const git = (...args) => execFileSync("git", ["-c", "user.name=e2e", "-c", "user.email=e2e@luxe.test", ...args], { cwd: project });
  git("init", "-q");
  // a few lines, so a phone can pick a range of them and select part of one
  writeFileSync(join(project, "README.md"), "# e2e\n\nThe port is 4519.\nThe host is luxe.local.\n");
  git("add", ".");
  git("commit", "-qm", "start");
  return project;
}

// The mock model, and a rule that stops its write for a permission the phone has to grant
function writeConfig(configDir, port) {
  const models = {
    providers: { mock: { base_url: `http://127.0.0.1:${port}/v1`, api_key: "unused", api: "openai-completions" } },
    models: { "mock/scripted": { name: "Scripted", context_window: 32000, max_tokens: 4000 } },
  };
  writeFileSync(join(configDir, "models.json"), JSON.stringify(models));
  writeFileSync(join(configDir, "permissions.json"), JSON.stringify({ permission: { edit: "ask" } }));
}

// `--rpc` keeps the session while its stdin is open, and its `ready` event names it. Its events
// keep being read after that, so a full pipe never stalls it.
function startSession(env, project, children) {
  const luxe = spawn(LUXE, ["--rpc", "--model", "mock/scripted"], { cwd: project, env, stdio: ["pipe", "pipe", "inherit"] });
  children.push(luxe);
  return new Promise((ready, failed) => {
    createInterface({ input: luxe.stdout }).on("line", (line) => {
      const event = JSON.parse(line);
      if (event.kind === "ready") ready(event.content.session_id);
    });
    luxe.once("exit", (code) => failed(new Error(`luxe --rpc exited (${code}) before it was ready`)));
  });
}

// The token is the link's fragment, or printed on its own where Tailscale can't name the machine
function invite(env) {
  const out = execFileSync(LUXE, ["remote", "pair"], { env, encoding: "utf8" });
  const token = /#([\w-]+)/.exec(out)?.[1];
  if (!token) throw new Error(`no invitation in: ${out}`);
  return token;
}

async function startBridge(env, children) {
  const port = await freePort();
  const bridge = spawn(LUXE, ["remote", "--port", String(port), "--allow", LOGIN], { env, stdio: "inherit" });
  children.push(bridge);
  const url = `http://127.0.0.1:${port}`;
  await until(() => fetch(url).then(() => true, () => false), "the bridge to listen");
  return url;
}

function freePort() {
  return new Promise((done) => {
    const probe = createServer().listen(0, "127.0.0.1", () => {
      const { port } = /** @type {import("node:net").AddressInfo} */ (probe.address());
      probe.close(() => done(port));
    });
  });
}

/** Polls `check` until it's true, for at most ten seconds. */
export async function until(check, what) {
  for (let tries = 0; tries < 100; tries += 1) {
    if (await check()) return;
    await new Promise((done) => setTimeout(done, 100));
  }
  throw new Error(`timed out waiting for ${what}`);
}

async function stop(host) {
  for (const child of host.children) child.kill();
  await Promise.all(host.children.map((child) => child.exitCode ?? new Promise((done) => child.once("exit", done))));
  rmSync(host.root, { recursive: true, force: true });
}
