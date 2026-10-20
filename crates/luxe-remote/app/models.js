// @ts-check
// The models a session can switch to, asked for each time the details sheet opens and answered to
// this device alone. An older luxe doesn't know the question and refuses it, so the sheet says so
// rather than waiting for an answer that won't come.

import { signal } from "@preact/signals";

/** @import { Frame } from "./connection.js" */
/**
 * One model `list_models` offers, as `models_listed` carries it.
 * @typedef {{ provider: string, model: string, name: string, thinking_levels: string[] }} ModelChoice
 * @typedef {"idle" | "asking" | "listed" | "unsupported" | "lost"} ListingState
 * @typedef {{ state: ListingState, models: ModelChoice[] }} Listing
 * @typedef {{ provider?: string, model?: string }} CurrentModel
 */

export function createModels() {
  const listing = signal(/** @type {Listing} */ ({ state: "idle", models: [] }));
  let asked = "";
  let count = 0;
  /** Asks again, keeping the last answer on screen meanwhile. @param {(frame: Frame) => boolean} send */
  const ask = (send) => {
    asked = `models-${++count}`;
    const sent = send({ kind: "list_models", content: { request_id: asked } });
    listing.value = { ...listing.value, state: sent ? "asking" : "lost" };
  };
  /** Takes the answer to the latest question, or a refusal of the question itself. @param {Frame} frame */
  const apply = ({ kind, content }) => {
    if (kind === "models_listed" && content.request_id === asked) listing.value = { state: "listed", models: content.models };
    if (kind === "action_rejected" && content.kind === "list_models") listing.value = { state: "unsupported", models: [] };
  };
  return { listing, ask, apply };
}

/** Whether `choice` is the model running now. @param {ModelChoice} choice @param {CurrentModel | undefined} current */
export function isCurrent(choice, current) {
  return choice.provider === current?.provider && choice.model === current?.model;
}

/**
 * The thinking levels the running model honors; none until it's found in the listing, since a
 * level it doesn't honor would be clamped without a word.
 * @param {ModelChoice[]} models @param {CurrentModel | undefined} current
 */
export function thinkingLevels(models, current) {
  return models.find((choice) => isCurrent(choice, current))?.thinking_levels ?? [];
}

/** @param {ModelChoice} choice @returns {Frame} */
export function selectModelFrame(choice) {
  return { kind: "select_model", content: { provider: choice.provider, model: choice.model } };
}

/** @param {string} level @returns {Frame} */
export function thinkingFrame(level) {
  return { kind: "set_thinking_level", content: level };
}
