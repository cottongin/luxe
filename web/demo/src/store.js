/** Reading and writing the task file. */

import { readFile, writeFile } from "node:fs/promises";

// `TODO_FILE` points it elsewhere, so trying the CLI never touches the real list
const FILE = process.env.TODO_FILE ?? "tasks.json";

/** Every task on disk, or an empty list the first time. */
export async function load() {
  try {
    return JSON.parse(await readFile(FILE, "utf8"));
  } catch (error) {
    if (error.code === "ENOENT") return [];
    throw error;
  }
}

/** Replaces the file. Callers own the whole list, so there is nothing to merge. */
export async function save(tasks) {
  await writeFile(FILE, `${JSON.stringify(tasks, null, 2)}\n`);
}

/** A new task, with today's date and no tags unless asked for. */
export function make(title, { due = null, tags = [] } = {}) {
  return { title, due, done: false, added: new Date().toISOString().slice(0, 10), tags };
}
