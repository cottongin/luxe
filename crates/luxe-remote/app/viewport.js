// @ts-check
// Fields share the page's text size, which is under the 16px at which iOS zooms into a focused
// field. maximum-scale stops that zoom on iOS, where pinching still zooms anyway; Android honors it
// as written and would stop pinching too, and it doesn't zoom on focus, so only iOS gets it.

/**
 * Whether this is an iPhone or iPad. An iPad asks for desktop pages, so it says it's a Mac, and
 * only the touch screen gives it away.
 * @param {{ userAgent: string, platform: string, maxTouchPoints: number }} device
 */
export function isAppleTouch(device) {
  if (/iPhone|iPad|iPod/.test(device.userAgent)) return true;
  return device.platform === "MacIntel" && device.maxTouchPoints > 1;
}

/** Keeps iOS from zooming into a focused field. @param {Document} page @param {Navigator} device */
export function holdFieldZoom(page = document, device = navigator) {
  if (!isAppleTouch(device)) return;
  const viewport = /** @type {HTMLMetaElement | null} */ (page.querySelector('meta[name="viewport"]'));
  if (!viewport || viewport.content.includes("maximum-scale")) return;
  viewport.content += ", maximum-scale=1";
}
