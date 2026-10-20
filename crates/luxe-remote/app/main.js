// @ts-check
import { render } from "preact";
import { html } from "./html.js";
import { App } from "./app.js";
import { startWorker } from "./notifications.js";
import { holdFieldZoom } from "./viewport.js";

holdFieldZoom();
startWorker();
const root = document.getElementById("app");
if (root) render(html`<${App} />`, root);
