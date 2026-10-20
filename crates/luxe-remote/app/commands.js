// @ts-check
// The slash commands the phone can carry out: the TUI's commands that the protocol has an action
// for, and `/dock`, which works the app's own screen as the TUI's works the terminal's. Named and
// worded as the TUI's are. The rest work the terminal's own screen, so they aren't offered, and a
// message that only looks like one goes to the agent as written.

import { DOCK_VERBS } from "./layout.js";
import { nextModeFrame } from "./status.js";

/** @import { Frame } from "./connection.js" */
/**
 * A command the picker offers. One that `takes` words goes into the box to finish; one that
 * doesn't runs when picked.
 * @typedef {{ name: string, title: string, takes?: string }} Command
 * @typedef {{ frame: Frame } | { details: true } | { dock: string } | null} CommandAction
 * @typedef {{ send: (frame: Frame) => boolean, details: () => void, dock: (verb: string) => void }} CommandHooks
 */

/** @type {Command[]} */
export const COMMANDS = [
  { name: "compact", title: "Compact the conversation", takes: "what the summary should keep" },
  { name: "cost", title: "Session cost" },
  { name: "dock", title: "Context Dock", takes: "show, hide, auto, left or right" },
  { name: "mode", title: "Switch mode", takes: "plan or build" },
  { name: "model", title: "Change model" },
  { name: "thinking", title: "Set thinking level" },
];

/** The commands `text` could be the start of: a slash and part of a name, nothing after. @param {string} text */
export function matchCommands(text) {
  const typed = /^\/(\S*)$/.exec(text);
  if (!typed) return [];
  return COMMANDS.filter((command) => command.name.startsWith(typed[1].toLowerCase()));
}

/**
 * What sending `text` does when it's one of the commands: a frame for the session, or the details
 * sheet. Null for a message, which includes a known command given words it doesn't take.
 * @param {string} text @param {string} mode The mode now, which a bare `/mode` moves on from
 * @returns {CommandAction}
 */
export function commandAction(text, mode) {
  const typed = /^\/([a-z]+)(?:\s+([\s\S]*))?$/i.exec(text.trim());
  if (!typed) return null;
  const [, name, words = ""] = typed;
  // a phone's keyboard capitalizes the start of a message
  switch (name.toLowerCase()) {
    case "compact": return { frame: words ? { kind: "compact_now", content: { focus: words } } : { kind: "compact_now" } };
    case "mode": return modeAction(words, mode);
    case "dock": return dockAction(words.toLowerCase());
    case "cost": case "model": case "thinking": return words ? null : { details: true };
  }
  return null;
}

/**
 * Carries out a command: null when `action` isn't one, else whether it went out.
 * @param {CommandAction} action @param {CommandHooks} hooks
 */
export function runCommand(action, { send, details, dock }) {
  if (!action) return null;
  if ("details" in action) return (details(), true);
  if ("dock" in action) return (dock(action.dock), true);
  return send(action.frame);
}

/** A bare `/dock` toggles, as the TUI's does. @param {string} words @returns {CommandAction} */
function dockAction(words) {
  if (!words) return { dock: "toggle" };
  return DOCK_VERBS.includes(words) ? { dock: words } : null;
}

/** @param {string} words @param {string} mode @returns {CommandAction} */
function modeAction(words, mode) {
  if (!words) return { frame: nextModeFrame(mode) };
  return words === "plan" || words === "build" ? { frame: { kind: "set_mode", content: words } } : null;
}
