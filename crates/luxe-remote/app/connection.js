// One session's socket, and what to do when it closes.
//
// How it closed says what next: 1000 is the session ending, 4001 this device being unpaired, and
// anything else a dropped connection, the one case worth retrying. A drop can also hide one of
// the other two (the upgrade is refused before a close code can say so), so a retry first asks
// the session list.
//
// The socket is closed while the app is out of sight and opened again on the way back, which iOS
// would mostly do anyway: an open socket is how the bridge knows you're looking at a session, and
// so whether to send this device a notification about it instead.

const UNPAIRED = 4001;
const LONGEST_WAIT_MS = 15000;

/**
 * One protocol message, either way: an event from the session or an action for it.
 * @typedef {{ kind: string, content?: any }} Frame
 * @typedef {"connecting" | "live" | "reconnecting" | "ended" | "unpaired"} Status
 * @typedef {(frame: Frame) => void} OnFrame
 * @typedef {(status: Status) => void} OnStatus
 * @typedef {{
 *   socket: WebSocket | null, attempts: number, timer: ReturnType<typeof setTimeout> | undefined,
 *   stopped: boolean,
 * }} Link
 */

/**
 * @param {string} sessionId
 * @param {{ onFrame: OnFrame, onStatus: OnStatus }} callbacks
 */
export function connect(sessionId, { onFrame, onStatus }) {
  /** @type {Link} */
  const link = { socket: null, attempts: 0, timer: undefined, stopped: false };
  const open = () => {
    if (document.visibilityState === "hidden") return;
    openSocket(link, sessionId, onFrame, onStatus, () => retry(link, sessionId, open, onStatus));
  };
  const resume = () => {
    if (link.stopped || link.socket) return;
    clearTimeout(link.timer);
    open();
  };
  const seen = () => (document.visibilityState === "visible" ? resume() : rest(link));
  document.addEventListener("visibilitychange", seen);
  open();
  return controls(link, seen);
}

/**
 * Closes the socket while the app is out of sight. Its handlers go first, so the close isn't taken
 * for the session ending and a return before it finishes opens a new one.
 * @param {Link} link
 */
function rest(link) {
  clearTimeout(link.timer);
  const socket = link.socket;
  if (!socket) return;
  link.socket = null;
  socket.onclose = null;
  socket.onmessage = null;
  socket.close(1000);
}

/** @param {Link} link @param {() => void} seen */
function controls(link, seen) {
  return {
    /**
     * Whether `frame` went out: false while the socket is down, so nothing typed is lost.
     * @param {Frame} frame
     */
    send(frame) {
      if (link.socket?.readyState !== WebSocket.OPEN) return false;
      link.socket.send(JSON.stringify(frame));
      return true;
    },
    close() {
      link.stopped = true;
      clearTimeout(link.timer);
      document.removeEventListener("visibilitychange", seen);
      link.socket?.close(1000);
    },
  };
}

/**
 * @param {Link} link @param {string} sessionId @param {OnFrame} onFrame @param {OnStatus} onStatus
 * @param {() => void} onDrop
 */
function openSocket(link, sessionId, onFrame, onStatus, onDrop) {
  const scheme = location.protocol === "https:" ? "wss" : "ws";
  const socket = new WebSocket(`${scheme}://${location.host}/api/sessions/${encodeURIComponent(sessionId)}/socket`);
  link.socket = socket;
  onStatus(link.attempts ? "reconnecting" : "connecting");
  listen(socket, link, onFrame, onStatus, onDrop);
}

/**
 * @param {WebSocket} socket @param {Link} link @param {OnFrame} onFrame @param {OnStatus} onStatus
 * @param {() => void} onDrop
 */
function listen(socket, link, onFrame, onStatus, onDrop) {
  socket.onopen = () => {
    link.attempts = 0;
    onStatus("live");
  };
  socket.onmessage = (event) => {
    const frame = parse(event.data);
    if (frame) onFrame(frame);
  };
  socket.onclose = (event) => {
    link.socket = null;
    if (link.stopped) return;
    if (event.code === 1000) return onStatus("ended");
    if (event.code === UNPAIRED) return onStatus("unpaired");
    onDrop();
  };
}

/** @param {Link} link @param {string} sessionId @param {() => void} open @param {OnStatus} onStatus */
async function retry(link, sessionId, open, onStatus) {
  link.attempts += 1;
  onStatus("reconnecting");
  const verdict = await stillThere(sessionId);
  if (link.stopped) return;
  if (verdict !== "yes") return onStatus(verdict);
  const wait = Math.min(LONGEST_WAIT_MS, 1000 * 2 ** (link.attempts - 1));
  link.timer = setTimeout(open, wait);
}

/** @param {string} sessionId @returns {Promise<"yes" | "ended" | "unpaired">} */
async function stillThere(sessionId) {
  try {
    const response = await fetch("/api/sessions");
    if (response.status === 401 || response.status === 403) return "unpaired";
    if (!response.ok) return "yes";
    /** @type {{ session_id: string }[]} */
    const sessions = await response.json();
    return sessions.some((session) => session.session_id === sessionId) ? "yes" : "ended";
  } catch {
    // Offline: the list can't say either way, so keep trying
    return "yes";
  }
}

/** @param {string} text @returns {Frame | null} */
function parse(text) {
  try {
    return JSON.parse(text);
  } catch {
    console.warn("ignoring a frame that isn't JSON", text.slice(0, 80));
    return null;
  }
}
