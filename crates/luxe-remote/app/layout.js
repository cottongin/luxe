// @ts-check
// Whether the dock shows, and on which side: started from your config, which the bridge writes on
// the page, as the TUI starts from it. `/dock` and the header's button change them while the app
// is open, as `/dock` does in the TUI. On a wide screen the dock is a rail beside the conversation;
// on a phone it's the project panel's first tab, behind the header's one button.

import { signal } from "@preact/signals";
import { useEffect, useState } from "preact/hooks";

/**
 * @typedef {"auto" | "shown" | "hidden"} DockMode
 * @typedef {"dock" | "files" | "github" | "review"} Tab
 */

/** The dock, plan and changes both as on the rail, as a tab of the project panel on a phone. */
const DOCK_TABS = /** @type {{ id: Tab, title: string }[]} */ ([{ id: "dock", title: "Dock" }]);

/** The project's views, the panel's tabs on every screen. */
const PROJECT_TABS = /** @type {{ id: Tab, title: string }[]} */ ([
  { id: "files", title: "Files" }, { id: "github", title: "GitHub" }, { id: "review", title: "Review" },
]);

/**
 * The project panel's tabs. A phone has one header button for everything, so the dock comes first
 * there; a wide screen keeps the dock as its own rail.
 * @param {boolean} wide
 */
export function panelTabs(wide) {
  return wide ? PROJECT_TABS : [...DOCK_TABS, ...PROJECT_TABS];
}

/**
 * The tab the panel shows: `tab` when this screen has it, else the screen's first. `tab` can be
 * one the screen lacks after an iPad turns or a Duo opens with the panel on the dock.
 * @param {Tab | null} tab @param {boolean} wide @returns {Tab}
 */
export function shownTab(tab, wide) {
  const tabs = panelTabs(wide);
  return tabs.find(({ id }) => id === tab)?.id ?? tabs[0].id;
}

/**
 * What a `/dock` verb does on a phone, where the dock is the panel's Dock tab: open the panel on
 * it, close it, or null for the verbs that set the dock's own mode or side.
 * @param {string} verb @param {Tab | null} tab The panel's tab, null when it's closed
 * @returns {"open" | "close" | null}
 */
export function phoneDock(verb, tab) {
  if (verb === "hide" || (verb === "toggle" && tab === "dock")) return "close";
  return verb === "show" || verb === "toggle" ? "open" : null;
}

/** Wide and tall enough for the dock beside the conversation; dock.css's breakpoint. */
export const WIDE = "(min-width: 50rem) and (min-height: 30rem)";

/** The verbs `/dock` takes, as the TUI's does; a bare `/dock` toggles. */
export const DOCK_VERBS = ["show", "hide", "auto", "left", "right"];

/** The mode the page was served with, auto when it says nothing known. @param {DOMStringMap} written @returns {DockMode} */
export function startingMode(written) {
  return written.dockMode === "shown" || written.dockMode === "hidden" ? written.dockMode : "auto";
}

/** Whether the dock shows: always when shown, and on a wide screen when auto. @param {DockMode} mode @param {boolean} wide */
export function dockVisible(mode, wide) {
  return mode === "shown" || (mode === "auto" && wide);
}

/**
 * The mode after `verb`: "toggle" flips what's on screen, the rest are `/dock`'s. Null for a verb
 * that doesn't change the mode.
 * @param {DockMode} mode @param {string} verb @param {boolean} wide @returns {DockMode | null}
 */
export function dockAfter(mode, verb, wide) {
  if (verb === "toggle") return dockVisible(mode, wide) ? "hidden" : "shown";
  if (verb === "show") return "shown";
  if (verb === "hide") return "hidden";
  return verb === "auto" ? "auto" : null;
}

/**
 * The dock's mode for the app, and `apply` for a verb. The side lives on the page's own
 * `data-dock-side`, which the stylesheet reads.
 * @param {HTMLElement} page @param {() => boolean} wide
 */
export function createDock(page, wide) {
  const mode = signal(startingMode(page.dataset));
  /** @param {string} verb */
  const apply = (verb) => {
    if (verb === "left" || verb === "right") return void (page.dataset.dockSide = verb);
    const next = dockAfter(mode.value, verb, wide());
    if (next) mode.value = next;
  };
  return { mode, apply };
}

/** @type {ReturnType<typeof createDock> | undefined} */
let appDock;

/** The one dock the app's sessions share, made on first use. */
export function theDock() {
  appDock ??= createDock(document.documentElement, () => matchMedia(WIDE).matches);
  return appDock;
}

/** Whether the screen is wide, following a rotation or a resized window. */
export function useWide() {
  const [wide, setWide] = useState(() => matchMedia(WIDE).matches);
  useEffect(() => {
    const query = matchMedia(WIDE);
    const change = () => setWide(query.matches);
    query.addEventListener("change", change);
    return () => query.removeEventListener("change", change);
  }, []);
  return wide;
}
