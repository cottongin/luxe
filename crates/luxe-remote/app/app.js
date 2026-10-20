// @ts-check
// The screens, picked by the address's hash so the back gesture and a home-screen reload
// land where you were.

import { signal } from "@preact/signals";
import { NewSession } from "./folders.js";
import { html } from "./html.js";
import { SessionList } from "./sessions.js";
import { SessionView } from "./session.js";

const route = signal(location.hash);
addEventListener("hashchange", () => (route.value = location.hash));

export function App() {
  if (route.value === "#/new") return html`<${NewSession} />`;
  const match = /^#\/session\/(.+)$/.exec(route.value);
  if (!match) return html`<${SessionList} />`;
  const sessionId = decodeURIComponent(match[1]);
  return html`<${SessionView} key=${sessionId} sessionId=${sessionId} />`;
}
