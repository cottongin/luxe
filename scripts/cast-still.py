#!/usr/bin/env python3
"""Render one frame of a recording as an SVG screenshot.

Some surfaces do not survive being watched. Guided Review puts a chapter's argument beside the code
it argues about, and a reader meeting it for the first time needs to *stop* on it — played at demo
speed it registers as motion rather than as information. So the same capture that makes a demo also
makes the stills, and there is still exactly one source of truth for what Luxe looks like.

**The drawing is not done here.** This replays the recording with `pyte` to work out what is on the
screen, then hands that grid to `fixture_svg --grid`, which is the exporter the generated
screenshots already come out of. That matters because box-drawing and block characters have to be
drawn as *geometry* rather than typeset: a `│` glyph is shorter than the row pitch, so a column of
them typeset as text breaks into dashes, and a slab's triangle cap detaches from the rect behind it.
That logic is a hundred-odd lines of careful Rust, and a second copy of it in Python would drift
from the first within a release.

    scripts/cast-still.py review.cast --at 12.5 --out review.svg
    scripts/cast-still.py review.cast --at end --crop 0,0,60,20 --out detail.svg

`--crop` takes `col,row,cols,rows` and is how a surface earns a place on a phone: a cropped detail
of the statusline reads at 390px, and the whole 150-column workspace never will.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import subprocess
import sys

import pyte

# The default colors a recording's cells fall back to. `pyte` reports them by name; the exporter
# wants either a real color or nothing, so "default" becomes absent and the SVG's own canvas shows
# through.
ANSI = {
    "black": "#12100e",
    "red": "#e68485",
    "green": "#81caa0",
    "brown": "#faaa51",
    "yellow": "#faaa51",
    "blue": "#85c7d7",
    "magenta": "#d2a5cd",
    "cyan": "#85c7d7",
    "white": "#e4dfd6",
}


def color(value: str) -> str | None:
    """Turns a pyte color into `#rrggbb`, or `None` for the terminal's default."""
    if value == "default":
        return None
    if value in ANSI:
        return ANSI[value]
    if len(value) == 6 and all(c in "0123456789abcdefABCDEF" for c in value):
        return f"#{value}"
    return None


def replay(path: pathlib.Path, until: float, cols: int, rows: int) -> pyte.Screen:
    """Feeds the recording into a screen up to `until` seconds."""
    screen = pyte.Screen(cols, rows)
    stream = pyte.Stream(screen)
    for line in path.read_text(encoding="utf8").splitlines():
        if not line.strip():
            continue
        event = json.loads(line)
        if isinstance(event, dict):
            continue
        if event[0] > until:
            break
        stream.feed(event[2])
    return screen


def grid(screen: pyte.Screen, crop: tuple[int, int, int, int]) -> dict:
    """The visible cells, cropped, in the shape `fixture_svg --grid` reads."""
    left, top, width, height = crop
    cells = []
    for row in range(top, top + height):
        line = screen.buffer[row]
        for column in range(left, left + width):
            cell = line[column]
            entry: dict[str, object] = {"c": cell.data}
            foreground, background = color(cell.fg), color(cell.bg)
            # Reverse video is a real thing on this screen — a selected row, a slab — and swapping
            # here means the exporter never has to know about it.
            if cell.reverse:
                foreground, background = background or "#12100e", foreground or "#e4dfd6"
            if foreground:
                entry["fg"] = foreground
            if background:
                entry["bg"] = background
            if cell.bold:
                entry["bold"] = True
            cells.append(entry)
    return {"width": width, "height": height, "cells": cells}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=pathlib.Path)
    parser.add_argument("--out", type=pathlib.Path, required=True)
    parser.add_argument("--at", default="end", help="seconds, or `end` for the last frame")
    parser.add_argument("--crop", help="col,row,cols,rows")
    args = parser.parse_args()

    header = json.loads(args.source.read_text(encoding="utf8").splitlines()[0])
    cols, rows = header["width"], header["height"]
    until = float("inf") if args.at == "end" else float(args.at)

    screen = replay(args.source, until, cols, rows)
    window = (0, 0, cols, rows)
    if args.crop:
        left, top, width, height = (int(n) for n in args.crop.split(","))
        if left + width > cols or top + height > rows:
            sys.exit(f"crop {args.crop} does not fit a {cols}x{rows} screen")
        window = (left, top, width, height)

    exporter = pathlib.Path(__file__).resolve().parent.parent / "target/release/examples/fixture_svg"
    if not exporter.exists():
        sys.exit(
            f"no exporter at {exporter} — "
            "cargo build --release -p luxe-tui --features fixture --example fixture_svg"
        )

    done = subprocess.run(
        [str(exporter), "--grid"],
        input=json.dumps(grid(screen, window)),
        capture_output=True,
        text=True,
        check=False,
    )
    if done.returncode != 0:
        sys.exit(f"the exporter refused the grid: {done.stderr.strip()}")

    args.out.write_text(done.stdout, encoding="utf8")
    print(f"{args.out.name}: {window[2]}x{window[3]} cells, {args.out.stat().st_size // 1024}KB")


if __name__ == "__main__":
    main()
