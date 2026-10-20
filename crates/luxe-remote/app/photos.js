// @ts-check
// Photos for the composer: from the camera or the library, made small enough to send.
//
// A phone's photo is 12 megapixels and several megabytes, and the model sees no more than about
// 1568 pixels on the longest side (Anthropic's advice; larger is scaled down on arrival), so it
// is scaled here, before it crosses the network, and sent as a JPEG.

import { useState } from "preact/hooks";

/** The longest side a photo is sent at. */
export const LONGEST_EDGE = 1568;

/** JPEG quality: text in a screenshot stays sharp, and a photo stays a few hundred kilobytes. */
const QUALITY = 0.85;

/**
 * `width` by `height` scaled to fit within `longest` on its longer side, never scaled up.
 * @param {number} width @param {number} height @param {number} longest
 */
export function fitWithin(width, height, longest = LONGEST_EDGE) {
  const scale = Math.min(1, longest / Math.max(width, height));
  return { width: Math.round(width * scale), height: Math.round(height * scale) };
}

/**
 * A photo as the engine takes it, plus a preview to show meanwhile.
 * @typedef {{ data: string, mime: string, name: string | null, preview: string }} Photo
 */

/**
 * `file` as an attachment the engine takes, plus a preview to show meanwhile.
 * @param {File} file @returns {Promise<Photo>}
 */
export async function attachment(file) {
  const bitmap = await createImageBitmap(file);
  const size = fitWithin(bitmap.width, bitmap.height);
  const canvas = document.createElement("canvas");
  Object.assign(canvas, size);
  const context = canvas.getContext("2d");
  if (!context) throw new Error("this browser can't prepare photos");
  // a JPEG has no transparency, and black is what a transparent screenshot would get otherwise
  context.fillStyle = "#fff";
  context.fillRect(0, 0, size.width, size.height);
  context.drawImage(bitmap, 0, 0, size.width, size.height);
  bitmap.close();
  const preview = canvas.toDataURL("image/jpeg", QUALITY);
  return { data: dataOf(preview), mime: "image/jpeg", name: file.name || null, preview };
}

/** The base64 part of a `data:` URL. @param {string} url */
export function dataOf(url) {
  return url.slice(url.indexOf(",") + 1);
}

/**
 * The attachments as `submit_user_message` takes them, without their previews.
 * @param {Photo[]} photos
 */
export function forTheWire(photos) {
  return photos.map(({ data, mime, name }) => ({ data, mime, name }));
}

/** The photos attached to the message being written. */
export function usePhotos() {
  const [list, setList] = useState(/** @type {Photo[]} */ ([]));
  const [problem, setProblem] = useState("");
  /** @param {FileList} files */
  const add = async (files) => {
    setProblem("");
    try {
      const made = await Promise.all([...files].map(attachment));
      setList((now) => [...now, ...made]);
    } catch (error) {
      setProblem(`couldn't use that photo: ${/** @type {Error} */ (error).message}`);
    }
  };
  /** @param {number} index */
  const remove = (index) => setList((now) => now.filter((_, at) => at !== index));
  return { list, problem, add, remove, clear: () => setList([]) };
}
