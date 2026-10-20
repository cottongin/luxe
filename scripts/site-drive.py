#!/usr/bin/env python3
"""Drive a real luxe session from a script, and record what the terminal saw.

Run *inside* a terminal, with the app under a pseudo-terminal this process owns:

    ghostty -e scripts/site-drive.py --script demo.rec --out raw.cast -- luxe

Two rules shaped it.

**Keystrokes go into the pty, never to the compositor.** The obvious way to drive a TUI on a
desktop is to synthesize key events with `wtype`, and it works until focus moves — after which
every remaining keystroke, `Ctrl+C` included, is typed into whatever window took it. Writing to the
pty cannot reach another window at all, and it does not need the recording to hold focus.

**The relay is transparent both ways.** luxe asks the terminal what it can do — cursor position,
device attributes, the glyph probe — and a driver that answers none of those hangs on the first
question. So the app's output goes to our stdout (the real terminal, which answers) and the real
terminal's replies go back into the pty. Only the outbound half is recorded; the terminal's answers
are not something a viewer ever saw.

The output is asciicast v2, which `scripts/cast-edit.py` then cuts down to a demo.

Steps, one per line, `#` comments allowed:

    sleep <seconds>       wait
    type <text>           type literal text at a readable cadence
    enter [text]          type text (optionally none) then Return
    key <name>[ xN]       press a named key N times (Return, Escape, Down, PageUp, F5, …)
    mod <mods> <name>     a chord: mods among ctrl/alt/shift joined by `+`
    send <escaped>        raw bytes, Python escapes (`\\x1b[27;5u`) — the escape hatch
    settle <secs> [max N] wait until the app has drawn nothing for <secs>
    expect <text> [max N] wait until <text> appears in what the app has drawn
    answer <key> [hold N] when <text>
                          from now on, press <key> (or chord, `ctrl+g`) whenever <text> appears
    mark <name>           record a timestamp; `start` and `end` bound the finished cast
    log <text>            a note in the driver log

`expect` and `answer` match against the *screen*, reconstructed with `pyte`, not against the byte
stream. Ratatui writes only the cells that changed and moves the cursor between them, so a phrase
that is plainly on screen — `review ready · Ctrl+G opens it` — is essentially never contiguous in
what the terminal received. Whitespace is collapsed before matching, so a needle does not have to
know how a surface pads its columns.

Prefer `settle` over `sleep` for anything that waits on a model. A turn is finished when the
screen stops changing, and a fixed sleep cannot tell that apart from a session that is never going
to answer — which is how a take ends up as a recording of a hung terminal nobody was watching.

Two things `settle` cannot see, both worth knowing before writing a script:

- **An animation is not stillness.** The startup wordmark and the working indicator redraw
  continuously, so a `settle` across either spends its whole budget. Use `sleep` on the dashboard
  and `expect` for anything with a spinner in front of it.
- **A modal is stillness.** A permission prompt waiting for a person looks exactly like a finished
  turn, except that the turn behind it never ends. `answer` exists for that: it presses the key
  every time the prompt appears, so a take cannot stall on a dialog nobody is there to dismiss.
"""

from __future__ import annotations

import argparse
import codecs
import errno
import fcntl
import json
import os
import pathlib
import pty
import random
import re
import select

import pyte
import signal
import struct
import sys
import termios
import threading
import time
import tty

# Typing, at reading speed rather than at typing speed. A prompt entered at 40ms a character
# replays faster than anyone can read a sentence they are meeting for the first time, which is the
# commonest reason a terminal recording is unwatchable. With the jitter and pauses below this
# averages about 60ms a character; it was 80, which read as someone typing carefully rather than
# as someone who knows what they want.
CADENCE = 0.046
PUNCTUATION_BEAT = 0.15
PUNCTUATION = ",.;:?!"

# Nobody types on a metronome, and a perfectly even cadence is the tell that gives a scripted demo
# away — it reads as output being printed rather than as somebody at a keyboard. So each keystroke
# is scaled by a factor in this range, and now and then the typist stops to think.
JITTER = (0.6, 1.6)
HESITATION_ODDS = 0.025
HESITATION = (0.2, 0.55)
# Fingers are faster inside a word than across one.
SPACE_DRAG = 1.45

# === Keys ===

# Named keys the legacy encoding can express on their own.
LEGACY = {
    "return": "\r",
    "enter": "\r",
    "escape": "\x1b",
    "esc": "\x1b",
    "tab": "\t",
    "backspace": "\x7f",
    "space": " ",
    "up": "\x1b[A",
    "down": "\x1b[B",
    "right": "\x1b[C",
    "left": "\x1b[D",
    "home": "\x1b[H",
    "end": "\x1b[F",
    "pageup": "\x1b[5~",
    "pagedown": "\x1b[6~",
    "delete": "\x1b[3~",
    "insert": "\x1b[2~",
    **{f"f{n}": f"\x1b[{10 + n}~" for n in range(5, 13)},
    **dict(zip(("f1", "f2", "f3", "f4"), ("\x1bOP", "\x1bOQ", "\x1bOR", "\x1bOS"))),
}

# The same keys as Unicode codepoints, for the CSI-u form. luxe negotiates the Kitty keyboard
# protocol, so a chord the legacy encoding cannot spell — `Ctrl+Esc`, which is how a turn is
# interrupted — has a representation here rather than being unreachable.
CSI_U_CODE = {
    "return": 13,
    "enter": 13,
    "escape": 27,
    "esc": 27,
    "tab": 9,
    "backspace": 127,
    "space": 32,
}
CSI_U_TILDE = {"up": "A", "down": "B", "right": "C", "left": "D", "home": "H", "end": "F"}
MODIFIER_BIT = {"shift": 1, "alt": 2, "ctrl": 4}


def chord(mods: list[str], name: str) -> str:
    """Encodes a modified key press.

    Prefers the legacy encoding, because every terminal and every parser understands it, and falls
    back to CSI-u only where legacy has no spelling for the combination.
    """
    name = name.lower()
    bits = 0
    for mod in mods:
        if mod not in MODIFIER_BIT:
            sys.exit(f"unknown modifier {mod!r}")
        bits |= MODIFIER_BIT[mod]

    # Ctrl with a plain letter is a control character, which is what a terminal has always sent.
    if bits == MODIFIER_BIT["ctrl"] and len(name) == 1 and name.isalpha():
        return chr(ord(name.upper()) & 0x1F)
    if bits == MODIFIER_BIT["alt"] and len(name) == 1:
        return "\x1b" + name
    if bits == (MODIFIER_BIT["alt"] | MODIFIER_BIT["ctrl"]) and len(name) == 1 and name.isalpha():
        return "\x1b" + chr(ord(name.upper()) & 0x1F)

    parameter = bits + 1
    if name in CSI_U_TILDE:
        return f"\x1b[1;{parameter}{CSI_U_TILDE[name]}"
    code = CSI_U_CODE.get(name) or (ord(name) if len(name) == 1 else None)
    if code is None:
        sys.exit(f"no encoding for {'+'.join(mods)}+{name}")
    return f"\x1b[{code};{parameter}u"


def named(name: str) -> str:
    """Encodes an unmodified key press."""
    key = LEGACY.get(name.lower())
    if key is None:
        sys.exit(f"unknown key {name!r}")
    return key


def press(token: str) -> str:
    """Encodes one key from a script token: `Down`, `ctrl+g`, or a bare character."""
    if "+" in token:
        *mods, name = token.split("+")
        return chord(mods, name)
    return named(token) if len(token) > 1 else token


# === What may come back in ===

# CSI sequences that are answers to a question the app asked. Everything else arriving from the
# outer terminal is a person, and a person is not part of the take.
REPLY_FINALS = frozenset("Rcnut")

# Everything that is not a printable cell, for `expect`.
ESCAPES = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b[P\]_^].*?(?:\x1b\\|\x07)|\x1b[=>()][A-Za-z0-9]?", re.S)


def device_replies(buffer: str) -> tuple[str, str]:
    """Splits inbound bytes into the device reports worth forwarding and an unparsed remainder.

    The app's own escape sequences reach the real terminal through our stdout, so the terminal
    duly turns on mouse tracking and focus reporting and starts reporting *the operator* back at
    us. Relayed, that lands a stray pointer move in the middle of a take — which is how two
    recordings came out as a corrupted screen when a mouse crossed the window.

    So the inbound half carries exactly what it exists for: cursor position reports, device
    attributes, the keyboard-protocol flags, and the string replies (DCS/OSC/APC). Keystrokes,
    mouse events and focus changes are dropped.
    """
    out: list[str] = []
    i = 0
    while i < len(buffer):
        if buffer[i] != "\x1b":
            i += 1  # a bare keystroke
            continue
        rest = buffer[i:]
        if len(rest) < 2:
            return "".join(out), rest  # an ESC that may yet become a sequence

        kind = rest[1]
        if kind == "[":
            match = re.match(r"\x1b\[[\x30-\x3f]*[\x20-\x2f]*([\x40-\x7e])", rest)
            if not match:
                return "".join(out), rest
            if match.group(1) in REPLY_FINALS and not rest.startswith("\x1b[<"):
                out.append(match.group(0))
            i += match.end()
            continue
        if kind in "P]_^":
            match = re.match(r"\x1b[P\]_^].*?(?:\x1b\\|\x07)", rest, re.S)
            if not match:
                return "".join(out), rest
            out.append(match.group(0))
            i += match.end()
            continue
        i += 2  # ESC-something we did not ask for
    return "".join(out), ""


# === The session ===


class Session:
    """A pty running the app, a recording of what it drew, and a way to type at it."""

    def __init__(
        self,
        command: list[str],
        cols: int,
        rows: int,
        cwd: str,
        env: dict[str, str],
        seed: int = 0,
    ):
        # Seeded, so the unevenness is a property of the take rather than of the run. Re-recording
        # a demo after a visual change should differ in what the model said, not in how the prompt
        # was typed.
        self.random = random.Random(seed)
        self.events: list[tuple[float, str]] = []
        self.marks: dict[str, float] = {}
        self.cols, self.rows = cols, rows
        self.started = time.time()
        self.alive = True
        self.last_output = time.time()
        self._lock = threading.Lock()
        # A model of what is actually on screen. Cheap enough to keep current — 200k characters of
        # redraw costs about a tenth of a second — and the only thing a script can reliably wait
        # on, because the byte stream carries cell updates rather than sentences.
        self._screen = pyte.Screen(cols, rows)
        self._stream = pyte.Stream(self._screen)
        # Incremental, and that matters. A read returns whatever bytes happened to be available, so
        # a multi-byte character — every box-drawing rule and every Nerd Font glyph luxe draws — can
        # be split across two of them. Decoding each read on its own turns both halves into U+FFFD,
        # which is how stray `�` diamonds ended up scattered through otherwise good takes. This
        # holds the incomplete tail until the rest of it arrives.
        self._decoder = codecs.getincrementaldecoder("utf8")("replace")

        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(cwd)
            os.execvpe(command[0], command, env)
        # The app is told the size we want the recording to be, not the size of the window this is
        # running in. That is the whole reason the driver owns the pty: a demo's column count is
        # its type size on the page, and it cannot be left to whatever the compositor handed the
        # terminal.
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

    def relay(self) -> None:
        """Pumps both directions until the app exits. Runs on the main thread.

        Our own stdin has to be raw for the duration. The app's questions to the terminal come back
        as ordinary input, and a cooked line discipline holds them until a newline that a cursor
        position report never contains — so the app asks, the answer sits in a buffer, and the
        launch dies on `the cursor position could not be read`.
        """
        stdin = sys.stdin.fileno()
        try:
            saved = termios.tcgetattr(stdin)
            tty.setraw(stdin)
        except termios.error:  # not a terminal — nothing to answer with, and nothing to restore
            saved = None
        pending = ""
        while self.alive:
            try:
                readable, _, _ = select.select([self.fd, stdin], [], [], 0.05)
            except (OSError, select.error):
                break
            if self.fd in readable:
                try:
                    data = os.read(self.fd, 65536)
                except OSError as error:
                    if error.errno == errno.EIO:  # the app closed its end
                        break
                    raise
                if not data:
                    break
                text = self._decoder.decode(data)
                if not text:
                    continue
                with self._lock:
                    self.events.append((time.time() - self.started, text))
                    self.last_output = time.time()
                    self._stream.feed(text)
                sys.stdout.write(text)
                sys.stdout.flush()
            # The terminal's answers to the app's questions. Relayed, never recorded: nobody
            # watching the demo ever saw them.
            if stdin in readable:
                try:
                    inbound = os.read(stdin, 4096)
                except OSError:
                    inbound = b""
                if inbound:
                    pending += inbound.decode("utf8", "replace")
                    replies, pending = device_replies(pending)
                    if replies:
                        os.write(self.fd, replies.encode("utf8"))
        self.alive = False
        if saved is not None:
            termios.tcsetattr(stdin, termios.TCSADRAIN, saved)

    def send(self, text: str) -> None:
        if self.alive:
            try:
                os.write(self.fd, text.encode("utf8"))
            except OSError:
                self.alive = False

    def type_text(self, text: str) -> None:
        for char in text:
            self.send(char)
            delay = CADENCE * self.random.uniform(*JITTER)
            if char == " ":
                delay *= SPACE_DRAG
            time.sleep(delay)
            if char in PUNCTUATION:
                time.sleep(PUNCTUATION_BEAT * self.random.uniform(*JITTER))
            elif self.random.random() < HESITATION_ODDS:
                time.sleep(self.random.uniform(*HESITATION))

    def mark(self, name: str) -> None:
        self.marks[name] = time.time() - self.started

    def screen_text(self) -> str:
        """What is on screen now, with runs of whitespace collapsed."""
        with self._lock:
            lines = self._screen.display
        return re.sub(r"\s+", " ", " ".join(lines))

    def on_screen(self, needle: str) -> bool:
        return re.sub(r"\s+", " ", needle.strip()) in self.screen_text()

    def settle(self, quiet: float, limit: float) -> bool:
        """Waits until the app has drawn nothing for `quiet` seconds. False if `limit` ran out."""
        deadline = time.time() + limit
        while time.time() < deadline:
            if not self.alive:
                return True
            if time.time() - self.last_output >= quiet:
                return True
            time.sleep(0.1)
        return False

    def expect(self, needle: str, limit: float) -> bool:
        """Waits until `needle` is drawn. False if `limit` ran out.

        Matched against the text with the escape sequences taken out, because on screen a phrase is
        rarely one run: `review ready · Ctrl+G opens it` reaches the terminal as several styled
        pieces with SGR codes between them, so a raw search for it never matches.
        """
        deadline = time.time() + limit
        while time.time() < deadline:
            if not self.alive:
                return False
            if self.on_screen(needle):
                return True
            time.sleep(0.1)
        return False

    def autoanswer(self, needle: str, key: str, hold: float) -> None:
        """Presses `key` whenever `needle` appears, for the rest of the take.

        The hold is so a viewer reads the prompt before it is answered — a dialog that vanishes the
        instant it appears reads as a glitch rather than as a decision.
        """

        def watch() -> None:
            armed = not self.on_screen(needle)
            while self.alive:
                showing = self.on_screen(needle)
                if showing and armed:
                    armed = False
                    time.sleep(hold)
                    self.send(press(key))
                    note(f"   answered {needle!r} with {key!r}")
                elif not showing:
                    # Re-arm only once it has gone, or a prompt still on screen while the app
                    # processes the keystroke gets answered a second time.
                    armed = True
                time.sleep(0.2)

        threading.Thread(target=watch, daemon=True).start()

    def stop(self, grace: float) -> None:
        """Ends the app, politely and then not."""
        deadline = time.time() + grace
        while time.time() < deadline:
            if os.waitpid(self.pid, os.WNOHANG) != (0, 0):
                self.alive = False
                return
            time.sleep(0.1)
        for sig in (signal.SIGTERM, signal.SIGKILL):
            try:
                os.kill(self.pid, sig)
            except ProcessLookupError:
                break
            time.sleep(0.5)
        self.alive = False

    def write(self, cast: pathlib.Path, marks: pathlib.Path, title: str) -> None:
        with self._lock, cast.open("w", encoding="utf8") as handle:
            handle.write(
                json.dumps(
                    {
                        "version": 2,
                        "width": self.cols,
                        "height": self.rows,
                        "title": title,
                        "env": {"TERM": "xterm-256color"},
                    }
                )
                + "\n"
            )
            for at, text in self.events:
                handle.write(json.dumps([round(at, 6), "o", text]) + "\n")
        marks.write_text("".join(f"{n}\t{t:.3f}\n" for n, t in self.marks.items()), encoding="utf8")


# === The script ===

STEP = re.compile(r"^(\w+)(?:\s+(.*))?$")

# Where progress goes. Not stderr: stderr is the recording terminal, so a note written there lands
# in the middle of the shot — and when a take goes wrong the window is usually already gone, which
# is the exact moment the notes are worth having.
LOG: object | None = None


def note(message: str) -> None:
    if LOG is not None:
        print(message, file=LOG, flush=True)


def run_script(session: Session, steps: list[str]) -> None:
    """Plays the step list against the session. Runs on its own thread.

    Anything raised here would otherwise vanish: this is not the main thread, so an exception ends
    the script and leaves the relay recording a screen nobody is driving any more. The take still
    gets written, which is worse than failing — it looks like a demo that stopped early rather than
    a script with a bad step in it.
    """
    try:
        play(session, steps)
    except BaseException as error:  # noqa: BLE001 - the alternative is losing it entirely
        note(f"!! step failed: {error!r}")


def play(session: Session, steps: list[str]) -> None:
    for raw in steps:
        line = raw.rstrip("\r\n")
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        match = STEP.match(line.strip())
        if not match:
            sys.exit(f"cannot parse step: {line!r}")
        verb, rest = match.group(1), (match.group(2) or "")

        if not session.alive:
            note(f"!! app exited before step: {line!r}")
            return

        note(f"[{time.time() - session.started:7.1f}s] {line.strip()}")

        if verb == "sleep":
            time.sleep(float(rest))
        elif verb == "log":
            note(f"-- {rest}")
        elif verb == "settle":
            words = rest.split()
            quiet = float(words[0]) if words else 3.0
            limit = float(words[2]) if len(words) > 2 and words[1] == "max" else 300.0
            if not session.settle(quiet, limit):
                note(f"!! never settled within {limit}s — carrying on")
        elif verb == "expect":
            words = rest.rsplit(" max ", 1)
            limit = float(words[1]) if len(words) > 1 else 300.0
            if not session.expect(words[0], limit):
                note(f"!! never saw {words[0]!r} within {limit}s — carrying on")
        elif verb == "answer":
            # Key first and the text last, after a `when`. The text is the part with spaces in it,
            # so any other order needs the parser to guess where it ends — and a guess that came
            # out wrong watched for "Allow" and tried to press a key named "once", which failed
            # silently on a background thread and let a take stall on the very prompt it was
            # supposed to dismiss.
            match = re.match(r"^(\S+)(?:\s+hold\s+([\d.]+))?\s+when\s+(.+)$", rest)
            if not match:
                sys.exit(f"answer wants `<key> [hold N] when <text>`, got: {rest!r}")
            key, hold, needle = match.group(1), match.group(2), match.group(3)
            session.autoanswer(needle.strip(), key, float(hold) if hold else 2.5)
        elif verb == "mark":
            session.mark(rest.strip())
        elif verb == "type":
            session.type_text(rest)
        elif verb == "enter":
            if rest:
                session.type_text(rest)
            time.sleep(0.2)
            session.send("\r")
        elif verb == "send":
            session.send(rest.encode("utf8").decode("unicode_escape"))
        elif verb == "key":
            parts = rest.split()
            count = int(parts[1][1:]) if len(parts) > 1 and parts[1].startswith("x") else 1
            for _ in range(count):
                # `press`, not `named`: a step is as likely to want a letter the surface binds
                # (`c` to comment, `a` to ask) as a named key, and `named` has no entry for one —
                # it exited the script thread mid-take, silently, because that thread is where the
                # exit was raised.
                session.send(press(parts[0]))
                time.sleep(0.12)
        elif verb == "mod":
            parts = rest.split()
            session.send(chord(parts[0].split("+"), parts[1]))
            time.sleep(0.12)
        else:
            sys.exit(f"unknown step {verb!r}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--script", type=pathlib.Path, required=True)
    parser.add_argument("--out", type=pathlib.Path, required=True)
    parser.add_argument("--marks", type=pathlib.Path, required=True)
    parser.add_argument("--log", type=pathlib.Path)
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--cols", type=int, default=92)
    parser.add_argument("--rows", type=int, default=28)
    parser.add_argument("--cwd", default=os.getcwd())
    parser.add_argument("--title", default="luxe")
    parser.add_argument(
        "--quit",
        default="",
        help="keys sent to end the take, `|`-separated (default: none — the app is signalled)",
    )
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()

    command = [c for c in args.command if c != "--"]
    if not command:
        sys.exit("nothing to run: pass the command after --")

    global LOG
    if args.log:
        LOG = args.log.open("w", encoding="utf8")

    steps = args.script.read_text(encoding="utf8").splitlines()
    session = Session(command, args.cols, args.rows, args.cwd, dict(os.environ), args.seed)

    # A take is usually abandoned by closing the window, which is a signal rather than a return —
    # and a recording that is only written on the happy path is one that never survives the run you
    # most wanted to look at.
    def rescue(*_: object) -> None:
        session.alive = False
        note("!! interrupted — saving what was captured")
        session.write(args.out, args.marks, args.title)
        os._exit(1)

    for sig in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT):
        signal.signal(sig, rescue)

    driver = threading.Thread(target=run_script, args=(session, steps), daemon=True)
    driver.start()

    # The relay owns the main thread; the script thread only writes. When the script finishes, the
    # app is asked to leave and then made to.
    def finish() -> None:
        driver.join()
        time.sleep(0.4)
        if "end" not in session.marks:
            session.mark("end")
        # Nothing is typed to leave. A quit key is a key like any other, so it lands in whatever
        # has focus — `q` in the composer, on screen, in the last seconds of the take. The cast is
        # already cut at the `end` mark, so the tidy exit buys nothing and costs a visible
        # keystroke.
        for key in filter(None, args.quit.split("|")):
            session.send(key.encode("utf8").decode("unicode_escape"))
            time.sleep(0.6)
        # Then made to. Everything worth keeping is already recorded and the cast is cut at the
        # `end` mark, so a take that will not leave politely costs nothing by being ended.
        session.stop(grace=4.0)

    threading.Thread(target=finish, daemon=True).start()

    try:
        session.relay()
    except KeyboardInterrupt:
        pass
    finally:
        session.alive = False
        try:
            os.kill(session.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        session.write(args.out, args.marks, args.title)

    note(f"recorded {len(session.events)} frames to {args.out}")


if __name__ == "__main__":
    main()
