#!/usr/bin/env python3
"""Cut a raw screen recording down to a demo.

A real session is recorded at the speed a real session runs: a provider thinks for eleven seconds,
a build takes forty, and the interesting part is the two seconds either side. Played back untouched
that is a video of a progress bar. This trims the dead air without touching a single frame's
content — every byte the terminal emitted is still there, in order, and only the *gaps between*
them move.

Three edits, in this order:

  window   keep what lies between two marks the recording script dropped, so the take can start
           before the app is up and end after it has been told to quit — opening with a rebuilt
           picture of the screen as it stood at the mark, because a terminal recording is *state*
           and a prefix cannot simply be dropped
  clamp    cap any gap longer than `--max-idle`, which is the whole trick: waiting is compressed,
           reading time is not, because a hold the script asked for is shorter than the cap
  pad      hold the last frame, so a demo that ends on its own final state does not blink away

Before them all, `squeeze`: the stretch between a `squeeze` mark and a `resume` mark (or any
`squeeze-NAME` and `resume-NAME` pair) plays in `--squeeze-to` seconds. It is for a wait that is not dead air, a spinner turning while a model
writes, which clamping cannot touch because the screen never stops changing. Every frame is kept,
in order, and a clock on screen still says how long the wait really was.

`--speed` scales what is left. Clamping only reaches the *gaps*, and a model that streams for
ninety seconds without pausing has no gaps to give — so a long take needs the stream itself played
faster. Applied last, and sparingly: text arriving quickly is still readable, because what a reader
actually reads is the frame it settles on.

Input is asciicast v2 — a JSON header line, then one `[time, "o", data]` array per line. Output is
the same, which is what `asciinema-player` reads.

    scripts/cast-edit.py raw.cast --out demo.cast --marks raw.marks --max-idle 1.2
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

import pyte

# A gap shorter than this is somebody typing, not somebody waiting. Left alone entirely, so the
# cadence of a typed prompt survives the edit.
KEYSTROKE = 0.25


def load(path: pathlib.Path) -> tuple[dict, list[list]]:
    """Reads an asciicast v2 file into its header and its events."""
    header: dict | None = None
    events: list[list] = []
    for line in path.read_text(encoding="utf8").splitlines():
        if not line.strip():
            continue
        parsed = json.loads(line)
        if header is None:
            if not isinstance(parsed, dict):
                sys.exit(f"{path}: first line is not an asciicast header")
            header = parsed
            continue
        events.append(parsed)
    if header is None:
        sys.exit(f"{path}: empty recording")
    return header, events


def window(
    events: list[list],
    start: float | None,
    end: float | None,
    size: tuple[int, int],
) -> list[list]:
    """Keeps the events between two marks, opening on the screen as it stood at the first.

    A recording is not a film strip, it is a stream of *edits* to a screen. Ratatui writes only the
    cells that changed, so the frame that drew the sidebar, the borders and the composer may be
    thousands of events before the moment a demo wants to begin. Slicing the prefix away therefore
    does not trim a run-up — it deletes the picture, and the take opens on a blank terminal with a
    half-typed line floating in it, which is exactly how the commit demo shipped.

    So the prefix is *replayed* rather than dropped, and the take opens with one synthetic event
    that paints the resulting screen in full. Everything after the mark is a genuine recording.
    """
    before = [e for e in events if start is None or e[0] < start]
    kept = [e for e in events if (start is None or e[0] >= start) and (end is None or e[0] <= end)]
    if not kept:
        return []

    offset = kept[0][0]
    rebased = [[round(e[0] - offset, 6), *e[1:]] for e in kept]
    if not before:
        return rebased
    return [[0.0, "o", repaint(before, size)], *rebased]


def repaint(events: list[list], size: tuple[int, int]) -> str:
    """Replays events into a screen and returns the escape sequence that redraws it."""
    columns, rows = size
    screen = pyte.Screen(columns, rows)
    stream = pyte.Stream(screen)
    for event in events:
        stream.feed(event[2])

    # Alternate screen, hidden cursor, cleared — the same state the app put the terminal in, so the
    # player starts from what it would have had rather than from a fresh terminal's defaults.
    out = ["\x1b[?1049h", "\x1b[?25l", "\x1b[H", "\x1b[2J"]
    for row in range(rows):
        line = screen.buffer[row]
        out.append(f"\x1b[{row + 1};1H")
        run, style = "", None
        for column in range(columns + 1):
            cell = line[column] if column < columns else None
            here = (cell.fg, cell.bg, cell.bold, cell.reverse) if cell else None
            if here != style:
                if run:
                    out.append(run)
                out.append(sgr(here) if here else "\x1b[0m")
                run, style = "", here
            if cell:
                run += cell.data or " "
        if run:
            out.append(run)
        out.append("\x1b[0m")
    return "".join(out)


def sgr(style: tuple[str, str, bool, bool]) -> str:
    """One cell style as an SGR sequence."""
    foreground, background, bold, reverse = style
    parts = ["0"]
    if bold:
        parts.append("1")
    if reverse:
        parts.append("7")
    for value, prefix in ((foreground, "38"), (background, "48")):
        if value == "default" or not value:
            continue
        if len(value) == 6 and all(c in "0123456789abcdefABCDEF" for c in value):
            parts += [prefix, "2", str(int(value[0:2], 16)), str(int(value[2:4], 16)), str(int(value[4:6], 16))]
    return f"\x1b[{';'.join(parts)}m"


def clamp(events: list[list], max_idle: float) -> list[list]:
    """Caps every gap at `max_idle`, leaving the events themselves untouched.

    Deliberately not a uniform speed-up. Scaling the whole timeline would squeeze the pauses the
    script asked for — the beat after a prompt lands, the seconds a review screen needs to be read
    — by exactly as much as it squeezes the provider's thinking time, and those are opposite
    problems. So only the long gaps give.
    """
    out: list[list] = []
    previous = 0.0
    shift = 0.0
    for event in events:
        gap = event[0] - previous
        if gap > max_idle:
            shift += gap - max_idle
        previous = event[0]
        out.append([round(event[0] - shift, 6), *event[1:]])
    return out


def accelerate(events: list[list], factor: float) -> list[list]:
    """Plays the whole timeline `factor` times faster."""
    if factor == 1.0:
        return events
    return [[round(e[0] / factor, 6), *e[1:]] for e in events]


def pad(events: list[list], seconds: float) -> list[list]:
    """Holds the last frame by emitting a no-op after it.

    The player stops on its final frame, but "stops" is measured from the last event — so a demo
    whose last event is the keystroke that produced the payoff cuts the instant the payoff appears.
    A zero-width sequence costs nothing and buys the beat.
    """
    if not events or seconds <= 0:
        return events
    return [*events, [round(events[-1][0] + seconds, 6), "o", ""]]


def squeeze(events: list[list], found: dict[str, float], seconds: float) -> list[list]:
    """Plays each `squeeze`…`resume` stretch in `seconds`, moving every later event and mark up by
    the time saved. Taken latest first, so an earlier pair's marks are not yet moved when it is."""
    pairs = sorted(
        (name for name in found if name == "squeeze" or name.startswith("squeeze-")),
        key=lambda name: found[name],
        reverse=True,
    )
    for name in pairs:
        events = squeeze_one(events, found, name, "resume" + name[len("squeeze"):], seconds)
    return events


def squeeze_one(
    events: list[list], found: dict[str, float], open_mark: str, close_mark: str, seconds: float
) -> list[list]:
    start, end = found.get(open_mark), found.get(close_mark)
    if start is None or end is None or end - start <= seconds:
        return events
    span = end - start
    saved = span - seconds

    def moved(at: float) -> float:
        if at < start:
            return at
        if at <= end:
            return start + (at - start) * seconds / span
        return at - saved

    for name, at in list(found.items()):
        found[name] = moved(at)
    return [[round(moved(e[0]), 6), *e[1:]] for e in events]


def marks(path: pathlib.Path | None) -> dict[str, float]:
    """Reads the `name<TAB>seconds` sidecar the recording script writes."""
    if path is None or not path.exists():
        return {}
    found: dict[str, float] = {}
    for line in path.read_text(encoding="utf8").splitlines():
        name, _, seconds = line.partition("\t")
        if seconds:
            found[name.strip()] = float(seconds)
    return found


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=pathlib.Path)
    parser.add_argument("--out", type=pathlib.Path, required=True)
    parser.add_argument("--marks", type=pathlib.Path)
    parser.add_argument("--max-idle", type=float, default=1.2, help="cap on a gap, in seconds")
    parser.add_argument("--tail", type=float, default=2.5, help="hold on the last frame")
    parser.add_argument("--speed", type=float, default=1.0, help="playback multiplier")
    parser.add_argument(
        "--squeeze-to", type=float, default=2.0, help="seconds the squeeze/resume stretch plays in"
    )
    parser.add_argument("--from-mark", default="start")
    parser.add_argument("--to-mark", default="end")
    parser.add_argument("--title")
    args = parser.parse_args()

    if args.max_idle <= KEYSTROKE:
        sys.exit(f"--max-idle must exceed {KEYSTROKE}s, or typing is compressed with the waiting")

    header, events = load(args.source)
    found = marks(args.marks)
    events = squeeze(events, found, args.squeeze_to)
    size = (int(header.get("width", 80)), int(header.get("height", 24)))
    events = window(events, found.get(args.from_mark), found.get(args.to_mark), size)
    if not events:
        sys.exit(f"{args.source}: nothing between the {args.from_mark} and {args.to_mark} marks")

    raw_duration = events[-1][0]
    events = pad(accelerate(clamp(events, args.max_idle), args.speed), args.tail)

    if args.title:
        header["title"] = args.title
    # Regenerated on every edit rather than trusted from the recorder: the player reserves its box
    # from this number, and a stale one is a page that jumps when the demo loads.
    header["duration"] = events[-1][0]

    with args.out.open("w", encoding="utf8") as handle:
        handle.write(json.dumps(header) + "\n")
        for event in events:
            handle.write(json.dumps(event) + "\n")

    print(
        f"{args.out.name}: {raw_duration:.1f}s recorded → {events[-1][0]:.1f}s played"
        f" ({len(events)} frames)"
    )


if __name__ == "__main__":
    main()
