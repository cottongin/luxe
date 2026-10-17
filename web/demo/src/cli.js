#!/usr/bin/env node
/** todo — a task list small enough to read in one sitting. */

import { dueBy, open, tagged } from "./filter.js";
import { load, make, save } from "./store.js";

const USAGE = `todo add <title> [--due DATE] [--tag TAG]
todo list [--tag TAG]
todo due <DATE>
todo done <title>`;

function show(tasks) {
  if (tasks.length === 0) {
    console.log("nothing here");
    return;
  }
  for (const task of tasks) {
    const mark = task.done ? "x" : " ";
    const due = task.due ? `  due ${task.due}` : "";
    console.log(`[${mark}] ${task.title}${due}`);
  }
}

const [command, ...rest] = process.argv.slice(2);
const flag = (name) => {
  const at = rest.indexOf(`--${name}`);
  return at === -1 ? null : rest[at + 1];
};
const words = rest.filter((word, i) => !word.startsWith("--") && !rest[i - 1]?.startsWith("--"));
const tasks = await load();

switch (command) {
  case "add": {
    const tag = flag("tag");
    tasks.push(make(words.join(" "), { due: flag("due"), tags: tag ? [tag] : [] }));
    await save(tasks);
    show(tasks.slice(-1));
    break;
  }
  case "list": {
    const tag = flag("tag");
    show(tag ? tagged(open(tasks), [tag]) : open(tasks));
    break;
  }
  case "due":
    show(dueBy(open(tasks), words[0]));
    break;
  case "done": {
    const match = tasks.find((task) => task.title === words.join(" "));
    if (!match) {
      console.error(`no task called ${words.join(" ")}`);
      process.exit(1);
    }
    match.done = true;
    await save(tasks);
    show([match]);
    break;
  }
  default:
    console.log(USAGE);
}
